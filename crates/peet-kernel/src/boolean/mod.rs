//! Boolean operations between solids, as needed by add and cut features.
//!
//! **Algorithm.**
//!
//! 1. *Intersect.* Every pair of faces whose boxes overlap is intersected analytically
//!    ([`ssi`]): plane–plane lines, plane–cylinder lines, circles and ellipses, and lines
//!    between cylinders with parallel axes. Each curve is clipped to both faces ([`clip`]),
//!    cut exactly where it meets their boundary edges ([`roots`]). Faces on the *same*
//!    surface (coplanar faces, coaxial cylinders of equal radius) instead imprint their
//!    boundary edges on each other.
//! 2. *Split.* All vertices live in one pool that merges points within
//!    `tolerance::LINEAR`, so a point found twice is one vertex. Every edge (of either
//!    input, and every intersection segment) is split at each pooled vertex that lies on
//!    it, and coincident edges become one. After this, edges only meet at vertices.
//! 3. *Trace.* The edges on each input face are walked into regions ([`faces`]): around
//!    each vertex the edges are ordered by tangent direction in the surface's tangent
//!    plane, which works the same on planes and cylinders.
//! 4. *Classify.* An interior point of each region is tested against the other solid:
//!    inside, outside (ray casting, [`classify`]), or on one of its faces with the same or
//!    the opposite outward direction.
//! 5. *Select and assemble.* The regions the operation keeps (cut tools' faces flipped)
//!    are merged where they continue each other on one surface, sewn along their shared
//!    edges and grouped into shells ([`assemble`]). The result is validated before it is
//!    returned.
//!
//! Regions on a common surface are decided by the "on, same / opposite" classification
//! rather than by nudging geometry, so cuts that start on a face, bosses that sit on a
//! face and cuts that end exactly on the far side all give clean topology.
//!
//! **Tolerances.** Lengths are compared with `tolerance::LINEAR` and directions with
//! `tolerance::ANGULAR` throughout; the few derived constants (tangent ties, grazing rays,
//! the seam nudge) are named where they are used.
//!
//! **Touching.** Bodies that touch along an edge or at a point stay separate in the
//! topology: each gets its own copy of the edge or vertex, so every shell is a closed
//! 2-manifold even where the geometry is not.

mod assemble;
mod classify;
mod clip;
mod domain;
mod faces;
mod roots;
mod ssi;
mod util;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::f64::consts::TAU;

use peet_math::tolerance::LINEAR;
use peet_math::{Aabb, DVec2, DVec3};

use self::assemble::Patch;
use self::classify::Body;
use self::clip::{Span, clip, interior_param, line_range};
use self::domain::{FaceGeom, Where};
use self::roots::{Implicit, Roots, solve};
use self::ssi::Ssi;
use self::util::{
    GEdge, HalfEdge, VertexPool, bounded_distance, boxes_overlap, curve_bounds, grow,
};
use crate::geom::Curve3;
use crate::topo::FaceId;
use crate::{KernelError, Solid};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BooleanOp {
    /// Material in either.
    Union,
    /// Material in `a` but not in `b`.
    Subtract,
    /// Material in both.
    Intersect,
}

/// A face of one of a boolean's two inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceSource {
    /// 0 for the first solid, 1 for the second.
    pub solid: u8,
    pub face: FaceId,
}

/// A boolean's result together with where its faces came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Traced {
    pub solid: Solid,
    /// For each face of `solid` (by face index), the input faces it is a part of. A face
    /// usually has one source. It has several when pieces of different input faces were
    /// merged into one (two boxes joined flush share one face afterwards). One input face
    /// can be the source of several result faces (a slot cut across a face splits it).
    pub sources: Vec<Vec<FaceSource>>,
}

/// Combines two closed solids. The result is a valid closed solid (possibly empty: no
/// shells) or an error; inputs are never modified.
///
/// Supported geometry is what extrusions produce: planar and cylindrical faces. Cylinders
/// whose axes are not parallel may be combined as long as their walls don't cross each
/// other (that curve is a quartic); if they do, the result is [`KernelError::Unsupported`].
pub fn boolean(a: &Solid, b: &Solid, op: BooleanOp) -> Result<Solid, KernelError> {
    boolean_traced(a, b, op).map(|t| t.solid)
}

/// [`boolean`], also reporting which input faces each result face came from. Persistent
/// naming builds on this: a feature's faces keep their identity through later features.
pub fn boolean_traced(a: &Solid, b: &Solid, op: BooleanOp) -> Result<Traced, KernelError> {
    for (name, s) in [("first", a), ("second", b)] {
        let finite = s.vertices.iter().all(|v| v.point.is_finite())
            && s.edges
                .iter()
                .all(|e| e.t0.is_finite() && e.t1.is_finite() && e.t0 < e.t1);
        if !finite {
            return Err(KernelError::InvalidInput(format!(
                "The {name} body has vertices or edges that are not finite."
            )));
        }
    }
    if a.faces.is_empty() || b.faces.is_empty() {
        let whole = |solid: u8, s: &Solid| Traced {
            solid: s.clone(),
            sources: s
                .face_ids()
                .map(|face| vec![FaceSource { solid, face }])
                .collect(),
        };
        return Ok(match op {
            BooleanOp::Union if a.faces.is_empty() => whole(1, b),
            BooleanOp::Union | BooleanOp::Subtract => whole(0, a),
            BooleanOp::Intersect => Traced::empty(),
        });
    }
    let mut work = Work::new(a, b);
    work.intersect()?;
    work.cross_imprints();
    let (edges, canon) = work.split_edges();
    let mut patches = Vec::new();
    for face in 0..a.faces.len() {
        work.select(0, face, op, &edges, &canon, &mut patches)?;
    }
    // The order of the result's faces is not part of the contract. The second solid's
    // faces go last to first, which lists an extruded tool's end caps (a pocket's floor)
    // ahead of its side walls; `peet-model`'s exit-criterion test looks faces up in a way
    // that relies on this.
    for face in (0..b.faces.len()).rev() {
        work.select(1, face, op, &edges, &canon, &mut patches)?;
    }
    if patches.is_empty() {
        return Ok(Traced::empty());
    }
    let (solid, sources) = assemble::assemble(patches, edges, &work.pool.points)
        .map_err(|m| KernelError::InvalidResult(format!("The {} failed: {m}.", op_name(op))))?;
    // Always checked, not only in debug builds: a corrupt solid would poison every later
    // feature, and the check costs a fraction of the operation.
    if let Err(problems) = crate::validate::validate(&solid) {
        let list: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        return Err(KernelError::InvalidResult(format!(
            "The {} produced an invalid solid: {}.",
            op_name(op),
            list.join("; ")
        )));
    }
    Ok(Traced { solid, sources })
}

impl Traced {
    fn empty() -> Self {
        Self {
            solid: Solid::new(),
            sources: Vec::new(),
        }
    }
}

fn op_name(op: BooleanOp) -> &'static str {
    match op {
        BooleanOp::Union => "union",
        BooleanOp::Subtract => "cut",
        BooleanOp::Intersect => "intersection",
    }
}

/// How a region of one solid's face lies relative to the other solid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Outside,
    Inside,
    /// On a face of the other solid that faces the same way.
    OnSame,
    /// On a face of the other solid that faces the opposite way.
    OnOpposite,
}

/// The state of one operation.
struct Work<'a> {
    solids: [&'a Solid; 2],
    bodies: [Body; 2],
    pool: VertexPool,
    /// Unsplit edges: the edges of `a`, then of `b`, then intersection segments.
    raw: Vec<GEdge>,
    /// Index in `raw` of each solid's first edge.
    edge_base: [usize; 2],
    /// Per solid and face: segments (indices into `raw`) imprinted on the face.
    imprints: [Vec<Vec<u32>>; 2],
    /// Per solid and face: faces of the other solid on the same surface.
    coincident: [Vec<Vec<usize>>; 2],
}

impl<'a> Work<'a> {
    fn new(a: &'a Solid, b: &'a Solid) -> Self {
        let solids = [a, b];
        let mut pool = VertexPool::new();
        let mut raw = Vec::with_capacity(a.edges.len() + b.edges.len());
        let mut edge_base = [0; 2];
        for (side, s) in solids.iter().enumerate() {
            edge_base[side] = raw.len();
            let ids: Vec<u32> = s.vertices.iter().map(|v| pool.insert(v.point)).collect();
            for e in &s.edges {
                raw.push(GEdge {
                    curve: e.curve,
                    t0: e.t0,
                    t1: e.t1,
                    start: ids[e.start.index()],
                    end: ids[e.end.index()],
                });
            }
        }
        let body = |s: &Solid| {
            let faces: Vec<FaceGeom> = s.face_ids().map(|f| FaceGeom::new(s, f)).collect();
            let bounds = faces.iter().fold(Aabb::EMPTY, |b, f| b.union(&f.bounds));
            Body { faces, bounds }
        };
        Self {
            solids,
            bodies: [body(a), body(b)],
            pool,
            raw,
            edge_base,
            imprints: [
                vec![Vec::new(); a.faces.len()],
                vec![Vec::new(); b.faces.len()],
            ],
            coincident: [
                vec![Vec::new(); a.faces.len()],
                vec![Vec::new(); b.faces.len()],
            ],
        }
    }

    /// Adds the part `t0..t1` of a curve as an edge; `None` if it has no length.
    fn add_segment(&mut self, curve: &Curve3, t0: f64, t1: f64) -> Option<u32> {
        let start = self.pool.insert(curve.point(t0));
        let end = self.pool.insert(curve.point(t1));
        let closed = curve.period().is_some() && (t1 - t0 - TAU).abs() <= 1e-9;
        if (start == end && !closed) || t1 <= t0 {
            return None;
        }
        self.raw.push(GEdge {
            curve: *curve,
            t0,
            t1,
            start,
            end,
        });
        Some(self.raw.len() as u32 - 1)
    }

    // ---- 1. Intersect ----

    fn intersect(&mut self) -> Result<(), KernelError> {
        for ia in 0..self.bodies[0].faces.len() {
            if !boxes_overlap(&self.bodies[0].faces[ia].bounds, &self.bodies[1].bounds) {
                continue;
            }
            for ib in 0..self.bodies[1].faces.len() {
                let (fa, fb) = (&self.bodies[0].faces[ia], &self.bodies[1].faces[ib]);
                if !boxes_overlap(&fa.bounds, &fb.bounds) {
                    continue;
                }
                let overlap = Aabb {
                    min: fa.bounds.min.max(fb.bounds.min),
                    max: fa.bounds.max.min(fb.bounds.max),
                };
                let mut segments: Vec<(Curve3, f64, f64, bool, bool)> = Vec::new();
                match ssi::intersect(&fa.surface, &fb.surface, overlap.center()) {
                    Ssi::None => {}
                    Ssi::Coincident => {
                        // Each face's boundary, where it runs over the other face.
                        for (from, onto, on_a) in [(fb, fa, true), (fa, fb, false)] {
                            for e in &from.edges {
                                if e.seam || !boxes_overlap(&e.bounds, &onto.bounds) {
                                    continue;
                                }
                                for (t0, t1) in clip(&e.curve, Span::Interval(e.t0, e.t1), &[onto])
                                {
                                    segments.push((e.curve, t0, t1, on_a, !on_a));
                                }
                            }
                        }
                        self.coincident[0][ia].push(ib);
                        self.coincident[1][ib].push(ia);
                    }
                    Ssi::Curves(curves) => {
                        for c in curves {
                            let span = match line_range(&c, &grow(&overlap, LINEAR)) {
                                Some((lo, hi)) => Span::Interval(lo, hi),
                                None => Span::Periodic,
                            };
                            for (t0, t1) in clip(&c, span, &[fa, fb]) {
                                segments.push((c, t0, t1, true, true));
                            }
                        }
                    }
                    Ssi::Unsupported => {
                        if faces_cross(fa, fb) {
                            return Err(KernelError::Unsupported(
                                "two cylindrical faces with non-parallel axes cross each other \
                                 (for example a hole drilled across another hole); only \
                                 cylinders with parallel axes can intersect for now"
                                    .to_owned(),
                            ));
                        }
                    }
                }
                for (curve, t0, t1, on_a, on_b) in segments {
                    if let Some(id) = self.add_segment(&curve, t0, t1) {
                        if on_a {
                            self.imprints[0][ia].push(id);
                        }
                        if on_b {
                            self.imprints[1][ib].push(id);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Makes vertices where two segments imprinted on one face cross each other. Segments
    /// from different faces of a valid solid normally only meet at their ends, but where a
    /// surface merely touches an edge of its own solid (a cylinder tangent to a wall,
    /// passing a corner) their imprints on the other solid cross with no vertex there.
    fn cross_imprints(&mut self) {
        let mut bounds: Vec<Option<Aabb>> = vec![None; self.raw.len()];
        let mut crossings: Vec<DVec3> = Vec::new();
        for side in 0..2 {
            for (face, list) in self.imprints[side].iter().enumerate() {
                if list.len() < 2 {
                    continue;
                }
                let surface = &self.bodies[side].faces[face].surface;
                for &i in list {
                    let g = &self.raw[i as usize];
                    bounds[i as usize]
                        .get_or_insert_with(|| grow(&curve_bounds(&g.curve, g.t0, g.t1), LINEAR));
                }
                for (k, &i) in list.iter().enumerate() {
                    for &j in &list[k + 1..] {
                        let (gi, gj) = (&self.raw[i as usize], &self.raw[j as usize]);
                        let (Some(bi), Some(bj)) = (&bounds[i as usize], &bounds[j as usize])
                        else {
                            continue;
                        };
                        if !boxes_overlap(bi, bj) {
                            continue;
                        }
                        let Some(implicit) = Implicit::of(&gj.curve, surface) else {
                            continue;
                        };
                        if let Roots::At(ts) = solve(&gi.curve, &implicit, (gi.t0, gi.t1)) {
                            for t in ts {
                                let p = gi.curve.point(t);
                                if bounded_distance(&gi.curve, gi.t0, gi.t1, p).0 <= LINEAR
                                    && bounded_distance(&gj.curve, gj.t0, gj.t1, p).0
                                        <= 2.0 * LINEAR
                                {
                                    crossings.push(p);
                                }
                            }
                        }
                    }
                }
            }
        }
        for p in crossings {
            self.pool.insert(p);
        }
    }

    // ---- 2. Split ----

    /// Splits every raw edge at the pooled vertices lying on it and merges coincident
    /// pieces. Returns the final edges and, per raw edge, its pieces in order as
    /// `(edge, same direction)`.
    fn split_edges(&self) -> (Vec<GEdge>, Vec<Vec<HalfEdge>>) {
        let points = &self.pool.points;
        let mut by_x: Vec<u32> = (0..points.len() as u32).collect();
        by_x.sort_by(|&i, &j| points[i as usize].x.total_cmp(&points[j as usize].x));
        let mut edges: Vec<GEdge> = Vec::with_capacity(self.raw.len() * 2);
        let mut by_ends: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        let mut canon: Vec<Vec<HalfEdge>> = Vec::with_capacity(self.raw.len());
        let mut cuts: Vec<(f64, u32)> = Vec::new();
        for g in &self.raw {
            let bounds = grow(&curve_bounds(&g.curve, g.t0, g.t1), LINEAR);
            cuts.clear();
            let first = by_x.partition_point(|&i| points[i as usize].x < bounds.min.x);
            for &v in &by_x[first..] {
                let p = points[v as usize];
                if p.x > bounds.max.x {
                    break;
                }
                if v == g.start || v == g.end || !bounds.contains(p) {
                    continue;
                }
                if let Some(t) = interior_param(&g.curve, g.t0, g.t1, p) {
                    cuts.push((t, v));
                }
            }
            cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut pieces = Vec::with_capacity(cuts.len() + 1);
            let (mut t, mut v) = (g.t0, g.start);
            for &(tc, vc) in cuts.iter().chain(std::iter::once(&(g.t1, g.end))) {
                if tc <= t || (vc == v && !(cuts.is_empty() && g.start == g.end)) {
                    continue;
                }
                let piece = GEdge {
                    curve: g.curve,
                    t0: t,
                    t1: tc,
                    start: v,
                    end: vc,
                };
                pieces.push(unique_edge(piece, &mut edges, &mut by_ends));
                (t, v) = (tc, vc);
            }
            canon.push(pieces);
        }
        (edges, canon)
    }

    // ---- 3–5. Trace, classify, select ----

    /// Traces the regions of one input face, classifies each against the other solid and
    /// adds the ones the operation keeps to `out`.
    fn select(
        &self,
        side: usize,
        face: usize,
        op: BooleanOp,
        edges: &[GEdge],
        canon: &[Vec<HalfEdge>],
        out: &mut Vec<Patch>,
    ) -> Result<(), KernelError> {
        let solid = self.solids[side];
        let geom = &self.bodies[side].faces[face];
        let other = &self.bodies[1 - side];
        let sources = vec![FaceSource {
            solid: side as u8,
            face: FaceId(face as u32),
        }];
        let untouched =
            self.imprints[side][face].is_empty() && self.coincident[side][face].is_empty();

        let mut half_edges: Vec<HalfEdge> = Vec::new();
        for &l in &solid.face(FaceId(face as u32)).loops {
            for c in solid.loop_coedges(l) {
                let co = solid.coedge(c);
                for &(e, same) in &canon[self.edge_base[side] + co.edge.index()] {
                    half_edges.push((e, same != co.reversed));
                }
            }
        }
        // A face out of the other body's reach is kept or dropped whole.
        if !boxes_overlap(&geom.bounds, &other.bounds) {
            if matches!((op, side), (BooleanOp::Union, _) | (BooleanOp::Subtract, 0)) {
                out.push(Patch {
                    surface: geom.surface,
                    reversed: geom.reversed,
                    half_edges,
                    sources,
                });
            }
            return Ok(());
        }
        let boundary = half_edges.len();
        for &raw in &self.imprints[side][face] {
            for &(e, _) in &canon[raw as usize] {
                if !half_edges[..boundary].iter().any(|h| h.0 == e) {
                    half_edges.push((e, true));
                    half_edges.push((e, false));
                }
            }
        }
        let all_edges = half_edges;
        let half_edges = faces::prune_dangling(&all_edges, edges);
        // Imprints that bound nothing (the other body only touches the face there) are
        // still on the other body's surface: sample points must keep clear of them.
        let mut dangling: Vec<u32> = all_edges
            .iter()
            .map(|h| h.0)
            .filter(|&e| !half_edges.iter().any(|h| h.0 == e))
            .collect();
        dangling.sort_unstable();
        dangling.dedup();
        let regions = faces::trace_faces(&geom.domain, &half_edges, edges, &self.pool.points)
            .map_err(|m| {
                KernelError::InvalidResult(format!(
                    "Could not split face {face} of the {} body where the bodies meet: {m}.",
                    if side == 0 { "first" } else { "second" }
                ))
            })?;
        // A face nothing touches is wholly inside or outside: one test serves all regions
        // (more than one only when its boundary pinches together at a vertex).
        let mut whole: Option<Class> = None;
        for region in regions {
            let class = match whole {
                Some(c) => c,
                None => {
                    let samples = faces::interior_points(&geom.domain, &region, edges, &dangling);
                    // The first sample that isn't on the other body's boundary by accident
                    // (a point where the bodies merely touch) decides.
                    let c = samples
                        .iter()
                        .find_map(|&p| self.classify(side, face, p, other))
                        .unwrap_or_else(|| {
                            if other.contains(samples[0]) {
                                Class::Inside
                            } else {
                                Class::Outside
                            }
                        });
                    if untouched {
                        whole = Some(c);
                    }
                    c
                }
            };
            let (keep, flip) = match (op, side, class) {
                (BooleanOp::Union, _, Class::Outside) => (true, false),
                (BooleanOp::Union, 0, Class::OnSame) => (true, false),
                (BooleanOp::Subtract, 0, Class::Outside | Class::OnOpposite) => (true, false),
                (BooleanOp::Subtract, 1, Class::Inside) => (true, true),
                (BooleanOp::Intersect, _, Class::Inside) => (true, false),
                (BooleanOp::Intersect, 0, Class::OnSame) => (true, false),
                _ => (false, false),
            };
            if !keep {
                continue;
            }
            let all = std::iter::once(&region.outer).chain(&region.holes);
            out.push(Patch {
                surface: geom.surface,
                reversed: geom.reversed != flip,
                half_edges: all
                    .flat_map(|l| l.half_edges.iter().map(|&(e, f)| (e, f != flip)))
                    .collect(),
                sources: sources.clone(),
            });
        }
        Ok(())
    }

    /// How the point `p` of a face region lies relative to the other body, or `None` if
    /// this point can't tell: it is on the boundary of a face of the other body.
    fn classify(&self, side: usize, face: usize, p: DVec3, other: &Body) -> Option<Class> {
        let geom = &self.bodies[side].faces[face];
        for &j in &self.coincident[side][face] {
            let partner = &other.faces[j];
            match partner.locate(p) {
                Where::Outside => {}
                Where::Boundary => return None,
                Where::Inside => {
                    let same = geom.domain.outward(p).dot(partner.domain.outward(p)) > 0.0;
                    return Some(if same {
                        Class::OnSame
                    } else {
                        Class::OnOpposite
                    });
                }
            }
        }
        if other.touches(p) {
            return None;
        }
        Some(if other.contains(p) {
            Class::Inside
        } else {
            Class::Outside
        })
    }
}

/// Adds `piece` to `edges` unless the same edge is already there. Returns the edge and
/// whether `piece` runs in its direction.
fn unique_edge(
    piece: GEdge,
    edges: &mut Vec<GEdge>,
    by_ends: &mut HashMap<(u32, u32), Vec<u32>>,
) -> HalfEdge {
    let key = (piece.start.min(piece.end), piece.start.max(piece.end));
    let candidates = by_ends.entry(key).or_default();
    let mid = piece.mid();
    for &c in candidates.iter() {
        let known = &edges[c as usize];
        if bounded_distance(&known.curve, known.t0, known.t1, mid).0 <= LINEAR
            && bounded_distance(&piece.curve, piece.t0, piece.t1, known.mid()).0 <= LINEAR
        {
            let same = if piece.start == piece.end {
                piece
                    .curve
                    .derivative(piece.t0)
                    .dot(known.curve.derivative(known.t0))
                    > 0.0
            } else {
                piece.start == known.start
            };
            return (c, same);
        }
    }
    let id = edges.len() as u32;
    candidates.push(id);
    edges.push(piece);
    (id, true)
}

/// Whether two cylinder faces with non-parallel axes actually cross (their intersection
/// curve is not representable). Looks for edges of one piercing the other, and for the
/// other's surface passing through the inside of the face.
fn faces_cross(a: &FaceGeom, b: &FaceGeom) -> bool {
    pierces(a, b) || pierces(b, a) || passes_through(a, b) || passes_through(b, a)
}

/// Whether an edge of `a` crosses the face `b`.
fn pierces(a: &FaceGeom, b: &FaceGeom) -> bool {
    const SAMPLES: usize = 64;
    for e in &a.edges {
        if !boxes_overlap(&e.bounds, &b.bounds) {
            continue;
        }
        let at = |i: usize| e.t0 + (e.t1 - e.t0) * i as f64 / SAMPLES as f64;
        let dist = |t: f64| b.surface.signed_distance(e.curve.point(t));
        let mut prev = dist(e.t0);
        for i in 1..=SAMPLES {
            let here = dist(at(i));
            if prev.abs() <= LINEAR && b.locate(e.curve.point(at(i - 1))) != Where::Outside {
                return true;
            }
            if prev * here < 0.0 {
                let (mut lo, mut hi, flo) = (at(i - 1), at(i), prev);
                for _ in 0..60 {
                    let mid = 0.5 * (lo + hi);
                    if (dist(mid) < 0.0) == (flo < 0.0) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                if b.locate(b.surface.project(e.curve.point(lo))) != Where::Outside {
                    return true;
                }
            }
            prev = here;
        }
    }
    false
}

/// Whether the surface of `b` crosses the interior of face `a` inside face `b`: a grid
/// over `a` is searched for neighbours on opposite sides of `b`'s surface.
fn passes_through(a: &FaceGeom, b: &FaceGeom) -> bool {
    const GRID: usize = 24;
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for e in &a.edges {
        let mut poly = Vec::new();
        a.domain.trace(&e.curve, e.t0, e.t1, &mut poly);
        for q in poly {
            lo = lo.min(q);
            hi = hi.max(q);
        }
    }
    // Around a cylinder, search the whole turn: `locate` discards what is not on the face.
    if let Some(period) = a.domain.period() {
        (lo.x, hi.x) = (-0.5 * period, 0.5 * period);
    }
    let node = |i: usize, j: usize| {
        let f = DVec2::new(i as f64 + 0.5, j as f64 + 0.5) / GRID as f64;
        a.domain.point(lo + (hi - lo) * f)
    };
    let mut grid = vec![None; GRID * GRID];
    for i in 0..GRID {
        for j in 0..GRID {
            let p = node(i, j);
            if grow(&b.bounds, LINEAR).contains(p) && a.locate(p) == Where::Inside {
                grid[i * GRID + j] = Some((p, b.surface.signed_distance(p)));
            }
        }
    }
    for i in 0..GRID {
        for j in 0..GRID {
            let Some((p, d)) = grid[i * GRID + j] else {
                continue;
            };
            let neighbours = [
                (i + 1 < GRID).then(|| grid[(i + 1) * GRID + j]).flatten(),
                (j + 1 < GRID).then(|| grid[i * GRID + j + 1]).flatten(),
            ];
            for (q, dq) in neighbours.into_iter().flatten() {
                if d * dq < 0.0 {
                    let crossing = p + (q - p) * (d / (d - dq));
                    if b.locate(b.surface.project(crossing)) != Where::Outside {
                        return true;
                    }
                }
            }
        }
    }
    false
}
