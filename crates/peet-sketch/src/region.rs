//! Profile and region detection: closed loops and nested regions for features to use.
//!
//! **Algorithm.** All non-construction lines, arcs and circles form a planar arrangement:
//!
//! 1. Every curve is split at its intersections with the others and wherever another
//!    curve's endpoint touches it. Points within [`tolerance::LINEAR`] merge into one
//!    vertex (endpoints joined by coincident relations sit at the same place after solving,
//!    so geometric merging is all that's needed; shared point entities work the same way).
//!    Duplicate pieces (overlapping collinear or co-circular curves) are kept once.
//! 2. Dangling edges (trees hanging off the loops, open chains) are pruned repeatedly.
//! 3. Faces are walked with a half-edge structure. Outgoing edges around each vertex are
//!    ordered by tangent angle; tangent edges (a slot's line leaving an arc) are ordered by
//!    signed curvature. Walking "next = previous edge clockwise from the twin" traces
//!    bounded faces counter-clockwise and each connected component's outside clockwise.
//!    Bridges (edges with the same face on both sides) are removed and the walk repeated,
//!    so loops never double back along an edge.
//! 4. Each component's clockwise outside becomes a hole of the smallest bounded face of
//!    another component that contains it.
//!
//! Areas are exact for arcs: the shoelace sum over chord endpoints plus the circular
//! segment area of every arc.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

use peet_math::{DVec2, tolerance};

use crate::curve::Curve;
use crate::sketch::{EntityId, Sketch};

/// Points closer than this are the same vertex of the arrangement.
const MERGE: f64 = tolerance::LINEAR;

/// Outgoing edge directions within this angle (radians) of each other are treated as
/// tangent and ordered by curvature. Looser than [`tolerance::ANGULAR`] because tangency
/// in a solved sketch only holds to the solver's accuracy.
const TANGENT_TIE: f64 = 1e-7;

/// One piece of a loop: a (possibly split) part of a sketch curve.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopEdge {
    /// The sketch curve this piece comes from.
    pub entity: EntityId,
    /// The geometry of the piece, which may be only part of the curve when curves cross.
    pub curve: Curve,
    /// Whether the loop traverses the piece against the curve's own parameter direction.
    pub reversed: bool,
}

impl LoopEdge {
    /// Start and end of the piece in loop order.
    pub fn ends(&self) -> (DVec2, DVec2) {
        if self.reversed {
            (self.curve.end(), self.curve.start())
        } else {
            (self.curve.start(), self.curve.end())
        }
    }
}

/// A closed loop of edges, in order. Outer boundaries run counter-clockwise, holes clockwise.
#[derive(Clone, Debug, PartialEq)]
pub struct Loop {
    pub edges: Vec<LoopEdge>,
    /// Signed area (positive for counter-clockwise).
    pub signed_area: f64,
}

impl Loop {
    /// Winding number of the loop around `p` (0 outside; ±1 inside a simple loop).
    /// Points on the loop itself give an unspecified result.
    pub fn winding_number(&self, p: DVec2) -> i32 {
        let mut total = 0.0;
        for e in &self.edges {
            let (a, b) = (e.curve.start(), e.curve.end());
            let chord = (a - p).angle_to(b - p);
            let w = match e.curve {
                // Seen from inside its circle, a counter-clockwise arc always sweeps a
                // positive angle: the chord's angle taken in 0..2π (a full turn when the
                // arc is the whole circle). From outside, it sweeps what its chord does.
                Curve::Arc {
                    center,
                    radius,
                    sweep,
                    ..
                } if p.distance(center) < radius => {
                    let w = chord.rem_euclid(TAU);
                    if sweep > std::f64::consts::PI && w < 1e-9 {
                        TAU
                    } else if sweep < std::f64::consts::PI && w > TAU - 1e-9 {
                        0.0
                    } else {
                        w
                    }
                }
                Curve::Circle { center, radius } if p.distance(center) < radius => TAU,
                _ => chord,
            };
            total += if e.reversed { -w } else { w };
        }
        (total / TAU).round() as i32
    }

    /// Whether `p` is enclosed by the loop.
    pub fn contains(&self, p: DVec2) -> bool {
        self.winding_number(p) != 0
    }

    /// Axis-aligned bounds `(min, max)`.
    pub fn bounds(&self) -> (DVec2, DVec2) {
        let mut min = DVec2::splat(f64::INFINITY);
        let mut max = DVec2::splat(f64::NEG_INFINITY);
        for e in &self.edges {
            let (lo, hi) = e.curve.bounds();
            min = min.min(lo);
            max = max.max(hi);
        }
        (min, max)
    }
}

/// A connected face bounded by one outer loop and any number of hole loops.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub outer: Loop,
    pub holes: Vec<Loop>,
}

impl Region {
    /// Area of the face (outer area minus holes).
    pub fn area(&self) -> f64 {
        self.outer.signed_area.abs() - self.holes.iter().map(|h| h.signed_area.abs()).sum::<f64>()
    }

    /// Whether `p` lies inside the outer loop and outside every hole.
    pub fn contains(&self, p: DVec2) -> bool {
        self.outer.contains(p) && !self.holes.iter().any(|h| h.contains(p))
    }
}

/// Result of analysing a sketch's profile.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    /// The minimal faces of the arrangement of all non-construction curves.
    pub regions: Vec<Region>,
    /// Curve endpoints not connected to anything: they indicate an open profile.
    pub open_ends: Vec<DVec2>,
}

/// Finds all closed regions formed by the sketch's non-construction curves. Crossing
/// curves are split at their intersections, so overlapping shapes produce every minimal
/// face (like contour selection in SolidWorks).
pub fn find_regions(sketch: &Sketch) -> Profile {
    let inputs: Vec<(EntityId, Curve)> = sketch
        .entities()
        .filter(|(_, e)| !e.construction && e.kind().is_curve())
        .filter_map(|(id, _)| Some((id, sketch.curve(id)?)))
        .filter(|(_, c)| c.length() > MERGE)
        .collect();
    let mut graph = Arrangement::build(&inputs);
    let open_ends = graph.open_ends();
    graph.prune_dangling();
    let faces = loop {
        let faces = graph.walk_faces();
        if !graph.remove_bridges(&faces) {
            break faces;
        }
        graph.prune_dangling();
    };
    Profile {
        regions: graph.regions(faces),
        open_ends,
    }
}

impl Profile {
    /// Index of the innermost region containing `p`, if any.
    pub fn region_at(&self, p: DVec2) -> Option<usize> {
        self.regions
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                let (min, max) = r.outer.bounds();
                p.cmpge(min).all() && p.cmple(max).all() && r.contains(p)
            })
            .min_by(|a, b| {
                a.1.outer
                    .signed_area
                    .abs()
                    .total_cmp(&b.1.outer.signed_area.abs())
            })
            .map(|(i, _)| i)
    }
}

// ---- The arrangement ----

struct Edge {
    entity: EntityId,
    /// A line or arc piece, running from vertex `from` to vertex `to`.
    curve: Curve,
    from: usize,
    to: usize,
    alive: bool,
}

/// Half-edge `h` runs along edge `h / 2`, forwards for even `h`, backwards for odd `h`.
struct Arrangement {
    vertices: Vec<DVec2>,
    edges: Vec<Edge>,
    /// Vertices at the ends of the input lines and arcs (for the open-end diagnostic).
    curve_ends: Vec<usize>,
}

/// A walked face: its half-edges in order and its signed area.
struct Face {
    half_edges: Vec<usize>,
    area: f64,
}

impl Arrangement {
    fn build(inputs: &[(EntityId, Curve)]) -> Self {
        let bounds: Vec<(DVec2, DVec2)> = inputs
            .iter()
            .map(|(_, c)| {
                let (lo, hi) = c.bounds();
                (lo - MERGE, hi + MERGE)
            })
            .collect();
        let overlap = |i: usize, j: usize| {
            let (a0, a1) = bounds[i];
            let (b0, b1) = bounds[j];
            a0.cmple(b1).all() && b0.cmple(a1).all()
        };

        // Split parameters of every curve.
        let mut params: Vec<Vec<f64>> = vec![Vec::new(); inputs.len()];
        for i in 0..inputs.len() {
            for j in i + 1..inputs.len() {
                if !overlap(i, j) {
                    continue;
                }
                for hit in inputs[i].1.intersect(&inputs[j].1) {
                    params[i].push(hit.t_a);
                    params[j].push(hit.t_b);
                }
            }
        }
        // Endpoints touching other curves (T-junctions the intersection test can miss,
        // and the ends of overlapping collinear/co-circular pieces).
        for i in 0..inputs.len() {
            let c = &inputs[i].1;
            for (j, (_, other)) in inputs.iter().enumerate() {
                if i == j || other.is_closed() || !overlap(i, j) {
                    continue;
                }
                for p in [other.start(), other.end()] {
                    if c.distance(p) <= MERGE {
                        params[i].push(c.project(p));
                    }
                }
            }
        }

        let mut graph = Self {
            vertices: Vec::new(),
            edges: Vec::new(),
            curve_ends: Vec::new(),
        };
        let mut grid = VertexGrid::default();
        let mut seen: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for ((entity, curve), mut ts) in inputs.iter().zip(params) {
            if curve.is_closed() {
                ensure_two_splits(curve, &mut ts);
            } else {
                let a = grid.vertex(&mut graph.vertices, curve.start());
                let b = grid.vertex(&mut graph.vertices, curve.end());
                graph.curve_ends.extend([a, b]);
            }
            for piece in curve.split(&ts) {
                graph.add_piece(*entity, piece, &mut grid, &mut seen);
            }
        }
        graph
    }

    fn add_piece(
        &mut self,
        entity: EntityId,
        piece: Curve,
        grid: &mut VertexGrid,
        seen: &mut HashMap<(usize, usize), Vec<usize>>,
    ) {
        let from = grid.vertex(&mut self.vertices, piece.start());
        let to = grid.vertex(&mut self.vertices, piece.end());
        if from == to {
            // A closed arc: split it so the graph has no self loops. Lines this short are
            // degenerate and dropped.
            if matches!(piece, Curve::Arc { .. }) && piece.length() > 2.0 * MERGE {
                self.add_piece(entity, piece.sub_curve(0.0, 0.5), grid, seen);
                self.add_piece(entity, piece.sub_curve(0.5, 1.0), grid, seen);
            }
            return;
        }
        let key = (from.min(to), from.max(to));
        let mid = piece.point_at(0.5);
        let same_pair = seen.entry(key).or_default();
        if same_pair
            .iter()
            .any(|&e| self.edges[e].curve.point_at(0.5).distance(mid) <= MERGE)
        {
            return;
        }
        same_pair.push(self.edges.len());
        self.edges.push(Edge {
            entity,
            curve: piece,
            from,
            to,
            alive: true,
        });
    }

    fn degrees(&self) -> Vec<usize> {
        let mut deg = vec![0; self.vertices.len()];
        for e in self.edges.iter().filter(|e| e.alive) {
            deg[e.from] += 1;
            deg[e.to] += 1;
        }
        deg
    }

    fn open_ends(&self) -> Vec<DVec2> {
        let deg = self.degrees();
        let mut ends: Vec<usize> = self
            .curve_ends
            .iter()
            .copied()
            .filter(|&v| deg[v] == 1)
            .collect();
        ends.sort_unstable();
        ends.dedup();
        ends.into_iter().map(|v| self.vertices[v]).collect()
    }

    /// Removes dangling edges repeatedly until every remaining vertex has degree >= 2.
    fn prune_dangling(&mut self) {
        let mut deg = self.degrees();
        let mut incident: Vec<Vec<usize>> = vec![Vec::new(); self.vertices.len()];
        for (i, e) in self.edges.iter().enumerate().filter(|(_, e)| e.alive) {
            incident[e.from].push(i);
            incident[e.to].push(i);
        }
        let mut queue: Vec<usize> = (0..deg.len()).filter(|&v| deg[v] == 1).collect();
        while let Some(v) = queue.pop() {
            if deg[v] != 1 {
                continue;
            }
            let Some(&e) = incident[v].iter().find(|&&e| self.edges[e].alive) else {
                continue;
            };
            self.edges[e].alive = false;
            let other = if self.edges[e].from == v {
                self.edges[e].to
            } else {
                self.edges[e].from
            };
            deg[v] -= 1;
            deg[other] -= 1;
            if deg[other] == 1 {
                queue.push(other);
            }
        }
    }

    fn origin(&self, h: usize) -> usize {
        let e = &self.edges[h / 2];
        if h % 2 == 1 { e.to } else { e.from }
    }

    /// Direction leaving the origin, and signed curvature (positive turning left).
    fn outgoing(&self, h: usize) -> (DVec2, f64) {
        let c = &self.edges[h / 2].curve;
        let kappa = match *c {
            Curve::Arc { radius, .. } => 1.0 / radius,
            _ => 0.0,
        };
        if h % 2 == 1 {
            (-c.tangent_at(1.0), -kappa)
        } else {
            (c.tangent_at(0.0), kappa)
        }
    }

    fn walk_faces(&self) -> Vec<Face> {
        let n_half = self.edges.len() * 2;
        // Outgoing half-edges around each vertex, counter-clockwise.
        let mut around: Vec<Vec<(f64, f64, usize)>> = vec![Vec::new(); self.vertices.len()];
        for h in 0..n_half {
            if !self.edges[h / 2].alive {
                continue;
            }
            let (dir, kappa) = self.outgoing(h);
            let mut angle = dir.y.atan2(dir.x);
            if angle < -PI + TANGENT_TIE {
                angle += TAU; // keep directions near ±π together
            }
            around[self.origin(h)].push((angle, kappa, h));
        }
        let mut position = vec![usize::MAX; n_half];
        for list in &mut around {
            sort_around(list);
            for (i, &(_, _, h)) in list.iter().enumerate() {
                position[h] = i;
            }
        }

        let mut visited = vec![false; n_half];
        let mut faces = Vec::new();
        for start in 0..n_half {
            if visited[start] || !self.edges[start / 2].alive {
                continue;
            }
            let mut half_edges = Vec::new();
            let mut area = 0.0;
            let mut h = start;
            while !visited[h] && half_edges.len() < n_half {
                visited[h] = true;
                half_edges.push(h);
                area += half_edge_area(&self.edges[h / 2].curve, h % 2 == 1);
                // Next: the outgoing edge just clockwise of the twin at the far vertex.
                let twin = h ^ 1;
                let list = &around[self.origin(twin)];
                let i = position[twin];
                h = list[(i + list.len() - 1) % list.len()].2;
            }
            faces.push(Face { half_edges, area });
        }
        faces
    }

    /// Kills edges with the same face on both sides. Returns whether any were found.
    fn remove_bridges(&mut self, faces: &[Face]) -> bool {
        let mut face_of = vec![usize::MAX; self.edges.len() * 2];
        for (f, face) in faces.iter().enumerate() {
            for &h in &face.half_edges {
                face_of[h] = f;
            }
        }
        let mut any = false;
        for (e, edge) in self.edges.iter_mut().enumerate() {
            if edge.alive && face_of[2 * e] == face_of[2 * e + 1] {
                edge.alive = false;
                any = true;
            }
        }
        any
    }

    fn component_of_vertices(&self) -> Vec<usize> {
        let mut parent: Vec<usize> = (0..self.vertices.len()).collect();
        fn find(parent: &mut [usize], mut v: usize) -> usize {
            while parent[v] != v {
                parent[v] = parent[parent[v]];
                v = parent[v];
            }
            v
        }
        for e in self.edges.iter().filter(|e| e.alive) {
            let (a, b) = (find(&mut parent, e.from), find(&mut parent, e.to));
            parent[a] = b;
        }
        (0..self.vertices.len())
            .map(|v| find(&mut parent, v))
            .collect()
    }

    fn to_loop(&self, face: &Face) -> Loop {
        Loop {
            edges: face
                .half_edges
                .iter()
                .map(|&h| {
                    let e = &self.edges[h / 2];
                    LoopEdge {
                        entity: e.entity,
                        curve: e.curve,
                        reversed: h % 2 == 1,
                    }
                })
                .collect(),
            signed_area: face.area,
        }
    }

    fn regions(&self, faces: Vec<Face>) -> Vec<Region> {
        let component = self.component_of_vertices();
        let comp_of = |face: &Face| component[self.origin(face.half_edges[0])];
        let mut regions: Vec<(usize, Region, (DVec2, DVec2))> = Vec::new();
        let mut outsides: Vec<(usize, Loop)> = Vec::new();
        for face in &faces {
            let lp = self.to_loop(face);
            if face.area > 0.0 {
                let bounds = lp.bounds();
                regions.push((
                    comp_of(face),
                    Region {
                        outer: lp,
                        holes: Vec::new(),
                    },
                    bounds,
                ));
            } else {
                outsides.push((comp_of(face), lp));
            }
        }
        // Each component's outside is a hole in the smallest face of another component
        // that encloses it (components don't touch, so any point of it will do).
        for (comp, hole) in outsides {
            let p = hole.edges[0].curve.point_at(0.5);
            let container = regions
                .iter()
                .enumerate()
                .filter(|(_, (c, r, (min, max)))| {
                    *c != comp && p.cmpge(*min).all() && p.cmple(*max).all() && r.outer.contains(p)
                })
                .min_by(|a, b| a.1.1.outer.signed_area.total_cmp(&b.1.1.outer.signed_area))
                .map(|(i, _)| i);
            if let Some(i) = container {
                regions[i].1.holes.push(hole);
            }
        }
        regions.into_iter().map(|(_, r, _)| r).collect()
    }
}

/// Sorts outgoing half-edges `(angle, curvature, h)` counter-clockwise. Runs of nearly
/// equal angle (tangent curves) are ordered by curvature: of two curves leaving in the
/// same direction, the one turning further left lies counter-clockwise of the other.
fn sort_around(list: &mut [(f64, f64, usize)]) {
    list.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut start = 0;
    for i in 1..=list.len() {
        if i == list.len() || list[i].0 - list[i - 1].0 > TANGENT_TIE {
            if i - start > 1 {
                list[start..i].sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
            }
            start = i;
        }
    }
}

/// Contribution of a (possibly reversed) line or arc to its loop's signed area:
/// `½ ∮ (x dy - y dx)`, exact for arcs.
fn half_edge_area(c: &Curve, reversed: bool) -> f64 {
    let mut a = 0.5 * c.start().perp_dot(c.end());
    if let Curve::Arc { radius, sweep, .. } = *c {
        // Circular segment between the chord and a counter-clockwise arc.
        a += 0.5 * radius * radius * (sweep - sweep.sin());
    }
    if reversed { -a } else { a }
}

/// Circles need at least two vertices so that no edge is a self loop.
fn ensure_two_splits(circle: &Curve, ts: &mut Vec<f64>) {
    let len = circle.length();
    let mut distinct: Vec<f64> = ts.iter().map(|t| t.rem_euclid(1.0)).collect();
    distinct.sort_by(f64::total_cmp);
    distinct.dedup_by(|a, b| (*a - *b) * len <= MERGE);
    if distinct.len() > 1 && (1.0 - distinct[distinct.len() - 1] + distinct[0]) * len <= MERGE {
        distinct.pop(); // the same point on either side of angle zero
    }
    match distinct.as_slice() {
        [] => ts.extend([0.0, 0.5]),
        [t] => ts.push((t + 0.5).rem_euclid(1.0)),
        _ => {}
    }
}

/// Spatial hash for merging points within [`MERGE`].
#[derive(Default)]
struct VertexGrid {
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl VertexGrid {
    fn cell(p: DVec2) -> (i64, i64) {
        ((p.x / MERGE).floor() as i64, (p.y / MERGE).floor() as i64)
    }

    /// The vertex at `p`, creating it if no vertex lies within [`MERGE`].
    fn vertex(&mut self, vertices: &mut Vec<DVec2>, p: DVec2) -> usize {
        let (cx, cy) = Self::cell(p);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &v in self.cells.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let d = vertices[v].distance(p);
                    if d <= MERGE && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, v));
                    }
                }
            }
        }
        if let Some((_, v)) = best {
            return v;
        }
        vertices.push(p);
        let v = vertices.len() - 1;
        self.cells.entry((cx, cy)).or_default().push(v);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::ConstraintKind;

    fn rect(s: &mut Sketch, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<EntityId> {
        let p = [
            DVec2::new(x0, y0),
            DVec2::new(x1, y0),
            DVec2::new(x1, y1),
            DVec2::new(x0, y1),
        ];
        let lines: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
        for i in 0..4 {
            let (_, end) = s.endpoints(lines[i]).unwrap();
            let (start, _) = s.endpoints(lines[(i + 1) % 4]).unwrap();
            s.add_constraint(ConstraintKind::Coincident(end, start))
                .unwrap();
        }
        lines
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn sorted_areas(p: &Profile) -> Vec<f64> {
        let mut a: Vec<f64> = p.regions.iter().map(Region::area).collect();
        a.sort_by(f64::total_cmp);
        a
    }

    #[test]
    fn rectangle() {
        let mut s = Sketch::new();
        rect(&mut s, 0.0, 0.0, 4.0, 3.0);
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 1);
        let r = &p.regions[0];
        assert_eq!(r.outer.edges.len(), 4);
        assert!(r.holes.is_empty());
        assert!(close(r.outer.signed_area, 12.0), "ccw outer");
        assert!(p.open_ends.is_empty());
        assert_eq!(p.region_at(DVec2::new(1.0, 1.0)), Some(0));
        assert_eq!(p.region_at(DVec2::new(5.0, 1.0)), None);
        // Loop edges chain end to start.
        let edges = &r.outer.edges;
        for i in 0..edges.len() {
            let (_, end) = edges[i].ends();
            let (start, _) = edges[(i + 1) % edges.len()].ends();
            assert!(end.distance(start) < 1e-9);
        }
    }

    #[test]
    fn rectangle_with_circular_hole() {
        let mut s = Sketch::new();
        rect(&mut s, 0.0, 0.0, 10.0, 10.0);
        s.add_circle(DVec2::new(5.0, 5.0), 2.0);
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 2);
        let disk = PI * 4.0;
        assert!(close(sorted_areas(&p)[0], disk));
        assert!(close(sorted_areas(&p)[1], 100.0 - disk));
        let plate = p.region_at(DVec2::new(1.0, 1.0)).unwrap();
        assert_eq!(p.regions[plate].holes.len(), 1);
        assert!(
            p.regions[plate].holes[0].signed_area < 0.0,
            "holes run clockwise"
        );
        let inner = p.region_at(DVec2::new(5.0, 5.0)).unwrap();
        assert_ne!(inner, plate);
        assert!(close(p.regions[inner].area(), disk));
    }

    #[test]
    fn overlapping_rectangles() {
        let mut s = Sketch::new();
        rect(&mut s, 0.0, 0.0, 4.0, 4.0);
        rect(&mut s, 2.0, 2.0, 6.0, 6.0);
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 3);
        assert_eq!(sorted_areas(&p), vec![4.0, 12.0, 12.0]);
        let overlap = p.region_at(DVec2::new(3.0, 3.0)).unwrap();
        assert!(close(p.regions[overlap].area(), 4.0));
    }

    #[test]
    fn slot_lines_and_arcs() {
        // Slot: centres (0,0) and (10,0), radius 2; lines tangent to the arcs.
        let mut s = Sketch::new();
        s.add_line(DVec2::new(0.0, -2.0), DVec2::new(10.0, -2.0));
        s.add_arc(
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, -2.0),
            DVec2::new(10.0, 2.0),
        );
        s.add_line(DVec2::new(10.0, 2.0), DVec2::new(0.0, 2.0));
        s.add_arc(
            DVec2::new(0.0, 0.0),
            DVec2::new(0.0, 2.0),
            DVec2::new(0.0, -2.0),
        );
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 1);
        assert!(p.open_ends.is_empty());
        let expected = 40.0 + PI * 4.0;
        assert!(close(p.regions[0].area(), expected));
        assert!(close(p.regions[0].outer.signed_area, expected));
        assert!(
            p.region_at(DVec2::new(-1.5, 0.0)).is_some(),
            "inside an end arc"
        );
        assert!(p.region_at(DVec2::new(-1.5, 1.9)).is_none());
    }

    #[test]
    fn circle_tangent_to_lines() {
        // A circle touching the top and bottom edges of a rectangle: at each contact the
        // line and the circle leave in the same direction, so the order around the vertex
        // is decided by curvature.
        let mut s = Sketch::new();
        rect(&mut s, -3.0, -2.0, 3.0, 2.0);
        s.add_circle(DVec2::ZERO, 2.0);
        let p = find_regions(&s);
        let a = sorted_areas(&p);
        assert_eq!(a.len(), 3);
        let side = (24.0 - 4.0 * PI) / 2.0;
        assert!(close(a[0], side) && close(a[1], side));
        assert!(close(a[2], 4.0 * PI));
        for r in &p.regions {
            assert!(r.holes.is_empty());
        }
    }

    #[test]
    fn circle_alone() {
        let mut s = Sketch::new();
        s.add_circle(DVec2::new(1.0, 2.0), 3.0);
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 1);
        assert!(close(p.regions[0].area(), 9.0 * PI));
        assert!(p.regions[0].holes.is_empty());
        assert!(p.open_ends.is_empty());
    }

    #[test]
    fn figure_eight_and_touching_shapes() {
        let mut s = Sketch::new();
        s.add_circle(DVec2::new(-1.0, 0.0), 1.0);
        s.add_circle(DVec2::new(1.0, 0.0), 1.0);
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 2);
        for r in &p.regions {
            assert!(close(r.area(), PI));
            assert!(r.holes.is_empty());
        }
        // Two squares sharing a corner.
        let mut s = Sketch::new();
        rect(&mut s, 0.0, 0.0, 1.0, 1.0);
        rect(&mut s, 1.0, 1.0, 3.0, 3.0);
        let p = find_regions(&s);
        assert_eq!(sorted_areas(&p), vec![1.0, 4.0]);
        // Two squares sharing an edge (overlapping collinear lines).
        let mut s = Sketch::new();
        rect(&mut s, 0.0, 0.0, 1.0, 1.0);
        rect(&mut s, 1.0, -1.0, 3.0, 2.0);
        let p = find_regions(&s);
        assert_eq!(sorted_areas(&p), vec![1.0, 6.0]);
    }

    #[test]
    fn open_chain_has_no_region() {
        let mut s = Sketch::new();
        s.add_line(DVec2::ZERO, DVec2::new(1.0, 0.0));
        s.add_line(DVec2::new(1.0, 0.0), DVec2::new(1.0, 1.0));
        s.add_line(DVec2::new(1.0, 1.0), DVec2::new(0.0, 1.0));
        let p = find_regions(&s);
        assert!(p.regions.is_empty());
        assert_eq!(p.open_ends.len(), 2);
        assert!(p.open_ends.contains(&DVec2::ZERO));
        assert!(p.open_ends.contains(&DVec2::new(0.0, 1.0)));
    }

    #[test]
    fn dangling_edges_and_bridges_are_dropped() {
        let mut s = Sketch::new();
        rect(&mut s, 0.0, 0.0, 10.0, 10.0);
        // A tail sticking out and a tree inside.
        s.add_line(DVec2::new(10.0, 5.0), DVec2::new(15.0, 5.0));
        s.add_line(DVec2::new(0.0, 5.0), DVec2::new(3.0, 5.0));
        // A square hanging inside on a bridge from the outer rectangle.
        rect(&mut s, 6.0, 6.0, 8.0, 8.0);
        s.add_line(DVec2::new(8.0, 7.0), DVec2::new(10.0, 7.0));
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 2);
        let plate = p.region_at(DVec2::new(1.0, 1.0)).unwrap();
        let r = &p.regions[plate];
        // Without the tail, tree and bridge, but split where they met it.
        assert_eq!(r.outer.edges.len(), 7);
        assert_eq!(r.holes.len(), 1);
        assert!(close(r.area(), 96.0));
        assert_eq!(p.open_ends.len(), 2);
    }

    #[test]
    fn construction_geometry_is_ignored() {
        let mut s = Sketch::new();
        rect(&mut s, 0.0, 0.0, 4.0, 4.0);
        let diag = s.add_line(DVec2::ZERO, DVec2::new(4.0, 4.0));
        s.set_construction(diag, true);
        let c = s.add_circle(DVec2::new(2.0, 2.0), 1.0);
        s.set_construction(c, true);
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 1);
        assert!(close(p.regions[0].area(), 16.0));
    }

    #[test]
    fn nested_rectangles() {
        let mut s = Sketch::new();
        rect(&mut s, 0.0, 0.0, 10.0, 10.0);
        rect(&mut s, 2.0, 2.0, 8.0, 8.0);
        rect(&mut s, 4.0, 4.0, 6.0, 6.0);
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 3);
        for r in &p.regions {
            assert!(r.holes.len() <= 1);
        }
        let outer = &p.regions[p.region_at(DVec2::new(1.0, 1.0)).unwrap()];
        assert!(close(outer.area(), 64.0));
        assert!(close(outer.holes[0].signed_area, -36.0));
        let middle = &p.regions[p.region_at(DVec2::new(3.0, 3.0)).unwrap()];
        assert!(close(middle.area(), 32.0));
        assert!(close(middle.holes[0].signed_area, -4.0));
        let inner = &p.regions[p.region_at(DVec2::new(5.0, 5.0)).unwrap()];
        assert!(close(inner.area(), 4.0));
        assert!(inner.holes.is_empty());
    }

    #[test]
    fn arc_areas_are_exact() {
        // Half disk (arc + diameter), and a rectangle crossing a circle.
        let mut s = Sketch::new();
        s.add_arc(DVec2::ZERO, DVec2::new(3.0, 0.0), DVec2::new(-3.0, 0.0));
        s.add_line(DVec2::new(-3.0, 0.0), DVec2::new(3.0, 0.0));
        let p = find_regions(&s);
        assert_eq!(p.regions.len(), 1);
        assert!(close(p.regions[0].area(), 4.5 * PI));

        let mut s = Sketch::new();
        s.add_circle(DVec2::ZERO, 2.0);
        rect(&mut s, 0.0, -5.0, 5.0, 5.0);
        let p = find_regions(&s);
        // Half disk inside, half disk outside the rectangle, rectangle minus half disk.
        assert_eq!(sorted_areas(&p).len(), 3);
        let a = sorted_areas(&p);
        assert!(close(a[0], 2.0 * PI));
        assert!(close(a[1], 2.0 * PI));
        assert!(close(a[2], 50.0 - 2.0 * PI));
        // Minor arc segment: arc from 0 to 60 degrees closed by its chord.
        let mut s = Sketch::new();
        let end = DVec2::from_angle(PI / 3.0) * 2.0;
        s.add_arc(DVec2::ZERO, DVec2::new(2.0, 0.0), end);
        s.add_line(end, DVec2::new(2.0, 0.0));
        let p = find_regions(&s);
        let theta = PI / 3.0;
        assert!(close(p.regions[0].area(), 2.0 * (theta - theta.sin())));
    }

    #[test]
    fn bracket_with_many_curves_is_fast() {
        let mut s = Sketch::new();
        rect(&mut s, 0.0, 0.0, 200.0, 100.0);
        for i in 0..8 {
            for j in 0..4 {
                let c = DVec2::new(15.0 + 24.0 * f64::from(i), 15.0 + 23.0 * f64::from(j));
                if (i + j) % 2 == 0 {
                    s.add_circle(c, 5.0);
                } else {
                    rect(&mut s, c.x - 4.0, c.y - 4.0, c.x + 4.0, c.y + 4.0);
                }
            }
        }
        // 4 + 16 circles + 16 * 4 lines = 84 curves.
        let start = std::time::Instant::now();
        let p = find_regions(&s);
        let elapsed = start.elapsed();
        assert_eq!(p.regions.len(), 33);
        let plate = &p.regions[p.region_at(DVec2::new(1.0, 1.0)).unwrap()];
        assert_eq!(plate.holes.len(), 32);
        eprintln!("find_regions on 84 curves: {elapsed:?}");
    }
}

#[cfg(test)]
mod winding_tests {
    use super::*;
    use crate::Sketch;

    #[test]
    fn region_at_centres_and_centrelines() {
        let mut s = Sketch::new();
        s.add_circle(DVec2::new(5.0, 5.0), 3.0);
        let slot = crate::shapes::slot(&mut s, DVec2::new(20.0, 0.0), DVec2::new(40.0, 0.0), 4.0);
        // The slot's construction centreline must not matter.
        assert!(!slot.curves.is_empty());
        let profile = find_regions(&s);
        assert_eq!(profile.regions.len(), 2);
        assert!(
            profile.region_at(DVec2::new(5.0, 5.0)).is_some(),
            "circle centre"
        );
        assert!(
            profile.region_at(DVec2::new(30.0, 0.0)).is_some(),
            "slot centreline"
        );
        assert!(
            profile.region_at(DVec2::new(20.0, 0.0)).is_some(),
            "slot arc centre"
        );
        assert!(
            profile.region_at(DVec2::new(43.9, 0.0)).is_some(),
            "inside the end arc"
        );
        assert!(profile.region_at(DVec2::new(44.1, 0.0)).is_none());
        assert!(profile.region_at(DVec2::new(5.0, 8.1)).is_none());
        assert!(profile.region_at(DVec2::new(12.0, 0.0)).is_none());
    }
}
