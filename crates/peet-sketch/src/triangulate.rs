//! Triangulation of sketch regions (polygons with holes), for shading profiles.
//!
//! Loops are flattened to polylines within a chord tolerance, holes are joined to the
//! outer boundary by bridge edges (Eberly, "Triangulation by Ear Clipping"), and the
//! resulting simple polygon is ear clipped. `O(n²)` in the number of polyline vertices,
//! which is fine for sketch profiles (a few thousand vertices at most), computed once per
//! edit rather than per frame.

use peet_math::DVec2;

use crate::region::{Loop, Region};

/// A triangle mesh in sketch coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Triangles {
    pub points: Vec<DVec2>,
    /// Counter-clockwise triangles, as indices into `points`.
    pub triangles: Vec<[u32; 3]>,
}

impl Triangles {
    /// Total area of the triangles.
    pub fn area(&self) -> f64 {
        self.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.points[i as usize]);
                (b - a).perp_dot(c - a) * 0.5
            })
            .sum()
    }
}

/// Flattens a loop to a polyline (without repeating the first point) within `tolerance`.
pub fn loop_polyline(l: &Loop, tolerance: f64) -> Vec<DVec2> {
    let mut pts: Vec<DVec2> = Vec::new();
    for e in &l.edges {
        let mut p = e.curve.tessellate(tolerance);
        if e.reversed {
            p.reverse();
        }
        p.pop();
        for q in p {
            if pts.last().is_none_or(|last| last.distance_squared(q) > 0.0) {
                pts.push(q);
            }
        }
    }
    while pts.len() > 1 && pts[0] == *pts.last().expect("non-empty") {
        pts.pop();
    }
    pts
}

fn signed_area(p: &[DVec2]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum::<f64>() * 0.5
}

/// Triangulates a region (outer boundary minus holes).
pub fn triangulate_region(region: &Region, tolerance: f64) -> Triangles {
    let mut outer = loop_polyline(&region.outer, tolerance);
    if outer.len() < 3 {
        return Triangles::default();
    }
    if signed_area(&outer) < 0.0 {
        outer.reverse();
    }
    let mut holes: Vec<Vec<DVec2>> = region
        .holes
        .iter()
        .map(|h| {
            let mut p = loop_polyline(h, tolerance);
            if signed_area(&p) > 0.0 {
                p.reverse();
            }
            p
        })
        .filter(|h| h.len() >= 3)
        .collect();
    // Bridge holes from the rightmost one inwards, so bridges never cross each other.
    holes.sort_by(|a, b| max_x(b).total_cmp(&max_x(a)));
    let mut poly = outer;
    for hole in holes {
        poly = bridge(&poly, &hole);
    }
    ear_clip(poly)
}

fn max_x(p: &[DVec2]) -> f64 {
    p.iter().map(|q| q.x).fold(f64::NEG_INFINITY, f64::max)
}

/// Joins a clockwise hole into a counter-clockwise polygon with a pair of bridge edges.
fn bridge(poly: &[DVec2], hole: &[DVec2]) -> Vec<DVec2> {
    let (m_idx, m) = hole
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.x.total_cmp(&b.1.x))
        .expect("hole has points");
    // Nearest edge hit by the ray from M towards +x.
    let n = poly.len();
    let mut best: Option<(f64, usize)> = None;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > m.y) == (b.y > m.y) {
            continue;
        }
        let x = a.x + (m.y - a.y) / (b.y - a.y) * (b.x - a.x);
        if x >= m.x && best.is_none_or(|(bx, _)| x < bx) {
            best = Some((x, i));
        }
    }
    let Some((x, i)) = best else {
        return poly.to_vec(); // hole not inside: ignore it
    };
    let hit = DVec2::new(x, m.y);
    let j = (i + 1) % n;
    let cross = |a: DVec2, b: DVec2, c: DVec2| (b - a).perp_dot(c - a);
    // Whether `m` is inside the polygon's interior wedge at position `k`. A vertex already
    // used by a bridge occurs several times in the polygon, each with its own wedge; only
    // the occurrence whose wedge contains `m` gives a bridge that doesn't cross the others.
    let sees_m = |k: usize| {
        let q = poly[k];
        let prev = poly[(k + n - 1) % n];
        let next = poly[(k + 1) % n];
        let (left_of_out, left_of_in) = (cross(q, next, m) >= 0.0, cross(prev, q, m) >= 0.0);
        if cross(prev, q, next) > 0.0 {
            left_of_out && left_of_in
        } else {
            left_of_out || left_of_in
        }
    };
    let endpoint = if poly[i].x > poly[j].x { i } else { j };
    let p = poly[endpoint];
    let mut p_idx = (0..n)
        .find(|&k| poly[k] == p && sees_m(k))
        .unwrap_or(endpoint);
    // A reflex vertex inside triangle (M, hit, P) would block the bridge: take the one
    // closest in angle to the ray instead.
    let mut best_angle = f64::INFINITY;
    let mut best_dist = f64::INFINITY;
    for k in 0..n {
        let q = poly[k];
        if q == p {
            continue;
        }
        let prev = poly[(k + n - 1) % n];
        let next = poly[(k + 1) % n];
        let reflex = cross(prev, q, next) <= 0.0;
        if reflex && q.x >= m.x && point_in_triangle(q, m, hit, p) && sees_m(k) {
            let d = q - m;
            let angle = (d.y / d.length().max(f64::MIN_POSITIVE)).abs();
            let dist = d.length_squared();
            if angle < best_angle || (angle == best_angle && dist < best_dist) {
                best_angle = angle;
                best_dist = dist;
                p_idx = k;
            }
        }
    }
    let mut out = Vec::with_capacity(poly.len() + hole.len() + 2);
    out.extend_from_slice(&poly[..=p_idx]);
    out.extend(hole[m_idx..].iter().chain(&hole[..=m_idx]));
    out.extend_from_slice(&poly[p_idx..]);
    out
}

fn point_in_triangle(p: DVec2, a: DVec2, b: DVec2, c: DVec2) -> bool {
    let d1 = (b - a).perp_dot(p - a);
    let d2 = (c - b).perp_dot(p - b);
    let d3 = (a - c).perp_dot(p - c);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Ear clipping of a counter-clockwise simple polygon (bridge edges allowed).
fn ear_clip(points: Vec<DVec2>) -> Triangles {
    let mut idx: Vec<u32> = (0..points.len() as u32).collect();
    let mut triangles = Vec::with_capacity(points.len().saturating_sub(2));
    let mut guard = 0;
    while idx.len() > 3 {
        let n = idx.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let (a, b, c) = (
                points[ia as usize],
                points[ib as usize],
                points[ic as usize],
            );
            if (b - a).perp_dot(c - b) <= 0.0 {
                continue; // reflex or degenerate
            }
            let blocked = idx.iter().any(|&k| {
                let q = points[k as usize];
                k != ia
                    && k != ib
                    && k != ic
                    && q != a
                    && q != b
                    && q != c
                    && point_in_triangle(q, a, b, c)
            });
            if blocked {
                continue;
            }
            triangles.push([ia, ib, ic]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Numerically degenerate leftovers (collinear runs): drop the flattest vertex.
            guard += 1;
            if guard > points.len() {
                break;
            }
            let flattest = (0..n)
                .min_by(|&x, &y| {
                    let turn = |i: usize| {
                        let (a, b, c) = (
                            points[idx[(i + n - 1) % n] as usize],
                            points[idx[i] as usize],
                            points[idx[(i + 1) % n] as usize],
                        );
                        (b - a).perp_dot(c - b).abs()
                    };
                    turn(x).total_cmp(&turn(y))
                })
                .expect("n > 3");
            idx.remove(flattest);
        }
    }
    if idx.len() == 3 {
        let [a, b, c] = [idx[0], idx[1], idx[2]].map(|i| points[i as usize]);
        if (b - a).perp_dot(c - b) > 0.0 {
            triangles.push([idx[0], idx[1], idx[2]]);
        }
    }
    Triangles { points, triangles }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sketch;
    use crate::region::find_regions;

    fn rect(s: &mut Sketch, a: DVec2, b: DVec2) {
        crate::shapes::rectangle(s, a, b);
    }

    #[test]
    fn rectangle_with_holes() {
        let mut s = Sketch::new();
        rect(&mut s, DVec2::ZERO, DVec2::new(100.0, 50.0));
        s.add_circle(DVec2::new(20.0, 25.0), 10.0);
        rect(&mut s, DVec2::new(50.0, 10.0), DVec2::new(80.0, 40.0));
        let profile = find_regions(&s);
        let plate = profile
            .regions
            .iter()
            .find(|r| r.holes.len() == 2)
            .expect("plate region");
        let tris = triangulate_region(plate, 0.01);
        let expected = plate.area();
        assert!(
            (tris.area() - expected).abs() < expected * 1e-3,
            "{} vs {expected}",
            tris.area()
        );
        for t in &tris.triangles {
            let [a, b, c] = t.map(|i| tris.points[i as usize]);
            assert!((b - a).perp_dot(c - a) > 0.0, "counter-clockwise");
        }
    }

    #[test]
    fn many_holes_bridge_without_crossing() {
        let mut s = Sketch::new();
        rect(&mut s, DVec2::ZERO, DVec2::new(100.0, 60.0));
        for ix in 0..4 {
            for iy in 0..3 {
                s.add_circle(
                    DVec2::new(15.0 + 22.0 * f64::from(ix), 12.0 + 18.0 * f64::from(iy)),
                    5.0,
                );
            }
        }
        let profile = find_regions(&s);
        let plate = profile
            .regions
            .iter()
            .find(|r| r.holes.len() == 12)
            .expect("plate region");
        let tris = triangulate_region(plate, 0.01);
        let expected = plate.area();
        assert!(
            // Circles are flattened to polygons, which accounts for about 0.05%.
            (tris.area() - expected).abs() < expected * 1e-3,
            "{} vs {expected}",
            tris.area()
        );
        for t in &tris.triangles {
            let [a, b, c] = t.map(|i| tris.points[i as usize]);
            assert!((b - a).perp_dot(c - a) > 0.0);
        }
    }

    #[test]
    fn concave_outline() {
        let mut s = Sketch::new();
        // An L shape.
        let pts = [
            DVec2::ZERO,
            DVec2::new(80.0, 0.0),
            DVec2::new(80.0, 10.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(10.0, 60.0),
            DVec2::new(0.0, 60.0),
        ];
        for i in 0..pts.len() {
            s.add_line(pts[i], pts[(i + 1) % pts.len()]);
        }
        let profile = find_regions(&s);
        assert_eq!(profile.regions.len(), 1);
        let tris = triangulate_region(&profile.regions[0], 0.01);
        assert_eq!(tris.triangles.len(), 4);
        assert!((tris.area() - 1300.0).abs() < 1e-9);
    }
}
