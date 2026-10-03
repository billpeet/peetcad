//! Exact-ish mass properties from the boundary: face areas and shell volumes.
//!
//! Both come from Green's theorem in each face's parameter space, integrated along the
//! loops' edges with Gauss–Legendre quadrature (exact for lines, accurate to rounding for
//! circles and ellipses), so they need no tessellation.
//!
//! - Plane: area `= ½ ∮ (u dv − v du)`.
//! - Surfaces of revolution (`u`, `v`): area `= ∮ u ρ(v) σ(v) dv`, where `ρ` is the
//!   meridian's distance from the axis and `σ` its speed, with the loop lifted into
//!   parameter space as described in [`crate::geom`] (a cylinder gives `r ∮ θ dv`).
//! - Volume `= ⅓ ∬ (x − ref) · n dA`, summed over the faces of a closed shell. Integrated
//!   along each loop *as oriented*, so a shell whose loops all run the wrong way gets a
//!   negative volume regardless of the faces' `reversed` flags.

use std::f64::consts::FRAC_PI_8;

use peet_math::{Aabb, DVec2, DVec3};

use crate::Solid;
use crate::geom::{Curve3, Surface, pole_exit};
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
    /// Net change of a torus's tube angle around the loop (0 otherwise).
    pub winding_v: f64,
    /// Range of the unwrapped angle about the axis along the loop (`(∞, −∞)` for planes).
    pub theta_range: (f64, f64),
    /// Range of the lifted `v` along the loop (`(∞, −∞)` for planes).
    pub v_range: (f64, f64),
}

impl Default for LoopIntegrals {
    fn default() -> Self {
        Self {
            signed_area: 0.0,
            volume: 0.0,
            winding: 0.0,
            winding_v: 0.0,
            v_range: (f64::INFINITY, f64::NEG_INFINITY),
            theta_range: (f64::INFINITY, f64::NEG_INFINITY),
        }
    }
}

pub(crate) fn loop_integrals(solid: &Solid, l: LoopId, reference: DVec3) -> LoopIntegrals {
    let face = solid.face(solid.loop_(l).face);
    let surface = face.surface;
    let ccw = !face.reversed;
    let curved = surface.revolution_frame().is_some();
    let mut out = LoopIntegrals::default();
    // The lifted parameters of the point last visited, and of the loop's first point.
    let mut at: Option<DVec2> = None;
    let mut start = DVec2::ZERO;
    let mut end_pole = None;
    let see = |uv: DVec2, out: &mut LoopIntegrals| {
        if curved {
            out.theta_range = (out.theta_range.0.min(uv.x), out.theta_range.1.max(uv.x));
            out.v_range = (out.v_range.0.min(uv.y), out.v_range.1.max(uv.y));
        }
    };
    for c in solid.loop_coedges(l) {
        let co = solid.coedge(c);
        let e = solid.edge(co.edge);
        let (ta, tb) = if co.reversed {
            (e.t1, e.t0)
        } else {
            (e.t0, e.t1)
        };
        // Where the edge starts: at a pole the loop first runs along the pole's line.
        let (raw, pole) = surface.param_toward(&e.curve, ta, tb);
        let mut uv = match (at, pole) {
            (None, _) => {
                start = raw;
                raw
            }
            (Some(prev), Some(pole)) => DVec2::new(pole_exit(prev.x, raw.x, pole.top, ccw), pole.v),
            (Some(prev), None) => prev,
        };
        see(uv, &mut out);
        let pieces = if matches!(e.curve, Curve3::Line(_)) {
            1
        } else {
            ((tb - ta).abs() / MAX_PIECE).ceil().max(1.0) as usize
        };
        let span = (tb - ta) / pieces as f64;
        for k in 0..pieces {
            let a = ta + span * k as f64;
            for &(x, w) in &GAUSS {
                let t = a + span * 0.5 * (x + 1.0);
                let p = e.curve.point(t);
                let d = e.curve.derivative(t) * span * 0.5;
                uv = surface.param_near(surface.param(p), uv);
                see(uv, &mut out);
                integrand(&surface, p, uv, d, reference, w, &mut out);
            }
        }
        let (raw, pole) = surface.param_toward(&e.curve, tb, ta);
        uv = surface.param_near(raw, uv);
        see(uv, &mut out);
        at = Some(uv);
        end_pole = pole;
    }
    if let Some(mut uv) = at {
        if let Some(pole) = end_pole {
            uv.x = pole_exit(uv.x, start.x, pole.top, ccw);
            see(uv, &mut out);
        }
        out.winding = uv.x - start.x;
        out.winding_v = uv.y - start.y;
    }
    out
}

/// Adds one quadrature node: point `p` with lifted parameters `uv`, derivative `d` (per
/// unit of the `[-1, 1]` node variable) and weight `w`.
///
/// On a surface of revolution with meridian `(ρ(v), z(v))` the area element is
/// `ρ √(ρ'² + z'²) du dv` and `(x − ref) · N du dv` has the antiderivative in `u` used
/// below, so both integrals become `∮ F(u, v) dv` along the loop. The runs along a pole's
/// line have `dv = 0` and add nothing.
fn integrand(
    surface: &Surface,
    p: DVec3,
    uv: DVec2,
    d: DVec3,
    reference: DVec3,
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
        _ => {
            let (Some(frame), Some(m)) = (surface.revolution_frame(), surface.meridian(uv.y))
            else {
                return;
            };
            let (_, sv) = surface.derivatives(uv);
            let dv = d.dot(sv) / sv.length_squared();
            let u = uv.x;
            out.signed_area += u * m.rho * m.drho.hypot(m.dz) * dv * w;
            let o = frame.origin - reference;
            let a = o.dot(frame.x_axis());
            let b = o.dot(frame.y_axis());
            let c = o.dot(frame.z_axis());
            let (s, co) = u.sin_cos();
            out.volume +=
                m.rho * (m.dz * (a * s - b * co + m.rho * u) - m.drho * (c + m.z) * u) * dv * w
                    / 3.0;
        }
    }
}

/// Bounds of the part of a sphere or torus a face covers, beyond what its edges reach: a
/// doubly curved face bulges out between its edges. Empty for other surfaces, whose faces
/// stay within the bounds of their edges.
pub(crate) fn bulge_bounds(solid: &Solid, face: FaceId) -> Aabb {
    const GRID: usize = 48;
    let f = solid.face(face);
    let mut b = Aabb::EMPTY;
    // The radii the surface curves with around its axis and along its meridian.
    let (around, along) = match &f.surface {
        Surface::Sphere(s) => (s.radius, s.radius),
        Surface::Torus(t) => (t.major + t.minor, t.minor),
        _ => return b,
    };
    let Some(&outer) = f.loops.first() else {
        return b;
    };
    let m = loop_integrals(solid, outer, DVec3::ZERO);
    let ((u0, u1), (v0, v1)) = (m.theta_range, m.v_range);
    if !(u0 <= u1 && v0 <= v1) {
        return b;
    }
    for i in 0..=GRID {
        for j in 0..=GRID {
            let f_ij = DVec2::new(i as f64, j as f64) / GRID as f64;
            let uv = DVec2::new(u0 + (u1 - u0) * f_ij.x, v0 + (v1 - v0) * f_ij.y);
            b.extend(f.surface.point(uv));
        }
    }
    // Between the grid's points the surface leaves their chords by at most the sagitta
    // in each direction.
    let sagitta = |radius: f64, span: f64| radius * (1.0 - (0.5 * span / GRID as f64).cos());
    let margin = sagitta(around, u1 - u0) + sagitta(along, v1 - v0);
    Aabb {
        min: b.min - DVec3::splat(margin),
        max: b.max + DVec3::splat(margin),
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
