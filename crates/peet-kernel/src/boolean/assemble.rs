//! Building the result solid from the selected face pieces.
//!
//! The pieces arrive as sets of directed edges on their surfaces. Assembly
//!
//! 1. merges pieces that lie on the same surface and share an edge (a union of two boxes
//!    that touch should not keep a seam across their common face plane), as long as the
//!    merged region is still a valid face;
//! 2. checks that every edge is used exactly once in each direction, pairing the faces
//!    around an edge where two bodies only touch along it;
//! 3. removes vertices that merely divide one curve between the same two faces;
//! 4. traces each face's loops, groups faces into shells by connectivity, and gives every
//!    fan of faces around a vertex its own vertex (bodies touching at a point).

use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;

use peet_math::{Aabb, DVec3};

use super::FaceSource;
use super::domain::Domain;
use super::faces::{TracedFace, prune_dangling, sort_ccw, trace_faces};
use super::util::{GEdge, HalfEdge, same_carrier, same_surface};
use crate::geom::Surface;
use crate::topo::{CoedgeId, EdgeId, Solid, VertexId};

/// A face of the result in the making: a surface, its orientation and its directed edges.
#[derive(Clone, Debug)]
pub(crate) struct Patch {
    pub surface: Surface,
    pub reversed: bool,
    pub half_edges: Vec<HalfEdge>,
    /// The input faces this piece is part of: more than one once pieces were merged.
    pub sources: Vec<FaceSource>,
}

impl Patch {
    fn domain(&self) -> Domain {
        Domain::new(&self.surface, self.reversed)
    }

    fn has(&self, h: HalfEdge) -> bool {
        self.half_edges.contains(&h)
    }
}

/// Builds the result solid and, for each of its faces (in face order), the input faces it
/// is made of.
pub(crate) fn assemble(
    patches: Vec<Patch>,
    mut edges: Vec<GEdge>,
    points: &[DVec3],
) -> Result<(Solid, Vec<Vec<FaceSource>>), String> {
    let mut patches = merge_patches(patches, &edges, points);
    pair_edge_uses(&mut patches, &mut edges)?;
    merge_edges(&mut patches, &mut edges, points);
    build(&patches, &edges, points)
}

fn edge_uses(patches: &[Patch]) -> HashMap<u32, Vec<(usize, bool)>> {
    let mut uses: HashMap<u32, Vec<(usize, bool)>> = HashMap::new();
    for (pi, p) in patches.iter().enumerate() {
        for &(e, forward) in &p.half_edges {
            uses.entry(e).or_default().push((pi, forward));
        }
    }
    uses
}

// ---- 1. Merging pieces on one surface ----

fn merge_patches(patches: Vec<Patch>, edges: &[GEdge], points: &[DVec3]) -> Vec<Patch> {
    let mut patches: Vec<Option<Patch>> = patches.into_iter().map(Some).collect();
    let mut rejected: HashSet<(usize, usize)> = HashSet::new();
    loop {
        let mut uses: HashMap<u32, Vec<(usize, bool)>> = HashMap::new();
        for (pi, p) in patches.iter().enumerate() {
            for &(e, forward) in p.iter().flat_map(|p| &p.half_edges) {
                uses.entry(e).or_default().push((pi, forward));
            }
        }
        let mut keys: Vec<u32> = uses.keys().copied().collect();
        keys.sort_unstable();
        let mut dirty: HashSet<usize> = HashSet::new();
        for e in keys {
            let list = &uses[&e];
            'pairs: for &(p1, d1) in list {
                for &(p2, d2) in list {
                    if p1 >= p2 || d1 == d2 || dirty.contains(&p1) || dirty.contains(&p2) {
                        continue;
                    }
                    if rejected.contains(&(p1, p2)) {
                        continue;
                    }
                    let (Some(a), Some(b)) = (&patches[p1], &patches[p2]) else {
                        continue;
                    };
                    let at = edges[e as usize].mid();
                    if !same_surface(&a.surface, &b.surface)
                        || a.domain().outward(at).dot(b.domain().outward(at)) <= 0.0
                    {
                        rejected.insert((p1, p2));
                        continue;
                    }
                    let others = |e: u32| uses.get(&e).map_or(0, Vec::len);
                    match try_merge(a, b, edges, points, &others) {
                        Some(merged) => {
                            patches[p1] = Some(merged);
                            patches[p2] = None;
                            dirty.insert(p1);
                            dirty.insert(p2);
                            // Pairs involving the old patches must be looked at afresh.
                            rejected.retain(|&(x, y)| x != p1 && y != p1 && x != p2 && y != p2);
                            break 'pairs;
                        }
                        None => {
                            rejected.insert((p1, p2));
                        }
                    }
                }
            }
        }
        if dirty.is_empty() {
            break;
        }
    }
    patches.into_iter().flatten().collect()
}

/// The union of two pieces on one surface without the edges between them, if that is one
/// valid face. When removing every shared edge would leave a cylinder wall without a seam,
/// one shared edge is kept as the seam.
fn try_merge(
    a: &Patch,
    b: &Patch,
    edges: &[GEdge],
    points: &[DVec3],
    use_count: &dyn Fn(u32) -> usize,
) -> Option<Patch> {
    let mut shared: Vec<u32> = a
        .half_edges
        .iter()
        .filter(|&&(e, f)| b.has((e, !f)) && !a.has((e, !f)) && !b.has((e, f)))
        .map(|h| h.0)
        .collect();
    shared.sort_unstable();
    shared.dedup();
    if shared.is_empty() {
        return None;
    }
    let attempt = |keep: Option<u32>| -> Option<Patch> {
        let half_edges: Vec<HalfEdge> = a
            .half_edges
            .iter()
            .chain(&b.half_edges)
            .copied()
            .filter(|h| Some(h.0) == keep || !shared.contains(&h.0))
            .collect();
        let half_edges = prune_dangling(&half_edges, edges);
        if half_edges.is_empty() {
            return None;
        }
        let traced = trace_faces(&a.domain(), &half_edges, edges, points).ok()?;
        let mut sources = a.sources.clone();
        sources.extend(&b.sources);
        sources.sort_unstable();
        sources.dedup();
        (traced.len() == 1).then_some(Patch {
            surface: a.surface,
            reversed: a.reversed,
            half_edges,
            sources,
        })
    };
    attempt(None).or_else(|| {
        // Prefer a seam no other face uses (an edge where another body touches this
        // wall has more than these two uses).
        let mut seams = shared.clone();
        seams.sort_by_key(|&e| (use_count(e), e));
        (seams.len() > 1)
            .then(|| seams.iter().find_map(|&keep| attempt(Some(keep))))
            .flatten()
    })
}

// ---- 2. Two uses per edge ----

/// Checks that every edge is used once forwards and once backwards. An edge used by four
/// (or more) faces, where bodies touch along it, is duplicated so that each pair of faces
/// enclosing one wedge of material gets its own edge.
fn pair_edge_uses(patches: &mut [Patch], edges: &mut Vec<GEdge>) -> Result<(), String> {
    let uses = edge_uses(patches);
    let mut keys: Vec<u32> = uses.keys().copied().collect();
    keys.sort_unstable();
    for e in keys {
        let list = &uses[&e];
        let forward = list.iter().filter(|u| u.1).count();
        let backward = list.len() - forward;
        if forward == 1 && backward == 1 {
            continue;
        }
        if forward != backward {
            let at = edges[e as usize].mid();
            return Err(format!(
                "the result is not closed near ({:.4}, {:.4}, {:.4}): an edge is used by \
                 {forward} face(s) one way and {backward} the other",
                at.x, at.y, at.z
            ));
        }
        let g = edges[e as usize].clone();
        // A face using the edge both ways has it as a seam: those two uses belong
        // together, on an edge of their own.
        let mut rest: Vec<(usize, bool)> = Vec::new();
        let mut first = true;
        for &(pi, fwd) in list {
            if !list.contains(&(pi, !fwd)) {
                rest.push((pi, fwd));
            } else if fwd {
                if first {
                    first = false;
                    continue;
                }
                let copy = edges.len() as u32;
                edges.push(g.clone());
                for h in &mut patches[pi].half_edges {
                    if h.0 == e {
                        h.0 = copy;
                    }
                }
            }
        }
        let list = &rest;
        // Order the other faces around the edge by the direction in which each leaves it.
        let tm = 0.5 * (g.t0 + g.t1);
        let at = g.curve.point(tm);
        let tangent = g.curve.tangent(tm);
        let e1 = tangent.any_orthonormal_vector();
        let e2 = tangent.cross(e1);
        // (angle, bend, material on the counter-clockwise side, patch, forward)
        let mut fan: Vec<(f64, f64, bool, usize, bool)> = list
            .iter()
            .map(|&(pi, fwd)| {
                let n = patches[pi].domain().outward(at);
                let along = if fwd { tangent } else { -tangent };
                let into_face = n.cross(along);
                let ccw = tangent.cross(into_face);
                (
                    into_face.dot(e2).atan2(into_face.dot(e1)),
                    // Faces tangent to each other along the edge (a cylinder touching a
                    // plane) are told apart by how the surface curves away from it.
                    section_curvature(&patches[pi].surface, at, into_face).dot(ccw),
                    n.dot(ccw) < 0.0,
                    pi,
                    fwd,
                )
            })
            .collect();
        sort_ccw(&mut fan, |f| f.0, |f| f.1);
        let fan: Vec<(f64, bool, usize, bool)> =
            fan.into_iter().map(|f| (f.0, f.2, f.3, f.4)).collect();
        let n = fan.len();
        for i in 0..n {
            if !fan[i].1 {
                continue;
            }
            let j = (i + 1) % n;
            if fan[j].1 || fan[i].3 == fan[j].3 {
                return Err(format!(
                    "the bodies touch along an edge near ({:.4}, {:.4}, {:.4}) in a way                      that can't be separated into solids",
                    at.x, at.y, at.z
                ));
            }
            if first {
                first = false;
                continue;
            }
            let copy = edges.len() as u32;
            edges.push(g.clone());
            for &(_, _, pi, fwd) in &[fan[i], fan[j]] {
                for h in &mut patches[pi].half_edges {
                    if *h == (e, fwd) {
                        *h = (copy, fwd);
                    }
                }
            }
        }
    }
    Ok(())
}

// ---- 3. Removing redundant vertices ----

/// Joins two edges that meet at a vertex nothing else uses, lie on one curve and separate
/// the same two faces.
fn merge_edges(patches: &mut [Patch], edges: &mut Vec<GEdge>, points: &[DVec3]) {
    loop {
        let uses = edge_uses(patches);
        let mut incident: HashMap<u32, Vec<u32>> = HashMap::new();
        for &e in uses.keys() {
            let g = &edges[e as usize];
            incident.entry(g.start).or_default().push(e);
            incident.entry(g.end).or_default().push(e);
        }
        let mut vertices: Vec<u32> = incident.keys().copied().collect();
        vertices.sort_unstable();
        let mut touched: HashSet<u32> = HashSet::new();
        for v in vertices {
            let list = &incident[&v];
            if list.len() != 2 || list[0] == list[1] {
                continue;
            }
            let (e1, e2) = (list[0].min(list[1]), list[0].max(list[1]));
            if touched.contains(&e1) || touched.contains(&e2) {
                continue;
            }
            let (g1, g2) = (&edges[e1 as usize], &edges[e2 as usize]);
            if g1.start == g1.end
                || g2.start == g2.end
                || !same_carrier(&g1.curve, g1.t0, g1.t1, &g2.curve, g2.t0, g2.t1)
            {
                continue;
            }
            let far = if g2.start == v { g2.end } else { g2.start };
            let far_point = points[far as usize];
            let periodic = g1.curve.period().is_some();
            let closes = far == if g1.end == v { g1.start } else { g1.end };
            if closes && !periodic {
                continue;
            }
            // The joined edge keeps e1's curve and direction.
            let (joined, e2_aligned) = if g1.end == v {
                let t1 = if closes {
                    g1.t0 + TAU
                } else if periodic {
                    g1.t1 + (g1.curve.param(far_point) - g1.t1).rem_euclid(TAU)
                } else {
                    g1.curve.param(far_point)
                };
                (
                    GEdge {
                        curve: g1.curve,
                        t0: g1.t0,
                        t1,
                        start: g1.start,
                        end: far,
                    },
                    g2.start == v,
                )
            } else {
                let t0 = if closes {
                    g1.t1 - TAU
                } else if periodic {
                    g1.t0 - (g1.t0 - g1.curve.param(far_point)).rem_euclid(TAU)
                } else {
                    g1.curve.param(far_point)
                };
                (
                    GEdge {
                        curve: g1.curve,
                        t0,
                        t1: g1.t1,
                        start: far,
                        end: g1.end,
                    },
                    g2.end == v,
                )
            };
            let span = joined.t1 - joined.t0;
            let old_span = g1.t1 - g1.t0;
            if span <= old_span || (periodic && span > TAU + 1e-9) {
                continue;
            }
            let mut u1 = uses[&e1].clone();
            let mut u2: Vec<(usize, bool)> = uses[&e2]
                .iter()
                .map(|&(p, d)| (p, d == e2_aligned))
                .collect();
            u1.sort_unstable();
            u2.sort_unstable();
            if u1 != u2 {
                continue;
            }
            let id = edges.len() as u32;
            edges.push(joined);
            for &(p, d) in &u1 {
                let hes = &mut patches[p].half_edges;
                hes.retain(|h| h.0 != e2);
                for h in hes.iter_mut() {
                    if *h == (e1, d) {
                        *h = (id, d);
                    }
                }
            }
            touched.insert(e1);
            touched.insert(e2);
        }
        if touched.is_empty() {
            break;
        }
    }
}

// ---- 4. The solid ----

fn build(
    patches: &[Patch],
    edges: &[GEdge],
    points: &[DVec3],
) -> Result<(Solid, Vec<Vec<FaceSource>>), String> {
    let mut traced: Vec<TracedFace> = Vec::with_capacity(patches.len());
    for p in patches {
        let mut faces = trace_faces(&p.domain(), &p.half_edges, edges, points)?;
        if faces.len() != 1 {
            return Err(format!(
                "a face of the result falls apart into {} regions",
                faces.len()
            ));
        }
        traced.push(faces.remove(0));
    }

    // Shells: faces connected through edges.
    let mut parent: Vec<usize> = (0..patches.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let uses = edge_uses(patches);
    for list in uses.values() {
        if list.len() != 2 || list[0].1 == list[1].1 {
            return Err("an edge of the result is not shared by exactly two faces".to_owned());
        }
        let (a, b) = (find(&mut parent, list[0].0), find(&mut parent, list[1].0));
        parent[a] = b;
    }
    let mut groups: HashMap<usize, (Aabb, Vec<usize>)> = HashMap::new();
    for (pi, p) in patches.iter().enumerate() {
        let root = find(&mut parent, pi);
        let entry = groups.entry(root).or_insert((Aabb::EMPTY, Vec::new()));
        entry.1.push(pi);
        for &(e, _) in &p.half_edges {
            let g = &edges[e as usize];
            entry.0.extend(points[g.start as usize]);
            entry.0.extend(g.mid());
        }
    }
    // Biggest first, so outer shells come before the voids inside them.
    let mut groups: Vec<(Aabb, Vec<usize>)> = groups.into_values().collect();
    groups.sort_by(|a, b| {
        b.0.size()
            .length()
            .total_cmp(&a.0.size().length())
            .then(a.1[0].cmp(&b.1[0]))
    });

    let mut solid = Solid::new();
    let mut sources: Vec<Vec<FaceSource>> = Vec::with_capacity(patches.len());
    let mut vertex_ids: HashMap<u32, VertexId> = HashMap::new();
    let mut edge_ids: HashMap<u32, EdgeId> = HashMap::new();
    for (_, members) in &groups {
        let shell = solid.add_shell();
        for &pi in members {
            let p = &patches[pi];
            let face = solid.add_face(shell, p.surface, p.reversed);
            sources.push(p.sources.clone());
            let t = &traced[pi];
            for l in std::iter::once(&t.outer).chain(&t.holes) {
                let mut loop_uses = Vec::with_capacity(l.half_edges.len());
                for &(e, forward) in &l.half_edges {
                    let id = match edge_ids.get(&e) {
                        Some(&id) => id,
                        None => {
                            let g = &edges[e as usize];
                            let mut vertex = |v: u32| {
                                *vertex_ids
                                    .entry(v)
                                    .or_insert_with(|| solid.add_vertex(points[v as usize]))
                            };
                            let (s, en) = (vertex(g.start), vertex(g.end));
                            let id = solid.add_edge(g.curve, s, en, g.t0, g.t1);
                            edge_ids.insert(e, id);
                            id
                        }
                    };
                    loop_uses.push((id, !forward));
                }
                solid.add_loop(face, &loop_uses);
            }
        }
    }
    split_vertex_fans(&mut solid);
    Ok((solid, sources))
}

/// Gives every fan of faces around a vertex its own copy of the vertex, so bodies (or
/// parts of one body) that touch in a point are separate in the topology.
fn split_vertex_fans(solid: &mut Solid) {
    let mut outgoing: Vec<Vec<CoedgeId>> = vec![Vec::new(); solid.vertices.len()];
    for c in 0..solid.coedges.len() as u32 {
        let c = CoedgeId(c);
        outgoing[solid.coedge_start(c).index()].push(c);
    }
    for (v, list) in outgoing.iter().enumerate() {
        let mut visited: HashSet<CoedgeId> = HashSet::new();
        let mut first = true;
        for &start in list {
            if visited.contains(&start) {
                continue;
            }
            // One fan: from a coedge leaving the vertex to the next one around it.
            let mut fan = Vec::new();
            let mut c = start;
            while visited.insert(c) {
                fan.push(c);
                let Some(t) = solid.twin(solid.coedge(c).prev) else {
                    break;
                };
                c = t;
            }
            if first {
                first = false;
                continue;
            }
            let copy = solid.add_vertex(solid.vertices[v].point);
            for c in fan {
                let co = solid.coedge(c).clone();
                let edge = &mut solid.edges[co.edge.index()];
                if edge.start == edge.end {
                    edge.start = copy;
                    edge.end = copy;
                } else if co.reversed {
                    edge.end = copy;
                } else {
                    edge.start = copy;
                }
            }
        }
    }
}

/// Curvature vector of the surface's normal section at `p` in the unit tangent direction
/// `dir`: a point moving that way along the surface accelerates by this.
fn section_curvature(surface: &Surface, p: DVec3, dir: DVec3) -> DVec3 {
    match surface {
        Surface::Plane(_) => DVec3::ZERO,
        Surface::Cylinder(c) => {
            let around = 1.0 - dir.dot(c.axis()).powi(2);
            -surface.normal_at(p) * (around / c.radius)
        }
    }
}
