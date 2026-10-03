//! Triangulation of polygons with holes in a face's (metric) parameter space: hole
//! bridging plus ear clipping, Lawson flips towards a constrained Delaunay triangulation,
//! and splitting of interior edges that are too long in `u` (for cylinders).
//!
//! Boundary vertices are never dropped or moved, so the mesh boundary is exactly the input
//! polylines (no cracks against neighbouring faces).

use std::collections::HashMap;

use peet_math::DVec2;

/// Polygon with holes: `loops[0]` is the outer boundary (counter-clockwise), the others
/// holes (clockwise), as index ranges into `points`.
pub(super) struct Polygon<'a> {
    pub points: &'a [DVec2],
    pub loops: &'a [std::ops::Range<usize>],
}

/// Triangulates the polygon. Triangles are counter-clockwise, indices into `points`.
pub(super) fn triangulate(poly: &Polygon) -> Vec<[u32; 3]> {
    let pts = poly.points;
    let Some(outer) = poly.loops.first() else {
        return Vec::new();
    };
    if outer.len() < 3 {
        return Vec::new();
    }
    let mut ring: Vec<u32> = (outer.start as u32..outer.end as u32).collect();
    let mut holes: Vec<Vec<u32>> = poly.loops[1..]
        .iter()
        .filter(|r| r.len() >= 3)
        .map(|r| (r.start as u32..r.end as u32).collect())
        .collect();
    // Bridge holes from the rightmost one inwards, so bridges never cross each other.
    let max_x = |h: &Vec<u32>| {
        h.iter()
            .map(|&i| pts[i as usize].x)
            .fold(f64::NEG_INFINITY, f64::max)
    };
    holes.sort_by(|a, b| max_x(b).total_cmp(&max_x(a)));
    for hole in &holes {
        ring = bridge(pts, &ring, hole);
    }
    ear_clip(pts, &ring)
}

fn cross(o: DVec2, a: DVec2, b: DVec2) -> f64 {
    (a - o).perp_dot(b - o)
}

/// Joins a clockwise hole into a counter-clockwise ring with a pair of bridge edges
/// (Eberly, "Triangulation by Ear Clipping").
fn bridge(pts: &[DVec2], ring: &[u32], hole: &[u32]) -> Vec<u32> {
    let p = |i: u32| pts[i as usize];
    let (m_pos, m) = hole
        .iter()
        .enumerate()
        .max_by(|a, b| p(*a.1).x.total_cmp(&p(*b.1).x))
        .map(|(k, &i)| (k, p(i)))
        .expect("hole has points");
    let n = ring.len();
    let mut best: Option<(f64, usize)> = None;
    for i in 0..n {
        let (a, b) = (p(ring[i]), p(ring[(i + 1) % n]));
        if (a.y > m.y) == (b.y > m.y) {
            continue;
        }
        let x = a.x + (m.y - a.y) / (b.y - a.y) * (b.x - a.x);
        if x >= m.x && best.is_none_or(|(bx, _)| x < bx) {
            best = Some((x, i));
        }
    }
    let Some((x, i)) = best else {
        return ring.to_vec(); // hole not inside the ring: ignore it
    };
    let hit = DVec2::new(x, m.y);
    let j = (i + 1) % n;
    // Whether `m` is inside the ring's interior wedge at ring position `k`. A vertex already
    // used by a bridge occurs several times in the ring, each with its own wedge; only the
    // occurrence whose wedge contains `m` gives a bridge that doesn't cross the others.
    let sees_m = |k: usize| {
        let q = p(ring[k]);
        let prev = p(ring[(k + n - 1) % n]);
        let next = p(ring[(k + 1) % n]);
        let (left_of_out, left_of_in) = (cross(q, next, m) >= 0.0, cross(prev, q, m) >= 0.0);
        if cross(prev, q, next) > 0.0 {
            left_of_out && left_of_in
        } else {
            left_of_out || left_of_in
        }
    };
    let endpoint = if p(ring[i]).x > p(ring[j]).x { i } else { j };
    let candidate = p(ring[endpoint]);
    let mut k_best = (0..n)
        .find(|&k| p(ring[k]) == candidate && sees_m(k))
        .unwrap_or(endpoint);
    // A reflex vertex inside triangle (M, hit, P) would block the bridge: take the one
    // closest in angle to the ray instead.
    let mut best_key = (f64::INFINITY, f64::INFINITY);
    for k in 0..n {
        let q = p(ring[k]);
        if q == candidate {
            continue;
        }
        let prev = p(ring[(k + n - 1) % n]);
        let next = p(ring[(k + 1) % n]);
        let reflex = cross(prev, q, next) <= 0.0;
        if reflex && q.x >= m.x && in_triangle(q, m, hit, candidate) && sees_m(k) {
            let d = q - m;
            let key = (
                (d.y / d.length().max(f64::MIN_POSITIVE)).abs(),
                d.length_squared(),
            );
            if key < best_key {
                best_key = key;
                k_best = k;
            }
        }
    }
    let mut out = Vec::with_capacity(n + hole.len() + 2);
    out.extend_from_slice(&ring[..=k_best]);
    out.extend(hole[m_pos..].iter().chain(&hole[..=m_pos]));
    out.extend_from_slice(&ring[k_best..]);
    out
}

/// Whether `q` is inside or on triangle `abc` (either orientation).
fn in_triangle(q: DVec2, a: DVec2, b: DVec2, c: DVec2) -> bool {
    let d1 = cross(a, b, q);
    let d2 = cross(b, c, q);
    let d3 = cross(c, a, q);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Corners turning by less than this (as a sine) are treated as straight: parameter-space
/// coordinates of collinear boundary points carry rounding noise, and an "ear" whose area
/// is only noise would hide the real ears behind it.
const FLAT: f64 = 1e-9;

/// Sine of the turn at `b` going `a → b → c` (positive to the left); 0 for coincident points.
fn turn(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    let scale = (b - a).length() * (c - b).length();
    if scale > 0.0 {
        cross(a, b, c) / scale
    } else {
        0.0
    }
}

/// Ear clipping of a counter-clockwise ring (bridge edges allowed, so indices may repeat).
fn ear_clip(pts: &[DVec2], ring: &[u32]) -> Vec<[u32; 3]> {
    let n = ring.len();
    let mut tris = Vec::with_capacity(n.saturating_sub(2));
    if n < 3 {
        return tris;
    }
    let p = |k: usize| pts[ring[k] as usize];
    let mut next: Vec<usize> = (0..n).map(|k| (k + 1) % n).collect();
    let mut prev: Vec<usize> = (0..n).map(|k| (k + n - 1) % n).collect();
    let mut alive = n;
    // Reflex (or flat) vertices are the only ones that can block an ear.
    let mut reflex: Vec<bool> = (0..n)
        .map(|k| turn(p(prev[k]), p(k), p(next[k])) <= FLAT)
        .collect();
    let is_ear = |k: usize, next: &[usize], prev: &[usize], reflex: &[bool], strict: bool| {
        let (ia, ic) = (prev[k], next[k]);
        let (a, b, c) = (p(ia), p(k), p(ic));
        let t = turn(a, b, c);
        if (strict && t <= FLAT) || t < -FLAT {
            return false;
        }
        let mut q = next[ic];
        while q != ia {
            if reflex[q] {
                let v = p(q);
                if v != a && v != b && v != c && in_triangle(v, a, b, c) {
                    return false;
                }
            }
            q = next[q];
        }
        true
    };
    let mut k = 0;
    let mut misses = 0;
    let mut strict = true;
    let mut zigzag = true;
    while alive > 3 {
        if is_ear(k, &next, &prev, &reflex, strict) {
            let (ia, ic) = (prev[k], next[k]);
            tris.push([ring[ia], ring[k], ring[ic]]);
            next[ia] = ic;
            prev[ic] = ia;
            alive -= 1;
            for v in [ia, ic] {
                reflex[v] = turn(p(prev[v]), p(v), p(next[v])) <= FLAT;
            }
            // Continue alternately before and after the clipped ear: that zigzags across
            // strips instead of fanning out from one vertex, which leaves far fewer flips
            // for the Delaunay pass.
            k = if zigzag { ia } else { ic };
            zigzag = !zigzag;
            misses = 0;
            strict = true;
            continue;
        }
        k = next[k];
        misses += 1;
        if misses > alive {
            if strict {
                // No strict ear: allow flat ones (collinear runs).
                strict = false;
                misses = 0;
                continue;
            }
            // Degenerate input: clip the most convex vertex anyway so we always finish.
            let mut best = k;
            let mut best_turn = f64::NEG_INFINITY;
            let mut q = k;
            for _ in 0..alive {
                let t = turn(p(prev[q]), p(q), p(next[q]));
                if t > best_turn {
                    best_turn = t;
                    best = q;
                }
                q = next[q];
            }
            let (ia, ic) = (prev[best], next[best]);
            if best_turn >= -FLAT {
                tris.push([ring[ia], ring[best], ring[ic]]);
            }
            next[ia] = ic;
            prev[ic] = ia;
            alive -= 1;
            for v in [ia, ic] {
                reflex[v] = turn(p(prev[v]), p(v), p(next[v])) <= FLAT;
            }
            k = ic;
            misses = 0;
            strict = true;
        }
    }
    let a = prev[k];
    let c = next[k];
    if turn(p(a), p(k), p(c)) >= -FLAT {
        tris.push([ring[a], ring[k], ring[c]]);
    }
    tris
}

/// A triangle whose area is below this fraction of its edge length squared counts as
/// degenerate.
const SLIVER: f64 = 1e-12;

/// A cheap hasher for directed-edge keys (two `u32`s): the default SipHash dominates the
/// flip loop otherwise.
#[derive(Clone, Copy, Default)]
struct EdgeHasher(u64);

impl std::hash::Hasher for EdgeHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u32(u32::from(b));
        }
    }

    fn write_u32(&mut self, v: u32) {
        self.0 = (self.0.rotate_left(5) ^ u64::from(v)).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

type EdgeMap = HashMap<(u32, u32), usize, std::hash::BuildHasherDefault<EdgeHasher>>;

/// A triangle mesh in 2D with edge adjacency, for flips and splits.
pub(super) struct Mesh2 {
    pub points: Vec<DVec2>,
    pub tris: Vec<[u32; 3]>,
    /// Directed edge `(a, b)` → triangle containing it.
    edges: EdgeMap,
}

impl Mesh2 {
    pub fn new(points: Vec<DVec2>, tris: Vec<[u32; 3]>) -> Self {
        let mut edges = EdgeMap::with_capacity_and_hasher(tris.len() * 3, Default::default());
        for (t, tri) in tris.iter().enumerate() {
            for k in 0..3 {
                edges.insert((tri[k], tri[(k + 1) % 3]), t);
            }
        }
        Self {
            points,
            tris,
            edges,
        }
    }

    fn p(&self, i: u32) -> DVec2 {
        self.points[i as usize]
    }

    /// Third vertex of triangle `t` opposite directed edge `(a, b)`.
    fn opposite(&self, t: usize, a: u32, b: u32) -> u32 {
        let tri = self.tris[t];
        tri.into_iter()
            .find(|&v| v != a && v != b)
            .unwrap_or(tri[0])
    }

    fn set_tri(&mut self, t: usize, tri: [u32; 3]) {
        let old = self.tris[t];
        for k in 0..3 {
            let e = (old[k], old[(k + 1) % 3]);
            if self.edges.get(&e) == Some(&t) {
                self.edges.remove(&e);
            }
        }
        self.tris[t] = tri;
        for k in 0..3 {
            self.edges.insert((tri[k], tri[(k + 1) % 3]), t);
        }
    }

    fn push_tri(&mut self, tri: [u32; 3]) {
        self.tris.push([0; 3]);
        let t = self.tris.len() - 1;
        self.tris[t] = tri;
        for k in 0..3 {
            self.edges.insert((tri[k], tri[(k + 1) % 3]), t);
        }
    }

    /// Lawson flips towards a (constrained) Delaunay triangulation. Boundary edges (with
    /// one triangle) are never flipped. `allow(a, b)` can veto new diagonals.
    pub fn delaunay(&mut self, allow: impl Fn(DVec2, DVec2) -> bool) {
        let mut stack: Vec<(u32, u32)> =
            self.edges.keys().copied().filter(|&(a, b)| a < b).collect();
        // Lawson's algorithm needs O(n²) flips in the worst case (a fan to a strip).
        let mut budget = self.tris.len() * self.tris.len() + 1000;
        while let Some((a, b)) = stack.pop() {
            if budget == 0 {
                break;
            }
            let (Some(&t1), Some(&t2)) = (self.edges.get(&(a, b)), self.edges.get(&(b, a))) else {
                continue;
            };
            let c = self.opposite(t1, a, b);
            let d = self.opposite(t2, b, a);
            if c == d {
                continue;
            }
            let (pa, pb, pc, pd) = (self.p(a), self.p(b), self.p(c), self.p(d));
            // The flipped triangles must both be properly counter-clockwise.
            if cross(pa, pd, pc) <= 0.0 || cross(pd, pb, pc) <= 0.0 {
                continue;
            }
            // Zero-area triangles (collinear boundary points) are always worth removing;
            // otherwise flip only towards Delaunay.
            let sliver =
                |x: DVec2, y: DVec2, z: DVec2| cross(x, y, z) <= SLIVER * (y - x).length_squared();
            let degenerate = sliver(pa, pb, pc) || sliver(pb, pa, pd);
            if !(degenerate || in_circle(pa, pb, pc, pd)) || !allow(pc, pd) {
                continue;
            }
            if self.edges.contains_key(&(c, d)) || self.edges.contains_key(&(d, c)) {
                continue; // the diagonal exists elsewhere (degenerate configuration)
            }
            budget -= 1;
            self.set_tri(t1, [a, d, c]);
            self.set_tri(t2, [d, b, c]);
            stack.extend([(a, d), (d, b), (b, c), (c, a)]);
        }
    }

    /// Splits interior edges whose `|Δx|` exceeds `max_dx` at their midpoints until none
    /// remain (or `max_points` new points have been added). Boundary edges are left alone.
    ///
    /// The longest edge (in `|Δx|`) is always split first. With that order the third
    /// vertices of both adjacent triangles lie between the edge's ends in `x` (their edges
    /// are no longer than it), so every new edge is at most half as long and the process
    /// terminates; splitting in arbitrary order can chase its own midpoints forever.
    /// This needs boundary edges no longer than `max_dx`, which edge sampling guarantees
    /// for circles around the axis and for meridians. Where another kind of boundary edge
    /// is longer, the triangle on it can't be brought within the limit, so the edges of
    /// that triangle are left alone rather than split without end.
    ///
    /// `axis` selects the coordinate measured: 0 for `x`, 1 for `y`.
    pub fn split_long(&mut self, axis: usize, max_dx: f64, max_points: usize) {
        let dx = |m: &Self, a: u32, b: u32| (m.p(a)[axis] - m.p(b)[axis]).abs();
        let boundary_too_long = |m: &Self, t: usize| {
            let tri = m.tris[t];
            (0..3).any(|k| {
                let (u, v) = (tri[k], tri[(k + 1) % 3]);
                !m.edges.contains_key(&(v, u)) && dx(m, u, v) > max_dx
            })
        };
        // Non-negative floats order like their bit patterns.
        let mut heap: std::collections::BinaryHeap<(u64, u32, u32)> = self
            .edges
            .keys()
            .copied()
            .filter(|&(a, b)| a < b && dx(self, a, b) > max_dx)
            .map(|(a, b)| (dx(self, a, b).to_bits(), a, b))
            .collect();
        let start = self.points.len();
        while let Some((_, a, b)) = heap.pop() {
            if self.points.len() - start >= max_points {
                break;
            }
            let (Some(&t1), Some(&t2)) = (self.edges.get(&(a, b)), self.edges.get(&(b, a))) else {
                continue; // a boundary edge, or one that no longer exists
            };
            if boundary_too_long(self, t1) || boundary_too_long(self, t2) {
                continue;
            }
            let c = self.opposite(t1, a, b);
            let d = self.opposite(t2, b, a);
            let (pa, pb) = (self.p(a), self.p(b));
            let flat = |q: DVec2| cross(pa, pb, q).abs() <= SLIVER * (pb - pa).length_squared();
            if flat(self.p(c)) || flat(self.p(d)) {
                // A zero-area triangle along the boundary: its midpoint would land on the
                // boundary (a T-junction against the neighbouring face). Leave it.
                continue;
            }
            self.points.push((pa + pb) * 0.5);
            let m = (self.points.len() - 1) as u32;
            self.edges.remove(&(a, b));
            self.edges.remove(&(b, a));
            self.set_tri(t1, [a, m, c]);
            self.push_tri([m, b, c]);
            self.set_tri(t2, [b, m, d]);
            self.push_tri([m, a, d]);
            for (u, v) in [(a, m), (m, b), (c, m), (d, m)] {
                let len = dx(self, u, v);
                if len > max_dx {
                    heap.push((len.to_bits(), u, v));
                }
            }
        }
    }
}

/// Whether `d` lies strictly inside the circumcircle of counter-clockwise `abc` (with a
/// relative margin, so co-circular points don't flip back and forth).
fn in_circle(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let (ad, bd, cd) = (a - d, b - d, c - d);
    let (al, bl, cl) = (
        ad.length_squared(),
        bd.length_squared(),
        cd.length_squared(),
    );
    let det = al * bd.perp_dot(cd) - bl * ad.perp_dot(cd) + cl * ad.perp_dot(bd);
    let scale = al.max(bl).max(cl);
    det > 1e-10 * scale * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(points: &[DVec2], tris: &[[u32; 3]]) -> f64 {
        tris.iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| points[i as usize]);
                0.5 * cross(a, b, c)
            })
            .sum()
    }

    #[test]
    fn square_with_hole() {
        let mut pts = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(0.0, 10.0),
        ];
        // Clockwise hole.
        pts.extend([
            DVec2::new(3.0, 3.0),
            DVec2::new(3.0, 6.0),
            DVec2::new(6.0, 6.0),
            DVec2::new(6.0, 3.0),
        ]);
        let tris = triangulate(&Polygon {
            points: &pts,
            loops: &[0..4, 4..8],
        });
        assert_eq!(tris.len(), 8);
        assert!((area(&pts, &tris) - 91.0).abs() < 1e-12);
        let mut mesh = Mesh2::new(pts.clone(), tris);
        mesh.delaunay(|_, _| true);
        assert!((area(&mesh.points, &mesh.tris) - 91.0).abs() < 1e-12);
        for t in &mesh.tris {
            let [a, b, c] = t.map(|i| mesh.points[i as usize]);
            assert!(cross(a, b, c) > 0.0);
        }
    }

    #[test]
    fn grid_of_holes() {
        // Many holes sharing bridge targets and extreme coordinates: every boundary vertex
        // must be used and the area must be exact.
        let mut pts = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(60.0, 0.0),
            DVec2::new(60.0, 60.0),
            DVec2::new(0.0, 60.0),
        ];
        let mut loops = Vec::new();
        loops.push(0..4);
        let sides = 12;
        let mut hole_area = 0.0;
        for gx in 0..5 {
            for gy in 0..5 {
                let c = DVec2::new(10.0 + 10.0 * gx as f64, 10.0 + 10.0 * gy as f64);
                let start = pts.len();
                // Clockwise.
                for k in 0..sides {
                    let a = -(k as f64) * std::f64::consts::TAU / sides as f64;
                    pts.push(c + DVec2::from_angle(a) * 3.0);
                }
                loops.push(start..pts.len());
                let ring = &pts[start..];
                hole_area += (0..sides)
                    .map(|i| ring[i].perp_dot(ring[(i + 1) % sides]))
                    .sum::<f64>()
                    * 0.5;
            }
        }
        let tris = triangulate(&Polygon {
            points: &pts,
            loops: &loops,
        });
        assert_eq!(tris.len(), pts.len() + 2 * 25 - 2);
        assert!((area(&pts, &tris) - (3600.0 + hole_area)).abs() < 1e-9);
        let mut used = vec![false; pts.len()];
        for t in &tris {
            let [a, b, c] = t.map(|i| pts[i as usize]);
            assert!(cross(a, b, c) >= 0.0);
            for &i in t {
                used[i as usize] = true;
            }
        }
        assert!(used.iter().all(|&u| u));
        let mut mesh = Mesh2::new(pts, tris);
        mesh.delaunay(|_, _| true);
        assert!((area(&mesh.points, &mesh.tris) - (3600.0 + hole_area)).abs() < 1e-9);
    }

    #[test]
    fn strip_with_collinear_runs_and_splits() {
        // A long thin rectangle with many points along the long sides.
        let n = 20;
        let mut pts: Vec<DVec2> = (0..=n).map(|i| DVec2::new(i as f64, 0.0)).collect();
        pts.extend((0..=n).rev().map(|i| DVec2::new(i as f64, 3.0)));
        let len = pts.len();
        let tris = triangulate(&Polygon {
            points: &pts,
            loops: std::slice::from_ref(&(0..len)),
        });
        assert_eq!(tris.len(), len - 2);
        let mut mesh = Mesh2::new(pts, tris);
        mesh.delaunay(|_, _| true);
        mesh.split_long(0, 1.0, 1000);
        mesh.delaunay(|a, b| (a.x - b.x).abs() <= 1.0);
        assert!((area(&mesh.points, &mesh.tris) - 60.0).abs() < 1e-9);
        for t in &mesh.tris {
            let [a, b, c] = t.map(|i| mesh.points[i as usize]);
            let xs = [a.x, b.x, c.x];
            let span = xs.iter().copied().fold(f64::MIN, f64::max)
                - xs.iter().copied().fold(f64::MAX, f64::min);
            assert!(span <= 1.0 + 1e-12, "triangle spans {span}");
            assert!(cross(a, b, c) > 0.0);
        }
    }
}
