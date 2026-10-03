//! Recognising a sheet metal part in a plain solid: the way back from a folded B-rep to a
//! [`Layout`].
//!
//! Sheet metal bodies are native (see the crate docs): the layout is the definition and the
//! solids are derived. A solid that was modelled as a solid (extruded, shelled, filleted)
//! or imported from another CAD system has no layout. [`recognize`] reads one out of its
//! faces, so the body becomes a real sheet metal body: it unfolds, takes edge flanges and
//! sheet metal cuts, and exports a flat pattern.
//!
//! **What is recognised.** A sheet of one thickness `t`: flat walls joined by cylindrical
//! bends, cut square through the sheet along its edges and holes.
//!
//! **Method.**
//!
//! 1. *The two sides of the sheet.* The surface of a sheet is two smooth *skins* (its top
//!    and its bottom side), joined by the cut edges, which meet them at a sharp edge. The
//!    top skin is everything reached from the fixed face across smooth edges (where the
//!    two faces' normals agree). The thickness is the distance from the fixed face to the
//!    nearest flat face behind it, and the bottom skin is everything smooth with that
//!    one. A skin holds planes (the *walls*) and cylinders (the *bends*); anything else
//!    (a cone, a torus, a freeform face) is refused by name.
//! 2. *Walking the walls.* The fixed wall's placement puts flat-pattern `z` along its
//!    outward normal and `x` along its longest straight edge. From a placed wall, every
//!    cylinder that leaves it along a tangent line is a bend: its inner radius and the
//!    side it bends to come from which way the cylinder curves, its angle from how far
//!    the face runs round the axis, and its allowance from the body's bend model. The
//!    bend's far tangent line carries the next wall, placed by the parent's placement
//!    composed with the bend's fold ([`Bend::fold_frame`]). A wall reached twice means
//!    the walls close a loop (a box section), which can't be unrolled from one base.
//! 3. *Outlines.* A wall's outline is its top face's loops (holes included) taken into
//!    flat coordinates through its placement: lines stay lines, arcs stay arcs. A bend's
//!    outline is its cylinder face unrolled: edges along the axis become lines along the
//!    bend line and arcs round the axis become lines across the strip, scaled from the
//!    angle to the bend allowance. Any other edge on a bend has no flat form that
//!    [`build`] can fold back, and is refused.
//!    Faces of one cylinder that leave a wall along the same line with the same angle are
//!    one bend (a bend with a slot through it), and the flat faces at its far side are
//!    one wall, as the layout wants: a bend has one parent and one child.
//! 4. *Checks.* Every wall needs a flat face on the bottom skin exactly `t` behind it,
//!    every bend a coaxial cylinder whose radius differs by `t`, and every other face
//!    must be a plane or a cylinder that touches both skins (a cut edge).
//! 5. *Proof.* The layout is built, and the folded solid must have the input's volume
//!    and bounding box. Anything the checks above let through wrongly is caught here, so
//!    the result is either the same part or an error, never a different part.
//!
//! The K-factor (or other bend model) is not in the solid: it is the caller's, and only
//! changes the flat pattern.

use std::collections::{HashMap, VecDeque};
use std::f64::consts::{PI, TAU};

use peet_kernel::Solid;
use peet_kernel::geom::{Curve3, Cylinder, Surface};
use peet_kernel::topo::{CoedgeId, FaceId};
use peet_kernel::validate::measure;
use peet_math::{Aabb, DVec2, DVec3, Frame, tolerance};
use peet_sketch::Curve;

use crate::build::{SheetBody, SheetError, build};
use crate::layout::{Area, Bend, CurveTag, Edge2, Layout, Origin, Piece, PieceKind};
use crate::settings::{BendValues, SheetSettings};

#[cfg(test)]
mod tests;

/// Points closer than this are the same point (the tolerance [`build`] glues the flat
/// pattern with).
const SAME_POINT: f64 = 100.0 * tolerance::LINEAR;

/// Two faces are smooth across an edge, or two directions parallel, when the sine of the
/// angle between them is below this. Looser than [`tolerance::ANGULAR`] because imported
/// fillets are tangent only to the digits their file was written with.
const SMOOTH: f64 = 1000.0 * tolerance::ANGULAR;

/// Angles round a bend axis closer than this are the same angle.
const SAME_ANGLE: f64 = 100.0 * tolerance::ANGULAR;

/// How far the rebuilt solid's volume may differ from the input's, as a fraction of it.
const VOLUME_TOLERANCE: f64 = 1e-6;

/// How far the rebuilt solid's bounding box may differ from the input's, in mm.
const BOUNDS_TOLERANCE: f64 = 10.0 * SAME_POINT;

/// Most outline edges of one piece that get a wall number of their own block: a piece's
/// walls are numbered `part·256 + index`, with `part` starting at the piece's index times
/// this.
const PARTS_PER_PIECE: u32 = 1 << 12;

/// A solid recognised as sheet metal.
#[derive(Clone, Debug, PartialEq)]
pub struct Recognized {
    /// The folded solid built from the layout. It coincides with the input solid, but
    /// has the sheet metal topology (the same as the flat pattern's).
    pub solid: Solid,
    /// The sheet metal body: the layout, the flat pattern and what each face is.
    pub body: SheetBody,
    /// Where the layout sits: model from flat coordinates of the fixed wall.
    pub placement: Frame,
    /// The face of the input solid that became the top side of the fixed wall.
    pub fixed: FaceId,
}

/// Recognises `solid` as a sheet metal part and builds it.
///
/// `fixed` is the flat face that stays in place when the part is unfolded (the largest
/// flat face if `None`); it becomes the top side of the first flange. `hint` gives what
/// the solid can't: the bend model, and the relief settings for flanges added later. Its
/// thickness and radius are replaced by the part's. `owner` names the pieces and walls
/// (the converting feature's number).
pub fn recognize(
    solid: &Solid,
    fixed: Option<FaceId>,
    hint: &SheetSettings,
    owner: u32,
) -> Result<Recognized, SheetError> {
    let (layout, fixed) = recognize_layout(solid, fixed, hint, owner)?;
    let placement = layout.pieces[0].frame;
    let (folded, body) = build(layout).map_err(|e| match e {
        SheetError::Overlap { .. } => msg(
            "The part's walls overlap when it is unfolded, so it can't be cut from one flat blank. Pick another fixed face, or convert a part that unfolds without overlapping.",
        ),
        SheetError::AcrossBend { .. } => msg(ACROSS_BEND),
        other => other,
    })?;
    verify(solid, &folded)?;
    Ok(Recognized {
        solid: folded,
        body,
        placement,
        fixed,
    })
}

/// The layout of `solid` (see [`recognize`]) and the fixed face, without building it or
/// checking the result against the solid.
pub fn recognize_layout(
    solid: &Solid,
    fixed: Option<FaceId>,
    hint: &SheetSettings,
    owner: u32,
) -> Result<(Layout, FaceId), SheetError> {
    Recognizer::new(solid, fixed)?.layout(hint, owner)
}

fn msg(m: impl Into<String>) -> SheetError {
    SheetError::Message(m.into())
}

const NOT_SHEET: &str = "A face of the body is not a side of the sheet, a bend or a cut edge through the thickness. A body can be converted when it is a sheet of one thickness: flat walls joined by rounded bends, with edges cut square through the sheet. If its corners are sharp, round them first (the inside with the bend radius, the outside with the bend radius plus the thickness).";

const ACROSS_BEND: &str = "An edge crosses a bend at a slant or as a curve (a round hole or a slanted cut through a bend). Only straight edges along or square to the bend line can cross a bend: remove the hole or cut from the bend before converting.";

const CLOSED_LOOP: &str = "The walls form a closed loop (a box section or a tube), which can't be unfolded from one fixed face. Cut a seam through one wall or along one bend first, so the sheet has two ends.";

const BEND_MISMATCH: &str = "A bend doesn't join its two walls the way a cylindrical bend does (the walls aren't tangent to it along straight lines). Only bends of one radius between two flat walls can be converted.";

/// What a face's surface is, as far as recognition cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Plane,
    Cylinder,
    Cone,
    /// A sphere, a torus or a freeform surface, named for messages.
    Other(&'static str),
}

fn kind_of(surface: &Surface) -> Kind {
    match surface {
        Surface::Plane(_) => Kind::Plane,
        Surface::Cylinder(_) => Kind::Cylinder,
        Surface::Cone(_) => Kind::Cone,
        Surface::Sphere(_) => Kind::Other("a spherical face"),
        Surface::Torus(_) => Kind::Other("a doubly curved face (part of a torus)"),
        Surface::Nurbs(_) => Kind::Other("a freeform face"),
    }
}

/// A tag for curves that are only used for inside tests.
const NO_TAG: CurveTag = CurveTag::Generated {
    owner: 0,
    part: 0,
    index: 0,
};

/// The loops of a flat face as flat curves, in loop order: `(curve, reversed)`.
type FlatLoops = Vec<Vec<(Curve, bool)>>;

fn area_of(loops: &FlatLoops) -> Area {
    Area {
        loops: loops
            .iter()
            .map(|l| {
                l.iter()
                    .map(|&(curve, reversed)| Edge2 {
                        curve,
                        reversed,
                        tag: NO_TAG,
                    })
                    .collect()
            })
            .collect(),
    }
}

/// A point well inside an area: next to the middle of one of its edges, as far from the
/// boundary as the area allows.
fn interior_point(area: &Area) -> Option<DVec2> {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for e in area.edges() {
        let (a, b) = e.curve.bounds();
        lo = lo.min(a);
        hi = hi.max(b);
    }
    let size = lo.distance(hi);
    if !(size.is_finite() && size > SAME_POINT) {
        return None;
    }
    for fraction in [0.25, 0.1, 0.03, 0.01, 1e-3, 1e-4] {
        let step = size * fraction;
        for e in area.edges() {
            let mid = e.curve.point_at(0.5);
            let normal = e.curve.tangent_at(0.5).perp();
            for side in [1.0, -1.0] {
                let p = mid + normal * (side * step);
                let clear = area.edges().all(|o| o.curve.distance(p) >= 0.5 * step);
                if clear && area.contains(p) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// A bend seen from the wall it leaves: the cylinder of its top-skin face, and the
/// directions that unroll it.
#[derive(Clone, Debug)]
struct BendGeom {
    /// A point of the axis and its unit direction.
    axis_point: DVec3,
    axis: DVec3,
    /// From the axis to the parent's tangent line.
    e1: DVec3,
    /// Along the cylinder at the tangent line, away from the parent's material.
    e2: DVec3,
    /// Radius of the top skin's cylinder.
    radius: f64,
    /// The axis is on the top side: the bend folds towards it.
    up: bool,
    /// How far the faces run round the axis from the tangent line.
    angle: f64,
    faces: Vec<FaceId>,
}

impl BendGeom {
    /// Angle round the axis from the parent's tangent line (in `0..2π`) and height along
    /// the axis of a point of the cylinder.
    fn coords(&self, p: DVec3) -> (f64, f64) {
        let v = p - self.axis_point;
        let w = v.dot(self.axis);
        let radial = v - self.axis * w;
        let mut theta = radial.dot(self.e2).atan2(radial.dot(self.e1));
        if theta < 0.0 {
            theta += TAU;
        }
        if theta > TAU - SAME_ANGLE.max(SAME_POINT / self.radius) {
            theta = 0.0;
        }
        (theta, w)
    }

    /// Whether `other` (seen from the same wall) is the same bend: the same cylinder,
    /// leaving along the same line, through the same angle.
    fn same_bend(&self, other: &BendGeom) -> bool {
        let off = other.axis_point - self.axis_point;
        self.axis.cross(other.axis).length() <= SMOOTH
            && (off - self.axis * off.dot(self.axis)).length() <= SAME_POINT
            && (self.radius - other.radius).abs() <= SAME_POINT
            && self.e1.dot(other.e1) > 0.0
            && self.e1.cross(other.e1).length() <= SMOOTH
            && self.e2.dot(other.e2) > 0.0
            && (self.angle - other.angle).abs() * self.radius <= SAME_POINT
    }
}

/// Numbers the walls of the pieces, for naming their faces.
struct Tagger {
    owner: u32,
    /// Outline edges numbered so far in the piece being made.
    in_piece: u32,
    /// Hole edges numbered so far in the whole part.
    holes: u32,
}

impl Tagger {
    fn start_piece(&mut self) {
        self.in_piece = 0;
    }

    /// The next outline edge of `piece`.
    fn wall(&mut self, piece: usize) -> CurveTag {
        let n = self.in_piece;
        self.in_piece += 1;
        CurveTag::Generated {
            owner: self.owner,
            part: (piece as u32)
                .wrapping_mul(PARTS_PER_PIECE)
                .wrapping_add(n >> 8),
            index: (n & 0xff) as u8,
        }
    }

    /// The next edge of a hole or cutout: named like a cut's sketch curve, so the
    /// manufacturing checks treat it as one.
    fn hole(&mut self) -> CurveTag {
        let n = self.holes;
        self.holes += 1;
        CurveTag::Sketch {
            owner: self.owner,
            entity: n,
        }
    }
}

struct Recognizer<'a> {
    solid: &'a Solid,
    kinds: Vec<Kind>,
    /// Per edge: the faces on its two sides are smooth across it.
    smooth: Vec<bool>,
    /// Faces of the top and the bottom skin.
    top: Vec<bool>,
    bottom: Vec<bool>,
    seed: FaceId,
    thickness: f64,
}

impl<'a> Recognizer<'a> {
    /// Finds the two skins and the thickness.
    fn new(solid: &'a Solid, fixed: Option<FaceId>) -> Result<Self, SheetError> {
        if solid.faces.is_empty() {
            return Err(msg("There is no body to convert."));
        }
        if solid.shells.len() != 1 {
            return Err(msg(format!(
                "The body is in {} separate pieces (or has a closed hollow inside). A sheet metal part is one piece of sheet: convert a body that is in one piece.",
                solid.shells.len()
            )));
        }
        if solid.edges.iter().any(|e| e.coedges.len() != 2) {
            return Err(msg(
                "The body is not a closed solid, so its thickness can't be measured. Repair it (or import it again as a solid) before converting.",
            ));
        }
        let kinds: Vec<Kind> = solid.faces.iter().map(|f| kind_of(&f.surface)).collect();
        let smooth = solid
            .edges
            .iter()
            .map(|e| {
                let [a, b] = e.coedges[..] else { return false };
                let (fa, fb) = (solid.coedge_face(a), solid.coedge_face(b));
                if fa == fb {
                    return false;
                }
                let mid = e.point_at_fraction(0.5);
                let (na, nb) = (solid.face_normal_at(fa, mid), solid.face_normal_at(fb, mid));
                na.dot(nb) > 0.0 && na.cross(nb).length() <= SMOOTH
            })
            .collect();

        let seed = match fixed {
            Some(f) => {
                match kinds.get(f.index()) {
                    None => return Err(msg("The fixed face is not on this body.")),
                    Some(Kind::Plane) => {}
                    Some(_) => {
                        return Err(msg(
                            "The fixed face must be flat: pick one of the part's flat walls (the one that stays in place when the part is unfolded).",
                        ));
                    }
                }
                f
            }
            None => solid
                .face_ids()
                .filter(|f| kinds[f.index()] == Kind::Plane)
                .map(|f| (f, measure::face_area(solid, f)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(f, _)| f)
                .ok_or_else(|| {
                    msg("The body has no flat face. A sheet metal part needs at least one flat wall to unfold from.")
                })?,
        };

        let mut this = Self {
            solid,
            kinds,
            smooth,
            top: Vec::new(),
            bottom: Vec::new(),
            seed,
            thickness: 0.0,
        };
        this.top = this.flood(seed);
        this.check_skin(&this.top)?;

        // The other side: the nearest flat face behind the fixed face.
        let frame = this.face_frame(seed)?;
        let outline = this.face_loops(seed, &frame)?;
        let inside = interior_point(&area_of(&outline))
            .ok_or_else(|| msg("The fixed face is too small to work with."))?;
        let Some((other, thickness)) = this.face_behind(&frame, 0.0, inside, None) else {
            return Err(msg(
                "There is no flat face behind the fixed face, so the body has no thickness to measure there. Pick a flat wall of a sheet of constant thickness.",
            ));
        };
        if this.top[other.index()] {
            return Err(msg(
                "The two sides of the sheet run into each other smoothly (a fully rounded edge), so they can't be told apart. Convert a part whose edges are cut square through the sheet.",
            ));
        }
        this.thickness = thickness;
        this.bottom = this.flood(other);
        if this.bottom.iter().zip(&this.top).any(|(b, t)| *b && *t) {
            return Err(msg(
                "The two sides of the sheet run into each other smoothly (a fully rounded edge), so they can't be told apart. Convert a part whose edges are cut square through the sheet.",
            ));
        }
        this.check_skin(&this.bottom)?;
        Ok(this)
    }

    fn point(&self, c: CoedgeId, end: bool) -> DVec3 {
        let v = if end {
            self.solid.coedge_end(c)
        } else {
            self.solid.coedge_start(c)
        };
        self.solid.vertex(v).point
    }

    /// The coedges of every loop of a face.
    fn coedges(&self, face: FaceId) -> impl Iterator<Item = CoedgeId> + '_ {
        self.solid
            .face(face)
            .loops
            .iter()
            .flat_map(|&l| self.solid.loop_coedges(l))
    }

    /// The face on the other side of a coedge's edge, and whether the two are smooth
    /// across it.
    fn across(&self, c: CoedgeId) -> Option<(FaceId, bool)> {
        let twin = self.solid.twin(c)?;
        let edge = self.solid.coedge(c).edge;
        Some((self.solid.coedge_face(twin), self.smooth[edge.index()]))
    }

    /// Every face reached from `seed` across smooth edges.
    fn flood(&self, seed: FaceId) -> Vec<bool> {
        let mut inside = vec![false; self.solid.faces.len()];
        inside[seed.index()] = true;
        let mut queue = VecDeque::from([seed]);
        while let Some(f) = queue.pop_front() {
            for c in self.coedges(f) {
                if let Some((g, true)) = self.across(c)
                    && !inside[g.index()]
                {
                    inside[g.index()] = true;
                    queue.push_back(g);
                }
            }
        }
        inside
    }

    /// Refuses a skin with faces that are neither walls nor cylindrical bends.
    fn check_skin(&self, skin: &[bool]) -> Result<(), SheetError> {
        for (f, _) in skin.iter().enumerate().filter(|(_, s)| **s) {
            match self.kinds[f] {
                Kind::Plane | Kind::Cylinder => {}
                Kind::Cone => {
                    return Err(msg(
                        "One of the bends is conical (its radius changes along the bend line). Only cylindrical bends, with one radius all along a straight bend line, can be unfolded.",
                    ));
                }
                Kind::Other(what) => {
                    return Err(msg(format!(
                        "The sheet's surface has {what}. Only flat walls joined by cylindrical bends can be converted: formed or freeform areas have no exact flat pattern."
                    )));
                }
            }
        }
        Ok(())
    }

    /// Outward unit normal of a flat face.
    fn normal(&self, face: FaceId) -> DVec3 {
        let f = self.solid.face(face);
        let n = f.surface.normal_at(DVec3::ZERO);
        if f.reversed { -n } else { n }
    }

    /// A frame on a flat face: `z` along its outward normal, the origin on the face's
    /// plane.
    fn face_frame(&self, face: FaceId) -> Result<Frame, SheetError> {
        let Surface::Plane(plane) = &self.solid.face(face).surface else {
            return Err(msg(NOT_SHEET));
        };
        Frame::from_origin_z_x(plane.origin(), self.normal(face), plane.frame.x_axis())
            .ok_or_else(|| msg("A flat face of the body has no direction."))
    }

    /// One edge of a flat face in the coordinates of `frame` (whose `z` is square to the
    /// face), in the direction its loop runs.
    fn flat_edge(&self, c: CoedgeId, frame: &Frame) -> Result<(Curve, bool), SheetError> {
        let coedge = self.solid.coedge(c);
        let edge = self.solid.edge(coedge.edge);
        let local = |p: DVec3| frame.to_local(p).truncate();
        match &edge.curve {
            Curve3::Line(_) => Ok((
                Curve::Line {
                    a: local(self.point(c, false)),
                    b: local(self.point(c, true)),
                },
                false,
            )),
            Curve3::Circle(circle) => {
                let turn = frame.vector_to_local(circle.frame.z_axis()).z;
                if turn.abs() < 1.0 - SMOOTH {
                    return Err(msg(NOT_SHEET));
                }
                // The edge runs counter-clockwise about the circle's own axis; seen in
                // the frame that is counter-clockwise only if the axes agree.
                let ccw = turn > 0.0;
                let center = local(circle.frame.origin);
                let sweep = edge.t1 - edge.t0;
                if edge.is_closed() || sweep >= TAU - SAME_ANGLE {
                    return Ok((
                        Curve::Circle {
                            center,
                            radius: circle.radius,
                        },
                        coedge.reversed == ccw,
                    ));
                }
                let start = local(edge.curve.point(if ccw { edge.t0 } else { edge.t1 }));
                Ok((
                    Curve::Arc {
                        center,
                        radius: circle.radius,
                        start_angle: (start - center).to_angle(),
                        sweep,
                    },
                    coedge.reversed == ccw,
                ))
            }
            Curve3::Ellipse(_) | Curve3::Nurbs(_) => Err(msg(
                "A wall's outline has an elliptical or freeform edge. Outlines and cutouts can have straight lines and circular arcs: replace the curve, or remove the cut before converting.",
            )),
        }
    }

    /// The loops of a flat face in the coordinates of `frame`.
    fn face_loops(&self, face: FaceId, frame: &Frame) -> Result<FlatLoops, SheetError> {
        self.solid
            .face(face)
            .loops
            .iter()
            .map(|&l| {
                self.solid
                    .loop_coedges(l)
                    .into_iter()
                    .map(|c| self.flat_edge(c, frame))
                    .collect()
            })
            .collect()
    }

    /// The nearest flat face facing the other way behind the point `inside` (flat
    /// coordinates of `frame`) of a face at height `z` of `frame`, and how far behind.
    /// `skip` leaves a skin's faces out.
    fn face_behind(
        &self,
        frame: &Frame,
        z: f64,
        inside: DVec2,
        skip: Option<&[bool]>,
    ) -> Option<(FaceId, f64)> {
        let normal = frame.z_axis();
        let mut best: Option<(FaceId, f64)> = None;
        for g in self.solid.face_ids() {
            if self.kinds[g.index()] != Kind::Plane || skip.is_some_and(|s| s[g.index()]) {
                continue;
            }
            let n = self.normal(g);
            if n.dot(normal) > 0.0 || n.cross(normal).length() > SMOOTH {
                continue;
            }
            let Surface::Plane(plane) = &self.solid.face(g).surface else {
                continue;
            };
            let behind = z - frame.to_local(plane.origin()).z;
            if behind <= SAME_POINT || best.is_some_and(|(_, d)| d <= behind) {
                continue;
            }
            let Ok(loops) = self.face_loops(g, frame) else {
                continue;
            };
            if area_of(&loops).contains(inside) {
                best = Some((g, behind));
            }
        }
        best
    }

    /// `faces` and every flat face of the top skin that runs on from them without a bend
    /// (faces in one plane, split by an edge).
    fn with_coplanar(&self, faces: Vec<FaceId>) -> Vec<FaceId> {
        let mut out = faces;
        let mut next = 0;
        while next < out.len() {
            let f = out[next];
            next += 1;
            for c in self.coedges(f) {
                if let Some((g, true)) = self.across(c)
                    && self.top[g.index()]
                    && self.kinds[g.index()] == Kind::Plane
                    && !out.contains(&g)
                {
                    out.push(g);
                }
            }
        }
        out
    }

    /// The placement of the fixed wall: `z` along the fixed face's normal with the
    /// bottom side at `z = 0`, `x` along its longest straight edge (so the flat pattern
    /// lies square to it), the origin under that edge's start.
    fn base_frame(&self) -> Result<Frame, SheetError> {
        let normal = self.normal(self.seed);
        let mut best: Option<(f64, DVec3, DVec3)> = None;
        if let Some(&outer) = self.solid.face(self.seed).loops.first() {
            for c in self.solid.loop_coedges(outer) {
                let edge = self.solid.edge(self.solid.coedge(c).edge);
                if !matches!(edge.curve, Curve3::Line(_)) {
                    continue;
                }
                let (a, b) = (self.point(c, false), self.point(c, true));
                let len = a.distance(b);
                if best.is_none_or(|(l, _, _)| len > l + SAME_POINT) {
                    best = Some((len, a, b - a));
                }
            }
        }
        let frame = match best {
            Some((_, at, dir)) => Frame::from_origin_z_x(at - normal * self.thickness, normal, dir),
            None => self.face_frame(self.seed).ok().map(|f| Frame {
                origin: f.origin - normal * self.thickness,
                ..f
            }),
        };
        frame.ok_or_else(|| msg("The fixed face has no direction to lay the flat pattern along."))
    }

    /// The outline of a wall: its top faces' loops in its placement's flat coordinates.
    fn wall_outline(
        &self,
        faces: &[FaceId],
        frame: &Frame,
        piece: usize,
        tags: &mut Tagger,
    ) -> Result<Area, SheetError> {
        tags.start_piece();
        let mut loops = Vec::new();
        for &f in faces {
            for (k, &l) in self.solid.face(f).loops.iter().enumerate() {
                let mut edges = Vec::new();
                for c in self.solid.loop_coedges(l) {
                    let height = frame.to_local(self.point(c, false)).z;
                    if (height - self.thickness).abs() > SAME_POINT {
                        return Err(msg(BEND_MISMATCH));
                    }
                    let (curve, reversed) = self.flat_edge(c, frame)?;
                    let tag = if k == 0 {
                        tags.wall(piece)
                    } else {
                        tags.hole()
                    };
                    edges.push(Edge2 {
                        curve,
                        reversed,
                        tag,
                    });
                }
                loops.push(edges);
            }
        }
        Ok(Area { loops })
    }

    /// The bend a cylinder face of the top skin makes, seen from the wall with placement
    /// `frame` that it leaves along coedge `c` (a coedge of the wall's face).
    fn bend_geom(&self, face: FaceId, c: CoedgeId, frame: &Frame) -> Result<BendGeom, SheetError> {
        let Surface::Cylinder(Cylinder { frame: cyl, radius }) = &self.solid.face(face).surface
        else {
            return Err(msg(NOT_SHEET));
        };
        let (a, b) = (self.point(c, false), self.point(c, true));
        let normal = frame.z_axis();
        let axis = cyl.z_axis();
        let dir = (b - a).try_normalize().ok_or_else(|| msg(BEND_MISMATCH))?;
        if dir.cross(axis).length() > SMOOTH {
            return Err(msg(BEND_MISMATCH));
        }
        let v = a - cyl.origin;
        let e1 = (v - axis * v.dot(axis))
            .try_normalize()
            .ok_or_else(|| msg(BEND_MISMATCH))?;
        if e1.cross(normal).length() > SMOOTH {
            return Err(msg(BEND_MISMATCH));
        }
        // Seen from outside, a face lies to the left of its coedges: the wall's material
        // is towards `normal × dir`, and the bend leaves the other way.
        let e2 = dir.cross(normal);
        let mut geom = BendGeom {
            axis_point: cyl.origin,
            axis,
            e1,
            e2,
            radius: *radius,
            up: e1.dot(normal) < 0.0,
            angle: 0.0,
            faces: vec![face],
        };
        geom.angle = self
            .coedges(face)
            .map(|c| geom.coords(self.point(c, false)).0)
            .fold(0.0, f64::max);
        Ok(geom)
    }

    /// Walks the walls from the fixed one and lays them out.
    fn layout(&self, hint: &SheetSettings, owner: u32) -> Result<(Layout, FaceId), SheetError> {
        let t = self.thickness;
        let mut tags = Tagger {
            owner,
            in_piece: 0,
            holes: 0,
        };
        let mut pieces: Vec<Piece> = Vec::new();
        // The layout piece each face of the top skin went into.
        let mut piece_of: HashMap<FaceId, usize> = HashMap::new();
        // Each piece's faces, and for bends how they unroll.
        let mut faces_of: Vec<Vec<FaceId>> = Vec::new();
        let mut geoms: Vec<Option<BendGeom>> = Vec::new();

        let base = self.with_coplanar(vec![self.seed]);
        let frame = self.base_frame()?;
        pieces.push(Piece {
            origin: Origin { owner, part: 0 },
            kind: PieceKind::Flange,
            outline: self.wall_outline(&base, &frame, 0, &mut tags)?,
            trims: Vec::new(),
            frame,
        });
        for &f in &base {
            piece_of.insert(f, 0);
        }
        faces_of.push(base);
        geoms.push(None);

        let mut queue = VecDeque::from([0usize]);
        while let Some(parent) = queue.pop_front() {
            let frame = pieces[parent].frame;
            // The bends leaving this wall, faces of one bend together.
            let mut bends: Vec<BendGeom> = Vec::new();
            for &f in &faces_of[parent] {
                for c in self.coedges(f) {
                    let Some((g, true)) = self.across(c) else {
                        continue;
                    };
                    if !self.top[g.index()]
                        || self.kinds[g.index()] != Kind::Cylinder
                        || piece_of.contains_key(&g)
                        || bends.iter().any(|b| b.faces.contains(&g))
                    {
                        continue;
                    }
                    let geom = self.bend_geom(g, c, &frame)?;
                    match bends.iter_mut().find(|b| b.same_bend(&geom)) {
                        Some(b) => b.faces.push(g),
                        None => bends.push(geom),
                    }
                }
            }
            for geom in bends {
                let index = pieces.len();
                let radius = if geom.up {
                    geom.radius
                } else {
                    geom.radius - t
                };
                if radius <= SAME_POINT {
                    return Err(msg(
                        "A bend has a sharp inside corner (no inner radius). Round the inside of the bend with a small radius before converting.",
                    ));
                }
                let values = if geom.angle >= PI - 1e-6 {
                    BendValues::hem(hint.model, geom.angle, radius, t)
                } else {
                    BendValues::new(hint.model, geom.angle, radius, t)
                }
                .map_err(|m| msg(format!("A bend of the part can't be laid flat. {m}")))?;

                // The strip in the parent's flat coordinates.
                let line = frame
                    .to_local(geom.axis_point + geom.e1 * geom.radius)
                    .truncate();
                let direction = |v: DVec3| frame.vector_to_local(v).truncate().try_normalize();
                let (Some(along), Some(across)) = (direction(geom.axis), direction(geom.e2)) else {
                    return Err(msg(BEND_MISMATCH));
                };
                let across_strip = |theta: f64| {
                    if theta <= SAME_ANGLE {
                        0.0
                    } else if (theta - geom.angle).abs() <= SAME_ANGLE {
                        values.allowance
                    } else {
                        theta / geom.angle * values.allowance
                    }
                };
                let flat = |p: DVec3| {
                    let (theta, w) = geom.coords(p);
                    (across_strip(theta), w)
                };
                let place = |(s, w): (f64, f64)| line + along * w + across * s;

                // The outline: the faces unrolled, and the walls at the far side.
                tags.start_piece();
                let mut loops = Vec::new();
                let mut children: Vec<FaceId> = Vec::new();
                let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
                for &f in &geom.faces {
                    for &l in &self.solid.face(f).loops {
                        let mut edges = Vec::new();
                        for c in self.solid.loop_coedges(l) {
                            let edge = self.solid.edge(self.solid.coedge(c).edge);
                            let (p, q) = (flat(self.point(c, false)), flat(self.point(c, true)));
                            let folds = match &edge.curve {
                                Curve3::Line(_) => (p.0 - q.0).abs() <= SAME_POINT,
                                Curve3::Circle(circle) => {
                                    !edge.is_closed()
                                        && (p.1 - q.1).abs() <= SAME_POINT
                                        && circle.frame.z_axis().cross(geom.axis).length() <= SMOOTH
                                }
                                Curve3::Ellipse(_) | Curve3::Nurbs(_) => false,
                            };
                            if !folds {
                                return Err(msg(ACROSS_BEND));
                            }
                            low = low.min(p.1).min(q.1);
                            high = high.max(p.1).max(q.1);
                            edges.push(Edge2::line(place(p), place(q), tags.wall(index)));

                            let Some((g, true)) = self.across(c) else {
                                continue;
                            };
                            if !self.top[g.index()] || faces_of[parent].contains(&g) {
                                continue;
                            }
                            match self.kinds[g.index()] {
                                Kind::Plane if !children.contains(&g) => children.push(g),
                                Kind::Plane => {}
                                _ if geom.faces.contains(&g) => {}
                                _ => {
                                    return Err(msg(
                                        "Two bends run straight into each other with no flat wall between them. A bend must join two flat walls: split it into one bend, or leave a flat between the two.",
                                    ));
                                }
                            }
                        }
                        loops.push(edges);
                    }
                }
                let children = self.with_coplanar(children);
                if children.iter().any(|f| piece_of.contains_key(f)) {
                    return Err(msg(CLOSED_LOOP));
                }
                let bend = Bend {
                    parent,
                    child: (!children.is_empty()).then_some(index + 1),
                    origin: line + along * low,
                    along,
                    across,
                    length: high - low,
                    values,
                    up: geom.up,
                };
                pieces.push(Piece {
                    origin: Origin {
                        owner,
                        part: index as u32,
                    },
                    kind: PieceKind::Bend(bend),
                    outline: Area { loops },
                    trims: Vec::new(),
                    frame,
                });
                for &f in &geom.faces {
                    piece_of.insert(f, index);
                }
                faces_of.push(geom.faces.clone());
                geoms.push(Some(geom));
                if children.is_empty() {
                    continue;
                }
                let child_frame = frame.compose(&bend.fold_frame());
                let child = index + 1;
                pieces.push(Piece {
                    origin: Origin {
                        owner,
                        part: child as u32,
                    },
                    kind: PieceKind::Flange,
                    outline: self.wall_outline(&children, &child_frame, child, &mut tags)?,
                    trims: Vec::new(),
                    frame: child_frame,
                });
                for &f in &children {
                    piece_of.insert(f, child);
                }
                faces_of.push(children);
                geoms.push(None);
                queue.push_back(child);
            }
        }

        // Every face of the top skin must have found its place.
        for f in self.solid.face_ids() {
            if self.top[f.index()] && !piece_of.contains_key(&f) {
                return Err(msg(match self.kinds[f.index()] {
                    Kind::Cylinder => BEND_MISMATCH,
                    _ => NOT_SHEET,
                }));
            }
        }
        self.check_other_side(&pieces, &faces_of, &geoms)?;
        self.check_edges()?;

        // The body's default radius: the one most bends have.
        let mut radii: Vec<(f64, usize)> = Vec::new();
        for p in &pieces {
            if let Some(b) = p.bend() {
                match radii
                    .iter_mut()
                    .find(|(r, _)| (r - b.values.radius).abs() <= SAME_POINT)
                {
                    Some((_, n)) => *n += 1,
                    None => radii.push((b.values.radius, 1)),
                }
            }
        }
        let radius = radii
            .iter()
            .rev()
            .max_by_key(|(_, n)| *n)
            .map_or(hint.radius, |(r, _)| *r);
        let settings = SheetSettings {
            thickness: t,
            radius,
            ..*hint
        };
        settings.check().map_err(msg)?;
        Ok((
            Layout {
                settings,
                pieces,
                cuts: Vec::new(),
                attachments: Vec::new(),
                corners: Vec::new(),
                forms: Vec::new(),
            },
            self.seed,
        ))
    }

    /// Checks that the bottom skin mirrors the top one a thickness away: a flat face
    /// behind every wall, a coaxial cylinder behind every bend.
    fn check_other_side(
        &self,
        pieces: &[Piece],
        faces_of: &[Vec<FaceId>],
        geoms: &[Option<BendGeom>],
    ) -> Result<(), SheetError> {
        let t = self.thickness;
        let uneven = |other: f64| {
            msg(format!(
                "The walls are not all the same thickness: {t:.3} mm at the fixed face, {other:.3} mm elsewhere. A sheet metal part is made from one sheet: make the walls one thickness before converting."
            ))
        };
        // Walls first: a wall of another thickness says more than the bend next to it.
        for ((piece, faces), geom) in pieces.iter().zip(faces_of).zip(geoms) {
            if geom.is_some() {
                continue;
            }
            for &f in faces {
                let loops = self.face_loops(f, &piece.frame)?;
                let inside = interior_point(&area_of(&loops))
                    .ok_or_else(|| msg("A wall of the part is too small to work with."))?;
                match self.face_behind(&piece.frame, t, inside, Some(&self.top)) {
                    Some((g, d)) if (d - t).abs() <= SAME_POINT => {
                        if !self.bottom[g.index()] {
                            return Err(msg(NOT_SHEET));
                        }
                    }
                    Some((_, d)) => return Err(uneven(d)),
                    None => return Err(msg(NOT_SHEET)),
                }
            }
        }
        for geom in geoms.iter().flatten() {
            let wanted = if geom.up {
                geom.radius + t
            } else {
                geom.radius - t
            };
            // Coaxial cylinders that are not the top skin's: their distance
            // from the bend's own radius is the thickness there.
            let mut found = false;
            let mut other = None;
            for g in self.solid.face_ids() {
                if self.top[g.index()] {
                    continue;
                }
                let Surface::Cylinder(c) = &self.solid.face(g).surface else {
                    continue;
                };
                let off = c.frame.origin - geom.axis_point;
                if c.axis().cross(geom.axis).length() > SMOOTH
                    || (off - geom.axis * off.dot(geom.axis)).length() > SAME_POINT
                {
                    continue;
                }
                if (c.radius - wanted).abs() <= SAME_POINT && self.bottom[g.index()] {
                    found = true;
                } else if self.bottom[g.index()] {
                    other = Some((c.radius - geom.radius).abs());
                }
            }
            if !found {
                return Err(match other {
                    Some(d) => uneven(d),
                    None => msg(
                        "A bend is rounded on one side only. Round both sides of every bend about the same axis: the inside with the bend radius, the outside with the bend radius plus the thickness.",
                    ),
                });
            }
        }
        Ok(())
    }

    /// Checks that every face outside the two skins is a cut edge: a plane or a cylinder
    /// that touches both skins.
    fn check_edges(&self) -> Result<(), SheetError> {
        for f in self.solid.face_ids() {
            if self.top[f.index()] || self.bottom[f.index()] {
                continue;
            }
            match self.kinds[f.index()] {
                Kind::Plane | Kind::Cylinder => {}
                Kind::Cone => {
                    return Err(msg(
                        "An edge of the sheet is cut at a slant (a conical face, such as a countersink or a chamfer). Edges and holes must be cut square through the sheet: remove the countersink or chamfer before converting.",
                    ));
                }
                Kind::Other(what) => {
                    return Err(msg(format!(
                        "An edge of the sheet has {what}. Edges and holes must be cut square through the sheet: remove the rounding or forming before converting."
                    )));
                }
            }
            let (mut on_top, mut on_bottom) = (false, false);
            for c in self.coedges(f) {
                if let Some((g, _)) = self.across(c) {
                    on_top |= self.top[g.index()];
                    on_bottom |= self.bottom[g.index()];
                }
            }
            if !(on_top && on_bottom) {
                return Err(msg(NOT_SHEET));
            }
        }
        Ok(())
    }
}

/// The exact bounds of a solid whose edges are lines and circular arcs (other curves are
/// sampled).
fn bounds(solid: &Solid) -> Aabb {
    let mut b = Aabb::EMPTY;
    for v in &solid.vertices {
        b.extend(v.point);
    }
    for e in &solid.edges {
        match &e.curve {
            Curve3::Line(_) => {}
            Curve3::Circle(c) => {
                // Along each model axis the circle is furthest where its tangent is
                // square to the axis.
                let (x, y) = (c.frame.x_axis(), c.frame.y_axis());
                for k in [DVec3::X, DVec3::Y, DVec3::Z] {
                    let at = y.dot(k).atan2(x.dot(k));
                    for turn in [at, at + PI] {
                        let above = (turn - e.t0).rem_euclid(TAU);
                        if above <= e.t1 - e.t0 {
                            b.extend(e.curve.point(e.t0 + above));
                        }
                    }
                }
            }
            Curve3::Ellipse(_) | Curve3::Nurbs(_) => {
                for i in 0..=64 {
                    b.extend(e.point_at_fraction(f64::from(i) / 64.0));
                }
            }
        }
    }
    b
}

/// Checks that the solid built from the layout is the solid that was recognised.
fn verify(input: &Solid, built: &Solid) -> Result<(), SheetError> {
    let different = || {
        msg(
            "The body couldn't be converted: the sheet metal part worked out from it is not the same shape. It has something sheet metal can't make exactly (edges not cut square through the sheet, or a wall that isn't flat). Simplify that area and convert again.",
        )
    };
    let (before, after) = (measure::volume(input), measure::volume(built));
    if (before - after).abs() > VOLUME_TOLERANCE * before.abs() {
        return Err(different());
    }
    let (a, b) = (bounds(input), bounds(built));
    if !a.min.abs_diff_eq(b.min, BOUNDS_TOLERANCE) || !a.max.abs_diff_eq(b.max, BOUNDS_TOLERANCE) {
        return Err(different());
    }
    Ok(())
}
