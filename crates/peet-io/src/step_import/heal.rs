//! Healing of tolerant edges: edges a file writes a little off their faces.
//!
//! Parasolid (and so SolidWorks) keeps an edge whose curve and faces disagree by more
//! than its precision as a *tolerant* edge: the edge carries its own tolerance, and the
//! curve written to the file only comes within that of the faces (a cubic standing in
//! for the arc where a plane meets a cone, the boundary of a B-spline patch that sags
//! inside the cylinder next to it). The kernel's edges are exact, to within
//! [`EDGE_ON_SURFACE`]. An edge off its faces by more than that but no more than
//! [`HEAL_LIMIT`] is given a new curve: a cubic B-spline fitted, over the same parameter
//! range and between the same vertices, through points moved from the old curve onto
//! both faces' surfaces. Edges further off, or whose new curve would not pass through
//! their vertices, are left as they are, and validation refuses the body as before.

use std::sync::Arc;

use peet_kernel::geom::{Curve3, Surface};
use peet_kernel::nurbs::NurbsCurve;
use peet_kernel::topo::Solid;
use peet_kernel::validate::{EDGE_ON_SURFACE, VERTEX_ON_CURVE};
use peet_math::DVec3;

use super::HEAL_LIMIT;

/// How closely (mm) a healed edge follows its faces: well inside what validation allows.
pub(super) const HEAL_FIT: f64 = 0.1 * EDGE_ON_SURFACE;

/// How many pieces an edge is cut into to measure how far it is from its faces.
const DRIFT_SAMPLES: usize = 32;

/// Steps towards the surfaces, at most, for each point of a healed edge.
const STEPS: usize = 32;

/// Below this `1 - cos²` of the angle between two faces' normals the faces are taken to
/// touch (about 0.06°).
const TANGENT: f64 = 1e-6;

/// The edges of a body that were healed: how many, and the furthest any was off (mm).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Healed {
    pub edges: usize,
    pub worst: f64,
}

/// Heals the edges of `solid` that lie off their faces by more than validation allows
/// but no more than [`HEAL_LIMIT`]. Edges that can't be healed are left alone.
pub(super) fn heal_edges(solid: &mut Solid) -> Healed {
    let mut healed = Healed::default();
    for i in 0..solid.edges.len() {
        let e = &solid.edges[i];
        let (t0, t1) = (e.t0, e.t1);
        if !(t0.is_finite() && t1.is_finite() && t0 < t1) {
            continue;
        }
        let mut surfaces: Vec<&Surface> = Vec::new();
        for &c in &e.coedges {
            let s = &solid.face(solid.coedge_face(c)).surface;
            if !surfaces.contains(&s) {
                surfaces.push(s);
            }
        }
        if surfaces.is_empty() || surfaces.len() > 2 {
            continue;
        }
        // Validation looks at a few points of each edge: an edge it passes stays as it is.
        let passes = [0.25, 0.5, 0.75]
            .iter()
            .all(|&k| distance(e.point_at_fraction(k), &surfaces) <= EDGE_ON_SURFACE);
        if passes {
            continue;
        }
        let off = drift(&e.curve, t0, t1, &surfaces);
        if off > HEAL_LIMIT {
            continue;
        }
        let curve = &e.curve;
        let shape = |t: f64| Some(onto(curve.point(t), &surfaces));
        let miss = |_: f64, p: DVec3| Some(distance(p, &surfaces));
        let Ok(fit) = NurbsCurve::fitted_within(&shape, &miss, &[t0, t1], 3, HEAL_FIT) else {
            continue;
        };
        let new = Curve3::Nurbs(Arc::new(fit));
        // The vertices stay where they are, so the new curve has to pass by them.
        if new.point(t0).distance(solid.vertex(e.start).point) > VERTEX_ON_CURVE
            || new.point(t1).distance(solid.vertex(e.end).point) > VERTEX_ON_CURVE
            || drift(&new, t0, t1, &surfaces) > 0.5 * EDGE_ON_SURFACE
        {
            continue;
        }
        solid.edges[i].curve = new;
        healed.edges += 1;
        healed.worst = healed.worst.max(off);
    }
    healed
}

/// How far (mm) `p` is from the furthest of `surfaces`.
pub(super) fn distance(p: DVec3, surfaces: &[&Surface]) -> f64 {
    surfaces
        .iter()
        .map(|s| s.signed_distance(p).abs())
        .fold(0.0, f64::max)
}

/// How far (mm) `curve` strays from `surfaces` over `t0..=t1`.
pub(super) fn drift(curve: &Curve3, t0: f64, t1: f64, surfaces: &[&Surface]) -> f64 {
    (0..=DRIFT_SAMPLES)
        .map(|k| curve.point(t0 + (t1 - t0) * k as f64 / DRIFT_SAMPLES as f64))
        .map(|p| distance(p, surfaces))
        .fold(0.0, f64::max)
}

/// The point on `surfaces` (one or two) near `p`: moved along their normals by Newton
/// steps on their signed distances until it lies on them to well within [`HEAL_FIT`].
/// Moving along the normals, rather than to each surface's closest point in turn, lets
/// the point go a little past the end of a B-spline surface whose boundary is the edge
/// (where its closest point would stop): the signed distance along the normal, which is
/// what validation measures, stays zero there.
pub(super) fn onto(p: DVec3, surfaces: &[&Surface]) -> DVec3 {
    let mut q = p;
    for _ in 0..STEPS {
        if distance(q, surfaces) <= 0.01 * HEAL_FIT {
            break;
        }
        q -= match surfaces {
            [a, b] => {
                let (da, db) = (a.signed_distance(q), b.signed_distance(q));
                let (na, nb) = (a.normal_at(q), b.normal_at(q));
                // The step `x·na + y·nb` that takes both distances to zero, to first order.
                let c = na.dot(nb);
                let det = 1.0 - c * c;
                if det > TANGENT {
                    na * ((da - c * db) / det) + nb * ((db - c * da) / det)
                } else {
                    // The faces touch here: halfway between them.
                    (na * da + nb * db) * 0.5
                }
            }
            _ => surfaces[0].normal_at(q) * surfaces[0].signed_distance(q),
        };
    }
    q
}
