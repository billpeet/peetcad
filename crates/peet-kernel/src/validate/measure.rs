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

/// Pieces an edge is integrated in where its image in a freeform face's parameters is a
/// general curve (per polynomial span, for a freeform edge).
const FREEFORM_PIECES: usize = 6;

/// A freeform surface with more knot lines than this across the direction it is
/// integrated in is in pieces small enough for one Gauss panel each.
const MANY_PIECES: usize = 8;

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
    let surface = face.surface.clone();
    let ccw = !face.reversed;
    let curved = !matches!(surface, Surface::Plane(_));
    let freeform = matches!(surface, Surface::Nurbs(_));
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
        // The parameters between which the edge is integrated, piece by piece, in the
        // direction it is walked.
        let mut cuts = vec![ta];
        let evenly = |cuts: &mut Vec<f64>, from: f64, to: f64, pieces: usize| {
            cuts.extend((1..=pieces).map(|k| {
                if k == pieces {
                    to
                } else {
                    from + (to - from) * k as f64 / pieces as f64
                }
            }));
        };
        match &e.curve {
            Curve3::Line(_) if !freeform => cuts.push(tb),
            // Across a freeform face even a straight edge is a curve in its parameters.
            Curve3::Line(_) => evenly(&mut cuts, ta, tb, FREEFORM_PIECES),
            Curve3::Nurbs(n) => {
                // Each polynomial span the edge covers is one smooth piece, so the
                // pieces end at the curve's knots: a Gauss panel across a knot would be
                // integrating a function with a kink in it. Across a freeform face the
                // span's image bends a little more, and gets two pieces.
                let (lo, hi) = (ta.min(tb), ta.max(tb));
                let mut knots: Vec<f64> = Vec::new();
                for &k in n.knots() {
                    if k > lo && k < hi && knots.last().is_none_or(|l| k > *l) {
                        knots.push(k);
                    }
                }
                if tb < ta {
                    knots.reverse();
                }
                knots.push(tb);
                let per = if freeform { 2 } else { 1 };
                let mut from = ta;
                for to in knots {
                    evenly(&mut cuts, from, to, per);
                    from = to;
                }
            }
            _ => {
                let by_angle = ((tb - ta).abs() / MAX_PIECE).ceil().max(1.0) as usize;
                let pieces = if freeform {
                    by_angle.max(FREEFORM_PIECES)
                } else {
                    by_angle
                };
                evenly(&mut cuts, ta, tb, pieces);
            }
        }
        for piece in cuts.windows(2) {
            let (a, span) = (piece[0], piece[1] - piece[0]);
            for &(x, w) in &GAUSS {
                let t = a + span * 0.5 * (x + 1.0);
                let p = e.curve.point(t);
                let d = e.curve.derivative(t) * span * 0.5;
                uv = surface.param_near(surface.param_from(p, uv), uv);
                see(uv, &mut out);
                integrand(&surface, p, uv, start, d, reference, w, &mut out);
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
///
/// A freeform surface has no such antiderivative, so it is integrated numerically from
/// the line through `origin` (in parameters) to the point. Any line does, as long as it
/// is the same one all the way round the loop: moving it adds a function of the other
/// parameter alone, whose integral round a closed loop is zero. The loop's first point
/// is used, which is never far from the rest of the loop.
#[allow(clippy::too_many_arguments)]
fn integrand(
    surface: &Surface,
    p: DVec3,
    uv: DVec2,
    origin: DVec2,
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
        Surface::Nurbs(s) => {
            // No antiderivative in closed form: integrate along `u` from the loop's
            // starting line to the point, numerically (the integrand is a smooth rational
            // function between the knots).
            let [_, su, sv, ..] = s.evaluate(uv);
            // The edge's direction in parameters: d = Su du + Sv dv.
            let (e, f, g) = (su.dot(su), su.dot(sv), sv.dot(sv));
            let det = e * g - f * f;
            if det.abs() <= f64::MIN_POSITIVE {
                return;
            }
            let dv = (e * d.dot(sv) - f * d.dot(su)) / det;
            let (breaks, _) = s.breaks();
            // From the starting line to the point, either way.
            let (from, to, step) = if uv.x < origin.x {
                (uv.x, origin.x, -dv)
            } else {
                (origin.x, uv.x, dv)
            };
            // Two Gauss panels per piece; one is as good where the surface is in many
            // small pieces, each of them nearly straight.
            let panels = if breaks.len() > MANY_PIECES { 1 } else { 2 };
            let mut area = 0.0;
            let mut volume = 0.0;
            let mut from = from;
            for &next in breaks.iter().skip(1).chain(std::iter::once(&to)) {
                let next = next.min(to);
                if next <= from {
                    continue;
                }
                let width = (next - from) / f64::from(panels);
                for panel in 0..panels {
                    let a = from + width * f64::from(panel);
                    for &(x, w) in &GAUSS {
                        let t = a + 0.5 * width * (x + 1.0);
                        let [at, su, sv, ..] = s.evaluate(DVec2::new(t, uv.y));
                        let n = su.cross(sv);
                        let weight = w * 0.5 * width;
                        area += n.length() * weight;
                        volume += (at - reference).dot(n) * weight;
                    }
                }
                from = next;
                if from >= to {
                    break;
                }
            }
            out.signed_area += area * step * w;
            out.volume += volume * step * w / 3.0;
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
        Surface::Nurbs(s) => {
            // Sampled over the face's part of the parameter rectangle; between samples
            // the surface leaves their chords by at most |S''| h² / 8.
            let Some(&outer) = f.loops.first() else {
                return b;
            };
            let m = loop_integrals(solid, outer, DVec3::ZERO);
            let (lo, hi) = (
                DVec2::new(m.theta_range.0, m.v_range.0),
                DVec2::new(m.theta_range.1, m.v_range.1),
            );
            if !(lo.x <= hi.x && lo.y <= hi.y) {
                return b;
            }
            for i in 0..=GRID {
                for j in 0..=GRID {
                    let at = DVec2::new(i as f64, j as f64) / GRID as f64;
                    b.extend(s.point(lo + (hi - lo) * at));
                }
            }
            let step = (hi - lo) / GRID as f64;
            let bend = s.bend();
            let margin = (bend.x * step.x * step.x + bend.y * step.y * step.y) / 8.0;
            return Aabb {
                min: b.min - DVec3::splat(margin),
                max: b.max + DVec3::splat(margin),
            };
        }
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
