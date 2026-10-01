//! Exact-ish mass properties from the boundary: face areas and shell volumes.
//!
//! Both come from Green's theorem in each face's parameter space, integrated along the
//! loops' edges with Gauss–Legendre quadrature (exact for lines, accurate to rounding for
//! circles and ellipses), so they need no tessellation.
//!
//! - Plane: area `= ½ ∮ (u dv − v du)`.
//! - Cylinder (`θ`, `v`): area `= r ∮ θ dv`, with `θ` unwrapped continuously along the loop.
//! - Volume `= ⅓ ∬ (x − ref) · n dA`, summed over the faces of a closed shell. Integrated
//!   along each loop *as oriented*, so a shell whose loops all run the wrong way gets a
//!   negative volume regardless of the faces' `reversed` flags.

use std::f64::consts::FRAC_PI_8;

use peet_math::DVec3;

use crate::Solid;
use crate::geom::{Curve3, Surface, unwrap_angle};
use crate::topo::{FaceId, LoopId, ShellId};

/// Gauss–Legendre nodes and weights on `[-1, 1]`.
const GAUSS: [(f64, f64); 5] = [
    (-0.906_179_845_938_664, 0.236_926_885_056_189_1),
    (-0.538_469_310_105_683_1, 0.478_628_670_499_366_5),
    (0.0, 0.568_888_888_888_888_9),
    (0.538_469_310_105_683_1, 0.478_628_670_499_366_5),
    (0.906_179_845_938_664, 0.236_926_885_056_189_1),
];

/// Curved edges are integrated in pieces of at most this parameter span.
const MAX_PIECE: f64 = FRAC_PI_8;

/// Boundary integrals of one loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LoopIntegrals {
    /// Area enclosed in parameter space, scaled to model units (mm²): positive when the loop
    /// runs counter-clockwise in `(u, v)`, i.e. counter-clockwise seen from the surface's
    /// natural normal side.
    pub signed_area: f64,
    /// This loop's contribution to `∬ (x − ref) · n dA / 3`.
    pub volume: f64,
    /// Net change of the cylinder angle around the loop (0 for planes and for loops that
    /// don't wrap around the axis).
    pub winding: f64,
    /// Range of the unwrapped cylinder angle along the loop (`(∞, −∞)` for planes).
    pub theta_range: (f64, f64),
}

impl Default for LoopIntegrals {
    fn default() -> Self {
        Self {
            signed_area: 0.0,
            volume: 0.0,
            winding: 0.0,
            theta_range: (f64::INFINITY, f64::NEG_INFINITY),
        }
    }
}

pub(crate) fn loop_integrals(solid: &Solid, l: LoopId, reference: DVec3) -> LoopIntegrals {
    let surface = solid.face(solid.loop_(l).face).surface;
    let mut out = LoopIntegrals::default();
    let mut theta: Option<f64> = None;
    let mut theta_start = 0.0;
    let coedges = solid.loop_coedges(l);
    for &c in &coedges {
        let co = solid.coedge(c);
        let e = solid.edge(co.edge);
        let (ta, tb) = if co.reversed {
            (e.t1, e.t0)
        } else {
            (e.t0, e.t1)
        };
        let pieces = if matches!(e.curve, Curve3::Line(_)) {
            1
        } else {
            ((tb - ta).abs() / MAX_PIECE).ceil().max(1.0) as usize
        };
        let span = (tb - ta) / pieces as f64;
        for k in 0..pieces {
            let a = ta + span * k as f64;
            let nodes = std::iter::once((a, 0.0))
                .chain(GAUSS.iter().map(|&(x, w)| (a + span * 0.5 * (x + 1.0), w)));
            for (t, w) in nodes {
                let p = e.curve.point(t);
                let d = e.curve.derivative(t) * span * 0.5;
                integrand(&surface, p, d, reference, &mut theta, w, &mut out);
                if c == coedges[0] && k == 0 && w == 0.0 {
                    theta_start = theta.unwrap_or_default();
                }
            }
        }
    }
    if let Some(last) = coedges.last() {
        // Close the loop to measure the winding.
        let co = solid.coedge(*last);
        let e = solid.edge(co.edge);
        let t_end = if co.reversed { e.t0 } else { e.t1 };
        let mut dummy = LoopIntegrals::default();
        integrand(
            &surface,
            e.curve.point(t_end),
            DVec3::ZERO,
            reference,
            &mut theta,
            0.0,
            &mut dummy,
        );
        if let Some(th) = theta {
            out.winding = th - theta_start;
        }
    }
    out
}

/// Adds one quadrature node: point `p`, derivative `d` (per unit of the `[-1, 1]` node
/// variable) and weight `w`.
fn integrand(
    surface: &Surface,
    p: DVec3,
    d: DVec3,
    reference: DVec3,
    theta: &mut Option<f64>,
    w: f64,
    out: &mut LoopIntegrals,
) {
    match surface {
        Surface::Plane(pl) => {
            let uv = pl.to_plane_coords(p);
            let du = pl.frame.x_axis().dot(d);
            let dv = pl.frame.y_axis().dot(d);
            let area = 0.5 * (uv.x * dv - uv.y * du) * w;
            out.signed_area += area;
            out.volume += (pl.origin() - reference).dot(pl.normal()) * area / 3.0;
        }
        Surface::Cylinder(cy) => {
            let q = cy.frame.to_local(p);
            let raw = q.y.atan2(q.x);
            let th = match *theta {
                Some(prev) => unwrap_angle(raw, prev),
                None => raw,
            };
            *theta = Some(th);
            out.theta_range = (out.theta_range.0.min(th), out.theta_range.1.max(th));
            let dv = cy.frame.vector_to_local(d).z;
            let r = cy.radius;
            out.signed_area += r * th * dv * w;
            let o = cy.frame.origin - reference;
            let a = o.dot(cy.frame.x_axis());
            let b = o.dot(cy.frame.y_axis());
            let (s, co) = th.sin_cos();
            out.volume += r * (a * s - b * co + r * th) * dv * w / 3.0;
        }
    }
}

/// Area of a face (mm²): the outer loop's area minus its holes, from the loops' signed
/// parameter-space areas. Positive for a correctly oriented face.
pub fn face_area(solid: &Solid, face: FaceId) -> f64 {
    let f = solid.face(face);
    let sign = if f.reversed { -1.0 } else { 1.0 };
    f.loops
        .iter()
        .map(|&l| loop_integrals(solid, l, DVec3::ZERO).signed_area * sign)
        .sum()
}

/// Volume enclosed by a closed shell (mm³): positive when its faces point outwards,
/// negative for an inside-out shell (as an internal void's shell would be).
pub fn shell_volume(solid: &Solid, shell: ShellId) -> f64 {
    let reference = shell_reference(solid, shell);
    solid
        .shell(shell)
        .faces
        .iter()
        .flat_map(|&f| solid.face(f).loops.iter())
        .map(|&l| loop_integrals(solid, l, reference).volume)
        .sum()
}

/// Total volume of a solid (mm³): the sum of its shells' signed volumes.
pub fn volume(solid: &Solid) -> f64 {
    (0..solid.shells.len() as u32)
        .map(|s| shell_volume(solid, ShellId(s)))
        .sum()
}

/// A point near the shell, so the volume integrand stays small (less cancellation).
pub(crate) fn shell_reference(solid: &Solid, shell: ShellId) -> DVec3 {
    solid
        .shell(shell)
        .faces
        .first()
        .and_then(|&f| solid.face(f).loops.first())
        .map(|&l| {
            let c = solid.loop_(l).first;
            solid.vertex(solid.coedge_start(c)).point
        })
        .unwrap_or(DVec3::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::test_shapes::cuboid;

    #[test]
    fn cuboid_measures() {
        let s = cuboid(DVec3::new(1.0, -2.0, 3.0), DVec3::new(4.0, 2.0, 8.0));
        assert!((volume(&s) - 60.0).abs() < 1e-9);
        let areas: Vec<f64> = s.face_ids().map(|f| face_area(&s, f)).collect();
        let total: f64 = areas.iter().sum();
        assert!(
            (total - 2.0 * (12.0 + 15.0 + 20.0)).abs() < 1e-9,
            "{areas:?}"
        );
    }
}
