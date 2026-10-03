//! Building a sheet metal body from its [`Layout`]: the flat solid, then the folded one.
//!
//! **1. The flat faces.** Every curve of every piece outline, trim and cut goes into one
//! planar arrangement (`peet_sketch::region::regions_of_curves`). Each minimal face of it is
//! material of exactly one piece (the piece whose outline holds it, outside that piece's
//! trims and outside the cuts that apply to it), or not material at all. Two pieces
//! claiming one face is an error: the flat pattern overlaps itself.
//!
//! **2. Gluing.** Where two material faces share an arrangement edge:
//! - faces of the *same* piece merge (the edge was a curve of some other piece's outline
//!   or of a cut that doesn't apply here), so a flange stays one face;
//! - a bend and its parent or child flange are joined along the bend's lines;
//! - anything else is a *slit*: the two sides touch in the flat but are not connected (a
//!   tear relief), and each gets its own wall.
//!
//! Collinear boundary edges that only meet each other are joined again, so a flange's side
//! is one wall even where another piece's curve touched it.
//!
//! **3. Thickening.** The glued faces become a solid between `z = 0` (bottom side) and
//! `z = t` (top side): a top and a bottom face per merged face, shared edges along the
//! bend lines, a wall per boundary edge. Vertices are made once per fan of faces around a
//! point, so faces that only touch at a point (or along a slit) stay separate.
//!
//! **4. Folding.** The folded solid has *the same topology*; only geometry changes. A
//! flange's elements move rigidly to its placement. A bend's top and bottom faces become
//! cylinders around the bend axis, lines along the bend stay lines, lines across it become
//! arcs, and walls become planes through or across the axis. Curves crossing a bend at any
//! other angle (or curved ones) have no analytic folded form and are reported as such.
//!
//! Because both solids share their topology, faces have the same index in the folded and
//! the flat body: references, picking and names work in either view, and unfolding is
//! exact by construction.

use std::collections::HashMap;
use std::f64::consts::PI;

use peet_kernel::geom::{Circle3, Curve3, Cylinder, Surface};
use peet_kernel::topo::{EdgeId, FaceId, ShellId, VertexId};
use peet_kernel::{Solid, transform, validate};
use peet_math::{DVec2, DVec3, Frame, Plane, tolerance};
use peet_sketch::Curve;
use peet_sketch::region::{Region, regions_of_curves};
use peet_sketch::triangulate::triangulate_region;

use crate::form::{FormMark, face as form_face};
use crate::layout::{Bend, CurveTag, Edge2, EdgeSite, Layout, Origin, PieceKind, loop_winding};

/// Points closer than this are the same point of the flat pattern.
const SAME_POINT: f64 = 100.0 * tolerance::LINEAR;

/// Directions closer than this (in sine of the angle) count as parallel when deciding how
/// a curve folds.
const PARALLEL: f64 = 1e-7;

/// Why a sheet metal body couldn't be built.
#[derive(Clone, Debug, PartialEq)]
pub enum SheetError {
    /// A message for the user.
    Message(String),
    /// Two pieces overlap in the flat pattern.
    Overlap { a: Origin, b: Origin },
    /// A curve crosses a bend in a way that has no folded form.
    AcrossBend {
        tag: CurveTag,
        bend: Origin,
        curved: bool,
    },
    /// Every piece was cut away.
    Empty,
    /// The result failed validation (a bug: please report it).
    Invalid(String),
}

impl SheetError {
    /// The message, with owners named by `name`.
    pub fn message(&self, name: impl Fn(u32) -> String) -> String {
        match self {
            Self::Message(m) => m.clone(),
            Self::Overlap { a, b } => {
                if a.owner == b.owner {
                    format!(
                        "The flat pattern of {} overlaps itself. Shorten the flange or move it.",
                        name(a.owner)
                    )
                } else {
                    format!(
                        "The flat pattern overlaps itself: {} runs into {}. Shorten or move one of them.",
                        name(b.owner),
                        name(a.owner)
                    )
                }
            }
            Self::AcrossBend { tag, bend, curved } => {
                let what = if *curved {
                    "a curved edge"
                } else {
                    "an edge at an angle to the bend"
                };
                format!(
                    "{} crosses the bend of {} with {what}. Only straight edges parallel or square to the bend line can cross a bend; keep round holes and slanted edges on flat faces.",
                    name(tag.owner()),
                    name(bend.owner)
                )
            }
            Self::Empty => "Nothing is left of the sheet.".to_owned(),
            Self::Invalid(m) => {
                format!("The sheet metal body came out invalid ({m}). This is a bug.")
            }
        }
    }
}

/// What a face of a sheet metal body is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FaceTag {
    /// The top side (`z = t`) of a piece.
    Top { piece: usize },
    /// The bottom side (`z = 0`).
    Bottom { piece: usize },
    /// A wall through the thickness along a flat boundary curve. `curve` is in flat
    /// coordinates, traversed (`reversed`) with the material on its left.
    Wall {
        piece: usize,
        tag: CurveTag,
        curve: Curve,
        reversed: bool,
    },
    /// A face of a form (a dimple, an emboss, a louver) on a flange. `tag` names it:
    /// the form's owner and part, and the face's number in [`crate::form::face`].
    Form { piece: usize, tag: CurveTag },
}

impl FaceTag {
    pub fn piece(&self) -> usize {
        match *self {
            Self::Top { piece }
            | Self::Bottom { piece }
            | Self::Wall { piece, .. }
            | Self::Form { piece, .. } => piece,
        }
    }
}

/// A closed boundary loop of the flat pattern, in flat coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatLoop {
    /// `(curve, reversed)` in loop order: counter-clockwise for outer boundaries.
    pub edges: Vec<(Curve, bool)>,
    /// An outer boundary (else a cutout).
    pub outer: bool,
}

/// The bend line of one bend in the flat pattern (its centre line, where material is).
#[derive(Clone, Debug, PartialEq)]
pub struct BendLine {
    /// Index of the bend's piece in the layout.
    pub piece: usize,
    pub segments: Vec<[DVec2; 2]>,
}

/// A built sheet metal body: everything beyond the folded solid.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetBody {
    pub layout: Layout,
    /// The flat pattern as a solid in model space, with the fixed flange in place. Same
    /// topology and face order as the folded solid.
    pub flat: Solid,
    /// What each face is (by face index, in both solids).
    pub faces: Vec<FaceTag>,
    pub outline: Vec<FlatLoop>,
    pub bend_lines: Vec<BendLine>,
    /// The forms pressed into the sheet, for marking on the flat pattern.
    pub forms: Vec<FormMark>,
}

impl SheetBody {
    /// Where an edge flange on `edge` (of the folded or flat solid) would go: the edge must
    /// be where the top or bottom face of a flange meets a straight wall of it.
    pub fn edge_site(&self, solid: &Solid, edge: EdgeId) -> Result<EdgeSite, String> {
        let e = solid
            .edges
            .get(edge.index())
            .ok_or("The edge is not on this body.")?;
        let faces: Vec<FaceId> = e.coedges.iter().map(|&c| solid.coedge_face(c)).collect();
        let [f0, f1] = faces[..] else {
            return Err("The edge is not on this body.".to_owned());
        };
        let (Some(&t0), Some(&t1)) = (self.faces.get(f0.index()), self.faces.get(f1.index()))
        else {
            return Err("The edge is not on this body.".to_owned());
        };
        let pick = |cap: FaceTag, wall: FaceTag| match (cap, wall) {
            (
                FaceTag::Top { piece },
                FaceTag::Wall {
                    piece: wp,
                    curve,
                    reversed,
                    ..
                },
            )
            | (
                FaceTag::Bottom { piece },
                FaceTag::Wall {
                    piece: wp,
                    curve,
                    reversed,
                    ..
                },
            ) if piece == wp => Some((piece, matches!(cap, FaceTag::Top { .. }), curve, reversed)),
            _ => None,
        };
        let Some((piece, top, curve, reversed)) = pick(t0, t1).or_else(|| pick(t1, t0)) else {
            return Err(
                "Pick an edge where the top or bottom face of a flange meets its side (not an edge of a bend or a corner edge through the thickness)."
                    .to_owned(),
            );
        };
        if !self.layout.pieces[piece].is_flange() {
            return Err("Pick an edge of a flat face, not of a bend.".to_owned());
        }
        let Curve::Line { a, b } = curve else {
            return Err("Pick a straight edge: flanges can't go on curved edges.".to_owned());
        };
        let (a, b) = if reversed { (b, a) } else { (a, b) };
        Ok(EdgeSite { piece, a, b, top })
    }

    /// For a top or bottom face of a flange: the flange's piece, its placement (model
    /// from flat) and whether the face is the top side.
    pub fn flange_face(&self, face: FaceId) -> Option<(usize, Frame, bool)> {
        let (piece, top) = match *self.faces.get(face.index())? {
            FaceTag::Top { piece } => (piece, true),
            FaceTag::Bottom { piece } => (piece, false),
            FaceTag::Wall { .. } | FaceTag::Form { .. } => return None,
        };
        let p = &self.layout.pieces[piece];
        p.is_flange().then_some((piece, p.frame, top))
    }

    /// Size of the flat pattern's bounding box in flat coordinates: `(min, max)`.
    pub fn flat_bounds(&self) -> (DVec2, DVec2) {
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for l in &self.outline {
            for (c, _) in &l.edges {
                let (a, b) = c.bounds();
                lo = lo.min(a);
                hi = hi.max(b);
            }
        }
        (lo, hi)
    }
}

/// Builds the folded solid and the rest of the sheet body.
pub fn build(layout: Layout) -> Result<(Solid, SheetBody), SheetError> {
    // The geometry comes from the layout with its corners worked out; the body keeps the
    // layout as defined, so later features can add to it and change its corners.
    let resolved = layout.resolved();
    let flat = Flat::new(&resolved)?;
    let (local, faces, info) = flat.thicken(&resolved)?;
    if let Err(problems) = validate::validate(&local) {
        return Err(SheetError::Invalid(format!(
            "flat: {}",
            problems
                .iter()
                .map(|p| p.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let folded = fold(&resolved, &flat, &local, &info)?;
    if let Err(problems) = validate::validate(&folded) {
        return Err(SheetError::Invalid(format!(
            "folded: {}",
            problems
                .iter()
                .map(|p| p.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let flat_solid = transform::solid(&local, &layout.pieces[0].frame);
    let outline = flat.outline(&resolved);
    let bend_lines = flat.bend_lines(&resolved);
    let forms = resolved
        .forms
        .iter()
        .map(|f| {
            let area = f.area();
            FormMark {
                owner: f.owner,
                part: f.part,
                kind: f.kind,
                up: f.up,
                height: f.height,
                outline: area.edges().map(|e| (e.curve, e.reversed)).collect(),
                center: f.center(),
                lance: f.open_side.and_then(|o| match &f.shape {
                    crate::form::FormShape::Polygon(p) => Some([p[o], p[(o + 1) % p.len()]]),
                    crate::form::FormShape::Circle { .. } => None,
                }),
            }
        })
        .collect();
    Ok((
        folded,
        SheetBody {
            layout,
            flat: flat_solid,
            faces,
            outline,
            bend_lines,
            forms,
        },
    ))
}

// ---- Steps 1 and 2: flat faces, glued ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    /// A boundary of the material (including slits).
    None,
    /// Joined to its twin along a bend line.
    Fold,
    /// Inside a piece: the two sides merge.
    Dissolved,
}

/// One side of an edge of a material face of the arrangement.
#[derive(Clone, Debug)]
struct Half {
    region: usize,
    curve: Curve,
    reversed: bool,
    entity: u32,
    start: DVec2,
    end: DVec2,
    next: usize,
    twin: Option<usize>,
    link: Link,
}

impl Half {
    fn mid(&self) -> DVec2 {
        self.curve.point_at(0.5)
    }
}

/// A final edge of the flat pattern (after merging), on one merged face.
#[derive(Clone, Debug)]
struct FEdge {
    curve: Curve,
    reversed: bool,
    tag: CurveTag,
    /// Vertex groups at the start and end, in loop order.
    start: usize,
    end: usize,
    face: usize,
    twin: Option<usize>,
    /// The next edge of its loop.
    next: usize,
}

impl FEdge {
    fn ends(&self) -> (DVec2, DVec2) {
        if self.reversed {
            (self.curve.end(), self.curve.start())
        } else {
            (self.curve.start(), self.curve.end())
        }
    }
}

/// A merged face: a connected part of one piece.
struct MFace {
    piece: usize,
    /// Loops of edges (indices into `Flat::edges`), outer first.
    loops: Vec<Vec<usize>>,
}

/// The glued flat pattern.
struct Flat {
    edges: Vec<FEdge>,
    faces: Vec<MFace>,
    /// Position of each vertex group (flat 2D); groups no edge uses are unused.
    groups: Vec<DVec2>,
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

/// A point strictly inside a region (the centroid of its largest triangle).
fn interior_point(region: &Region) -> Option<DVec2> {
    let (lo, hi) = region.outer.bounds();
    let tris = triangulate_region(region, (lo.distance(hi) * 1e-3).max(1e-6));
    let area = |[a, b, c]: &[DVec2; 3]| (*b - *a).perp_dot(*c - *a).abs();
    tris.triangles
        .iter()
        .map(|t| t.map(|i| tris.points[i as usize]))
        .max_by(|a, b| area(a).total_cmp(&area(b)))
        .map(|[a, b, c]| (a + b + c) / 3.0)
}

/// `½ ∮ (x dy − y dx)` of a curve in traversal direction (exact for arcs).
fn area_term(c: &Curve, reversed: bool) -> f64 {
    let a = match *c {
        Curve::Circle { radius, .. } => PI * radius * radius,
        Curve::Arc { radius, sweep, .. } => {
            0.5 * c.start().perp_dot(c.end()) + 0.5 * radius * radius * (sweep - sweep.sin())
        }
        Curve::Line { a, b } => 0.5 * a.perp_dot(b),
    };
    if reversed { -a } else { a }
}

/// Splits closed curves (circles, full arcs) into two halves, in traversal order.
fn split_closed(curve: Curve, reversed: bool) -> Vec<(Curve, bool)> {
    let (center, radius, a0) = match curve {
        Curve::Circle { center, radius } => (center, radius, 0.0),
        Curve::Arc {
            center,
            radius,
            start_angle,
            sweep,
        } if sweep >= std::f64::consts::TAU - 1e-9 => (center, radius, start_angle),
        _ => return vec![(curve, reversed)],
    };
    let half = |start: f64| Curve::Arc {
        center,
        radius,
        start_angle: start,
        sweep: PI,
    };
    let (h1, h2) = (half(a0), half(a0 + PI));
    if reversed {
        vec![(h2, true), (h1, true)]
    } else {
        vec![(h1, false), (h2, false)]
    }
}

fn close(a: DVec2, b: DVec2) -> bool {
    a.distance(b) <= SAME_POINT
}

/// Whether the edge from `a` to `b` lies on the line through `o` along `dir`, within
/// `0..=len` of it.
fn on_line(a: DVec2, b: DVec2, o: DVec2, dir: DVec2, len: f64) -> bool {
    let off = |p: DVec2| (p - o).perp_dot(dir).abs();
    let along = |p: DVec2| (p - o).dot(dir);
    off(a) <= SAME_POINT
        && off(b) <= SAME_POINT
        && (-SAME_POINT..=len + SAME_POINT).contains(&along(a))
        && (-SAME_POINT..=len + SAME_POINT).contains(&along(b))
}

/// Whether an edge between pieces `pa` and `pb` is one of a bend's lines between them.
fn is_fold(layout: &Layout, pa: usize, pb: usize, a: DVec2, b: DVec2) -> bool {
    let check = |bend_piece: usize, other: usize| {
        let Some(bend) = layout.pieces[bend_piece].bend() else {
            return false;
        };
        let line = if other == bend.parent {
            bend.origin
        } else if Some(other) == bend.child {
            bend.origin + bend.across * bend.width()
        } else {
            return false;
        };
        let is_line = (b - a).normalize_or_zero().perp_dot(bend.along).abs() <= 1e-6;
        is_line && on_line(a, b, line, bend.along, bend.length)
    };
    check(pa, pb) || check(pb, pa)
}

impl Flat {
    fn new(layout: &Layout) -> Result<Self, SheetError> {
        // The arrangement of every curve, remembering each one's tag.
        let mut inputs: Vec<(peet_sketch::EntityId, Curve)> = Vec::new();
        let mut tags: Vec<CurveTag> = Vec::new();
        let mut add = |e: &Edge2| {
            inputs.push((peet_sketch::EntityId(inputs.len() as u32), e.curve));
            tags.push(e.tag);
        };
        for p in &layout.pieces {
            p.outline.edges().for_each(&mut add);
        }
        for p in &layout.pieces {
            p.trims.iter().flat_map(|t| t.edges()).for_each(&mut add);
        }
        for c in &layout.cuts {
            c.area.edges().for_each(&mut add);
        }
        let form_areas: Vec<_> = layout.forms.iter().map(|f| f.area()).collect();
        for a in &form_areas {
            a.edges().for_each(&mut add);
        }
        let profile = regions_of_curves(&inputs);

        // Material faces and their pieces.
        let mut kept: Vec<(Region, usize)> = Vec::new();
        for region in profile.regions {
            let Some(p) = interior_point(&region) else {
                continue;
            };
            let mut owner: Option<usize> = None;
            for (pi, piece) in layout.pieces.iter().enumerate() {
                if !piece.outline.contains(p) || piece.trims.iter().any(|t| t.contains(p)) {
                    continue;
                }
                if layout
                    .cuts
                    .iter()
                    .any(|c| c.applies_to(pi) && c.area.contains(p))
                {
                    continue;
                }
                // A form's outline is a hole that its own faces fill.
                if layout
                    .forms
                    .iter()
                    .zip(&form_areas)
                    .any(|(f, a)| f.piece == pi && a.contains(p))
                {
                    continue;
                }
                if let Some(prev) = owner {
                    return Err(SheetError::Overlap {
                        a: layout.pieces[prev].origin,
                        b: piece.origin,
                    });
                }
                owner = Some(pi);
            }
            if let Some(pi) = owner {
                kept.push((region, pi));
            }
        }
        if kept.is_empty() {
            return Err(SheetError::Empty);
        }

        // Half-edges, closed curves split in two so every edge has two vertices.
        let mut halves: Vec<Half> = Vec::new();
        let mut region_loops: Vec<Vec<Vec<usize>>> = Vec::new();
        for (ri, (region, _)) in kept.iter().enumerate() {
            let mut loops = Vec::new();
            for l in std::iter::once(&region.outer).chain(&region.holes) {
                let first = halves.len();
                for e in &l.edges {
                    for (curve, reversed) in split_closed(e.curve, e.reversed) {
                        let (start, end) = if reversed {
                            (curve.end(), curve.start())
                        } else {
                            (curve.start(), curve.end())
                        };
                        halves.push(Half {
                            region: ri,
                            curve,
                            reversed,
                            entity: e.entity.0,
                            start,
                            end,
                            next: 0,
                            twin: None,
                            link: Link::None,
                        });
                    }
                }
                let ids: Vec<usize> = (first..halves.len()).collect();
                for (k, &h) in ids.iter().enumerate() {
                    halves[h].next = ids[(k + 1) % ids.len()];
                }
                loops.push(ids);
            }
            region_loops.push(loops);
        }

        // Twins: the same arrangement edge seen from its two sides.
        let mut by_entity: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, h) in halves.iter().enumerate() {
            by_entity.entry(h.entity).or_default().push(i);
        }
        for list in by_entity.values() {
            for (k, &a) in list.iter().enumerate() {
                if halves[a].twin.is_some() {
                    continue;
                }
                for &b in &list[k + 1..] {
                    let (ha, hb) = (&halves[a], &halves[b]);
                    if hb.twin.is_some() || ha.region == hb.region {
                        continue;
                    }
                    if close(ha.start, hb.end)
                        && close(ha.end, hb.start)
                        && close(ha.mid(), hb.mid())
                    {
                        let (pa, pb) = (kept[ha.region].1, kept[hb.region].1);
                        let link = if pa == pb {
                            Link::Dissolved
                        } else if is_fold(layout, pa, pb, ha.start, ha.end) {
                            Link::Fold
                        } else {
                            break; // a slit: both sides stay boundaries
                        };
                        halves[a].twin = Some(b);
                        halves[b].twin = Some(a);
                        halves[a].link = link;
                        halves[b].link = link;
                        break;
                    }
                }
            }
        }

        // Vertex groups: corners around a point joined through glued edges.
        let mut uf = UnionFind::new(halves.len());
        for (a, h) in halves.iter().enumerate() {
            if let Some(b) = h.twin {
                uf.union(a, halves[b].next);
            }
        }
        // Merged faces: regions of one piece joined through dissolved edges.
        let mut fuf = UnionFind::new(kept.len());
        for h in &halves {
            if let (Some(b), Link::Dissolved) = (h.twin, h.link) {
                fuf.union(h.region, halves[b].region);
            }
        }

        // Walk the merged faces' loops, skipping dissolved edges.
        let mut visited = vec![false; halves.len()];
        let mut face_of_root: HashMap<usize, usize> = HashMap::new();
        let mut faces: Vec<MFace> = Vec::new();
        let mut half_loops: Vec<(usize, Vec<usize>)> = Vec::new();
        for (ri, loops) in region_loops.iter().enumerate() {
            for l in loops {
                for &h0 in l {
                    if visited[h0] || halves[h0].link == Link::Dissolved {
                        continue;
                    }
                    let mut lp = Vec::new();
                    let mut h = h0;
                    loop {
                        visited[h] = true;
                        lp.push(h);
                        let mut n = halves[h].next;
                        let mut guard = 0;
                        while halves[n].link == Link::Dissolved {
                            n = halves[halves[n].twin.expect("dissolved edges have twins")].next;
                            guard += 1;
                            if guard > halves.len() {
                                return Err(SheetError::Invalid(
                                    "a face loop doesn't close".to_owned(),
                                ));
                            }
                        }
                        h = n;
                        if h == h0 {
                            break;
                        }
                        if lp.len() > halves.len() {
                            return Err(SheetError::Invalid(
                                "a face loop doesn't close".to_owned(),
                            ));
                        }
                    }
                    let root = fuf.find(ri);
                    let face = *face_of_root.entry(root).or_insert_with(|| {
                        faces.push(MFace {
                            piece: kept[ri].1,
                            loops: Vec::new(),
                        });
                        faces.len() - 1
                    });
                    half_loops.push((face, lp));
                }
            }
        }

        // Outgoing glued-surface edges per vertex group, to find removable vertices.
        let group = |uf: &mut UnionFind, h: usize| uf.find(h);
        let mut out_count: HashMap<usize, usize> = HashMap::new();
        for (_, lp) in &half_loops {
            for &h in lp {
                *out_count.entry(group(&mut uf, h)).or_default() += 1;
            }
        }
        let mergeable = |uf: &mut UnionFind, a: usize, b: usize| {
            let (ha, hb) = (&halves[a], &halves[b]);
            if ha.twin.is_some() || hb.twin.is_some() {
                return false;
            }
            let (Curve::Line { .. }, Curve::Line { .. }) = (ha.curve, hb.curve) else {
                return false;
            };
            let da = (ha.end - ha.start).normalize_or_zero();
            let db = (hb.end - hb.start).normalize_or_zero();
            da.dot(db) > 0.0
                && da.perp_dot(db).abs() <= 1e-9
                && out_count.get(&uf.find(b)).copied() == Some(1)
        };

        // Final edges, joining collinear boundary pieces.
        let mut edges: Vec<FEdge> = Vec::new();
        let mut edge_of_half: HashMap<usize, usize> = HashMap::new();
        for (face, lp) in &half_loops {
            let n = lp.len();
            // Start where the vertex can't be removed (there always is one: a loop needs
            // corners).
            let start = (0..n)
                .find(|&i| !mergeable(&mut uf, lp[(i + n - 1) % n], lp[i]))
                .unwrap_or(0);
            let first_edge = edges.len();
            let mut i = 0;
            while i < n {
                let h0 = lp[(start + i) % n];
                let mut last = h0;
                let mut j = i + 1;
                while j < n && mergeable(&mut uf, last, lp[(start + j) % n]) {
                    last = lp[(start + j) % n];
                    j += 1;
                }
                let (a, b) = (halves[h0].start, halves[last].end);
                let curve = if last == h0 {
                    halves[h0].curve
                } else {
                    Curve::Line { a, b }
                };
                let reversed = if last == h0 {
                    halves[h0].reversed
                } else {
                    false
                };
                let id = edges.len();
                edge_of_half.insert(h0, id);
                edges.push(FEdge {
                    curve,
                    reversed,
                    tag: tags[halves[h0].entity as usize],
                    start: uf.find(h0),
                    end: uf.find(halves[last].next),
                    face: *face,
                    twin: None,
                    next: 0,
                });
                i = j;
            }
            let count = edges.len() - first_edge;
            for k in 0..count {
                edges[first_edge + k].next = first_edge + (k + 1) % count;
            }
            faces[*face].loops.push((first_edge..edges.len()).collect());
        }
        for (&h, &e) in &edge_of_half {
            if halves[h].link == Link::Fold {
                let t = halves[h].twin.expect("fold edges have twins");
                edges[e].twin = edge_of_half.get(&t).copied();
            }
        }
        // `end` must be the group of the following edge's start (merging skipped corners).
        for e in 0..edges.len() {
            let next = edges[e].next;
            edges[e].end = edges[next].start;
        }

        let mut groups = vec![DVec2::ZERO; halves.len()];
        for (h, half) in halves.iter().enumerate() {
            let g = uf.find(h);
            groups[g] = half.start;
        }

        // Outer loop first in each face.
        for f in &mut faces {
            let area = |l: &Vec<usize>| -> f64 {
                l.iter()
                    .map(|&e| area_term(&edges[e].curve, edges[e].reversed))
                    .sum()
            };
            f.loops.sort_by(|a, b| area(b).total_cmp(&area(a)));
            if f.loops.first().is_none_or(|l| area(l) <= 0.0)
                || f.loops.iter().skip(1).any(|l| area(l) > 0.0)
            {
                return Err(SheetError::Invalid(
                    "a flat face has no single outer boundary".to_owned(),
                ));
            }
        }
        Ok(Self {
            edges,
            faces,
            groups,
        })
    }

    /// The boundary loops of the glued flat pattern (the holes forms fill are not cut).
    fn outline(&self, layout: &Layout) -> Vec<FlatLoop> {
        let mut out = Vec::new();
        let mut seen = vec![false; self.edges.len()];
        for e0 in 0..self.edges.len() {
            if seen[e0] || self.edges[e0].twin.is_some() {
                continue;
            }
            if form_of(layout, self.edges[e0].tag).is_some() {
                continue;
            }
            let mut lp = Vec::new();
            let mut e = e0;
            loop {
                seen[e] = true;
                lp.push(e);
                let mut n = self.edges[e].next;
                let mut guard = 0;
                while let Some(t) = self.edges[n].twin {
                    n = self.edges[t].next;
                    guard += 1;
                    if guard > self.edges.len() {
                        break;
                    }
                }
                e = n;
                if e == e0 || lp.len() > self.edges.len() {
                    break;
                }
            }
            let area: f64 = lp
                .iter()
                .map(|&e| area_term(&self.edges[e].curve, self.edges[e].reversed))
                .sum();
            let edges: Vec<(Curve, bool)> = lp
                .iter()
                .map(|&e| (self.edges[e].curve, self.edges[e].reversed))
                .collect();
            out.push(FlatLoop {
                edges: join_pieces(edges),
                outer: area > 0.0,
            });
        }
        out
    }

    /// Whether `p` is inside merged face `f`.
    fn face_contains(&self, f: usize, p: DVec2) -> bool {
        let mut inside = false;
        for l in &self.faces[f].loops {
            let edges: Vec<Edge2> = l
                .iter()
                .map(|&e| Edge2 {
                    curve: self.edges[e].curve,
                    reversed: self.edges[e].reversed,
                    tag: self.edges[e].tag,
                })
                .collect();
            if loop_winding(&edges, p) != 0 {
                inside = !inside;
            }
        }
        inside
    }

    /// The centre line of each bend, where its strip still has material.
    fn bend_lines(&self, layout: &Layout) -> Vec<BendLine> {
        let mut out = Vec::new();
        for (pi, bend) in layout.bends() {
            let a = bend.origin + bend.across * (bend.width() / 2.0);
            let b = a + bend.along * bend.length;
            let line = Curve::Line { a, b };
            let faces: Vec<usize> = (0..self.faces.len())
                .filter(|&f| self.faces[f].piece == pi)
                .collect();
            let mut ts = vec![0.0, 1.0];
            for &f in &faces {
                for l in &self.faces[f].loops {
                    for &e in l {
                        for x in line.intersect(&self.edges[e].curve) {
                            if (0.0..=1.0).contains(&x.t_a) {
                                ts.push(x.t_a);
                            }
                        }
                    }
                }
            }
            ts.sort_by(f64::total_cmp);
            ts.dedup_by(|x, y| (*x - *y).abs() * bend.length <= SAME_POINT);
            let mut segments: Vec<[DVec2; 2]> = Vec::new();
            for w in ts.windows(2) {
                let mid = line.point_at((w[0] + w[1]) / 2.0);
                if faces.iter().any(|&f| self.face_contains(f, mid)) {
                    let (p, q) = (line.point_at(w[0]), line.point_at(w[1]));
                    match segments.last_mut() {
                        Some(last) if close(last[1], p) => last[1] = q,
                        _ => segments.push([p, q]),
                    }
                }
            }
            out.push(BendLine {
                piece: pi,
                segments,
            });
        }
        out
    }
}

/// Joins consecutive pieces of one line (or one arc) in a closed loop: a cut's edge that
/// crosses a bend is one edge of the flat pattern, though the body splits it at the bend
/// lines.
fn join_pieces(mut edges: Vec<(Curve, bool)>) -> Vec<(Curve, bool)> {
    let ends = |(c, r): &(Curve, bool)| {
        if *r {
            (c.end(), c.start())
        } else {
            (c.start(), c.end())
        }
    };
    let join = |a: &(Curve, bool), b: &(Curve, bool)| -> Option<(Curve, bool)> {
        let ((p, q), (q2, r)) = (ends(a), ends(b));
        if !close(q, q2) {
            return None;
        }
        match (a.0, b.0) {
            (Curve::Line { .. }, Curve::Line { .. }) => {
                let (d1, d2) = ((q - p).normalize_or_zero(), (r - q2).normalize_or_zero());
                (d1.dot(d2) > 0.0 && d1.perp_dot(d2).abs() <= 1e-9)
                    .then_some((Curve::Line { a: p, b: r }, false))
            }
            (
                Curve::Arc {
                    center: c1,
                    radius: r1,
                    sweep: s1,
                    ..
                },
                Curve::Arc {
                    center: c2,
                    radius: r2,
                    sweep: s2,
                    ..
                },
            ) if close(c1, c2)
                && (r1 - r2).abs() <= SAME_POINT
                && a.1 == b.1
                && s1 + s2 < std::f64::consts::TAU - 1e-9 =>
            {
                // In curve direction the pieces run first-to-second, or second-to-first
                // when traversed backwards.
                let (from, to) = if a.1 { (r, p) } else { (p, r) };
                Some((Curve::arc_from_points(c1, from, to), a.1))
            }
            _ => None,
        }
    };
    let mut changed = true;
    while changed && edges.len() > 2 {
        changed = false;
        let n = edges.len();
        for i in 0..n {
            let j = (i + 1) % n;
            if let Some(m) = join(&edges[i], &edges[j]) {
                edges[i] = m;
                edges.remove(j);
                changed = true;
                break;
            }
        }
    }
    edges
}

// ---- Step 3: thickening ----

/// What each element of the thickened solid came from, for folding.
struct Info {
    /// Per vertex: where it came from.
    vertices: Vec<VertexSource>,
    /// Per edge: what made it.
    edges: Vec<EdgeSource>,
    /// Per face: its tag's piece and kind.
    faces: Vec<FaceSource>,
}

#[derive(Clone, Copy)]
enum VertexSource {
    /// A vertex group of the flat pattern, on the top or the bottom side.
    Group { group: usize, top: bool },
    /// A point of a form, in flat coordinates, on a flange.
    Form { piece: usize, at: DVec3 },
}

#[derive(Clone, Copy)]
enum EdgeSource {
    /// A top or bottom copy of a flat edge.
    Cap { edge: usize, top: bool },
    /// The vertical edge at a vertex group (where walls meet).
    Vertical,
    /// An edge of a form, on a flange.
    Form { piece: usize, line: bool },
}

#[derive(Clone, Copy)]
enum FaceSource {
    Cap { face: usize, top: bool },
    Wall { edge: usize },
    Form { piece: usize },
}

/// The form an edge of the flat pattern outlines, and which side of it.
fn form_of(layout: &Layout, tag: CurveTag) -> Option<(usize, usize)> {
    layout
        .forms
        .iter()
        .enumerate()
        .find_map(|(i, f)| f.side_of(tag).map(|s| (i, s)))
}

impl Flat {
    fn thicken(&self, layout: &Layout) -> Result<(Solid, Vec<FaceTag>, Info), SheetError> {
        let t = layout.settings.thickness;
        let mut solid = Solid::new();
        let mut info = Info {
            vertices: Vec::new(),
            edges: Vec::new(),
            faces: Vec::new(),
        };
        let mut tags: Vec<FaceTag> = Vec::new();

        // Shells: faces joined through bend lines.
        let mut suf = UnionFind::new(self.faces.len());
        for e in &self.edges {
            if let Some(tw) = e.twin {
                suf.union(e.face, self.edges[tw].face);
            }
        }
        let mut shell_of: HashMap<usize, ShellId> = HashMap::new();
        for f in 0..self.faces.len() {
            let root = suf.find(f);
            shell_of.entry(root).or_insert_with(|| solid.add_shell());
        }

        // Vertices: a bottom and a top one per used group.
        let mut verts: HashMap<usize, (VertexId, VertexId)> = HashMap::new();
        for e in &self.edges {
            for g in [e.start, e.end] {
                verts.entry(g).or_insert_with(|| {
                    let p = self.groups[g];
                    let b = solid.add_vertex(p.extend(0.0));
                    info.vertices.push(VertexSource::Group {
                        group: g,
                        top: false,
                    });
                    let tv = solid.add_vertex(p.extend(t));
                    info.vertices.push(VertexSource::Group {
                        group: g,
                        top: true,
                    });
                    (b, tv)
                });
            }
        }

        // Cap edges, shared along bend lines, stored in the curve's own direction.
        let mut caps: Vec<Option<(EdgeId, EdgeId)>> = vec![None; self.edges.len()];
        for (i, e) in self.edges.iter().enumerate() {
            if let Some(tw) = e.twin
                && let Some(shared) = caps[tw]
            {
                caps[i] = Some(shared);
                continue;
            }
            let (gs, ge) = if e.reversed {
                (e.end, e.start)
            } else {
                (e.start, e.end)
            };
            let mut make = |top: bool, solid: &mut Solid| {
                let pick = |v: &(VertexId, VertexId)| if top { v.1 } else { v.0 };
                let (vs, ve) = (pick(&verts[&gs]), pick(&verts[&ge]));
                let z = if top { t } else { 0.0 };
                let id = match e.curve {
                    Curve::Line { .. } => solid.add_line_edge(vs, ve),
                    Curve::Arc {
                        center,
                        radius,
                        start_angle,
                        sweep,
                    } => solid.add_edge(
                        Curve3::Circle(Circle3 {
                            frame: Frame {
                                origin: center.extend(z),
                                rotation: peet_math::DQuat::IDENTITY,
                            },
                            radius,
                        }),
                        vs,
                        ve,
                        start_angle,
                        start_angle + sweep,
                    ),
                    Curve::Circle { .. } => unreachable!("circles are split into arcs"),
                };
                info.edges.push(EdgeSource::Cap { edge: i, top });
                id
            };
            let b = make(false, &mut solid);
            let tp = make(true, &mut solid);
            caps[i] = Some((b, tp));
        }
        let caps: Vec<(EdgeId, EdgeId)> = caps
            .into_iter()
            .map(|c| c.expect("every edge has caps"))
            .collect();

        // The sides of forms that the form's own faces close (all but a louver's lance).
        let formed: Vec<bool> = self
            .edges
            .iter()
            .map(|e| {
                form_of(layout, e.tag)
                    .is_some_and(|(f, side)| layout.forms[f].open_side != Some(side))
            })
            .collect();

        // Vertical edges where walls meet.
        let mut vertical: HashMap<usize, EdgeId> = HashMap::new();
        for (_, e) in self
            .edges
            .iter()
            .enumerate()
            .filter(|(i, e)| e.twin.is_none() && !formed[*i])
        {
            for g in [e.start, e.end] {
                vertical.entry(g).or_insert_with(|| {
                    let (b, tp) = verts[&g];
                    info.edges.push(EdgeSource::Vertical);
                    solid.add_line_edge(b, tp)
                });
            }
        }

        // Caps.
        for (fi, f) in self.faces.iter().enumerate() {
            let shell = shell_of[&suf.find(fi)];
            let top_plane = Plane {
                frame: Frame {
                    origin: DVec3::new(0.0, 0.0, t),
                    rotation: peet_math::DQuat::IDENTITY,
                },
            };
            let top = solid.add_face(shell, Surface::Plane(top_plane), false);
            tags.push(FaceTag::Top { piece: f.piece });
            info.faces.push(FaceSource::Cap {
                face: fi,
                top: true,
            });
            for l in &f.loops {
                let uses: Vec<(EdgeId, bool)> = l
                    .iter()
                    .map(|&e| (caps[e].1, self.edges[e].reversed))
                    .collect();
                solid.add_loop(top, &uses);
            }
            let bottom_plane =
                Plane::from_origin_normal_x(DVec3::ZERO, -DVec3::Z, DVec3::X).expect("valid plane");
            let bottom = solid.add_face(shell, Surface::Plane(bottom_plane), false);
            tags.push(FaceTag::Bottom { piece: f.piece });
            info.faces.push(FaceSource::Cap {
                face: fi,
                top: false,
            });
            for l in &f.loops {
                let uses: Vec<(EdgeId, bool)> = l
                    .iter()
                    .rev()
                    .map(|&e| (caps[e].0, !self.edges[e].reversed))
                    .collect();
                solid.add_loop(bottom, &uses);
            }
        }

        // Walls.
        for (i, e) in self.edges.iter().enumerate() {
            if e.twin.is_some() || formed[i] {
                continue;
            }
            let shell = shell_of[&suf.find(e.face)];
            let (surface, reversed) = match e.curve {
                Curve::Line { .. } => {
                    let (a, b) = e.ends();
                    let dir = (b - a).extend(0.0);
                    let plane =
                        Plane::from_origin_normal_x(a.extend(0.0), dir.cross(DVec3::Z), dir)
                            .ok_or_else(|| SheetError::Invalid("a zero-length edge".to_owned()))?;
                    (Surface::Plane(plane), false)
                }
                Curve::Arc { center, radius, .. } => (
                    Surface::Cylinder(Cylinder {
                        frame: Frame {
                            origin: center.extend(0.0),
                            rotation: peet_math::DQuat::IDENTITY,
                        },
                        radius,
                    }),
                    e.reversed,
                ),
                Curve::Circle { .. } => unreachable!("circles are split into arcs"),
            };
            let face = solid.add_face(shell, surface, reversed);
            tags.push(FaceTag::Wall {
                piece: self.faces[e.face].piece,
                tag: e.tag,
                curve: e.curve,
                reversed: e.reversed,
            });
            info.faces.push(FaceSource::Wall { edge: i });
            let (b, tp) = caps[i];
            solid.add_loop(
                face,
                &[
                    (b, e.reversed),
                    (vertical[&e.end], false),
                    (tp, !e.reversed),
                    (vertical[&e.start], true),
                ],
            );
        }
        let mut ctx = FormCtx {
            solid: &mut solid,
            tags: &mut tags,
            info: &mut info,
            caps: &caps,
            verts: &verts,
            vertical: &vertical,
        };
        for f in 0..layout.forms.len() {
            let shell = self
                .edges
                .iter()
                .find(|e| form_of(layout, e.tag).is_some_and(|(i, _)| i == f))
                .map(|e| shell_of[&suf.find(e.face)]);
            let Some(shell) = shell else {
                return Err(SheetError::Message(format!(
                    "A {} was cut away with the material it was on.",
                    layout.forms[f].kind.label().to_lowercase()
                )));
            };
            self.form(layout, f, shell, &mut ctx)?;
        }
        Ok((solid, tags, info))
    }
}

// ---- Step 4: folding ----

/// How a piece's flat coordinates go into the model.
enum Map<'a> {
    Rigid(Frame),
    Bend { parent: Frame, bend: &'a Bend },
}

impl Map<'_> {
    fn point(&self, p: DVec3) -> DVec3 {
        match self {
            Self::Rigid(f) => f.to_world(p),
            Self::Bend { parent, bend } => parent.to_world(bend.fold_point(p)),
        }
    }
}

fn piece_map(layout: &Layout, piece: usize) -> Map<'_> {
    let p = &layout.pieces[piece];
    match &p.kind {
        PieceKind::Flange => Map::Rigid(p.frame),
        PieceKind::Bend(b) => Map::Bend {
            parent: p.frame,
            bend: b,
        },
    }
}

fn fold(layout: &Layout, flat: &Flat, local: &Solid, info: &Info) -> Result<Solid, SheetError> {
    let t = layout.settings.thickness;
    let piece_of_edge = |e: usize| -> usize {
        // Along a bend line, use the flange side: its motion is rigid.
        let fe = &flat.edges[e];
        let p = flat.faces[fe.face].piece;
        match fe.twin {
            Some(tw) if !layout.pieces[p].is_flange() => flat.faces[flat.edges[tw].face].piece,
            _ => p,
        }
    };
    // A piece for each vertex group: a flange if any touches it.
    let mut group_piece: HashMap<usize, usize> = HashMap::new();
    for (i, e) in flat.edges.iter().enumerate() {
        let p = piece_of_edge(i);
        for g in [e.start, e.end] {
            let slot = group_piece.entry(g).or_insert(p);
            if !layout.pieces[*slot].is_flange() && layout.pieces[p].is_flange() {
                *slot = p;
            }
        }
    }

    let mut out = local.clone();
    for (v, src) in info.vertices.iter().enumerate() {
        out.vertices[v].point = match *src {
            VertexSource::Group { group, top } => {
                let p = flat.groups[group].extend(if top { t } else { 0.0 });
                piece_map(layout, group_piece[&group]).point(p)
            }
            VertexSource::Form { piece, at } => layout.pieces[piece].frame.to_world(at),
        };
    }
    let point = |out: &Solid, v: VertexId| out.vertices[v.index()].point;

    for (ei, src) in info.edges.iter().enumerate() {
        let edge = &local.edges[ei];
        let (vs, ve) = (point(&out, edge.start), point(&out, edge.end));
        let (curve, t0, t1) = match *src {
            EdgeSource::Vertical => line_between(vs, ve)?,
            EdgeSource::Form { line: true, .. } => line_between(vs, ve)?,
            EdgeSource::Form { piece, line: false } => (
                transform::curve(&edge.curve, &layout.pieces[piece].frame),
                edge.t0,
                edge.t1,
            ),
            EdgeSource::Cap { edge: fe, top } => {
                let p = piece_of_edge(fe);
                let fedge = &flat.edges[fe];
                match piece_map(layout, p) {
                    Map::Rigid(frame) => match fedge.curve {
                        Curve::Line { .. } => line_between(vs, ve)?,
                        _ => (transform::curve(&edge.curve, &frame), edge.t0, edge.t1),
                    },
                    Map::Bend { parent, bend } => {
                        let Curve::Line { a, b } = fedge.curve else {
                            return Err(SheetError::AcrossBend {
                                tag: fedge.tag,
                                bend: layout.pieces[p].origin,
                                curved: true,
                            });
                        };
                        let dir = (b - a).normalize_or_zero();
                        if dir.perp_dot(bend.along).abs() <= PARALLEL {
                            line_between(vs, ve)?
                        } else if dir.perp_dot(bend.across).abs() <= PARALLEL {
                            let z = if top { t } else { 0.0 };
                            arc_across(parent, bend, a, b, z, vs)
                        } else {
                            return Err(SheetError::AcrossBend {
                                tag: fedge.tag,
                                bend: layout.pieces[p].origin,
                                curved: false,
                            });
                        }
                    }
                }
            }
        };
        let e = &mut out.edges[ei];
        e.curve = curve;
        e.t0 = t0;
        e.t1 = t1;
    }

    for (fi, src) in info.faces.iter().enumerate() {
        let face = &local.faces[fi];
        let (surface, reversed) = match *src {
            FaceSource::Form { piece } => (
                transform::surface(&face.surface, &layout.pieces[piece].frame),
                face.reversed,
            ),
            FaceSource::Cap { face: mf, top } => {
                let p = flat.faces[mf].piece;
                match piece_map(layout, p) {
                    Map::Rigid(frame) => (transform::surface(&face.surface, &frame), face.reversed),
                    Map::Bend { parent, bend } => {
                        let z = if top { t } else { 0.0 };
                        // The outward normal is +z on top, -z below; the radius falls with z
                        // in an upward bend.
                        let outward_up = top;
                        let radius_grows_up = !bend.up;
                        (
                            bend_cylinder(parent, bend, z),
                            outward_up != radius_grows_up,
                        )
                    }
                }
            }
            FaceSource::Wall { edge: fe } => {
                let fedge = &flat.edges[fe];
                let p = flat.faces[fedge.face].piece;
                match piece_map(layout, p) {
                    Map::Rigid(frame) => (transform::surface(&face.surface, &frame), face.reversed),
                    Map::Bend { parent, bend } => {
                        let Curve::Line { .. } = fedge.curve else {
                            return Err(SheetError::AcrossBend {
                                tag: fedge.tag,
                                bend: layout.pieces[p].origin,
                                curved: true,
                            });
                        };
                        let (a, b) = fedge.ends();
                        let dir = (b - a).normalize_or_zero();
                        let normal = DVec2::new(dir.y, -dir.x);
                        let (s, _) = bend.strip_coords(a);
                        let n3 = if dir.perp_dot(bend.along).abs() <= PARALLEL
                            || dir.perp_dot(bend.across).abs() <= PARALLEL
                        {
                            parent.vector_to_world(bend.fold_vector(s, normal.extend(0.0)))
                        } else {
                            return Err(SheetError::AcrossBend {
                                tag: fedge.tag,
                                bend: layout.pieces[p].origin,
                                curved: false,
                            });
                        };
                        let at = parent.to_world(bend.fold_point(a.extend(0.0)));
                        let x = parent.vector_to_world(bend.fold_vector(s, dir.extend(0.0)));
                        let plane = Plane::from_origin_normal_x(at, n3, x)
                            .ok_or_else(|| SheetError::Invalid("a degenerate wall".to_owned()))?;
                        (Surface::Plane(plane), false)
                    }
                }
            }
        };
        out.faces[fi].surface = surface;
        out.faces[fi].reversed = reversed;
    }
    Ok(out)
}

/// A straight edge between two placed vertices.
fn line_between(a: DVec3, b: DVec3) -> Result<(Curve3, f64, f64), SheetError> {
    let c = Curve3::line_through(a, b)
        .ok_or_else(|| SheetError::Invalid("an edge folded to zero length".to_owned()))?;
    Ok((c, 0.0, a.distance(b)))
}

/// The arc a flat line across a bend folds into, at height `z`.
fn arc_across(
    parent: Frame,
    bend: &Bend,
    a: DVec2,
    b: DVec2,
    z: f64,
    vs: DVec3,
) -> (Curve3, f64, f64) {
    let (sa, w) = bend.strip_coords(a);
    let (sb, _) = bend.strip_coords(b);
    let (axis_point, axis_dir) = bend.axis();
    let center = parent.to_world(axis_point + axis_dir * w);
    let axis = parent.vector_to_world(axis_dir);
    // Turning from a to b about `along` by the fold's signed angle difference.
    let c = bend.along.perp_dot(bend.across);
    let side = if bend.up { 1.0 } else { -1.0 };
    let turn = side * c * (bend.angle_at(sb) - bend.angle_at(sa));
    let z_axis = if turn >= 0.0 { axis } else { -axis };
    let frame = Frame::from_origin_z_x(center, z_axis, vs - center).unwrap_or(Frame {
        origin: center,
        rotation: parent.rotation,
    });
    (
        Curve3::Circle(Circle3 {
            frame,
            radius: bend.radius_at(z),
        }),
        0.0,
        turn.abs(),
    )
}

/// The cylinder a bend's top or bottom side (at height `z`) folds onto.
fn bend_cylinder(parent: Frame, bend: &Bend, z: f64) -> Surface {
    let (axis_point, axis_dir) = bend.axis();
    // The parameter seam goes opposite the middle of the bend, far from the face.
    let radial = if bend.up { -DVec3::Z } else { DVec3::Z };
    let mid = bend.fold_vector(bend.width() / 2.0, radial);
    let frame = Frame::from_origin_z_x(
        parent.to_world(axis_point),
        parent.vector_to_world(axis_dir),
        parent.vector_to_world(mid),
    )
    .unwrap_or(parent);
    Surface::Cylinder(Cylinder {
        frame,
        radius: bend.radius_at(z),
    })
}

// ---- Forms ----

/// What the thickened solid has so far, for adding forms to it.
struct FormCtx<'a> {
    solid: &'a mut Solid,
    tags: &'a mut Vec<FaceTag>,
    info: &'a mut Info,
    /// Bottom and top copies of each flat edge.
    caps: &'a [(EdgeId, EdgeId)],
    /// Bottom and top vertices of each vertex group.
    verts: &'a HashMap<usize, (VertexId, VertexId)>,
    /// The vertical edge at a vertex group.
    vertical: &'a HashMap<usize, EdgeId>,
}

/// An edge used by a loop: the edge, and whether the loop runs against its direction.
type Use = (EdgeId, bool);

/// The right-hand normal of a curve traversed in loop order, at `p`.
fn right_normal(curve: &Curve, reversed: bool, p: DVec2) -> DVec2 {
    let tangent = match *curve {
        Curve::Line { a, b } => (b - a).normalize_or_zero(),
        Curve::Arc { center, .. } | Curve::Circle { center, .. } => {
            let r = (p - center).normalize_or_zero();
            DVec2::new(-r.y, r.x) // counter-clockwise
        }
    };
    let tangent = if reversed { -tangent } else { tangent };
    DVec2::new(tangent.y, -tangent.x)
}

impl Flat {
    /// Builds form `f`'s faces into the hole its outline left in the flange.
    ///
    /// The faces are made for a form standing out of the top side, at *canonical*
    /// heights: 0 at the face it leaves, `t` at the other face, `h` and `h + t` at the
    /// plateau. A form on the bottom side is the mirror image: heights are flipped and
    /// every loop is turned round.
    fn form(
        &self,
        layout: &Layout,
        f: usize,
        shell: ShellId,
        cx: &mut FormCtx<'_>,
    ) -> Result<(), SheetError> {
        let form = &layout.forms[f];
        let t = layout.settings.thickness;
        let h = form.height;
        let up = form.up;
        let piece = form.piece;
        let z = |c: f64| if up { c } else { t - c };
        let what = form.kind.label().to_lowercase();

        // The hole's loop, in order (the flange's material on the left).
        let first = (0..self.edges.len())
            .find(|&e| form_of(layout, self.edges[e].tag).is_some_and(|(i, _)| i == f))
            .expect("the caller found an edge");
        let face = &self.faces[self.edges[first].face];
        let lp = face
            .loops
            .iter()
            .find(|l| l.contains(&first))
            .expect("every edge is on a loop");
        if !lp
            .iter()
            .all(|&e| form_of(layout, self.edges[e].tag).is_some_and(|(i, _)| i == f))
        {
            return Err(SheetError::Message(format!(
                "The {what} touches the edge of the sheet, a cut or another form. Move it clear."
            )));
        }
        let n = lp.len();
        let edge = |k: usize| &self.edges[lp[k % n]];
        let side = |k: usize| form_of(layout, edge(k).tag).map_or(0, |(_, s)| s);
        let open = (0..n).find(|&k| form.open_side == Some(side(k)));
        let start = |k: usize| edge(k).ends().0;

        // The inside of the wall: each side moved a thickness into the form (a louver's
        // open side stays), meeting at the corners.
        let shift = |k: usize| if open == Some(k % n) { 0.0 } else { t };
        let mut w = Vec::with_capacity(n);
        for k in 0..n {
            let (prev, next) = (edge(k + n - 1), edge(k));
            let v = start(k);
            let n2 = right_normal(&next.curve, next.reversed, v);
            let n1 = right_normal(&prev.curve, prev.reversed, v);
            let point = match (prev.curve, next.curve) {
                (Curve::Line { .. }, Curve::Line { .. }) if n1.perp_dot(n2).abs() > 1e-9 => {
                    // Where the two moved lines cross.
                    let (p1, d1) = (v + n1 * shift(k + n - 1), DVec2::new(-n1.y, n1.x));
                    let (p2, d2) = (v + n2 * shift(k), DVec2::new(-n2.y, n2.x));
                    let s = (p2 - p1).perp_dot(d2) / d1.perp_dot(d2);
                    p1 + d1 * s
                }
                _ => v + n2 * shift(k),
            };
            w.push(point);
        }
        // The inside curve of side k, from w[k] to w[k + 1], and whether it runs against
        // its own direction.
        let inner = |k: usize| -> (Curve, bool) {
            let e = edge(k);
            match e.curve {
                Curve::Arc {
                    center,
                    start_angle,
                    sweep,
                    ..
                } => (
                    Curve::Arc {
                        center,
                        radius: w[k].distance(center),
                        start_angle,
                        sweep,
                    },
                    e.reversed,
                ),
                _ => (
                    Curve::Line {
                        a: w[k],
                        b: w[(k + 1) % n],
                    },
                    false,
                ),
            }
        };

        // Vertices.
        let group = |k: usize| edge(k).start;
        let cap_vertex = |k: usize, level: f64| {
            let (b, tp) = cx.verts[&group(k % n)];
            if (level == 0.0) == up { b } else { tp }
        };
        let vertex = |cx: &mut FormCtx<'_>, p: DVec2, c: f64| {
            let at = p.extend(z(c));
            cx.info.vertices.push(VertexSource::Form { piece, at });
            cx.solid.add_vertex(at)
        };
        let vht: Vec<VertexId> = (0..n).map(|k| vertex(cx, start(k), h + t)).collect();
        let w0: Vec<VertexId> = (0..n).map(|k| vertex(cx, w[k], 0.0)).collect();
        let wh: Vec<VertexId> = (0..n).map(|k| vertex(cx, w[k], h)).collect();

        // Edges. `curve_edge` makes a copy of a flat curve at height c, used from `a` to
        // `b`.
        let curve_edge = |cx: &mut FormCtx<'_>,
                          curve: Curve,
                          reversed: bool,
                          c: f64,
                          a: VertexId,
                          b: VertexId|
         -> Use {
            match curve {
                Curve::Arc {
                    center,
                    radius,
                    start_angle,
                    sweep,
                } => {
                    let (cs, ce) = if reversed { (b, a) } else { (a, b) };
                    cx.info.edges.push(EdgeSource::Form { piece, line: false });
                    let id = cx.solid.add_edge(
                        Curve3::Circle(Circle3 {
                            frame: Frame {
                                origin: center.extend(z(c)),
                                rotation: peet_math::DQuat::IDENTITY,
                            },
                            radius,
                        }),
                        cs,
                        ce,
                        start_angle,
                        start_angle + sweep,
                    );
                    (id, reversed)
                }
                _ => {
                    cx.info.edges.push(EdgeSource::Form { piece, line: true });
                    (cx.solid.add_line_edge(a, b), false)
                }
            }
        };
        let line_edge = |cx: &mut FormCtx<'_>, a: VertexId, b: VertexId| -> EdgeId {
            cx.info.edges.push(EdgeSource::Form { piece, line: true });
            cx.solid.add_line_edge(a, b)
        };
        // The flat's own copies of the outline at heights 0 and t.
        let cap_use = |k: usize, level: f64| -> Use {
            let (b, tp) = cx.caps[lp[k % n]];
            let id = if (level == 0.0) == up { b } else { tp };
            (id, edge(k).reversed)
        };
        let c0: Vec<Use> = (0..n).map(|k| cap_use(k, 0.0)).collect();
        let ct: Vec<Use> = (0..n).map(|k| cap_use(k, t)).collect();
        let cht: Vec<Use> = (0..n)
            .map(|k| {
                let e = edge(k);
                curve_edge(cx, e.curve, e.reversed, h + t, vht[k], vht[(k + 1) % n])
            })
            .collect();
        let mut cw0: Vec<Option<Use>> = vec![None; n];
        for (k, slot) in cw0.iter_mut().enumerate() {
            if open != Some(k) {
                let (c, r) = inner(k);
                *slot = Some(curve_edge(cx, c, r, 0.0, w0[k], w0[(k + 1) % n]));
            }
        }
        let cwh: Vec<Use> = (0..n)
            .map(|k| {
                let (c, r) = inner(k);
                curve_edge(cx, c, r, h, wh[k], wh[(k + 1) % n])
            })
            .collect();
        // Verticals: up the outside of the wall at the outline's corners, up the inside
        // at the inside's corners.
        let vt: Vec<EdgeId> = (0..n)
            .map(|k| line_edge(cx, cap_vertex(k, t), vht[k]))
            .collect();
        let vw: Vec<EdgeId> = (0..n).map(|k| line_edge(cx, w0[k], wh[k])).collect();
        // A louver's mouth: the foot's two ends at the open side.
        let legs = open.map(|o| {
            (
                line_edge(cx, cap_vertex(o, 0.0), w0[o]),
                line_edge(cx, w0[(o + 1) % n], cap_vertex(o + 1, 0.0)),
            )
        });

        // Faces.
        let flip = |l: &[Use]| -> Vec<Use> { l.iter().rev().map(|&(e, r)| (e, !r)).collect() };
        let add_face = |cx: &mut FormCtx<'_>,
                        surface: Surface,
                        reversed: bool,
                        loops: Vec<Vec<Use>>,
                        index: u8| {
            let face = cx.solid.add_face(shell, surface, reversed);
            for l in loops {
                // The mirror image turns every loop round.
                let l = if up { l } else { flip(&l) };
                cx.solid.add_loop(face, &l);
            }
            cx.tags.push(FaceTag::Form {
                piece,
                tag: CurveTag::Generated {
                    owner: form.owner,
                    part: form.part,
                    index,
                },
            });
            cx.info.faces.push(FaceSource::Form { piece });
        };
        let level_plane = |c: f64, outward_up: bool| {
            // The canonical outward direction, mirrored for a form on the bottom side.
            let n = if outward_up == up {
                DVec3::Z
            } else {
                -DVec3::Z
            };
            Surface::Plane(
                Plane::from_origin_normal_x(DVec3::new(0.0, 0.0, z(c)), n, DVec3::X)
                    .expect("a valid plane"),
            )
        };
        // A wall along a curve, facing its left (`left`) or its right.
        let wall =
            |curve: &Curve, reversed: bool, left: bool| -> Result<(Surface, bool), SheetError> {
                let mid = curve.point_at(0.5);
                let right = right_normal(curve, reversed, mid);
                let normal = if left { -right } else { right };
                Ok(match *curve {
                    Curve::Line { a, b } => {
                        let d = (b - a).extend(0.0);
                        let plane =
                            Plane::from_origin_normal_x(a.extend(0.0), normal.extend(0.0), d)
                                .ok_or_else(|| {
                                    SheetError::Invalid("a form wall has no length".to_owned())
                                })?;
                        (Surface::Plane(plane), false)
                    }
                    Curve::Arc { center, radius, .. } | Curve::Circle { center, radius } => (
                        Surface::Cylinder(Cylinder {
                            frame: Frame {
                                origin: center.extend(0.0),
                                rotation: peet_math::DQuat::IDENTITY,
                            },
                            radius,
                        }),
                        normal.dot(mid - center) < 0.0,
                    ),
                })
            };

        // The foot: where the wall leaves the sheet, between the outline and the inside.
        let foot = match (open, legs) {
            (Some(o), Some((leg_a, leg_b))) => {
                let mut l = Vec::new();
                for k in o + 1..o + n {
                    l.push(c0[k % n]);
                }
                l.push((leg_a, false));
                for k in (o + 1..o + n).rev() {
                    let (e, r) = cw0[k % n].expect("formed sides have an inside edge");
                    l.push((e, !r));
                }
                l.push((leg_b, false));
                vec![l]
            }
            _ => {
                let inside: Vec<Use> = cw0.iter().map(|u| u.expect("formed")).collect();
                vec![c0.clone(), flip(&inside)]
            }
        };
        add_face(cx, level_plane(0.0, false), false, foot, form_face::FOOT);
        for k in 0..n {
            if open == Some(k) {
                continue;
            }
            let e = edge(k);
            let k1 = (k + 1) % n;
            // Outside of the wall, facing away from the form.
            let (surface, rev) = wall(&e.curve, e.reversed, true)?;
            let (ct_e, ct_r) = ct[k];
            add_face(
                cx,
                surface,
                rev,
                vec![vec![(ct_e, !ct_r), (vt[k], false), cht[k], (vt[k1], true)]],
                form_face::OUTER + k as u8,
            );
            // Inside of the wall, facing into the form.
            let (c, r) = inner(k);
            let (surface, rev) = wall(&c, r, false)?;
            let (cwh_e, cwh_r) = cwh[k];
            add_face(
                cx,
                surface,
                rev,
                vec![vec![
                    cw0[k].expect("formed"),
                    (vw[k1], false),
                    (cwh_e, !cwh_r),
                    (vw[k], true),
                ]],
                form_face::INNER + k as u8,
            );
        }
        add_face(
            cx,
            level_plane(h + t, true),
            false,
            vec![flip(&cht)],
            form_face::TOP,
        );
        add_face(
            cx,
            level_plane(h, false),
            false,
            vec![cwh.clone()],
            form_face::UNDER,
        );
        if let (Some(o), Some((leg_a, leg_b))) = (open, legs) {
            let o1 = (o + 1) % n;
            let e = edge(o);
            let (surface, rev) = wall(&e.curve, e.reversed, true)?;
            let (cwh_e, cwh_r) = cwh[o];
            // The existing verticals run bottom to top: canonical up is that way only for
            // a form on the top side.
            let mouth = vec![
                (cx.vertical[&group(o)], !up),
                (vt[o], false),
                cht[o],
                (vt[o1], true),
                (cx.vertical[&group(o1)], up),
                (leg_b, true),
                (vw[o1], false),
                (cwh_e, !cwh_r),
                (vw[o], true),
                (leg_a, true),
            ];
            add_face(cx, surface, rev, vec![mouth], form_face::MOUTH);
        }
        Ok(())
    }
}
