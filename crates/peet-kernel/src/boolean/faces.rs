//! Tracing faces from a set of directed edges on one surface.
//!
//! The edges of a face (its boundary plus everything imprinted on it) form a planar graph
//! on the surface. Around each vertex the edge ends are ordered counter-clockwise (seen
//! from outside) by their tangent direction in the tangent plane, with tangent edges
//! ordered by geodesic curvature. Walking "arrive, then take the next edge clockwise from
//! the one just used" traces every region with the region on its left, which is exactly
//! the loop convention of [`crate::topo`]. Working in the tangent plane makes this the same
//! for planes and cylinders, with no parameter-space seams to worry about.

use std::collections::HashMap;
use std::f64::consts::TAU;

use peet_math::{DVec2, DVec3};

use peet_math::tolerance::LINEAR;

use super::domain::{Domain, ExactLoop, exact_winding, polygon_winding, signed_area};
use super::util::{GEdge, HalfEdge, bounded_distance, second_derivative};
use crate::geom::Curve3;

/// Edge ends leaving a vertex within this angle (radians) of each other are tangent, and
/// are ordered by curvature instead. Looser than `tolerance::ANGULAR` because tangent
/// directions of intersection curves are only as exact as the curves themselves.
const TANGENT_TIE: f64 = 1e-7;

/// How many interior points [`interior_points`] offers at most.
const MAX_SAMPLES: usize = 6;

/// How many boundary edges [`interior_points`] starts chords from, at first.
const MAX_STARTS: usize = 16;

/// A closed loop of half-edges with its image in the face's domain.
#[derive(Clone, Debug)]
pub(crate) struct TracedLoop {
    pub half_edges: Vec<HalfEdge>,
    pub poly: Vec<DVec2>,
    /// Index in `poly` of each half-edge's first point.
    pub starts: Vec<usize>,
    pub area: f64,
}

/// A region: an outer loop and its holes.
#[derive(Clone, Debug)]
pub(crate) struct TracedFace {
    pub outer: TracedLoop,
    pub holes: Vec<TracedLoop>,
}

/// One end of an edge at a vertex.
#[derive(Clone, Copy, Debug)]
struct End {
    edge: u32,
    /// The end at the edge's start (leaving forwards) or at its end (leaving backwards).
    at_start: bool,
    angle: f64,
    curvature: f64,
}

/// Removes edges that dangle: used in both directions with an end no other edge reaches.
/// They cannot separate two regions. Returns the remaining half-edges.
///
/// An end at a pole of the surface is not loose: a seam that runs up to a sphere's pole
/// or a cone's apex ends there, and the face wraps around that point.
pub(crate) fn prune_dangling(
    half_edges: &[HalfEdge],
    edges: &[GEdge],
    domain: &Domain,
    points: &[DVec3],
) -> Vec<HalfEdge> {
    let loose = |v: u32, degree: &HashMap<u32, u32>| {
        degree[&v] == 1 && domain.surface.pole_at(points[v as usize]).is_none()
    };
    let mut set: Vec<HalfEdge> = half_edges.to_vec();
    set.sort_unstable();
    set.dedup();
    loop {
        let mut degree: HashMap<u32, u32> = HashMap::new();
        let mut last = u32::MAX;
        for &(e, _) in &set {
            if e != last {
                let g = &edges[e as usize];
                *degree.entry(g.start).or_default() += 1;
                *degree.entry(g.end).or_default() += 1;
                last = e;
            }
        }
        let before = set.len();
        let both = |e: u32| {
            set.binary_search(&(e, false)).is_ok() && set.binary_search(&(e, true)).is_ok()
        };
        let dangling: Vec<u32> = set
            .iter()
            .map(|&(e, _)| e)
            .filter(|&e| {
                let g = &edges[e as usize];
                both(e) && g.start != g.end && (loose(g.start, &degree) || loose(g.end, &degree))
            })
            .collect();
        set.retain(|(e, _)| !dangling.contains(e));
        if set.len() == before {
            return set;
        }
    }
}

/// Traces the regions bounded by `half_edges` on the surface of `domain`. Every half-edge
/// ends up in exactly one loop; loops are grouped into outer loops with their holes.
pub(crate) fn trace_faces(
    domain: &Domain,
    half_edges: &[HalfEdge],
    edges: &[GEdge],
    points: &[DVec3],
) -> Result<Vec<TracedFace>, String> {
    // Edge ends around each vertex, counter-clockwise seen from outside.
    let mut around: HashMap<u32, Vec<End>> = HashMap::new();
    let mut seen_edges: Vec<u32> = half_edges.iter().map(|h| h.0).collect();
    seen_edges.sort_unstable();
    seen_edges.dedup();
    for &e in &seen_edges {
        let g = &edges[e as usize];
        for at_start in [true, false] {
            let v = if at_start { g.start } else { g.end };
            let p = points[v as usize];
            let normal = domain.outward(p);
            let t = if at_start { g.t0 } else { g.t1 };
            let d = g.curve.derivative(t);
            let speed2 = d.length_squared();
            let tangent = if at_start { d } else { -d }.normalize_or_zero();
            let left = normal.cross(tangent);
            let e1 = normal.any_orthonormal_vector();
            let e2 = normal.cross(e1);
            around.entry(v).or_default().push(End {
                edge: e,
                at_start,
                angle: tangent.dot(e2).atan2(tangent.dot(e1)),
                curvature: if speed2 > 0.0 {
                    second_derivative(&g.curve, t).dot(left) / speed2
                } else {
                    0.0
                },
            });
        }
    }
    let mut position: HashMap<(u32, bool), (u32, usize)> = HashMap::new();
    for (&v, ends) in &mut around {
        sort_ccw(ends, |e| e.angle, |e| e.curvature);
        for (i, e) in ends.iter().enumerate() {
            position.insert((e.edge, e.at_start), (v, i));
        }
    }

    let mut visited: HashMap<HalfEdge, bool> = half_edges.iter().map(|&h| (h, false)).collect();
    let mut order: Vec<HalfEdge> = visited.keys().copied().collect();
    order.sort_unstable();
    let mut loops: Vec<TracedLoop> = Vec::new();
    for &start in &order {
        if visited[&start] {
            continue;
        }
        let mut hes = Vec::new();
        let mut h = start;
        loop {
            match visited.get_mut(&h) {
                Some(v) if !*v => *v = true,
                Some(_) => return Err("a face boundary crosses itself".to_owned()),
                None => {
                    return Err(
                        "a face boundary runs into an edge it may not use in that direction"
                            .to_owned(),
                    );
                }
            }
            hes.push(h);
            // Arriving forwards we stand at the edge's end; the way back is that end.
            let (v, i) = position[&(h.0, !h.1)];
            let ends = &around[&v];
            let next = ends[(i + ends.len() - 1) % ends.len()];
            h = (next.edge, next.at_start);
            if h == start {
                break;
            }
            if hes.len() > half_edges.len() {
                return Err("a face boundary does not close".to_owned());
            }
        }
        let mut poly = Vec::new();
        let mut starts = Vec::with_capacity(hes.len());
        let directed = |&(e, forward): &HalfEdge| {
            let g = &edges[e as usize];
            if forward { (g.t0, g.t1) } else { (g.t1, g.t0) }
        };
        for h in &hes {
            let (ta, tb) = directed(h);
            domain.trace(&edges[h.0 as usize].curve, ta, tb, &mut poly);
            starts.push(poly.len() - 1 - domain.segments(&edges[h.0 as usize].curve, tb - ta));
        }
        // The trace returns to its start; on a curved surface it must not have gone around.
        let (ta, tb) = directed(&hes[0]);
        if domain
            .close(&edges[hes[0].0 as usize].curve, ta, tb, &mut poly)
            .is_err()
        {
            return Err("a face boundary wraps around a curved face without a seam".to_owned());
        }
        let area = signed_area(&poly);
        loops.push(TracedLoop {
            half_edges: hes,
            poly,
            starts,
            area,
        });
    }

    let (outers, holes): (Vec<TracedLoop>, Vec<TracedLoop>) =
        loops.into_iter().partition(|l| l.area > 0.0);
    let mut faces: Vec<TracedFace> = outers
        .into_iter()
        .map(|outer| TracedFace {
            outer,
            holes: Vec::new(),
        })
        .collect();
    for mut hole in holes {
        let probe = probe_point(&hole.poly);
        let mut best: Option<(f64, usize, DVec2)> = None;
        for (i, f) in faces.iter().enumerate() {
            // A loop sharing an edge with the hole lies on the other side of that edge:
            // it is inside the hole, not around it. (The probe is on that edge, where the
            // winding number would be a coin toss.)
            let inside_hole = f
                .outer
                .half_edges
                .iter()
                .any(|h| hole.half_edges.iter().any(|g| g.0 == h.0));
            if inside_hole {
                continue;
            }
            // An outer loop on a cylinder can be wider than one turn, so the hole may sit
            // a turn to either side of the nearest placement.
            let nearest = turn_shift(domain, &hole.poly, &f.outer.poly);
            let period = domain.period().unwrap_or(0.0);
            let turns: &[f64] = if domain.period().is_some() {
                &[0.0, -1.0, 1.0]
            } else {
                &[0.0]
            };
            for k in turns {
                let shift = nearest + DVec2::new(k * period, 0.0);
                if polygon_winding(&f.outer.poly, probe + shift) != 0
                    && best.is_none_or(|(a, _, _)| f.outer.area < a)
                {
                    best = Some((f.outer.area, i, shift));
                    break;
                }
            }
        }
        let Some((_, i, shift)) = best else {
            return Err("a hole lies outside every face region".to_owned());
        };
        for q in &mut hole.poly {
            *q += shift;
        }
        faces[i].holes.push(hole);
    }
    Ok(faces)
}

/// Sorts directions around a point counter-clockwise by `angle`. Directions tangent to
/// each other (within [`TANGENT_TIE`]) are ordered by `bend`: how fast each turns
/// counter-clockwise as it leaves, so the one bending left comes later.
pub(crate) fn sort_ccw<T>(items: &mut [T], angle: impl Fn(&T) -> f64, bend: impl Fn(&T) -> f64) {
    items.sort_by(|a, b| angle(a).total_cmp(&angle(b)));
    let n = items.len();
    if n < 2 {
        return;
    }
    // Start the cyclic order at a real gap, so tangent groups are contiguous.
    let gap = |i: usize| {
        let d = angle(&items[i]) - angle(&items[(i + n - 1) % n]);
        if i == 0 { d + TAU } else { d }
    };
    let Some(first) = (0..n).find(|&i| gap(i) > TANGENT_TIE) else {
        items.sort_by(|a, b| bend(a).total_cmp(&bend(b)));
        return;
    };
    items.rotate_left(first);
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && (angle(&items[j]) - angle(&items[j - 1])).rem_euclid(TAU) <= TANGENT_TIE {
            j += 1;
        }
        items[i..j].sort_by(|a, b| bend(a).total_cmp(&bend(b)));
        i = j;
    }
}

/// A point on a hole's boundary that is unlikely to touch another loop: the middle of its
/// longest side.
fn probe_point(poly: &[DVec2]) -> DVec2 {
    let mut best = (0.0, poly[0]);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let d = a.distance_squared(b);
        if d >= best.0 {
            best = (d, 0.5 * (a + b));
        }
    }
    best.1
}

/// The whole number of turns (as an offset) that brings `poly` next to `target`.
fn turn_shift(domain: &Domain, poly: &[DVec2], target: &[DVec2]) -> DVec2 {
    let center = |p: &[DVec2]| {
        let (lo, hi) = p.iter().fold(
            (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
            |(lo, hi), q| (lo.min(*q), hi.max(*q)),
        );
        0.5 * (lo + hi)
    };
    let gap = center(target) - center(poly);
    let turns = |gap: f64, period: Option<f64>| match period {
        Some(period) => (gap / period).round() * period,
        None => 0.0,
    };
    DVec2::new(
        turns(gap.x, domain.period()),
        turns(gap.y, domain.period_y()),
    )
}

/// Points inside a traced face, best first: as far as can be found from the face's
/// boundary and from `obstacles` (edges lying in the face that bound nothing, where the
/// other body merely touches it).
///
/// Candidates come from chords that start at the middle of a boundary edge and run
/// straight into the face (measured on the polygons in the domain). Each candidate is then
/// checked against the true curves: it must be inside by the exact winding number, and the
/// one with the largest exact distance from every edge wins. Regions too thin for their
/// polygons to mean anything (the cusp beside a tangency) are entered by stepping in from
/// the middle of each edge by ever smaller amounts.
///
/// Several are returned because the best one may still be unusable: the other body can
/// touch the face in a single point (two cylinders tangent to each other), and symmetric
/// parts tend to put that point right in the middle of a face.
pub(crate) fn interior_points(
    domain: &Domain,
    face: &TracedFace,
    edges: &[GEdge],
    obstacles: &[u32],
) -> Vec<DVec3> {
    let loops: Vec<&TracedLoop> = std::iter::once(&face.outer).chain(&face.holes).collect();
    let exact: Vec<ExactLoop> = loops
        .iter()
        .map(|l| {
            let parts: Vec<(Curve3, f64, f64)> = l
                .half_edges
                .iter()
                .map(|&(e, forward)| {
                    let g = &edges[e as usize];
                    if forward {
                        (g.curve, g.t0, g.t1)
                    } else {
                        (g.curve, g.t1, g.t0)
                    }
                })
                .collect();
            ExactLoop::new(domain, &parts, Some(l.poly[0]))
        })
        .collect();
    let mut near: Vec<u32> = loops
        .iter()
        .flat_map(|l| l.half_edges.iter().map(|h| h.0))
        .chain(obstacles.iter().copied())
        .collect();
    near.sort_unstable();
    near.dedup();
    // Exact distance from the nearest edge, or `None` for a point outside the region.
    let clearance = |q: DVec2| -> Option<f64> {
        if exact_winding(domain, &exact, q) == 0 {
            return None;
        }
        let p = domain.point(q);
        Some(near.iter().fold(f64::INFINITY, |d, &e| {
            let g = &edges[e as usize];
            d.min(bounded_distance(&g.curve, g.t0, g.t1, p).0)
        }))
    };

    let mut walls: Vec<Vec<DVec2>> = loops
        .iter()
        .map(|l| {
            let mut closed = l.poly.clone();
            closed.push(l.poly[0]);
            closed
        })
        .collect();
    for &e in obstacles {
        let g = &edges[e as usize];
        let mut poly = Vec::new();
        domain.trace(&g.curve, g.t0, g.t1, &mut poly);
        let shift = turn_shift(domain, &poly, &face.outer.poly);
        // On a cylinder the obstacle may straddle the region's window: add both turns.
        let turns: &[f64] = if domain.period().is_some() {
            &[-1.0, 0.0, 1.0]
        } else {
            &[0.0]
        };
        for k in turns {
            let by = shift + DVec2::new(k * domain.period().unwrap_or(0.0), 0.0);
            walls.push(poly.iter().map(|q| *q + by).collect());
        }
    }
    let scale = face
        .outer
        .poly
        .iter()
        .fold(0.0_f64, |m, q| m.max((*q - face.outer.poly[0]).length()))
        .max(f64::MIN_POSITIVE);

    // The middle of each boundary edge's polygon, with the direction into the face.
    let mut starts: Vec<(DVec2, DVec2)> = Vec::new();
    for l in &loops {
        let poly = &l.poly;
        for (&(e, _), &start) in l.half_edges.iter().zip(&l.starts) {
            let g = &edges[e as usize];
            let n = domain.segments(&g.curve, g.t1 - g.t0);
            let i = start + n / 2;
            let (a, b) = (poly[i % poly.len()], poly[(i + 1) % poly.len()]);
            let side = b - a;
            let len = side.length();
            if len > 1e-9 * scale {
                starts.push((0.5 * (a + b), side.perp() / len));
            }
        }
    }

    // Chord candidates with the clearance their chord promises (the distance to its ends),
    // most promising first. The exact check is the expensive part, so it stops once
    // enough candidates have passed.
    let chords = |starts: &mut dyn Iterator<Item = &(DVec2, DVec2)>| {
        let mut hopefuls: Vec<(f64, DVec2)> = Vec::new();
        for &(origin, inward) in starts {
            let mut nearest = f64::INFINITY;
            for w in &walls {
                for s in w.windows(2) {
                    if let Some(t) = ray_segment(origin, inward, s[0], s[1])
                        && t > 1e-9 * scale
                    {
                        nearest = nearest.min(t);
                    }
                }
            }
            if nearest.is_finite() {
                for fraction in [0.5_f64, 0.25, 0.75] {
                    let promise = fraction.min(1.0 - fraction) * nearest;
                    hopefuls.push((promise, origin + inward * (fraction * nearest)));
                }
            }
        }
        hopefuls.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut found: Vec<(f64, DVec2)> = Vec::new();
        for &(_, q) in &hopefuls {
            if let Some(c) = clearance(q) {
                found.push((c, q));
                if found.len() >= 2 * MAX_SAMPLES {
                    break;
                }
            }
        }
        found
    };
    // A face with many edges (a plate full of holes) doesn't need a chord from each.
    let stride = starts.len().div_ceil(MAX_STARTS).max(1);
    let poor = |found: &[(f64, DVec2)]| found.iter().all(|&(c, _)| c < 1e-4 * scale);
    let mut found = chords(&mut starts.iter().step_by(stride));
    if stride > 1 && poor(&found) {
        found = chords(&mut starts.iter());
    }
    // Good enough when comfortably clear; otherwise look harder for a thin region.
    if poor(&found) {
        for &(origin, inward) in &starts {
            let mut step = 0.25 * scale;
            while step > LINEAR {
                if let Some(c) = clearance(origin + inward * step) {
                    found.push((c, origin + inward * step));
                }
                step *= 0.5;
            }
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    if found.is_empty() {
        found.push((0.0, face.outer.poly[0]));
    }
    // A handful, spread out: skip candidates close to one already taken.
    let mut picked: Vec<(f64, DVec2)> = Vec::new();
    for (c, q) in found {
        if picked.len() < MAX_SAMPLES && picked.iter().all(|(_, o)| o.distance(q) > 0.25 * c) {
            picked.push((c, q));
        }
    }
    picked.into_iter().map(|(_, q)| domain.point(q)).collect()
}

/// Distance along the ray `o + t·d` to segment `a..b`, if it hits.
fn ray_segment(o: DVec2, d: DVec2, a: DVec2, b: DVec2) -> Option<f64> {
    let s = b - a;
    let denom = d.perp_dot(s);
    if denom.abs() <= 1e-300 {
        return None;
    }
    let w = a - o;
    let t = w.perp_dot(s) / denom;
    let u = w.perp_dot(d) / denom;
    // Generous at the ends: a ray through a corner must not slip between two sides. An
    // extra hit only shortens the chord, which is always safe.
    (t >= 0.0 && (-1e-9..=1.0 + 1e-9).contains(&u)).then_some(t)
}
