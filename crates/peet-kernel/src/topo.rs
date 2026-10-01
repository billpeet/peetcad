//! Boundary representation (B-rep) topology: Solid → Shell → Face → Loop → Coedge → Edge →
//! Vertex.
//!
//! A [`Solid`] owns flat arenas of each element, addressed by typed ids. Kernel operations
//! build new solids rather than editing old ones in place, so ids are never recycled and a
//! solid is immutable once built (which makes caching tessellation per face safe).
//!
//! **Conventions** (checked by [`crate::validate`]):
//! - Every edge is a parameter range `t0 < t1` of a [`Curve3`], running from its `start`
//!   vertex at `t0` to its `end` vertex at `t1`. A closed edge (a full circle) has
//!   `start == end` and spans one period.
//! - A coedge is one use of an edge by a loop. `reversed` means the loop traverses the edge
//!   from `end` to `start`. In a closed manifold solid each edge has exactly two coedges,
//!   in opposite directions, on two faces (or twice on one face for a seam).
//! - A face is a region of a [`Surface`]. Its outward normal is the surface's natural
//!   normal, flipped when `reversed` is set.
//! - Loops run counter-clockwise when seen from outside (against the outward normal), so
//!   the face lies to the left of every coedge; holes therefore run clockwise. The first
//!   loop of a face is its outer loop.
//! - Periodic faces (a full cylinder wall) carry a **seam** edge (a line along the axis)
//!   used twice by the face's loop, so every face is simply connected in parameter space.

use peet_math::{Aabb, DVec3};
use serde::{Deserialize, Serialize};

use crate::geom::{Curve3, Surface};

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(pub u32);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id_type!(VertexId);
id_type!(EdgeId);
id_type!(CoedgeId);
id_type!(LoopId);
id_type!(FaceId);
id_type!(ShellId);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vertex {
    pub point: DVec3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub curve: Curve3,
    pub start: VertexId,
    pub end: VertexId,
    /// Curve parameter at `start`.
    pub t0: f64,
    /// Curve parameter at `end` (`t1 > t0`).
    pub t1: f64,
    /// The coedges using this edge (two in a closed manifold solid).
    pub coedges: Vec<CoedgeId>,
}

impl Edge {
    pub fn is_closed(&self) -> bool {
        self.start == self.end
    }

    /// Point at a fraction `s` in `0..=1` of the edge's parameter range.
    pub fn point_at_fraction(&self, s: f64) -> DVec3 {
        self.curve.point(self.t0 + (self.t1 - self.t0) * s)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Coedge {
    pub edge: EdgeId,
    /// Traversed from the edge's `end` to its `start`.
    pub reversed: bool,
    pub loop_id: LoopId,
    pub next: CoedgeId,
    pub prev: CoedgeId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Loop {
    pub face: FaceId,
    /// Any coedge of the loop; follow `next` to walk it.
    pub first: CoedgeId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Face {
    pub surface: Surface,
    /// The outward normal is opposite to the surface's natural normal.
    pub reversed: bool,
    /// Outer loop first, then holes.
    pub loops: Vec<LoopId>,
    pub shell: ShellId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shell {
    pub faces: Vec<FaceId>,
}

/// A solid body: one outer shell, plus inner shells for internal voids.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Solid {
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub coedges: Vec<Coedge>,
    pub loops: Vec<Loop>,
    pub faces: Vec<Face>,
    pub shells: Vec<Shell>,
}

impl Solid {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- Access ----

    pub fn vertex(&self, id: VertexId) -> &Vertex {
        &self.vertices[id.index()]
    }

    pub fn edge(&self, id: EdgeId) -> &Edge {
        &self.edges[id.index()]
    }

    pub fn coedge(&self, id: CoedgeId) -> &Coedge {
        &self.coedges[id.index()]
    }

    pub fn loop_(&self, id: LoopId) -> &Loop {
        &self.loops[id.index()]
    }

    pub fn face(&self, id: FaceId) -> &Face {
        &self.faces[id.index()]
    }

    pub fn shell(&self, id: ShellId) -> &Shell {
        &self.shells[id.index()]
    }

    pub fn face_ids(&self) -> impl Iterator<Item = FaceId> + '_ {
        (0..self.faces.len() as u32).map(FaceId)
    }

    pub fn edge_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        (0..self.edges.len() as u32).map(EdgeId)
    }

    pub fn vertex_ids(&self) -> impl Iterator<Item = VertexId> + '_ {
        (0..self.vertices.len() as u32).map(VertexId)
    }

    /// The coedges of a loop, in order.
    pub fn loop_coedges(&self, id: LoopId) -> Vec<CoedgeId> {
        let first = self.loop_(id).first;
        let mut out = vec![first];
        let mut c = self.coedge(first).next;
        while c != first {
            out.push(c);
            c = self.coedge(c).next;
            if out.len() > self.coedges.len() {
                break; // corrupt loop: never spin forever
            }
        }
        out
    }

    /// The vertex a coedge starts from (in loop direction).
    pub fn coedge_start(&self, id: CoedgeId) -> VertexId {
        let c = self.coedge(id);
        let e = self.edge(c.edge);
        if c.reversed { e.end } else { e.start }
    }

    /// The vertex a coedge ends at (in loop direction).
    pub fn coedge_end(&self, id: CoedgeId) -> VertexId {
        let c = self.coedge(id);
        let e = self.edge(c.edge);
        if c.reversed { e.start } else { e.end }
    }

    /// The other coedge of the same edge (the neighbouring face's side), if the edge has
    /// exactly two.
    pub fn twin(&self, id: CoedgeId) -> Option<CoedgeId> {
        let e = self.edge(self.coedge(id).edge);
        match e.coedges[..] {
            [a, b] if a == id => Some(b),
            [a, b] if b == id => Some(a),
            _ => None,
        }
    }

    /// The face a coedge belongs to.
    pub fn coedge_face(&self, id: CoedgeId) -> FaceId {
        self.loop_(self.coedge(id).loop_id).face
    }

    /// Outward unit normal of a face at the surface point closest to `p`.
    pub fn face_normal_at(&self, face: FaceId, p: DVec3) -> DVec3 {
        let f = self.face(face);
        let n = f.surface.normal_at(p);
        if f.reversed { -n } else { n }
    }

    /// Faces sharing an edge with `face`.
    pub fn adjacent_faces(&self, face: FaceId) -> Vec<FaceId> {
        let mut out = Vec::new();
        for &l in &self.face(face).loops {
            for c in self.loop_coedges(l) {
                if let Some(t) = self.twin(c) {
                    let f = self.coedge_face(t);
                    if f != face && !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
        }
        out
    }

    /// Bounds of the vertices, plus sample points along curved edges.
    pub fn bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for v in &self.vertices {
            b.extend(v.point);
        }
        for e in &self.edges {
            if !matches!(e.curve, Curve3::Line(_)) {
                for i in 0..=32 {
                    b.extend(e.point_at_fraction(f64::from(i) / 32.0));
                }
            }
        }
        b
    }

    // ---- Building ----

    pub fn add_vertex(&mut self, point: DVec3) -> VertexId {
        self.vertices.push(Vertex { point });
        VertexId(self.vertices.len() as u32 - 1)
    }

    /// Adds an edge over `curve` from `t0` (at `start`) to `t1` (at `end`).
    pub fn add_edge(
        &mut self,
        curve: Curve3,
        start: VertexId,
        end: VertexId,
        t0: f64,
        t1: f64,
    ) -> EdgeId {
        self.edges.push(Edge {
            curve,
            start,
            end,
            t0,
            t1,
            coedges: Vec::new(),
        });
        EdgeId(self.edges.len() as u32 - 1)
    }

    /// Adds a straight edge between two vertices.
    pub fn add_line_edge(&mut self, start: VertexId, end: VertexId) -> EdgeId {
        let (a, b) = (self.vertex(start).point, self.vertex(end).point);
        let curve = Curve3::line_through(a, b).unwrap_or(Curve3::Line(crate::geom::Line3 {
            origin: a,
            dir: DVec3::X,
        }));
        self.add_edge(curve, start, end, 0.0, a.distance(b))
    }

    pub fn add_shell(&mut self) -> ShellId {
        self.shells.push(Shell { faces: Vec::new() });
        ShellId(self.shells.len() as u32 - 1)
    }

    pub fn add_face(&mut self, shell: ShellId, surface: Surface, reversed: bool) -> FaceId {
        let id = FaceId(self.faces.len() as u32);
        self.faces.push(Face {
            surface,
            reversed,
            loops: Vec::new(),
            shell,
        });
        self.shells[shell.index()].faces.push(id);
        id
    }

    /// Adds a loop to `face` made of `(edge, reversed)` uses, in order. The first loop
    /// added to a face is its outer loop.
    pub fn add_loop(&mut self, face: FaceId, uses: &[(EdgeId, bool)]) -> LoopId {
        assert!(!uses.is_empty(), "a loop needs at least one coedge");
        let loop_id = LoopId(self.loops.len() as u32);
        let base = self.coedges.len() as u32;
        let n = uses.len() as u32;
        for (i, &(edge, reversed)) in uses.iter().enumerate() {
            let i = i as u32;
            let id = CoedgeId(base + i);
            self.coedges.push(Coedge {
                edge,
                reversed,
                loop_id,
                next: CoedgeId(base + (i + 1) % n),
                prev: CoedgeId(base + (i + n - 1) % n),
            });
            self.edges[edge.index()].coedges.push(id);
        }
        self.loops.push(Loop {
            face,
            first: CoedgeId(base),
        });
        self.faces[face.index()].loops.push(loop_id);
        loop_id
    }
}

#[cfg(test)]
pub(crate) mod test_shapes {
    //! Hand-built solids for tests in this crate (independent of the extrude code).

    use super::*;
    use crate::geom::Surface;
    use peet_math::{DVec3, Plane};

    /// An axis-aligned box from `min` to `max`, with outward-facing faces.
    pub fn cuboid(min: DVec3, max: DVec3) -> Solid {
        let mut s = Solid::new();
        let shell = s.add_shell();
        let c = |i: usize| {
            DVec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            )
        };
        let v: Vec<VertexId> = (0..8).map(|i| s.add_vertex(c(i))).collect();
        // Each face: four corner indices, counter-clockwise seen from outside, and its normal.
        let faces: [([usize; 4], DVec3); 6] = [
            ([0, 2, 3, 1], -DVec3::Z),
            ([4, 5, 7, 6], DVec3::Z),
            ([0, 1, 5, 4], -DVec3::Y),
            ([2, 6, 7, 3], DVec3::Y),
            ([0, 4, 6, 2], -DVec3::X),
            ([1, 3, 7, 5], DVec3::X),
        ];
        let mut edges: std::collections::HashMap<(usize, usize), EdgeId> = Default::default();
        for (corners, normal) in faces {
            let origin = c(corners[0]);
            let x = c(corners[1]) - origin;
            let plane = Plane::from_origin_normal_x(origin, normal, x).expect("valid face");
            let face = s.add_face(shell, Surface::Plane(plane), false);
            let mut uses = Vec::new();
            for k in 0..4 {
                let (a, b) = (corners[k], corners[(k + 1) % 4]);
                let key = (a.min(b), a.max(b));
                let edge = *edges
                    .entry(key)
                    .or_insert_with(|| s.add_line_edge(v[key.0], v[key.1]));
                uses.push((edge, a > b));
            }
            s.add_loop(face, &uses);
        }
        s
    }

    #[test]
    fn cuboid_is_closed() {
        let s = cuboid(DVec3::ZERO, DVec3::new(1.0, 2.0, 3.0));
        assert_eq!((s.vertices.len(), s.edges.len(), s.faces.len()), (8, 12, 6));
        for e in &s.edges {
            assert_eq!(e.coedges.len(), 2);
        }
        for c in 0..s.coedges.len() as u32 {
            let c = CoedgeId(c);
            let t = s.twin(c).unwrap();
            assert_ne!(s.coedge(c).reversed, s.coedge(t).reversed);
            assert_eq!(s.coedge_end(s.coedge(c).prev), s.coedge_start(c));
        }
        assert_eq!(s.adjacent_faces(FaceId(0)).len(), 4);
    }
}
