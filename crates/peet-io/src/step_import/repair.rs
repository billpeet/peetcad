//! Repair of tolerant vertices and edges: where a file's faces don't meet where its
//! topology says they do.
//!
//! Parasolid, and so SolidWorks, gives a vertex or an edge its own tolerance, and a body
//! it has imported or healed can carry tolerances of up to a millimetre: a small fillet or
//! chamfer squeezed out of the body, its edges' curves left where the fillet's sides were
//! and their vertices put somewhere between (see the importer's module docs). The kernel's
//! vertices and edges are exact, so such a body is brought into its form before it is
//! built, in two ways.
//!
//! - **Moving.** A vertex whose edges' curves miss it is moved to the point where the
//!   surfaces of all the faces round it meet, if they meet near it, and the edges that
//!   end there are given new curves on both their faces (as tolerant edges are healed).
//!   A vertex its edges miss by no more than [`CORNER_SLACK`] stays put, and they are
//!   bent near their ends to reach it. An edge whose curve strays from its faces (by up
//!   to [`REPAIR_LIMIT`]) is moved onto them the same way, where they meet along it.
//! - **Slivers.** A face between two edges that lie within [`HEAL_LIMIT`] of each other
//!   all along is left out, and its neighbours meet along one of the two.
//! - **Filling.** Where the faces round a vertex don't meet (a fillet that is missing
//!   between them, two parallel faces either side of a squeezed-out one), the vertex is
//!   split: each edge that ends there ends where its own faces meet, or, if they don't,
//!   is split in two, one for each face, each lying on its face. A face whose two edges
//!   at the vertex now end apart gets an edge between them, on its surface. What is left
//!   open, between the two halves of a split edge and round a split vertex, is closed
//!   with new faces: Coons patches (B-spline surfaces through four boundary curves) on
//!   the gaps' edges, so the new faces meet the old exactly.
//!
//! What can't be repaired is left as it is, and validation refuses the body.

use std::collections::HashMap;
use std::sync::Arc;

use peet_kernel::geom::{Curve3, Surface};
use peet_kernel::nurbs::{NurbsCurve, NurbsSurface, compatible};
use peet_kernel::validate::{EDGE_ON_SURFACE, VERTEX_ON_CURVE};
use peet_math::{DMat3, DVec3, Plane};

use super::heal::{HEAL_FIT, distance, drift};
use super::{Builder, HEAL_LIMIT, RawEdge, RawFace, Use};

/// How far (mm) a vertex may be from where its faces meet, or an edge from its faces,
/// and still be repaired: the tolerances SolidWorks gives a body it imported from
/// elsewhere are mostly under a millimetre. Further off, the faces are not meant to
/// meet there (a cap that doesn't fit its tube), and the body is refused.
pub(super) const REPAIR_LIMIT: f64 = 1.0;

/// How many pieces an edge is cut into to measure how far it is from its faces.
const DRIFT_SAMPLES: usize = 32;

/// How far (mm) a corner may be from the surfaces that meet there: well within the
/// tolerance of the vertices that have it, and so close that the edges that end there
/// come onto their faces long before validation looks at them.
const CORNER_SLACK: f64 = 1e-3;

/// Corners of a split vertex closer than this (mm) are made one.
const MERGE: f64 = HEAL_LIMIT;

/// What repairing a body did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Repaired {
    /// Vertices moved to where their faces meet.
    pub moved: usize,
    /// Vertices split, with gaps filled round them.
    pub split: usize,
    /// Edges split in two, one for each face.
    pub split_edges: usize,
    /// Faces added to close the gaps.
    pub filled: usize,
    /// How far (mm) the worst vertex or edge was from its faces.
    pub worst: f64,
    /// Faces left out that were slivers between two edges in the same place.
    pub slivers: usize,
    /// Edges given new curves on both their faces (as tolerant edges are healed).
    pub refitted: usize,
    /// How far (mm) the worst of those was from its faces.
    pub refit_worst: f64,
}

impl Repaired {
    /// Whether anything more than refitting edges was done.
    pub(super) fn any(&self) -> bool {
        self.moved + self.split + self.split_edges + self.filled + self.slivers > 0
    }

    /// The warning for the body `name`.
    pub(super) fn describe(&self, name: &str) -> String {
        let count =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut done = Vec::new();
        if self.moved > 0 {
            done.push(format!(
                "{} moved to where the faces round {} meet",
                count(self.moved, "vertex", "vertices"),
                if self.moved == 1 { "it" } else { "them" }
            ));
        }
        if self.split > 0 || self.split_edges > 0 {
            done.push(format!(
                "{} and {} split where the faces round them don't meet, and {} added to close \
                 the gaps",
                count(self.split, "vertex", "vertices"),
                count(self.split_edges, "edge", "edges"),
                count(self.filled, "face", "faces")
            ));
        }
        if self.slivers > 0 {
            done.push(format!(
                "{} left out (each lay between two edges in the same place)",
                count(self.slivers, "sliver face", "sliver faces")
            ));
        }
        format!(
            "The body \"{name}\" had vertices up to {:.2} mm from where its faces meet (the \
             file gives vertices a tolerance, as SolidWorks does for a body it has imported) \
             and was repaired: {}. Its shape may differ from the file's by as much there.",
            self.worst,
            done.join("; ")
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    /// The vertex lies on its edges' curves.
    Fine,
    /// The vertex is moved to where its faces meet.
    Met(DVec3),
    /// The vertex is split.
    Split,
}

/// A face, by shell and index.
type FaceAt = (usize, usize);

/// A face's corner at a vertex: the face, and the edge ends (edge, whether its end)
/// its loop arrives by and leaves by.
type Corner = (FaceAt, (usize, bool), (usize, bool));

/// What is known of the edges and vertices before anything changes.
struct Survey {
    /// The faces that use each edge, with the way each uses it.
    uses: Vec<Vec<(FaceAt, bool)>>,
    /// Each vertex's edge ends: the edge and whether it is the edge's end (not start).
    ends: HashMap<usize, Vec<(usize, bool)>>,
}

impl Builder<'_> {
    /// Repairs tolerant vertices and edges (see the module docs). Run before the faces
    /// are brought into the kernel's form.
    pub(super) fn repair(&mut self) -> Repaired {
        let mut out = Repaired {
            slivers: self.remove_slivers(),
            ..Repaired::default()
        };
        let survey = self.survey();

        // Which vertices and edges are off, and how far.
        let mut state: HashMap<usize, State> = HashMap::new();
        let mut keys: Vec<usize> = survey.ends.keys().copied().collect();
        keys.sort_unstable();
        for &v in &keys {
            // How far the vertex is from its edges' curves and its faces' surfaces.
            let worst = survey.ends[&v]
                .iter()
                .map(|&(e, end)| self.curve_end(e, end).distance(self.vertices[v]))
                .chain(
                    self.vertex_surfaces(&survey, v)
                        .iter()
                        .map(|s| s.signed_distance(self.vertices[v]).abs()),
                )
                .fold(0.0, f64::max);
            if worst <= VERTEX_ON_CURVE {
                continue;
            }
            // Only manifold edges (two faces, or one face twice along a seam) can be
            // repaired.
            if survey.ends[&v]
                .iter()
                .any(|&(e, _)| !matches!(survey.uses[e].len(), 1 | 2))
            {
                continue;
            }
            out.worst = out.worst.max(worst);
            let surfaces = self.vertex_surfaces(&survey, v);
            let at = self.vertices[v];
            // A vertex its edges only just miss stays where it is, and they are brought
            // to it. Otherwise it may move a few times as far as it is off (but not
            // across a gap) to where its faces meet.
            let reach = REPAIR_LIMIT.min((4.0 * worst).max(0.05));
            let ends_off = survey.ends[&v]
                .iter()
                .map(|&(e, end)| self.curve_end(e, end).distance(at))
                .fold(0.0, f64::max);
            let s = if ends_off <= CORNER_SLACK {
                State::Met(at)
            } else {
                match meet(at, &surfaces, reach) {
                    Some(p) => State::Met(p),
                    None => State::Split,
                }
            };
            state.insert(v, s);
        }
        let bad_edge: Vec<bool> = (0..self.edges.len())
            .map(|e| {
                let edge = &self.edges[e];
                let surfaces = self.edge_surfaces(&survey, e);
                !surfaces.is_empty()
                    && edge.t0 < edge.t1
                    && sampled(&edge.curve, edge.t0, edge.t1, &surfaces) > EDGE_ON_SURFACE
            })
            .collect();
        if state.is_empty() && !bad_edge.contains(&true) {
            return out;
        }

        // A vertex is moved only if every edge that ends there can follow it; otherwise
        // it is split. Splitting one changes where its neighbours' edges end: repeat
        // until nothing changes.
        let mut whole: HashMap<usize, Option<(Curve3, f64, f64)>> = HashMap::new();
        loop {
            let mut changed = false;
            for &v in &keys {
                if !matches!(state.get(&v), Some(State::Met(_))) {
                    continue;
                }
                for &(e, _) in &survey.ends[&v] {
                    let fit = whole
                        .entry(e)
                        .or_insert_with(|| self.whole_fit(&survey, &state, e))
                        .is_some();
                    if !fit {
                        state.insert(v, State::Split);
                        changed = true;
                        // The edges that end here end somewhere else now.
                        for &(e, _) in &survey.ends[&v] {
                            whole.remove(&e);
                        }
                        break;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // What happens to each edge: a new curve on both faces, or split in two.
        let state_of = |v: usize| state.get(&v).copied().unwrap_or(State::Fine);
        let mut refits: Vec<(usize, Curve3, f64, f64)> = Vec::new();
        let mut split: Vec<usize> = Vec::new();
        for (e, &bad) in bad_edge.iter().enumerate() {
            let ends = [self.edges[e].start, self.edges[e].end].map(|v| self.rep(v));
            let touched = ends.iter().any(|&v| state_of(v) != State::Fine);
            if !touched && !bad {
                continue;
            }
            let fit = match whole.remove(&e) {
                Some(fit) => fit,
                None => self.whole_fit(&survey, &state, e),
            };
            match fit {
                Some((curve, t0, t1)) => refits.push((e, curve, t0, t1)),
                None if survey.uses[e].len() == 2
                    && survey.uses[e][0].0 != survey.uses[e][1].0
                    && !self.too_far(&survey, e) =>
                {
                    split.push(e)
                }
                None => {}
            }
        }
        out.refitted = refits.len();
        for (e, ..) in &refits {
            let edge = &self.edges[*e];
            let surfaces = self.edge_surfaces(&survey, *e);
            out.refit_worst = out
                .refit_worst
                .max(drift(&edge.curve, edge.t0, edge.t1, &surfaces));
        }
        // Moved vertices, and edges given new curves on both faces.
        for (&v, s) in &state {
            if let State::Met(p) = s {
                self.vertices[v] = *p;
                out.moved += 1;
            }
        }
        let split_set: std::collections::HashSet<usize> = split.iter().copied().collect();

        // The corners of each split vertex: where each edge ends, on each of its faces.
        // A node is (edge, end, face); a whole edge's two faces share one.
        let mut node_of: HashMap<(usize, bool, FaceAt), usize> = HashMap::new();
        let mut nodes: Vec<(DVec3, Vec<FaceAt>)> = Vec::new();
        let refit_end = |e: usize, end: bool| {
            refits
                .iter()
                .find(|r| r.0 == e)
                .map(|(_, c, t0, t1)| c.point(if end { *t1 } else { *t0 }))
        };
        for &v in &keys {
            if state_of(v) != State::Split {
                continue;
            }
            out.split += 1;
            for &(e, end) in &survey.ends[&v] {
                let faces: Vec<FaceAt> = survey.uses[e].iter().map(|u| u.0).collect();
                if split_set.contains(&e) {
                    for &f in &faces {
                        let surface = &self.shells[f.0][f.1].surface;
                        let p = onto(self.curve_end(e, end), &[surface]);
                        node_of.insert((e, end, f), nodes.len());
                        nodes.push((p, vec![f]));
                    }
                } else {
                    let p = refit_end(e, end).unwrap_or_else(|| self.curve_end(e, end));
                    for &f in &faces {
                        node_of.insert((e, end, f), nodes.len());
                    }
                    let mut unique = faces.clone();
                    unique.dedup();
                    nodes.push((p, unique));
                }
            }
        }
        // A face's two corners at a split vertex that are (nearly) one place are one.
        let mut group: Vec<usize> = (0..nodes.len()).collect();
        fn find(g: &mut [usize], mut x: usize) -> usize {
            while g[x] != x {
                g[x] = g[g[x]];
                x = g[x];
            }
            x
        }
        let corners = self.corners(&state);
        for &(f, (e_in, end_in), (e_out, end_out)) in &corners {
            let (Some(&a), Some(&b)) = (
                node_of.get(&(e_in, end_in, f)),
                node_of.get(&(e_out, end_out, f)),
            ) else {
                continue;
            };
            if nodes[a].0.distance(nodes[b].0) <= MERGE {
                let (ra, rb) = (find(&mut group, a), find(&mut group, b));
                group[ra] = rb;
            }
        }
        // So are the two ends of a split edge, one on each face, where the gap between
        // its faces closes up.
        for &e in &split {
            for end in [false, true] {
                let [(fa, _), (fb, _)] = [survey.uses[e][0], survey.uses[e][1]];
                let (Some(&a), Some(&b)) = (node_of.get(&(e, end, fa)), node_of.get(&(e, end, fb)))
                else {
                    continue;
                };
                if nodes[a].0.distance(nodes[b].0) <= MERGE {
                    let (ra, rb) = (find(&mut group, a), find(&mut group, b));
                    group[ra] = rb;
                }
            }
        }
        // Each group of corners becomes a vertex where its faces meet.
        let mut vertex_of_group: HashMap<usize, usize> = HashMap::new();
        let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
        for n in 0..nodes.len() {
            members.entry(find(&mut group, n)).or_default().push(n);
        }
        let mut roots: Vec<usize> = members.keys().copied().collect();
        roots.sort_unstable();
        for root in roots {
            let list = &members[&root];
            let average = list.iter().map(|&n| nodes[n].0).sum::<DVec3>() / list.len() as f64;
            let mut faces: Vec<FaceAt> = list.iter().flat_map(|&n| nodes[n].1.clone()).collect();
            faces.sort_unstable();
            faces.dedup();
            let surfaces: Vec<&Surface> = faces
                .iter()
                .map(|f| &self.shells[f.0][f.1].surface)
                .collect();
            let p = if list.len() == 1 {
                nodes[list[0]].0
            } else {
                meet(average, &surfaces, MERGE).unwrap_or(average)
            };
            let v = self.add_vertex(p);
            vertex_of_group.insert(root, v);
        }
        let mut vertex_of_node = |n: usize| vertex_of_group[&find(&mut group, n)];
        let node_vertex: HashMap<(usize, bool, FaceAt), usize> = node_of
            .iter()
            .map(|(k, &n)| (*k, vertex_of_node(n)))
            .collect();

        // Whole edges: new curves, ending at their corners where a vertex was split.
        for (e, curve, t0, t1) in refits {
            let ends = [false, true].map(|end| {
                let f = survey.uses[e][0].0;
                node_vertex.get(&(e, end, f)).copied()
            });
            let (a, b) = (
                ends[0].map(|v| self.vertices[v]),
                ends[1].map(|v| self.vertices[v]),
            );
            // A corner that moved when it was merged with another: fit again.
            let (curve, t0, t1) = match (a, b) {
                (None, None) => (curve, t0, t1),
                _ => {
                    let a = a.unwrap_or_else(|| curve.point(t0));
                    let b = b.unwrap_or_else(|| curve.point(t1));
                    let surfaces = self.edge_surfaces(&survey, e);
                    if curve.point(t0).distance(a) <= 0.1 * VERTEX_ON_CURVE
                        && curve.point(t1).distance(b) <= 0.1 * VERTEX_ON_CURVE
                    {
                        (curve, t0, t1)
                    } else {
                        refit(&curve, t0, t1, a, b, &surfaces).unwrap_or((curve, t0, t1))
                    }
                }
            };
            let edge = &mut self.edges[e];
            edge.curve = curve;
            edge.t0 = t0;
            edge.t1 = t1;
            if let Some(v) = ends[0] {
                edge.start = v;
            }
            if let Some(v) = ends[1] {
                edge.end = v;
            }
        }

        // Split edges: one for each face, on its surface.
        let mut halves: HashMap<(usize, FaceAt), usize> = HashMap::new();
        for &e in &split {
            out.split_edges += 1;
            for &(f, _) in &survey.uses[e] {
                let old = self.edges[e].clone();
                let at = |end: bool, b: &Self| -> usize {
                    node_vertex
                        .get(&(e, end, f))
                        .copied()
                        .unwrap_or_else(|| b.rep(if end { old.end } else { old.start }))
                };
                let (va, vb) = (at(false, self), at(true, self));
                let (a, b) = (self.vertices[va], self.vertices[vb]);
                let surface = self.shells[f.0][f.1].surface.clone();
                let new = refit(&old.curve, old.t0, old.t1, a, b, &[&surface]);
                let (curve, t0, t1) = new.unwrap_or_else(|| (straight(a, b), 0.0, 1.0));
                halves.insert((e, f), self.edges.len());
                self.edges.push(RawEdge {
                    curve,
                    start: va,
                    end: vb,
                    t0,
                    t1,
                });
            }
        }
        for (s, faces) in self.shells.iter_mut().enumerate() {
            for (fi, face) in faces.iter_mut().enumerate() {
                for uses in &mut face.loops {
                    for u in uses.iter_mut() {
                        if let Some(&h) = halves.get(&(u.0, (s, fi))) {
                            u.0 = h;
                        }
                    }
                }
            }
        }

        // Faces whose two edges at a split vertex now end apart get an edge between them.
        for s in 0..self.shells.len() {
            for f in 0..self.shells[s].len() {
                for l in 0..self.shells[s][f].loops.len() {
                    let mut i = 0;
                    while i < self.shells[s][f].loops[l].len() {
                        let uses = &self.shells[s][f].loops[l];
                        let n = uses.len();
                        let (a, b) = (self.use_end(uses[i]), self.use_start(uses[(i + 1) % n]));
                        if a != b {
                            let surface = self.shells[s][f].surface.clone();
                            let (pa, pb) = (self.vertices[a], self.vertices[b]);
                            let line = straight(pa, pb);
                            let (curve, t0, t1) = refit(&line, 0.0, 1.0, pa, pb, &[&surface])
                                .unwrap_or((line, 0.0, 1.0));
                            let edge = self.edges.len();
                            self.edges.push(RawEdge {
                                curve,
                                start: a,
                                end: b,
                                t0,
                                t1,
                            });
                            self.shells[s][f].loops[l].insert(i + 1, (edge, false));
                            i += 1;
                        }
                        i += 1;
                    }
                }
            }
        }

        // The gaps between the halves of each split edge.
        let mut gaps: Vec<(usize, Vec<Use>, u32)> = Vec::new();
        for &e in &split {
            let [(fa, ra), (fb, _)] = [survey.uses[e][0], survey.uses[e][1]];
            let (ea, eb) = (halves[&(e, fa)], halves[&(e, fb)]);
            let line = |b: &mut Self, from: usize, to: usize| -> Option<usize> {
                if from == to {
                    return None;
                }
                let (p, q) = (b.vertices[from], b.vertices[to]);
                b.edges.push(RawEdge {
                    curve: straight(p, q),
                    start: from,
                    end: to,
                    t0: 0.0,
                    t1: 1.0,
                });
                Some(b.edges.len() - 1)
            };
            let (sa, ea_end) = (self.edges[ea].start, self.edges[ea].end);
            let (sb, eb_end) = (self.edges[eb].start, self.edges[eb].end);
            let start_line = line(self, sa, sb);
            let end_line = line(self, ea_end, eb_end);
            let mut cycle = Vec::new();
            if !ra {
                cycle.push((ea, true));
                cycle.extend(start_line.map(|l| (l, false)));
                cycle.push((eb, false));
                cycle.extend(end_line.map(|l| (l, true)));
            } else {
                cycle.push((ea, false));
                cycle.extend(end_line.map(|l| (l, false)));
                cycle.push((eb, true));
                cycle.extend(start_line.map(|l| (l, true)));
            }
            gaps.push((fa.0, cycle, self.shells[fa.0][fa.1].id));
        }
        let mut pending: Vec<FaceAt> = Vec::new();
        for (s, cycle, id) in gaps {
            pending.push((s, self.shells[s].len()));
            self.shells[s].push(RawFace {
                id,
                surface: Surface::Plane(Plane::TOP),
                reversed: false,
                loops: vec![cycle],
            });
        }

        // What is still open (round split vertices) is closed the same way.
        for s in 0..self.shells.len() {
            self.close_gaps(s, &mut pending);
        }

        // Every face added has a placeholder surface: fit its patches.
        while let Some((s, f)) = pending.pop() {
            if self.patch(s, f, &mut pending).is_some() {
                out.filled += 1;
            }
        }
        out
    }

    /// Leaves out faces that are only slivers: a face bounded by two edges that lie
    /// within [`HEAL_LIMIT`] of each other all along (a tolerant modeller's leftover
    /// where two faces nearly touch). Its neighbours then meet along one of the two
    /// edges. Returns how many were left out.
    fn remove_slivers(&mut self) -> usize {
        let mut removed = 0;
        for s in 0..self.shells.len() {
            let mut f = 0;
            while f < self.shells[s].len() {
                let face = &self.shells[s][f];
                let sliver = match face.loops.as_slice() {
                    [uses] if uses.len() == 2 && uses[0].0 != uses[1].0 => {
                        let (a, b) = (uses[0].0, uses[1].0);
                        let (ea, eb) = (&self.edges[a], &self.edges[b]);
                        let close = (0..=16).all(|k| {
                            let p = ea.curve.point(ea.t0 + (ea.t1 - ea.t0) * k as f64 / 16.0);
                            let q = eb.curve.point(eb.curve.param(p));
                            p.distance(q) <= HEAL_LIMIT
                        });
                        (ea.t0 < ea.t1 && eb.t0 < eb.t1 && close).then_some((uses[0], uses[1]))
                    }
                    _ => None,
                };
                let Some((keep, gone)) = sliver else {
                    f += 1;
                    continue;
                };
                self.shells[s].remove(f);
                removed += 1;
                // The other face that used `gone` runs along it the other way: `keep`
                // the way the sliver ran along it does the same.
                for face in self.shells.iter_mut().flatten() {
                    for uses in &mut face.loops {
                        for u in uses.iter_mut() {
                            if u.0 == gone.0 {
                                *u = (keep.0, keep.1);
                            }
                        }
                    }
                }
            }
        }
        removed
    }

    fn survey(&self) -> Survey {
        let mut uses: Vec<Vec<(FaceAt, bool)>> = vec![Vec::new(); self.edges.len()];
        for (s, faces) in self.shells.iter().enumerate() {
            for (f, face) in faces.iter().enumerate() {
                for &(e, r) in face.loops.iter().flatten() {
                    uses[e].push(((s, f), r));
                }
            }
        }
        let mut ends: HashMap<usize, Vec<(usize, bool)>> = HashMap::new();
        for (i, e) in self.edges.iter().enumerate() {
            if uses[i].is_empty() {
                continue;
            }
            ends.entry(self.rep(e.start)).or_default().push((i, false));
            ends.entry(self.rep(e.end)).or_default().push((i, true));
        }
        Survey { uses, ends }
    }

    fn curve_end(&self, e: usize, end: bool) -> DVec3 {
        let edge = &self.edges[e];
        edge.curve.point(if end { edge.t1 } else { edge.t0 })
    }

    /// The distinct surfaces of the faces that use edge `e`.
    fn edge_surfaces(&self, survey: &Survey, e: usize) -> Vec<&Surface> {
        let mut out: Vec<&Surface> = Vec::new();
        for &((s, f), _) in &survey.uses[e] {
            let surface = &self.shells[s][f].surface;
            if !out.contains(&surface) {
                out.push(surface);
            }
        }
        out
    }

    fn vertex_surfaces(&self, survey: &Survey, v: usize) -> Vec<&Surface> {
        let mut out: Vec<&Surface> = Vec::new();
        for &(e, _) in &survey.ends[&v] {
            for s in self.edge_surfaces(survey, e) {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
        out
    }

    /// Where edge `e` ends at its `end`, given the vertices' states: `None` where it
    /// ends at a split vertex and its faces don't meet near the end of its curve.
    fn end_point(
        &self,
        survey: &Survey,
        state: &HashMap<usize, State>,
        e: usize,
        end: bool,
    ) -> Option<DVec3> {
        let edge = &self.edges[e];
        let v = self.rep(if end { edge.end } else { edge.start });
        match state.get(&v).copied().unwrap_or(State::Fine) {
            State::Fine => Some(self.vertices[v]),
            State::Met(p) => Some(p),
            State::Split => {
                let surfaces = self.edge_surfaces(survey, e);
                let c = self.curve_end(e, end);
                let q = onto(c, &surfaces);
                (distance(q, &surfaces) <= 0.1 * HEAL_FIT && q.distance(c) <= REPAIR_LIMIT)
                    .then_some(q)
            }
        }
    }

    /// Edge `e` with a curve on both its faces, between where it ends.
    fn whole_fit(
        &self,
        survey: &Survey,
        state: &HashMap<usize, State>,
        e: usize,
    ) -> Option<(Curve3, f64, f64)> {
        let a = self.end_point(survey, state, e, false)?;
        let b = self.end_point(survey, state, e, true)?;
        if self.too_far(survey, e) {
            return None;
        }
        let edge = &self.edges[e];
        let surfaces = self.edge_surfaces(survey, e);
        refit(&edge.curve, edge.t0, edge.t1, a, b, &surfaces)
    }

    /// Whether edge `e` strays further from its faces than [`REPAIR_LIMIT`]: then it is
    /// not where its faces are at all, and is left alone.
    fn too_far(&self, survey: &Survey, e: usize) -> bool {
        let edge = &self.edges[e];
        let surfaces = self.edge_surfaces(survey, e);
        edge.t0 < edge.t1 && drift(&edge.curve, edge.t0, edge.t1, &surfaces) > REPAIR_LIMIT
    }

    /// Each face's corners at split vertices: the face, and the edge ends that meet
    /// there (the one its loop arrives by, the one it leaves by).
    fn corners(&self, state: &HashMap<usize, State>) -> Vec<Corner> {
        let mut out = Vec::new();
        for (s, faces) in self.shells.iter().enumerate() {
            for (f, face) in faces.iter().enumerate() {
                for uses in &face.loops {
                    let n = uses.len();
                    for i in 0..n {
                        let (a, b) = (uses[i], uses[(i + 1) % n]);
                        let v = self.use_end(a);
                        if state.get(&v) != Some(&State::Split) {
                            continue;
                        }
                        // The use arrives at the edge's end unless it runs against it.
                        out.push(((s, f), (a.0, !a.1), (b.0, b.1)));
                    }
                }
            }
        }
        out
    }

    /// Closes what is left open in shell `s` (edges used by one face only) with new
    /// faces, one for each hole; two edges that bound a hole between them and lie in
    /// the same place are made one instead.
    fn close_gaps(&mut self, s: usize, pending: &mut Vec<FaceAt>) {
        let mut count: HashMap<usize, Vec<(usize, usize, bool)>> = HashMap::new();
        for (f, face) in self.shells[s].iter().enumerate() {
            for (l, uses) in face.loops.iter().enumerate() {
                for &(e, r) in uses {
                    count.entry(e).or_default().push((f, l, r));
                }
            }
        }
        // The half-edges a hole needs: each open edge the other way round.
        let mut open: Vec<Use> = count
            .iter()
            .filter(|(_, u)| u.len() == 1)
            .map(|(&e, u)| (e, !u[0].2))
            .collect();
        open.sort_unstable();
        let mut used = vec![false; open.len()];
        for k in 0..open.len() {
            if used[k] {
                continue;
            }
            let mut cycle = vec![open[k]];
            used[k] = true;
            let start = self.use_start(open[k]);
            let mut at = self.use_end(open[k]);
            let mut ok = true;
            while at != start {
                let next = (0..open.len()).find(|&j| !used[j] && self.use_start(open[j]) == at);
                let Some(j) = next else {
                    ok = false;
                    break;
                };
                used[j] = true;
                cycle.push(open[j]);
                at = self.use_end(open[j]);
                if cycle.len() > 64 {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            if cycle.len() == 2 && self.same_place(cycle[0].0, cycle[1].0) {
                // Two edges in one place: the second's user takes the first.
                let (keep, (gone, _)) = (cycle[0], cycle[1]);
                for face in &mut self.shells[s] {
                    for uses in &mut face.loops {
                        for u in uses.iter_mut() {
                            if u.0 == gone {
                                *u = keep;
                            }
                        }
                    }
                }
                continue;
            }
            let id = count[&cycle[0].0][0].0;
            let id = self.shells[s][id].id;
            pending.push((s, self.shells[s].len()));
            self.shells[s].push(RawFace {
                id,
                surface: Surface::Plane(Plane::TOP),
                reversed: false,
                loops: vec![cycle],
            });
        }
    }

    fn same_place(&self, a: usize, b: usize) -> bool {
        let (ea, eb) = (&self.edges[a], &self.edges[b]);
        (0..=8).all(|k| {
            let p = ea.curve.point(ea.t0 + (ea.t1 - ea.t0) * k as f64 / 8.0);
            let q = eb
                .curve
                .point(eb.t0 + (eb.t1 - eb.t0) * (8 - k) as f64 / 8.0);
            let r = eb.curve.point(eb.t0 + (eb.t1 - eb.t0) * k as f64 / 8.0);
            p.distance(q).min(p.distance(r)) <= 0.1 * EDGE_ON_SURFACE
        })
    }

    /// Fits Coons patches to the placeholder face `f` of shell `s`: one if it has four
    /// sides, after splitting sides if it has two or three, and one for each side round
    /// a middle point if it has more (the others are added to `pending`).
    fn patch(&mut self, s: usize, f: usize, pending: &mut Vec<FaceAt>) -> Option<()> {
        let n = self.shells[s][f].loops[0].len();
        match n {
            0 | 1 => return None,
            2 => {
                for k in (0..2).rev() {
                    let (e, _) = self.shells[s][f].loops[0][k];
                    let mid = 0.5 * (self.edges[e].t0 + self.edges[e].t1);
                    self.split_edge(e, mid).ok()?;
                }
            }
            3 => {
                let longest = (0..3)
                    .max_by(|&a, &b| {
                        let len = |k: usize| {
                            let (e, _) = self.shells[s][f].loops[0][k];
                            let edge = &self.edges[e];
                            edge.curve
                                .point(edge.t0)
                                .distance(edge.curve.point(edge.t1))
                                + edge
                                    .curve
                                    .point(0.5 * (edge.t0 + edge.t1))
                                    .distance(edge.curve.point(edge.t0))
                        };
                        len(a).total_cmp(&len(b))
                    })
                    .unwrap_or(0);
                let (e, _) = self.shells[s][f].loops[0][longest];
                let mid = 0.5 * (self.edges[e].t0 + self.edges[e].t1);
                self.split_edge(e, mid).ok()?;
            }
            4 => {}
            _ => {
                // Round a middle point: each side split at its middle, a quad for each
                // corner.
                let sides: Vec<Use> = self.shells[s][f].loops[0].clone();
                for &(e, _) in &sides {
                    let mid = 0.5 * (self.edges[e].t0 + self.edges[e].t1);
                    self.split_edge(e, mid).ok()?;
                }
                let uses = self.shells[s][f].loops[0].clone();
                let m = uses.len() / 2;
                // uses[2i] runs from corner i to the middle of side i, uses[2i+1] on.
                let mids: Vec<usize> = (0..m).map(|i| self.use_end(uses[2 * i])).collect();
                let centre = mids.iter().map(|&v| self.vertices[v]).sum::<DVec3>() / m as f64;
                let c = self.add_vertex(centre);
                let spokes: Vec<usize> = mids
                    .iter()
                    .map(|&v| {
                        self.edges.push(RawEdge {
                            curve: straight(self.vertices[v], centre),
                            start: v,
                            end: c,
                            t0: 0.0,
                            t1: 1.0,
                        });
                        self.edges.len() - 1
                    })
                    .collect();
                let id = self.shells[s][f].id;
                let mut quads = Vec::new();
                for i in 0..m {
                    let j = (i + 1) % m;
                    quads.push(vec![
                        uses[2 * i + 1],
                        uses[2 * j],
                        (spokes[j], false),
                        (spokes[i], true),
                    ]);
                }
                self.shells[s][f].loops[0] = quads[0].clone();
                for q in quads.into_iter().skip(1) {
                    pending.push((s, self.shells[s].len()));
                    self.shells[s].push(RawFace {
                        id,
                        surface: Surface::Plane(Plane::TOP),
                        reversed: false,
                        loops: vec![q],
                    });
                }
            }
        }
        self.coons(s, f)
    }

    /// Fits face `f` of shell `s`, a loop of four edges, with the Coons patch through
    /// them.
    fn coons(&mut self, s: usize, f: usize) -> Option<()> {
        let uses = self.shells[s][f].loops[0].clone();
        if uses.len() != 4 {
            return None;
        }
        let sides: Vec<NurbsCurve> = uses
            .iter()
            .map(|&(e, r)| {
                let c = nurbs_of(&self.edges[e])?;
                Some(if r { c.reversed() } else { c })
            })
            .collect::<Option<_>>()?;
        // Bottom and top run along u, left and right along v.
        let along_u = compatible(&[sides[0].clone(), sides[2].reversed()]).ok()?;
        let along_v = compatible(&[sides[3].reversed(), sides[1].clone()]).ok()?;
        let (c0, c1) = (&along_u[0], &along_u[1]);
        let (d0, d1) = (&along_v[0], &along_v[1]);
        let corners = [c0.point(0.0), c0.point(1.0), c1.point(1.0), c1.point(0.0)];
        let greville = |c: &NurbsCurve| -> Vec<f64> {
            let p = c.degree();
            let k = c.knots();
            (0..c.control_points().len())
                .map(|i| k[i + 1..=i + p].iter().sum::<f64>() / p as f64)
                .collect()
        };
        let (xi, eta) = (greville(c0), greville(d0));
        let mut points = Vec::with_capacity(xi.len() * eta.len());
        for (i, &u) in xi.iter().enumerate() {
            for (j, &v) in eta.iter().enumerate() {
                let ruled_v = c0.control_points()[i] * (1.0 - v) + c1.control_points()[i] * v;
                let ruled_u = d0.control_points()[j] * (1.0 - u) + d1.control_points()[j] * u;
                let bilinear = corners[0] * (1.0 - u) * (1.0 - v)
                    + corners[1] * u * (1.0 - v)
                    + corners[2] * u * v
                    + corners[3] * (1.0 - u) * v;
                points.push(ruled_v + ruled_u - bilinear);
            }
        }
        let surface = NurbsSurface::new(
            c0.degree(),
            d0.degree(),
            c0.knots().to_vec(),
            d0.knots().to_vec(),
            points,
            None,
        )
        .ok()?;
        let face = &mut self.shells[s][f];
        face.surface = Surface::Nurbs(Arc::new(surface));
        face.reversed = false;
        Some(())
    }
}

/// The edge's stretch of its curve as a B-spline (non-rational, so that Coons patches
/// through it are exact).
fn nurbs_of(edge: &RawEdge) -> Option<NurbsCurve> {
    match &edge.curve {
        Curve3::Line(_) => Some(NurbsCurve::line(
            edge.curve.point(edge.t0),
            edge.curve.point(edge.t1),
            1,
        )),
        Curve3::Nurbs(c) => {
            if c.weights().is_some() {
                return None;
            }
            let (lo, hi) = c.domain();
            if (edge.t0 - lo).abs() < 1e-12 && (edge.t1 - hi).abs() < 1e-12 {
                Some((**c).clone())
            } else {
                c.sub_curve(edge.t0, edge.t1).ok()
            }
        }
        _ => None,
    }
}

/// The straight segment from `a` to `b` as a curve over `[0, 1]`.
fn straight(a: DVec3, b: DVec3) -> Curve3 {
    Curve3::Nurbs(Arc::new(NurbsCurve::line(a, b, 1)))
}

/// The point within `reach` of `p` that comes nearest to all of `surfaces`, if it is
/// within [`CORNER_SLACK`] of every one of them. A file's faces may meet only loosely
/// at a corner (by a few ten-thousandths of a millimetre where four faces meet at a
/// vertex of a fillet), and there may be no point exactly on all of them.
fn meet(p: DVec3, surfaces: &[&Surface], reach: f64) -> Option<DVec3> {
    let q = closest(p, surfaces, 1e-9, reach)?;
    (distance(q, surfaces) <= CORNER_SLACK).then_some(q)
}

/// The point on (or nearest to) all of `surfaces` near `p`: Levenberg-Marquardt on the
/// surfaces' signed distances, each step the smallest that takes them all to zero to
/// first order, damped by at least `damping` and more where a step would make things
/// worse. Where the surfaces nearly touch, the damping keeps the steps short across
/// them, so that the point stays near `p` and points moved from along a curve stay
/// along a smooth one.
fn closest(p: DVec3, surfaces: &[&Surface], damping: f64, reach: f64) -> Option<DVec3> {
    let cost = |q: DVec3| -> f64 { surfaces.iter().map(|s| s.signed_distance(q).powi(2)).sum() };
    let mut q = p;
    let mut c = cost(q);
    let mut lambda = damping.max(1e-9);
    for _ in 0..100 {
        if c.sqrt() <= 0.01 * HEAL_FIT {
            break;
        }
        let mut jtj = DMat3::ZERO;
        let mut jtd = DVec3::ZERO;
        for s in surfaces {
            let n = s.normal_at(q);
            let d = s.signed_distance(q);
            jtj += DMat3::from_cols(n * n.x, n * n.y, n * n.z);
            jtd += n * d;
        }
        let mut moved = false;
        while lambda < 1e6 {
            let step = (jtj + DMat3::IDENTITY * lambda).inverse() * jtd;
            let mut next = q - step;
            // No further than `reach` from where it started.
            let away = next - p;
            if away.length() > reach {
                next = p + away * (reach / away.length());
            }
            let nc = cost(next);
            if next.is_finite() && nc < c {
                q = next;
                c = nc;
                lambda = (lambda * 0.1).max(damping.max(1e-9));
                moved = true;
                break;
            }
            lambda *= 10.0;
        }
        if !moved {
            break;
        }
    }
    Some(q)
}

/// `p` moved onto `surfaces` (one or two) by Newton steps along their normals, as
/// tolerant edges are healed, but damped: where two surfaces nearly touch, an undamped
/// step would slide the point a long way along them (and points moved from along a
/// curve would wander), so the step across is kept short.
fn onto(p: DVec3, surfaces: &[&Surface]) -> DVec3 {
    const DAMPING: f64 = 1e-3;
    let mut q = p;
    for _ in 0..48 {
        if distance(q, surfaces) <= 0.01 * HEAL_FIT {
            break;
        }
        q -= match surfaces {
            [a, b] => {
                let (da, db) = (a.signed_distance(q), b.signed_distance(q));
                let (na, nb) = (a.normal_at(q), b.normal_at(q));
                let c = na.dot(nb);
                let k = 1.0 + DAMPING;
                let det = k * k - c * c;
                na * ((k * da - c * db) / det) + nb * ((k * db - c * da) / det)
            }
            [a] => a.normal_at(q) * a.signed_distance(q),
            _ => return q,
        };
    }
    q
}

/// A curve from `a` to `b` on `surfaces` (one or two), close to `curve` over `t0..t1`:
/// the same curve over a new range if it passes through both, otherwise a cubic fitted
/// through the curve's points, moved by as much as its ends move and then onto the
/// surfaces.
pub(super) fn refit(
    curve: &Curve3,
    t0: f64,
    t1: f64,
    a: DVec3,
    b: DVec3,
    surfaces: &[&Surface],
) -> Option<(Curve3, f64, f64)> {
    if surfaces.is_empty() || surfaces.len() > 2 || t0 >= t1 {
        return None;
    }
    if !matches!(curve, Curve3::Nurbs(_)) {
        let (s0, mut s1) = (curve.param(a), curve.param(b));
        if curve.period().is_some() {
            while s1 <= s0 {
                s1 += std::f64::consts::TAU;
            }
        }
        if curve.point(s0).distance(a) <= 0.1 * VERTEX_ON_CURVE
            && curve.point(s1).distance(b) <= 0.1 * VERTEX_ON_CURVE
            && drift(curve, s0, s1, surfaces) <= 0.5 * EDGE_ON_SURFACE
        {
            return Some((curve.clone(), s0, s1));
        }
    }
    let (da, db) = (a - curve.point(t0), b - curve.point(t1));
    // Ends that move very little: the curve is kept, and only bent near its ends to
    // reach them, if it lies on the surfaces as it is.
    if da.length().max(db.length()) <= CORNER_SLACK {
        let shape = |t: f64| {
            let k = (t - t0) / (t1 - t0);
            Some(curve.point(t) + da * fade(k) + db * fade(1.0 - k))
        };
        if let Some(bent) = fitted(&shape, curve, t0, t1)
            && bent.point(t0).distance(a) <= 0.1 * VERTEX_ON_CURVE
            && bent.point(t1).distance(b) <= 0.1 * VERTEX_ON_CURVE
            && sampled(&bent, t0, t1, surfaces) <= 0.9 * EDGE_ON_SURFACE
            && (1..DRIFT_SAMPLES)
                .map(|k| k as f64 / DRIFT_SAMPLES as f64)
                .filter(|k| (0.2..=0.8).contains(k))
                .all(|k| {
                    distance(bent.point(t0 + (t1 - t0) * k), surfaces) <= 0.5 * EDGE_ON_SURFACE
                })
        {
            return Some((bent, t0, t1));
        }
    }
    // Ends a little off the surfaces (within what the kernel allows a vertex) stay
    // where they are; the curve comes onto the surfaces away from them.
    let (off_a, off_b) = (a - onto(a, surfaces), b - onto(b, surfaces));
    let shape = |t: f64| {
        let k = (t - t0) / (t1 - t0);
        let on = onto(curve.point(t) + da * (1.0 - k) + db * k, surfaces);
        Some(on + off_a * fade(k) + off_b * fade(1.0 - k))
    };
    // The curve may have creases at its knots; the fit may too.
    let mut breaks = vec![t0];
    if let Curve3::Nurbs(c) = curve {
        for &k in c.knots() {
            if k > t0 + 1e-9 * (t1 - t0)
                && k < t1 - 1e-9 * (t1 - t0)
                && k > breaks[breaks.len() - 1]
            {
                breaks.push(k);
            }
        }
    }
    breaks.push(t1);
    // How far off the surfaces the fit is (beyond what its ends are allowed): where two
    // surfaces nearly touch, the points moved onto both wander along them, and only
    // their distance from the surfaces matters.
    let (ea, eb) = (off_a.length(), off_b.length());
    let miss = |t: f64, p: DVec3| {
        let k = (t - t0) / (t1 - t0);
        Some((distance(p, surfaces) - ea * fade(k) - eb * fade(1.0 - k)).max(0.0))
    };
    // Where the surfaces don't meet along the curve no fit will do: say so before
    // trying.
    let meets = (1..16).all(|k| {
        let t = t0 + (t1 - t0) * k as f64 / 16.0;
        shape(t)
            .and_then(|p| miss(t, p))
            .is_some_and(|m| m <= HEAL_FIT)
    });
    if !meets {
        return None;
    }
    let fit = NurbsCurve::fitted_within(&shape, &miss, &breaks, 3, HEAL_FIT).ok()?;
    let new = Curve3::Nurbs(Arc::new(fit));
    // Off the surfaces by no more than the ends are, less further in; and well within
    // what validation allows where it looks.
    let inside = (1..DRIFT_SAMPLES)
        .map(|k| k as f64 / DRIFT_SAMPLES as f64)
        .map(|k| {
            let p = new.point(t0 + (t1 - t0) * k);
            distance(p, surfaces) - ea * fade(k) - eb * fade(1.0 - k)
        })
        .fold(0.0, f64::max);
    let sampled = sampled(&new, t0, t1, surfaces);
    if new.point(t0).distance(a) > 0.1 * VERTEX_ON_CURVE
        || new.point(t1).distance(b) > 0.1 * VERTEX_ON_CURVE
        || inside > 0.5 * EDGE_ON_SURFACE
        || sampled > 0.9 * EDGE_ON_SURFACE
    {
        return None;
    }
    Some((new, t0, t1))
}

/// How much of an end's distance from its surfaces a curve keeps at `k` (0 at that end,
/// 1 at the other): all of it at the end, none from a fifth of the way along.
fn fade(k: f64) -> f64 {
    (1.0 - 5.0 * k).max(0.0).powi(3)
}

/// How far `curve` is from `surfaces` where validation looks.
fn sampled(curve: &Curve3, t0: f64, t1: f64, surfaces: &[&Surface]) -> f64 {
    [0.25, 0.5, 0.75]
        .iter()
        .map(|k| distance(curve.point(t0 + (t1 - t0) * k), surfaces))
        .fold(0.0, f64::max)
}

/// A cubic through `shape` over `t0..t1`, with creases allowed at `like`'s knots.
fn fitted(shape: &dyn Fn(f64) -> Option<DVec3>, like: &Curve3, t0: f64, t1: f64) -> Option<Curve3> {
    let mut breaks = vec![t0];
    if let Curve3::Nurbs(c) = like {
        for &k in c.knots() {
            if k > t0 + 1e-9 * (t1 - t0)
                && k < t1 - 1e-9 * (t1 - t0)
                && k > breaks[breaks.len() - 1]
            {
                breaks.push(k);
            }
        }
    }
    breaks.push(t1);
    let fit = NurbsCurve::fitted(shape, &breaks, 3, HEAL_FIT).ok()?;
    Some(Curve3::Nurbs(Arc::new(fit)))
}
