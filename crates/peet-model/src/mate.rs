//! Mates: what holds the components of an assembly together.
//!
//! A [`Mate`] relates two components through geometry of their parts: a flat face (a
//! plane), a reference plane, a round face or a round edge (its axis), a straight edge (a line) or a corner
//! (a point). Its ends are persistent references ([`crate::naming`]) with the component
//! they are on, so a mate follows its faces through edits to the part.
//!
//! **Solving.** Every component that is not fixed has six unknowns: where its origin is,
//! and a rotation vector `w` that turns it from the orientation it had when the solve
//! started (`R = exp(w) R₀`). A mate becomes one to six residual equations in the world
//! positions `P = t + R p` and directions `D = R d` of its two ends:
//!
//! | equation | residual |
//! |---|---|
//! | `Dot` | `Dₐ · D_b − c`: two directions at an angle (square to each other for `c = 0`) |
//! | `Offset` | `Dₐ · (P_b − Pₐ) − c`: a point at a distance along a direction of the other |
//! | `Distance` | `|P_b − Pₐ| − c` |
//! | `LineDistance` | the distance of `P_b` from the line through `Pₐ` along `Dₐ`, `− c` |
//!
//! So "two planes coincide" is two `Dot`s (two directions in the first plane are square
//! to the second's normal) and an `Offset`; "two axes are in line" is two `Dot`s and two
//! `Offset`s. Lengths are in mm and the rotation unknowns are scaled by the component's
//! size, so every entry of the Jacobian is of order one.
//!
//! The equations are solved together by `peet-solve`, with minimum norm steps: the
//! components move as little as they can from where they are. Where they are is
//! therefore the suggestion: placing a mated component somewhere and rebuilding brings
//! it to the nearest place the mates allow.
//!
//! Components that mates join (directly or through others) are a *group*, solved on its
//! own; a fixed component joins nothing, since it doesn't move. A group whose mates
//! already hold is left exactly as it is, so adding a mate to one corner of an assembly
//! moves nothing in another, and an assembly that is solved stays bit for bit the same
//! however often it is rebuilt.
//!
//! **Dragging.** A [`Drag`] pulls a point of a component towards a place. Before the
//! mates are solved, the component's group is moved in steps, as the sketcher drags: a
//! step is the least squares answer to the mates' equations, linearised, together with
//! weak ones that ask the point to be at the place; then the mates are solved exactly
//! from there. The mates far outweigh the pull, so a step keeps them. The steps stop
//! when the point gets no nearer. This is done first without letting the dragged
//! component turn, then letting it: so a part that can slide to the place slides there,
//! and a hinged one swings round to follow. A component with no mates is simply moved.
//!
//! **Which way round.** Two planes are parallel with their normals opposed (faces
//! against each other, the default) or the same way (`flip`). Both satisfy the same
//! equations, and the solve goes to the nearer one, so a component that is the wrong way
//! round is turned over first.
//!
//! **Conflicts.** If the mates can't all hold, they are added one at a time in their
//! order; each that can't be satisfied with those before it is flagged and left out, so
//! the rest still hold.

use std::collections::HashMap;
use std::sync::Arc;

use peet_kernel::{Curve3, Surface};
use peet_math::{DQuat, DVec3, Frame};
use peet_sketch::expr::Parameters;
use peet_solve::{Equation, MAX_SLOTS, Options, Problem, SKIP, Space};
use serde::{Deserialize, Serialize};

use crate::PlaneRef;
use crate::assembly::CompId;
use crate::feature::{Scalar, ScalarKind};
use crate::naming::{Body, EdgeRef, FaceRef, VertexRef, find_edge, find_face, find_vertex};
use crate::regen::Status;

/// Identifies a mate within its assembly. Ids are not reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MateId(pub u32);

/// The geometry at one end of a mate, in its part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MateGeom {
    Face(FaceRef),
    Edge(EdgeRef),
    Vertex(VertexRef),
    Plane(PlaneRef),
    /// The part's origin (or a sub-assembly's, or the assembly's own): a point.
    Origin,
    /// A point, line, circle or arc of one of the part's sketches: a point, a line, or
    /// (a circle or an arc) a circle, as a round edge is.
    Sketch(SketchRef),
}

/// An entity of a sketch of a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SketchRef {
    pub feature: crate::FeatureId,
    pub entity: peet_sketch::EntityId,
}

/// One end of a mate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MateEnd {
    /// The component: the assembly's own first, then (for a part inside a sub-assembly)
    /// the components below it. Empty for the assembly itself: its standard planes and
    /// its origin, which are fixed.
    pub path: Vec<CompId>,
    /// The geometry, or `None` for the component as a whole (a fasten mate).
    pub geom: Option<MateGeom>,
}

impl MateEnd {
    /// The component of the assembly this end is on (none for the assembly itself).
    pub fn component(&self) -> Option<CompId> {
        self.path.first().copied()
    }

    /// An end on the assembly's own standard plane or origin.
    pub fn of_assembly(geom: MateGeom) -> Self {
        Self {
            path: Vec::new(),
            geom: Some(geom),
        }
    }

    /// Whether the end is on the assembly itself rather than on a component.
    pub fn is_assembly(&self) -> bool {
        self.path.is_empty()
    }
}

/// What a mate asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MateKind {
    /// The two ends lie in each other: faces against each other, a point on a plane,
    /// lines in line.
    Coincident,
    /// Two axes in line.
    Concentric,
    Parallel,
    /// The two ends a length apart.
    Distance(Scalar),
    /// The directions of the two ends at an angle (degrees).
    Angle(Scalar),
    /// The second component held where it is relative to the first: its placement in
    /// the first's coordinates.
    Fasten(Frame),
    /// Center the tab on the width pair's midplane. `Mate::a` and `a2` bound the
    /// width; `Mate::b` and `b2` bound the tab, or `b` alone is an axis.
    Width {
        a2: MateEnd,
        b2: Option<MateEnd>,
        mode: WidthMode,
    },
    /// A pin in a straight slot (SolidWorks' slot mate): one end is a face of the slot
    /// (one of its round ends or a flat side), the other an axis (a round face or edge).
    /// The axis stays parallel to the slot's ends and on the line between their axes:
    /// anywhere between them (`Free`), half way (`Centered`), a length from the end
    /// nearer the first slot face's own end (`Dimension`) or a percentage of the way.
    Slot {
        mode: WidthMode,
    },
    /// The directions of the two ends at an angle between `min` and `max` (degrees),
    /// measured as `Angle` is (SolidWorks' limit angle mate).
    LimitAngle {
        min: Scalar,
        max: Scalar,
    },
    /// The two ends a length between `min` and `max` apart, measured as `Distance` is
    /// (SolidWorks' limit distance mate).
    LimitDistance {
        min: Scalar,
        max: Scalar,
    },
    /// The two ends mirror images of each other in a plane (SolidWorks' symmetric mate).
    Symmetric {
        plane: MateEnd,
    },
    /// A point of the first end's component kept on a path of the second's (SolidWorks'
    /// path mate): `Mate::a` is a point (a vertex, a sketch point, an origin), `Mate::b`
    /// the path's first piece and `more` the rest, all on one component, joined end to
    /// end: straight or round edges, and sketch lines, arcs and circles. `along` says
    /// where on the path: anywhere (`Free`), a length from its start (`Dimension`, from
    /// its end with `flip_dimension`), a percentage of its length, or half way.
    /// `follow` keeps an axis of the point's component along the path (SolidWorks'
    /// pitch and yaw control, "follow path"); `roll` another of its axes along a
    /// direction (its "up vector").
    Path {
        more: Vec<MateEnd>,
        along: WidthMode,
        follow: Option<PathAxis>,
        roll: Option<PathRoll>,
    },
}

/// An axis of a component's own coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathAxis {
    X,
    Y,
    Z,
}

impl PathAxis {
    pub fn word(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "x" | "X" => Some(Self::X),
            "y" | "Y" => Some(Self::Y),
            "z" | "Z" => Some(Self::Z),
            _ => None,
        }
    }

    fn vector(self) -> DVec3 {
        match self {
            Self::X => DVec3::X,
            Self::Y => DVec3::Y,
            Self::Z => DVec3::Z,
        }
    }
}

/// A path mate's roll control: an axis of the point's component kept along a direction
/// (a flat face's or a plane's normal, a straight edge or an axis), either way round.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathRoll {
    pub axis: PathAxis,
    pub up: MateEnd,
}

/// Position a tab within the available clearance, measured from the first width end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WidthMode {
    Centered,
    Free,
    Dimension(Scalar),
    Percent(Scalar),
}

impl WidthMode {
    pub fn word(&self) -> &'static str {
        match self {
            Self::Centered => "centered",
            Self::Free => "free",
            Self::Dimension(_) => "dimension",
            Self::Percent(_) => "percent",
        }
    }
}

impl MateKind {
    /// The kind in one word, for names and messages.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Coincident => "coincident",
            Self::Concentric => "concentric",
            Self::Parallel => "parallel",
            Self::Distance(_) => "distance",
            Self::Angle(_) => "angle",
            Self::Fasten(_) => "fasten",
            Self::Width { .. } => "width",
            Self::Slot { .. } => "slot",
            Self::LimitAngle { .. } => "limit_angle",
            Self::LimitDistance { .. } => "limit_distance",
            Self::Symmetric { .. } => "symmetric",
            Self::Path { .. } => "path",
        }
    }

    /// The start of automatic names: `Coincident1`.
    pub(crate) fn name_prefix(&self) -> &'static str {
        match self {
            Self::Coincident => "Coincident",
            Self::Concentric => "Concentric",
            Self::Parallel => "Parallel",
            Self::Distance(_) => "Distance",
            Self::Angle(_) => "Angle",
            Self::Fasten(_) => "Fasten",
            Self::Width { .. } => "Width",
            Self::Slot { .. } => "Slot",
            Self::LimitAngle { .. } => "LimitAngle",
            Self::LimitDistance { .. } => "LimitDistance",
            Self::Symmetric { .. } => "Symmetric",
            Self::Path { .. } => "PathMate",
        }
    }
}

/// Which way round the directions of a mate's two ends go (SolidWorks' alignment): a
/// flat face's outward normal, an axis's direction (a round face's or a round edge's
/// axis, a straight edge's direction).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Align {
    /// The same way: two flat faces flush side by side, two axes pointing alike.
    Aligned,
    /// Against each other: two flat faces face to face, two axes pointing apart.
    AntiAligned,
    /// Whichever is nearer where the components are.
    Closest,
}

impl Align {
    pub fn word(self) -> &'static str {
        match self {
            Self::Aligned => "aligned",
            Self::AntiAligned => "anti_aligned",
            Self::Closest => "closest",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "aligned" => Self::Aligned,
            "anti_aligned" => Self::AntiAligned,
            "closest" => Self::Closest,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mate {
    pub id: MateId,
    /// Unique in the assembly: "Coincident1".
    pub name: String,
    pub kind: MateKind,
    pub a: MateEnd,
    pub b: MateEnd,
    /// Which way round the ends go. `None` is the default for what they are: two planes
    /// against each other, two axes whichever way is nearer.
    pub align: Option<Align>,
    /// The distance or angle measured the other way (SolidWorks' "flip dimension"): for a
    /// distance from a plane, the other end is behind it instead of in front of it.
    pub flip_dimension: bool,
    pub suppressed: bool,
}

/// A mate as model schemas 8 to 11 hold it: one `flip`, which put two planes the same
/// way round.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct MateV11 {
    id: MateId,
    name: String,
    kind: MateKind,
    a: MateEnd,
    b: MateEnd,
    flip: bool,
    suppressed: bool,
}

impl From<MateV11> for Mate {
    fn from(m: MateV11) -> Self {
        Self {
            id: m.id,
            name: m.name,
            kind: m.kind,
            a: m.a,
            b: m.b,
            align: m.flip.then_some(Align::Aligned),
            flip_dimension: false,
            suppressed: m.suppressed,
        }
    }
}

impl From<&Mate> for MateV11 {
    fn from(m: &Mate) -> Self {
        Self {
            id: m.id,
            name: m.name.clone(),
            kind: m.kind.clone(),
            a: m.a.clone(),
            b: m.b.clone(),
            flip: m.flip(),
            suppressed: m.suppressed,
        }
    }
}

impl Mate {
    /// Two planes (or axes) the same way round: what `flip` asks for.
    pub fn flip(&self) -> bool {
        self.align == Some(Align::Aligned)
    }

    /// Every persistent end, including the additional ends of a width mate and the plane
    /// of a symmetric one.
    pub fn ends(&self) -> impl Iterator<Item = &MateEnd> {
        let (a2, b2) = match &self.kind {
            MateKind::Width { a2, b2, .. } => (Some(a2), b2.as_ref()),
            MateKind::Symmetric { plane } => (Some(plane), None),
            _ => (None, None),
        };
        let path: Vec<&MateEnd> = match &self.kind {
            MateKind::Path { more, roll, .. } => {
                roll.iter().map(|r| &r.up).chain(more.iter()).collect()
            }
            _ => Vec::new(),
        };
        [&self.a, &self.b]
            .into_iter()
            .chain(a2)
            .chain(b2)
            .chain(path)
    }
}

// ---- Mates of a moved body ----

/// A mate that places the bodies a [`crate::MoveBodyFeature`] moves, as SolidWorks'
/// Move/Copy Body places them by constraints: between geometry of a moved body and
/// geometry of the rest of the part (another body, or a reference plane). The bodies are
/// solved into place at every rebuild, as an assembly's components are, starting from
/// the move's own distances and angles: so they follow the part when it changes, and
/// what the mates leave free stays as the move has it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyMate {
    /// Its name ("Distance2"), for messages.
    pub name: String,
    /// Coincident, concentric, parallel, distance or angle (perpendicular is an angle
    /// of 90°). As an assembly's mate of the same kind, with `to` its first end.
    pub kind: MateKind,
    /// On a body the feature moves, where the body is before the move.
    pub moved: MateGeom,
    /// On the rest of the part: another body, or a reference plane.
    pub to: MateGeom,
    /// Two flat faces with their outward normals the same way (SolidWorks' aligned);
    /// otherwise against each other (anti-aligned).
    pub aligned: bool,
    /// The distance or angle measured the other way: for a distance from a flat face,
    /// the moved end is behind it instead of in front of it.
    pub flip: bool,
}

impl BodyMate {
    pub fn new(name: impl Into<String>, kind: MateKind, moved: MateGeom, to: MateGeom) -> Self {
        Self {
            name: name.into(),
            kind,
            moved,
            to,
            aligned: false,
            flip: false,
        }
    }
}

/// Where the moved bodies go: the motion, nearest to `start`, that makes `mates` hold
/// between the bodies `moved` (as they are before the move) and the part's `rest` and
/// reference `planes`. With it, each mate that can't hold and why; the others still do.
pub(crate) fn solve_body(
    mates: &[BodyMate],
    moved: Vec<Arc<Body>>,
    rest: Vec<Arc<Body>>,
    planes: Vec<(PlaneRef, peet_math::Plane)>,
    start: Frame,
    params: &Parameters,
) -> (Frame, Vec<(String, String)>) {
    let (part, body) = (CompId(0), CompId(1));
    let placed = |id, name: &str, frame, fixed, bodies: Vec<Arc<Body>>, planes| Placed {
        id,
        name: name.to_owned(),
        frame,
        fixed,
        bodies: bodies
            .into_iter()
            .map(|b| (Vec::new(), b, Frame::WORLD))
            .collect(),
        planes,
        models: Vec::new(),
    };
    let components = vec![
        placed(
            part,
            "the part",
            Frame::WORLD,
            true,
            rest,
            planes
                .into_iter()
                .map(|(r, p)| (Vec::new(), r, p))
                .collect(),
        ),
        placed(body, "the moved body", start, false, moved, Vec::new()),
    ];
    let mut failed = Vec::new();
    let mut joined = Vec::new();
    for (i, m) in mates.iter().enumerate() {
        if matches!(
            m.kind,
            MateKind::Fasten(_)
                | MateKind::Width { .. }
                | MateKind::Slot { .. }
                | MateKind::Symmetric { .. }
        ) {
            failed.push((
                m.name.clone(),
                format!(
                    "A {} mate can't place a moved body: use coincident, concentric, parallel, distance or angle.",
                    m.kind.word()
                ),
            ));
            continue;
        }
        let id = MateId(i as u32);
        joined.push(Mate {
            id,
            name: m.name.clone(),
            kind: m.kind.clone(),
            a: MateEnd {
                path: vec![part],
                geom: Some(m.to.clone()),
            },
            b: MateEnd {
                path: vec![body],
                geom: Some(m.moved.clone()),
            },
            align: m.aligned.then_some(Align::Aligned),
            flip_dimension: m.flip,
            suppressed: false,
        });
    }
    let refs: Vec<&Mate> = joined.iter().collect();
    let solved = solve(&refs, components, params, 0, &mut Memo::default(), None);
    for m in &joined {
        if let Some(Status::Failed(why)) = solved.statuses.get(&m.id) {
            failed.push((m.name.clone(), why.replace("components", "bodies")));
        }
    }
    (solved.frames.get(&body).copied().unwrap_or(start), failed)
}

// ---- Geometry of an end ----

/// What an end is, in its component's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Element {
    /// A point of the plane and its outward normal.
    Plane(DVec3, DVec3),
    /// A point of the line and its direction.
    Line(DVec3, DVec3),
    Point(DVec3),
    /// A round edge (or a sketch's circle or arc): its centre and its axis. It is its
    /// axis to every mate but a coincident one with a plane, which puts the circle in
    /// the plane, as SolidWorks does.
    Circle(DVec3, DVec3),
}

impl Element {
    fn word(&self) -> &'static str {
        match self {
            Self::Plane(..) => "a plane or a flat face",
            Self::Line(..) => "an axis or a straight edge",
            Self::Point(_) => "a point",
            Self::Circle(..) => "a round edge",
        }
    }

    /// What the end is to a mate that takes a circle for its axis.
    fn axis(self) -> Self {
        match self {
            Self::Circle(p, d) => Self::Line(p, d),
            other => other,
        }
    }

    /// The element moved by `frame`.
    fn moved(self, frame: &Frame) -> Self {
        match self {
            Self::Plane(p, n) => Self::Plane(frame.to_world(p), frame.vector_to_world(n)),
            Self::Line(p, d) => Self::Line(frame.to_world(p), frame.vector_to_world(d)),
            Self::Point(p) => Self::Point(frame.to_world(p)),
            Self::Circle(p, d) => Self::Circle(frame.to_world(p), frame.vector_to_world(d)),
        }
    }
}


/// A body of a component: the components below it down to its part (none for a part
/// placed directly), the body, and where the part is in the component's coordinates.
pub(crate) type ComponentBody = (Vec<CompId>, Arc<Body>, Frame);

/// A reference plane below a component, resolved into that component's coordinates.
pub(crate) type ComponentPlane = (Vec<CompId>, PlaneRef, peet_math::Plane);

/// A component as the solve sees it.
pub(crate) struct Placed {
    pub id: CompId,
    pub name: String,
    pub frame: Frame,
    pub fixed: bool,
    pub bodies: Vec<ComponentBody>,
    pub planes: Vec<ComponentPlane>,
    /// The models below it, for their sketches: as `bodies` (an empty path for its own).
    pub models: Vec<ComponentModel>,
}

/// A model below a component (its part, or a part in a sub-assembly), and where it is
/// in the component's coordinates.
pub(crate) type ComponentModel = (Vec<CompId>, Arc<crate::Model>, Frame);

impl Placed {
    /// The assembly itself, as an end of a mate sees it: its standard planes and its
    /// origin, which don't move. `id` is one no component of the solve has.
    pub(crate) fn assembly(id: CompId) -> Self {
        Self {
            id,
            name: "the assembly".to_owned(),
            frame: Frame::WORLD,
            fixed: true,
            bodies: Vec::new(),
            planes: crate::StdPlane::ALL
                .into_iter()
                .map(|p| (Vec::new(), PlaneRef::Standard(p), p.plane()))
                .collect(),
            models: Vec::new(),
        }
    }

    /// A length that one radian of turning moves the component's far points by.
    fn size(&self) -> f64 {
        let mut bounds = peet_math::Aabb::EMPTY;
        for (_, body, rel) in &self.bodies {
            let b = body.solid.bounds();
            if b.min.cmple(b.max).all() {
                bounds.extend(rel.to_world(b.min));
                bounds.extend(rel.to_world(b.max));
            }
        }
        if bounds.min.cmple(bounds.max).all() {
            (bounds.size().length() * 0.5).max(1.0)
        } else {
            1.0
        }
    }
}

fn element(end: &MateEnd, component: &Placed) -> Result<Element, String> {
    let name = &component.name;
    let Some(geom) = &end.geom else {
        return Ok(Element::Point(DVec3::ZERO));
    };
    let inner = end.path.get(1..).unwrap_or_default();
    let plane_of = |reference: &PlaneRef| {
        component
            .planes
            .iter()
            .find(|(path, r, _)| path == inner && r == reference)
            .map(|(_, _, plane)| *plane)
    };
    match geom {
        MateGeom::Plane(reference @ (PlaneRef::Standard(_) | PlaneRef::Feature(_))) => {
            let plane = plane_of(reference).ok_or_else(|| format!("The reference plane of {name} is missing or not built. Restore the plane or its component, or delete the mate and select another plane."))?;
            return Ok(Element::Plane(plane.frame.origin, plane.normal()));
        }
        // The origin of the part (or sub-assembly) the path leads to: where its top
        // plane's frame is.
        MateGeom::Origin => {
            if inner.is_empty() {
                return Ok(Element::Point(DVec3::ZERO));
            }
            let plane = plane_of(&PlaneRef::Standard(crate::StdPlane::Top)).ok_or_else(|| {
                format!("The component inside {name} is missing or suppressed. Restore it, or delete the mate.")
            })?;
            return Ok(Element::Point(plane.frame.origin));
        }
        MateGeom::Sketch(reference) => return sketch_element(reference, inner, component),
        _ => {}
    }
    let (bodies, frames): (Vec<Arc<Body>>, Vec<Frame>) = component
        .bodies
        .iter()
        .filter(|(path, ..)| path == inner)
        .map(|(_, body, rel)| (body.clone(), *rel))
        .unzip();
    let gone = |what: &str| {
        format!(
            "The {what} of {name} it was on is gone: the part was changed. Edit the mate to pick it again, or delete the mate."
        )
    };
    let (found, local) = match geom {
        MateGeom::Face(r) | MateGeom::Plane(PlaneRef::Face(r)) => {
            let f = find_face(&bodies, r).ok_or_else(|| gone("face"))?;
            let body = &bodies[f.body];
            let face = body.solid.face(f.id);
            if matches!(geom, MateGeom::Plane(_)) && !matches!(face.surface, Surface::Plane(_)) {
                return Err(format!(
                    "The plane's face of {name} is no longer flat. Delete the mate and select another plane."
                ));
            }
            let e = match &face.surface {
                Surface::Plane(plane) => {
                    let n = if face.reversed {
                        -plane.normal()
                    } else {
                        plane.normal()
                    };
                    Element::Plane(plane.project_point(body.face_center(f.id)), n)
                }
                Surface::Cylinder(c) => Element::Line(c.axis_origin(), c.axis()),
                Surface::Cone(c) => Element::Line(c.axis_origin(), c.frame.z_axis()),
                Surface::Sphere(s) => Element::Point(s.frame.origin),
                Surface::Torus(_) | Surface::Nurbs(_) => {
                    return Err(format!(
                        "The face of {name} is neither flat nor round about one axis, so a mate has nothing to hold on to: pick a flat face, a hole or a shaft, an edge or a corner."
                    ));
                }
            };
            (f.body, e)
        }
        MateGeom::Edge(r) => {
            let f = find_edge(&bodies, r).ok_or_else(|| gone("edge"))?;
            let e = match &bodies[f.body].solid.edge(f.id).curve {
                Curve3::Line(l) => Element::Line(l.origin, l.dir),
                Curve3::Circle(c) => Element::Circle(c.frame.origin, c.frame.z_axis()),
                Curve3::Ellipse(_) | Curve3::Nurbs(_) => {
                    return Err(format!(
                        "The edge of {name} is neither straight nor round, so a mate has nothing to hold on to: pick a straight edge, a round one (its axis), a face or a corner."
                    ));
                }
            };
            (f.body, e)
        }
        MateGeom::Vertex(r) => {
            let f = find_vertex(&bodies, r).ok_or_else(|| gone("corner"))?;
            (
                f.body,
                Element::Point(bodies[f.body].solid.vertex(f.id).point),
            )
        }
        MateGeom::Plane(_) | MateGeom::Origin | MateGeom::Sketch(_) => {
            unreachable!("resolved above")
        }
    };
    Ok(local.moved(&frames[found]))
}

/// A point, line, circle or arc of a sketch, in its component's coordinates.
fn sketch_element(
    reference: &SketchRef,
    inner: &[CompId],
    component: &Placed,
) -> Result<Element, String> {
    use peet_sketch::Geometry;
    let name = &component.name;
    let gone = |what: &str| {
        format!(
            "The {what} of {name} it was on is gone: the part was changed. Edit the mate to pick it again, or delete the mate."
        )
    };
    let (_, model, rel) = component
        .models
        .iter()
        .find(|(path, ..)| path == inner)
        .ok_or_else(|| gone("part"))?;
    let sketch = model
        .feature(reference.feature)
        .and_then(crate::Feature::sketch)
        .ok_or_else(|| gone("sketch"))?;
    let frame = sketch.placement.frame;
    let at = |id| {
        sketch
            .sketch
            .try_point(id)
            .map(|p| frame.to_world(DVec3::new(p.x, p.y, 0.0)))
            .ok_or_else(|| gone("sketch point"))
    };
    let entity = sketch
        .sketch
        .entity(reference.entity)
        .ok_or_else(|| gone("sketch entity"))?;
    let local = match &entity.geometry {
        Geometry::Point { .. } => Element::Point(at(reference.entity)?),
        Geometry::Line { start, end } => {
            let (p, q) = (at(*start)?, at(*end)?);
            let d = (q - p).try_normalize().ok_or_else(|| {
                format!("The sketch line of {name} has no length, so it has no direction.")
            })?;
            Element::Line(p, d)
        }
        Geometry::Circle { center, .. } | Geometry::Arc { center, .. } => {
            Element::Circle(at(*center)?, frame.z_axis())
        }
        Geometry::Spline { .. } | Geometry::Text { .. } => {
            return Err(format!(
                "The sketch entity of {name} is a spline or text, which a mate can't hold on to: pick a sketch point, line, circle or arc."
            ));
        }
    };
    Ok(local.moved(rel))
}

/// A straight slot, in its component's coordinates: a point on the axis of each of its
/// round ends (the two in one plane square to the axes), and the axes' direction.
#[derive(Clone, Copy, Debug)]
struct SlotGeom {
    first: DVec3,
    second: DVec3,
    axis: DVec3,
}

/// The straight slot `end` is a face of (one of its round ends or a flat side), if it is
/// one: two round faces of one radius on parallel axes, joined by flat faces. The first
/// end is the face itself if it is round, else the one the part lists first.
fn slot_of(end: &MateEnd, component: &Placed) -> Option<SlotGeom> {
    let Some(MateGeom::Face(reference)) = &end.geom else {
        return None;
    };
    let inner = end.path.get(1..).unwrap_or_default();
    let (bodies, frames): (Vec<Arc<Body>>, Vec<Frame>) = component
        .bodies
        .iter()
        .filter(|(path, ..)| path == inner)
        .map(|(_, body, rel)| (body.clone(), *rel))
        .unzip();
    let found = find_face(&bodies, reference)?;
    let solid = &bodies[found.body].solid;
    let round = |id| match &solid.face(id).surface {
        Surface::Cylinder(c) => Some((c.axis_origin(), c.axis(), c.radius)),
        _ => None,
    };
    let flat_along = |id, axis: DVec3| match &solid.face(id).surface {
        Surface::Plane(p) => p.normal().dot(axis).abs() < 1e-9,
        _ => false,
    };
    // The other end of a slot from a round end: across a flat side.
    let partner = |(p, d, r): (DVec3, DVec3, f64), from| {
        solid
            .adjacent_faces(from)
            .into_iter()
            .filter(|side| flat_along(*side, d))
            .flat_map(|side| solid.adjacent_faces(side))
            .filter(|other| *other != from)
            .filter_map(round)
            .find(|(q, e, s)| {
                (s - r).abs() < 1e-6 && d.cross(*e).length() < 1e-9 && {
                    let off = *q - p;
                    (off - d * off.dot(d)).length() > 1e-6
                }
            })
    };
    let (first, second) = match round(found.id) {
        Some(own) => (own, partner(own, found.id)?),
        None => {
            let mut ends: Vec<_> = solid.adjacent_faces(found.id);
            ends.sort();
            let ends: Vec<_> = ends
                .into_iter()
                .filter_map(|id| round(id).map(|r| (id, r)))
                .filter(|(_, (_, d, _))| flat_along(found.id, *d))
                .collect();
            let (id, own) = *ends.first()?;
            (own, partner(own, id)?)
        }
    };
    let rel = frames[found.body];
    let (p, d) = (first.0, first.1);
    let q = second.0 - d * (second.0 - p).dot(d);
    Some(SlotGeom {
        first: rel.to_world(p),
        second: rel.to_world(q),
        axis: rel.vector_to_world(d),
    })
}

/// What an end is in the assembly: for a component pattern's direction or axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Located {
    /// A point of a plane and its outward normal.
    Plane(DVec3, DVec3),
    /// A point of a line and its direction.
    Line(DVec3, DVec3),
    Point(DVec3),
}

/// The geometry of `end`, which is on `component`, where the component is.
pub(crate) fn locate(end: &MateEnd, component: &Placed) -> Result<Located, String> {
    let f = &component.frame;
    Ok(match element(end, component)?.axis() {
        Element::Plane(p, n) => Located::Plane(f.to_world(p), f.vector_to_world(n)),
        Element::Line(p, d) | Element::Circle(p, d) => {
            Located::Line(f.to_world(p), f.vector_to_world(d))
        }
        Element::Point(p) => Located::Point(f.to_world(p)),
    })
}

/// The bodies under a component an end's path leads to, each with where it is in the
/// assembly.
fn bodies_at(end: &MateEnd, component: &Placed) -> (Vec<Arc<Body>>, Vec<Frame>) {
    let inner = end.path.get(1..).unwrap_or_default();
    component
        .bodies
        .iter()
        .filter(|(path, ..)| path == inner)
        .map(|(_, body, rel)| (body.clone(), component.frame.compose(rel)))
        .unzip()
}

/// The edge of a component's part an end is on, and where its body is in the assembly:
/// for what an assembly cut's sketch converts.
pub(crate) fn edge_at(
    end: &MateEnd,
    component: &Placed,
) -> Result<(peet_kernel::topo::Edge, Frame), String> {
    let Some(MateGeom::Edge(r)) = &end.geom else {
        return Err("Convert an edge of a component.".to_owned());
    };
    let (bodies, frames) = bodies_at(end, component);
    let f = find_edge(&bodies, r).ok_or_else(|| {
        format!(
            "The edge of {} it converts is gone: the part was changed.",
            component.name
        )
    })?;
    Ok((bodies[f.body].solid.edge(f.id).clone(), frames[f.body]))
}

/// The plane, with its own axes, of an end that is flat: a flat face (its outward normal)
/// or a reference plane of a component, or one of the assembly's standard planes. For an
/// assembly cut's sketch.
pub(crate) fn plane_at(end: &MateEnd, component: &Placed) -> Result<peet_math::Plane, String> {
    let name = &component.name;
    let inner = end.path.get(1..).unwrap_or_default();
    let not_flat = || {
        format!("The sketch must lie on a flat face or a plane: the end on {name} is neither.")
    };
    match &end.geom {
        Some(MateGeom::Face(r) | MateGeom::Plane(PlaneRef::Face(r))) => {
            let (bodies, frames) = bodies_at(end, component);
            let f = find_face(&bodies, r).ok_or_else(|| {
                format!("The face of {name} the sketch is on is gone: the part was changed.")
            })?;
            let face = bodies[f.body].solid.face(f.id);
            let Surface::Plane(plane) = &face.surface else {
                return Err(not_flat());
            };
            let mut local = plane.frame;
            if face.reversed {
                local.rotation *= DQuat::from_rotation_x(std::f64::consts::PI);
            }
            Ok(peet_math::Plane {
                frame: frames[f.body].compose(&local),
            })
        }
        Some(MateGeom::Plane(reference)) => {
            let plane = component
                .planes
                .iter()
                .find(|(path, r, _)| path == inner && r == reference)
                .map(|(_, _, plane)| *plane)
                .ok_or_else(|| {
                    format!("The plane of {name} the sketch is on is missing or not built.")
                })?;
            Ok(peet_math::Plane {
                frame: component.frame.compose(&plane.frame),
            })
        }
        _ => Err(not_flat()),
    }
}

// ---- Equations ----

#[derive(Clone, Copy, Debug, PartialEq)]
enum EqKind {
    Dot,
    Offset,
    Distance,
    LineDistance,
    /// `D₀ · Σ cₖ Pₖ`: the first side's direction along a weighted sum of the sides'
    /// points. For what ties more than two components: the middle of a width pair on
    /// two of them, say.
    Sum,
}

/// Most components (sides) one equation reads: six unknowns each.
const SIDES: usize = 4;

/// One residual before it is tied to components.
#[derive(Clone, Copy, Debug)]
struct Proto {
    kind: EqKind,
    /// Which of the mate's ends each side is on: 0 is `a`, 1 is `b`, 2 and 3 a width
    /// mate's second ends or a symmetric mate's plane (`MATE_ENDS`).
    on: [usize; SIDES],
    /// How many sides there are. The two-sided kinds read the first two.
    sides: usize,
    /// Each side's direction and point.
    dirs: [DVec3; SIDES],
    points: [DVec3; SIDES],
    /// For `Sum`: each side's weight.
    coef: [f64; SIDES],
    target: f64,
    /// Held anywhere between these instead of at `target` (a residual of nothing
    /// between them, and of how far outside otherwise).
    range: Option<(f64, f64)>,
    /// The first side mirrored in this plane of the assembly (a point of it and its
    /// unit normal) before anything is measured.
    mirror: Option<(DVec3, DVec3)>,
}

/// Which of a mate's ends a proto's side is on: `a`, `b`, and the others in the order
/// [`mate_ends`] gives them.
const END_A: usize = 0;
const END_B: usize = 1;

/// One side of an equation: a component's base orientation and scale, and the direction
/// and point it reads, in the component's coordinates.
#[derive(Clone, Copy, Debug)]
struct Side {
    base: DQuat,
    scale: f64,
    dir: DVec3,
    point: DVec3,
    /// Mirrored in this plane of the assembly (a point and a unit normal) first.
    mirror: Option<(DVec3, DVec3)>,
}

impl Side {
    const FIXED: Self = Self {
        base: DQuat::IDENTITY,
        scale: 1.0,
        dir: DVec3::ZERO,
        point: DVec3::ZERO,
        mirror: None,
    };
}

#[derive(Clone, Debug)]
struct Eq {
    kind: EqKind,
    slots: [u32; MAX_SLOTS],
    sides: [Side; SIDES],
    /// How many sides there are: the first `6 · count` slots are read.
    count: usize,
    coef: [f64; SIDES],
    target: f64,
    range: Option<(f64, f64)>,
    /// What the residual is multiplied by: 1 for a mate, less for a drag's pull.
    weight: f64,
}

/// `J_l(w)ᵀ v`, with `J_l` the left Jacobian of the rotation `exp(w)`: turns the
/// gradient by a small rotation of the world into the gradient by `w`.
fn rotation_gradient(w: DVec3, v: DVec3) -> DVec3 {
    let theta = w.length();
    let (b, c) = if theta < 1e-4 {
        (
            0.5 - theta * theta / 24.0,
            1.0 / 6.0 - theta * theta / 120.0,
        )
    } else {
        (
            (1.0 - theta.cos()) / (theta * theta),
            (theta - theta.sin()) / (theta * theta * theta),
        )
    };
    v - b * w.cross(v) + c * w.cross(w.cross(v))
}

/// A vector mirrored in a plane of unit normal `m`.
fn reflect(v: DVec3, m: DVec3) -> DVec3 {
    v - 2.0 * m.dot(v) * m
}

/// Where a side is: its world point and direction (mirrored if it is), the arm from its
/// component's origin and its direction before any mirror, and its rotation vector.
#[derive(Clone, Copy, Default)]
struct World {
    point: DVec3,
    dir: DVec3,
    arm: DVec3,
    unmirrored: DVec3,
    w: DVec3,
}

impl Eq {
    /// Where a side is, from the values of its unknowns at `at`.
    fn world(side: &Side, v: &[f64], at: usize) -> World {
        let t = DVec3::new(v[at], v[at + 1], v[at + 2]);
        let w = DVec3::new(v[at + 3], v[at + 4], v[at + 5]) / side.scale;
        let r = DQuat::from_scaled_axis(w) * side.base;
        let arm = r * side.point;
        let (point, dir) = (t + arm, r * side.dir);
        let (point, mirrored) = match side.mirror {
            Some((q, m)) => (point - 2.0 * m.dot(point - q) * m, reflect(dir, m)),
            None => (point, dir),
        };
        World {
            point,
            dir: mirrored,
            arm,
            unmirrored: dir,
            w,
        }
    }
}

impl Equation for Eq {
    fn slots(&self) -> &[u32] {
        &self.slots[..6 * self.count]
    }

    fn eval(&self, v: &[f64], g: &mut [f64; MAX_SLOTS]) -> f64 {
        let n = self.count;
        let mut world = [World::default(); SIDES];
        for (k, side) in self.sides.iter().enumerate().take(n) {
            world[k] = Self::world(side, v, self.slots[6 * k] as usize);
        }
        let (p1, d1, p2, d2) = (world[0].point, world[0].dir, world[1].point, world[1].dir);
        let r = p2 - p1;
        // The residual, and its gradient by each side's world point and direction.
        let mut gp = [DVec3::ZERO; SIDES];
        let mut gd = [DVec3::ZERO; SIDES];
        let f = match self.kind {
            EqKind::Dot => {
                (gd[0], gd[1]) = (d2, d1);
                d1.dot(d2)
            }
            EqKind::Offset => {
                (gp[0], gd[0], gp[1]) = (-d1, r, d1);
                d1.dot(r)
            }
            EqKind::Distance => {
                let e = r.try_normalize().unwrap_or(DVec3::X);
                (gp[0], gp[1]) = (-e, e);
                r.length()
            }
            EqKind::LineDistance => {
                let along = r.dot(d1);
                let q = r - along * d1;
                let e = q
                    .try_normalize()
                    .unwrap_or_else(|| d1.any_orthonormal_vector());
                (gp[0], gd[0], gp[1]) = (-e, -along * e, e);
                q.length()
            }
            EqKind::Sum => {
                let mut sum = DVec3::ZERO;
                for k in 0..n {
                    sum += self.coef[k] * world[k].point;
                    gp[k] = self.coef[k] * d1;
                }
                gd[0] = sum;
                d1.dot(sum)
            }
        };
        let residual = match self.range {
            Some((min, max)) if (min..=max).contains(&f) => {
                g[..6 * n].fill(0.0);
                return 0.0;
            }
            Some((min, max)) => f - f.clamp(min, max),
            None => f - self.target,
        };
        for k in 0..n {
            let (mut p, mut d) = (gp[k], gd[k]);
            if let Some((_, m)) = self.sides[k].mirror {
                (p, d) = (reflect(p, m), reflect(d, m));
            }
            let at = &world[k];
            let turn = rotation_gradient(at.w, at.arm.cross(p) + at.unmirrored.cross(d))
                / self.sides[k].scale;
            g[6 * k..6 * k + 3].copy_from_slice(&p.to_array());
            g[6 * k + 3..6 * k + 6].copy_from_slice(&turn.to_array());
        }
        for x in g[..6 * n].iter_mut() {
            *x *= self.weight;
        }
        residual * self.weight
    }
}

/// Two unit directions square to `d` and to each other.
fn square_to(d: DVec3) -> (DVec3, DVec3) {
    let u = d.any_orthonormal_vector();
    (u, d.cross(u).normalize())
}

fn proto(kind: EqKind, first_is_a: bool, d1: DVec3, p1: DVec3, d2: DVec3, p2: DVec3) -> Proto {
    let (first, other) = if first_is_a {
        (END_A, END_B)
    } else {
        (END_B, END_A)
    };
    Proto {
        kind,
        on: [first, other, 0, 0],
        sides: 2,
        dirs: [d1, d2, DVec3::ZERO, DVec3::ZERO],
        points: [p1, p2, DVec3::ZERO, DVec3::ZERO],
        coef: [0.0; SIDES],
        target: 0.0,
        range: None,
        mirror: None,
    }
}

/// `d · Σ cₖ pₖ = target`, `d` a direction of the first side: each side an end of the
/// mate (`on`), with its point and weight.
fn sum(d: DVec3, terms: &[(usize, DVec3, f64)], target: f64) -> Proto {
    let mut p = Proto {
        kind: EqKind::Sum,
        sides: terms.len(),
        target,
        ..proto(EqKind::Sum, true, d, DVec3::ZERO, DVec3::ZERO, DVec3::ZERO)
    };
    for (k, (on, point, c)) in terms.iter().enumerate() {
        p.on[k] = *on;
        p.points[k] = *point;
        p.coef[k] = *c;
    }
    p
}

/// The direction `d2` of the other end is along `d1` (either way): two equations.
fn parallel(out: &mut Vec<Proto>, first_is_a: bool, d1: DVec3, d2: DVec3) {
    let (u, v) = square_to(d1);
    for dir in [u, v] {
        out.push(proto(
            EqKind::Dot,
            first_is_a,
            dir,
            DVec3::ZERO,
            d2,
            DVec3::ZERO,
        ));
    }
}

/// The point `p2` of the other end is on the line through `p1` along `d1`: two equations.
fn on_line(out: &mut Vec<Proto>, first_is_a: bool, p1: DVec3, d1: DVec3, p2: DVec3) {
    let (u, v) = square_to(d1);
    for dir in [u, v] {
        out.push(proto(EqKind::Offset, first_is_a, dir, p1, DVec3::ZERO, p2));
    }
}

/// The point `p2` of the other end is `distance` along `n` from the plane through `p1`.
fn on_plane(out: &mut Vec<Proto>, first_is_a: bool, p1: DVec3, n: DVec3, p2: DVec3, distance: f64) {
    out.push(Proto {
        target: distance,
        ..proto(EqKind::Offset, first_is_a, n, p1, DVec3::ZERO, p2)
    });
}

/// A length or angle a mate holds: exactly, or anywhere between two (a limit mate's).
#[derive(Clone, Copy, Debug)]
enum Measure {
    Exactly(f64),
    Between(f64, f64),
}

impl Measure {
    /// The proto measuring it.
    fn on(self, p: Proto) -> Proto {
        match self {
            Self::Exactly(target) => Proto { target, ..p },
            Self::Between(min, max) => Proto {
                range: Some((min, max)),
                ..p
            },
        }
    }
}

/// The equations of a mate between two elements, each in its component's coordinates.
fn equations(
    kind: &MateKind,
    a: Element,
    b: Element,
    params: &Parameters,
) -> Result<Vec<Proto>, String> {
    use Element::{Circle, Line, Plane, Point};
    let mut out = Vec::new();
    let wrong = |takes: &str| {
        format!(
            "A {} mate can't join {} and {}: it takes {takes}.",
            kind.word(),
            a.word(),
            b.word()
        )
    };
    // A round edge coincident with a plane lies in it, as SolidWorks has it: its axis
    // square to the plane and its centre on it. To every other mate it is its axis.
    if *kind == MateKind::Coincident {
        match (a, b) {
            (Plane(pa, na), Circle(pb, db)) => {
                parallel(&mut out, true, na, db);
                on_plane(&mut out, true, pa, na, pb, 0.0);
                return Ok(out);
            }
            (Circle(pa, da), Plane(pb, nb)) => {
                parallel(&mut out, false, nb, da);
                on_plane(&mut out, false, pb, nb, pa, 0.0);
                return Ok(out);
            }
            _ => {}
        }
    }
    let (a, b) = (a.axis(), b.axis());
    match kind {
        MateKind::Width { .. }
        | MateKind::Slot { .. }
        | MateKind::Symmetric { .. }
        | MateKind::Path { .. } => {
            unreachable!("set up by their own functions")
        }
        MateKind::Fasten(relative) => {
            // B's origin stays where it is in A, and B's axes stay along the same
            // directions of A.
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                out.push(proto(
                    EqKind::Offset,
                    true,
                    axis,
                    relative.origin,
                    DVec3::ZERO,
                    DVec3::ZERO,
                ));
            }
            for (i, j) in [
                (DVec3::X, DVec3::Y),
                (DVec3::Y, DVec3::Z),
                (DVec3::Z, DVec3::X),
            ] {
                out.push(proto(
                    EqKind::Dot,
                    true,
                    relative.rotation * i,
                    DVec3::ZERO,
                    j,
                    DVec3::ZERO,
                ));
            }
        }
        MateKind::Coincident | MateKind::Concentric => match (a, b) {
            (Line(pa, da), Line(pb, db)) => {
                parallel(&mut out, true, da, db);
                on_line(&mut out, true, pa, da, pb);
            }
            _ if *kind == MateKind::Concentric => {
                return Err(wrong(
                    "two axes: round faces (holes, shafts) or round edges",
                ));
            }
            (Plane(pa, na), Plane(pb, nb)) => {
                parallel(&mut out, true, na, nb);
                on_plane(&mut out, true, pa, na, pb, 0.0);
            }
            (Point(pa), Point(pb)) => {
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    out.push(proto(EqKind::Offset, true, axis, pa, DVec3::ZERO, pb));
                }
            }
            (Plane(pa, na), Point(pb)) => on_plane(&mut out, true, pa, na, pb, 0.0),
            (Point(pa), Plane(pb, nb)) => on_plane(&mut out, false, pb, nb, pa, 0.0),
            (Line(pa, da), Point(pb)) => on_line(&mut out, true, pa, da, pb),
            (Point(pa), Line(pb, db)) => on_line(&mut out, false, pb, db, pa),
            (Plane(pa, na), Line(pb, db)) => {
                out.push(proto(EqKind::Dot, true, na, DVec3::ZERO, db, DVec3::ZERO));
                on_plane(&mut out, true, pa, na, pb, 0.0);
            }
            (Line(pa, da), Plane(pb, nb)) => {
                out.push(proto(EqKind::Dot, false, nb, DVec3::ZERO, da, DVec3::ZERO));
                on_plane(&mut out, false, pb, nb, pa, 0.0);
            }
            (Circle(..), _) | (_, Circle(..)) => unreachable!("circles taken as axes"),
        },
        MateKind::Parallel => match (a, b) {
            (Plane(_, da), Plane(_, db)) | (Line(_, da), Line(_, db)) => {
                parallel(&mut out, true, da, db);
            }
            (Plane(_, n), Line(_, d)) => {
                out.push(proto(EqKind::Dot, true, n, DVec3::ZERO, d, DVec3::ZERO));
            }
            (Line(_, d), Plane(_, n)) => {
                out.push(proto(EqKind::Dot, false, n, DVec3::ZERO, d, DVec3::ZERO));
            }
            _ => return Err(wrong("flat faces, axes and straight edges")),
        },
        MateKind::Distance(value) => {
            let d = value
                .evaluate(ScalarKind::Length, params)
                .map_err(|m| format!("Distance: {m}."))?;
            if d < 0.0 {
                return Err(
                    "The distance is negative: give it as a positive length (flip turns a face the other way round)."
                        .to_owned(),
                );
            }
            // Between two points, or a point and a line, a distance of nothing has no
            // direction to move along: that is what coincident is for.
            if d < 1e-9 && !matches!((a, b), (Plane(..), _) | (_, Plane(..))) {
                return Err("The distance is zero: use a coincident mate for that.".to_owned());
            }
            distance(&mut out, a, b, Measure::Exactly(d));
        }
        MateKind::LimitDistance { min, max } => {
            let length = |s: &Scalar, which: &str| {
                s.evaluate(ScalarKind::Length, params)
                    .map_err(|m| format!("The {which} distance: {m}."))
            };
            let (min, max) = (length(min, "least")?, length(max, "greatest")?);
            if !(0.0 <= min && min <= max) {
                return Err(
                    "A limit distance mate needs 0 <= min <= max: give the least distance first."
                        .to_owned(),
                );
            }
            distance(&mut out, a, b, Measure::Between(min, max));
        }
        MateKind::Angle(value) => {
            let degrees = value
                .evaluate(ScalarKind::Angle, params)
                .map_err(|m| format!("Angle: {m}."))?;
            if !(degrees > 1e-6 && degrees < 180.0 - 1e-6) {
                return Err(
                    "The angle must be between 0 and 180 degrees, and neither: for those, use a parallel mate."
                        .to_owned(),
                );
            }
            let (da, db) = match (a, b) {
                (Plane(_, da) | Line(_, da), Plane(_, db) | Line(_, db)) => (da, db),
                _ => return Err(wrong("flat faces, axes and straight edges")),
            };
            out.push(Proto {
                target: degrees.to_radians().cos(),
                ..proto(EqKind::Dot, true, da, DVec3::ZERO, db, DVec3::ZERO)
            });
        }
        MateKind::LimitAngle { min, max } => {
            let angle = |s: &Scalar, which: &str| {
                s.evaluate(ScalarKind::Angle, params)
                    .map_err(|m| format!("The {which} angle: {m}."))
            };
            let (min, max) = (angle(min, "least")?, angle(max, "greatest")?);
            if !(0.0 <= min && min <= max && max <= 180.0) {
                return Err(
                    "A limit angle mate needs 0 <= min <= max <= 180 degrees: give the least angle first."
                        .to_owned(),
                );
            }
            let (da, db) = match (a, b) {
                (Plane(_, da) | Line(_, da), Plane(_, db) | Line(_, db)) => (da, db),
                _ => return Err(wrong("flat faces, axes and straight edges")),
            };
            // The cosine falls as the angle grows.
            out.push(
                Measure::Between(max.to_radians().cos(), min.to_radians().cos()).on(proto(
                    EqKind::Dot,
                    true,
                    da,
                    DVec3::ZERO,
                    db,
                    DVec3::ZERO,
                )),
            );
        }
    }
    Ok(out)
}

/// The equations of a distance between two elements (neither a circle): from a plane
/// along its normal, else straight across.
fn distance(out: &mut Vec<Proto>, a: Element, b: Element, measure: Measure) {
    use Element::{Line, Plane, Point};
    let from_plane = |first_is_a, p1, n, p2| {
        measure.on(proto(EqKind::Offset, first_is_a, n, p1, DVec3::ZERO, p2))
    };
    let line_distance = |first_is_a, p1, d1, p2| {
        measure.on(proto(
            EqKind::LineDistance,
            first_is_a,
            d1,
            p1,
            DVec3::ZERO,
            p2,
        ))
    };
    match (a, b) {
        (Plane(pa, na), Plane(pb, nb)) => {
            parallel(out, true, na, nb);
            out.push(from_plane(true, pa, na, pb));
        }
        (Plane(pa, na), Point(pb)) => out.push(from_plane(true, pa, na, pb)),
        (Point(pa), Plane(pb, nb)) => out.push(from_plane(false, pb, nb, pa)),
        (Plane(pa, na), Line(pb, db)) => {
            out.push(proto(EqKind::Dot, true, na, DVec3::ZERO, db, DVec3::ZERO));
            out.push(from_plane(true, pa, na, pb));
        }
        (Line(pa, da), Plane(pb, nb)) => {
            out.push(proto(EqKind::Dot, false, nb, DVec3::ZERO, da, DVec3::ZERO));
            out.push(from_plane(false, pb, nb, pa));
        }
        (Point(pa), Point(pb)) => out.push(measure.on(proto(
            EqKind::Distance,
            true,
            DVec3::ZERO,
            pa,
            DVec3::ZERO,
            pb,
        ))),
        (Line(pa, da), Line(pb, db)) => {
            parallel(out, true, da, db);
            out.push(line_distance(true, pa, da, pb));
        }
        (Line(pa, da), Point(pb)) => out.push(line_distance(true, pa, da, pb)),
        (Point(pa), Line(pb, db)) => out.push(line_distance(false, pb, db, pa)),
        (Element::Circle(..), _) | (_, Element::Circle(..)) => {
            unreachable!("circles taken as axes")
        }
    }
}

/// A pair is rigid in one top-level component. Its midpoint is recomputed from
/// current geometry, so centering follows changes to either face or plane.
fn width_middle(first: Element, second: Element, which: &str) -> Result<(Element, f64), String> {
    let (Element::Plane(p1, n1), Element::Plane(p2, n2)) = (first, second) else {
        return Err(format!(
            "A width mate's {which} pair needs two planar faces or reference planes."
        ));
    };
    if n1.dot(n2).abs() < 1.0 - 1e-8 {
        return Err(format!(
            "A width mate's {which} planes must be parallel. Select two parallel faces or reference planes."
        ));
    }
    if n1.dot(p2 - p1).abs() < 1e-8 {
        return Err(format!(
            "A width mate's {which} planes coincide. Select two distinct planes."
        ));
    }
    let span = n1.dot(p2 - p1);
    Ok((
        Element::Plane((p1 + p2) * 0.5, n1 * span.signum()),
        span.abs(),
    ))
}

fn width_equations(
    mode: &WidthMode,
    a: Element,
    b: Element,
    clearance: f64,
    params: &Parameters,
) -> Result<Vec<Proto>, String> {
    // Centred, the two midplanes coincide whichever is wider (as in SolidWorks, whose
    // templates centre wide frames on narrower plates); the other modes place the tab
    // within the gap, so it has to fit.
    if clearance < -1e-8 && *mode != WidthMode::Centered {
        return Err(
            "The tab is wider than the width pair. Widen the gap or narrow the tab.".to_owned(),
        );
    }
    let clearance = clearance.max(0.0);
    let mut out = equations(&MateKind::Coincident, a, b, params)?;
    let offset = match mode {
        WidthMode::Centered => 0.0,
        WidthMode::Free => {
            if clearance >= 1e-8 {
                out.iter_mut()
                    .find(|e| e.kind == EqKind::Offset)
                    .expect("plane-to-plane or plane-to-axis offset")
                    .range = Some((-clearance * 0.5, clearance * 0.5));
            }
            return Ok(out);
        }
        WidthMode::Dimension(s) => {
            let gap = s.evaluate(ScalarKind::Length, params)?;
            if gap < -1e-8 || gap > clearance + 1e-8 || !gap.is_finite() {
                return Err(format!(
                    "The width mate's distance must be between 0 and {clearance} mm, the available clearance."
                ));
            }
            gap.clamp(0.0, clearance) - clearance * 0.5
        }
        WidthMode::Percent(s) => {
            let percent = s.evaluate(ScalarKind::Number, params)?;
            if !(0.0..=100.0).contains(&percent) {
                return Err("The width mate's percent must be between 0 and 100.".to_owned());
            }
            clearance * (percent / 100.0 - 0.5)
        }
    };
    out.iter_mut()
        .find(|e| e.kind == EqKind::Offset)
        .expect("plane-to-plane or plane-to-axis offset")
        .target = offset;
    Ok(out)
}

/// A cylindrical tab has its diameter; an edge selected as an axis has no thickness.
fn width_axis_radius(end: &MateEnd, component: &Placed) -> Result<f64, String> {
    let Some(MateGeom::Face(reference)) = &end.geom else {
        return Ok(0.0);
    };
    let inner = end.path.get(1..).unwrap_or_default();
    let bodies: Vec<_> = component
        .bodies
        .iter()
        .filter(|(path, ..)| path == inner)
        .map(|(_, body, _)| body.clone())
        .collect();
    let found = find_face(&bodies, reference)
        .ok_or_else(|| "The width mate's cylindrical tab face is gone.".to_owned())?;
    match bodies[found.body].solid.face(found.id).surface {
        Surface::Cylinder(c) => Ok(c.radius),
        _ => Err("A single width mate tab face must be cylindrical. Select two tab planes for other shapes.".to_owned()),
    }
}

/// Which of a mate's ends its protos' sides are on: `a`, `b`, then a width mate's
/// second width end and second tab end, or a symmetric mate's plane.
fn mate_ends(mate: &Mate) -> Vec<&MateEnd> {
    let mut ends = vec![&mate.a, &mate.b];
    match &mate.kind {
        MateKind::Width { a2, b2, .. } => {
            ends.push(a2);
            ends.extend(b2);
        }
        MateKind::Symmetric { plane } => ends.push(plane),
        MateKind::Path { more, roll, .. } => {
            ends.extend(roll.iter().map(|r| &r.up));
            ends.extend(more.iter());
        }
        _ => {}
    }
    ends
}

/// The index in [`mate_ends`] of a width mate's second width end, second tab end, and
/// a symmetric mate's plane.
const END_A2: usize = 2;
const END_B2: usize = 3;
const END_PLANE: usize = 2;

/// A width mate whose width pair or tab pair is on two components: the middles are not
/// fixed in any one component, so the equations weigh the four faces' places along the
/// first width face's normal (`EqKind::Sum`). `world` is where each end is now (to tell
/// which face of a pair is on which side); `radius` is a round tab's.
#[allow(clippy::too_many_arguments)]
fn width_spread(
    mode: &WidthMode,
    elements: &[Element],
    world: &[Element],
    radius: f64,
    params: &Parameters,
) -> Result<Vec<Proto>, String> {
    let along = |e: &Element| match e {
        Element::Plane(p, _) | Element::Line(p, _) | Element::Circle(p, _) | Element::Point(p) => {
            *p
        }
    };
    let (Element::Plane(_, na), Element::Plane(_, wn)) = (elements[END_A], world[END_A]) else {
        return Err(
            "A width mate's width pair needs two planar faces or reference planes.".to_owned(),
        );
    };
    if !matches!(elements[END_A2], Element::Plane(..)) {
        return Err(
            "A width mate's width pair needs two planar faces or reference planes.".to_owned(),
        );
    }
    let at = |i: usize| wn.dot(along(&world[i]));
    let span = at(END_A2) - at(END_A);
    if span.abs() < 1e-8 {
        return Err("A width mate's width planes coincide. Select two distinct planes.".to_owned());
    }
    // Measured along the first width face's normal, turned towards the second.
    let d = na * span.signum();
    let side = span.signum();
    let mut out = Vec::new();
    // The tab: its faces nearer and further from the first width face.
    let (near, far) = match elements.get(END_B2) {
        Some(Element::Plane(..)) => {
            if !matches!(elements[END_B], Element::Plane(..)) {
                return Err(
                    "A width mate's tab pair needs two planar faces or reference planes."
                        .to_owned(),
                );
            }
            if side * (at(END_B2) - at(END_B)) >= 0.0 {
                (END_B, END_B2)
            } else {
                (END_B2, END_B)
            }
        }
        Some(_) => {
            return Err(
                "A width mate's tab pair needs two planar faces or reference planes.".to_owned(),
            );
        }
        None => (END_B, END_B),
    };
    // Square to the width: a tab face parallel to the first width face, a round tab's
    // axis in its plane.
    match elements[END_B] {
        Element::Plane(_, nb) => parallel(&mut out, true, na, nb),
        Element::Line(_, db) | Element::Circle(_, db) => {
            out.push(proto(EqKind::Dot, true, na, DVec3::ZERO, db, DVec3::ZERO));
        }
        Element::Point(_) => {
            return Err("A width mate's single tab end must be an axis: select a cylindrical face, a round edge or a straight edge, or give two tab planes.".to_owned());
        }
    }
    let point = |i: usize| along(&elements[i]);
    let a = (END_A, point(END_A));
    let a2 = (END_A2, point(END_A2));
    let (lo, hi) = ((near, point(near)), (far, point(far)));
    let term = |(on, p): (usize, DVec3), c: f64| (on, p, c);
    match mode {
        WidthMode::Centered => out.push(sum(
            d,
            &[term(a, -0.5), term(a2, -0.5), term(lo, 0.5), term(hi, 0.5)],
            0.0,
        )),
        WidthMode::Free => {
            // The near tab face beyond the first width face, the far one short of the
            // second.
            out.push(Proto {
                range: Some((radius, f64::INFINITY)),
                ..sum(d, &[term(a, -1.0), term(lo, 1.0)], 0.0)
            });
            out.push(Proto {
                range: Some((radius, f64::INFINITY)),
                ..sum(d, &[term(a, 0.0), term(a2, 1.0), term(hi, -1.0)], 0.0)
            });
        }
        WidthMode::Dimension(s) => {
            let gap = s.evaluate(ScalarKind::Length, params)?;
            out.push(sum(d, &[term(a, -1.0), term(lo, 1.0)], gap + radius));
        }
        WidthMode::Percent(s) => {
            let f = s.evaluate(ScalarKind::Number, params)? / 100.0;
            if !(0.0..=1.0).contains(&f) {
                return Err("The width mate's percent must be between 0 and 100.".to_owned());
            }
            // The near gap is that share of the clearance: the width less the tab.
            out.push(sum(
                d,
                &[
                    term(a, f - 1.0),
                    term(a2, -f),
                    term(lo, 1.0 - f),
                    term(hi, f),
                ],
                radius * (1.0 - 2.0 * f),
            ));
        }
    }
    Ok(out)
}

/// The equations of a slot mate: `slot` is the slot (in the component of the end
/// `slot_end`), `pin` the other end's element.
fn slot_equations(
    mode: &WidthMode,
    slot: SlotGeom,
    slot_is_a: bool,
    pin: Element,
    flip: bool,
    params: &Parameters,
) -> Result<Vec<Proto>, String> {
    let (Element::Line(pp, dp) | Element::Circle(pp, dp)) = pin else {
        return Err(
            "A slot mate takes a slot and an axis: a round face or edge, or a straight edge."
                .to_owned(),
        );
    };
    let run = slot.second - slot.first;
    let length = run.length();
    let along = run / length;
    let across = slot.axis.cross(along).normalize();
    let mut out = Vec::new();
    // The pin parallel to the slot's ends, in the plane through their axes.
    parallel(&mut out, slot_is_a, slot.axis, dp);
    on_plane(&mut out, slot_is_a, slot.first, across, pp, 0.0);
    let from_first = |at: f64| if flip { length - at } else { at };
    let measure = match mode {
        WidthMode::Free => Measure::Between(0.0, length),
        WidthMode::Centered => Measure::Exactly(length * 0.5),
        WidthMode::Dimension(s) => {
            let at = s.evaluate(ScalarKind::Length, params)?;
            if !(-1e-9..=length + 1e-9).contains(&at) {
                return Err(format!(
                    "The slot mate's distance must be between 0 and {length:.3} mm, the slot's length between the centres of its ends."
                ));
            }
            Measure::Exactly(from_first(at))
        }
        WidthMode::Percent(s) => {
            let percent = s.evaluate(ScalarKind::Number, params)?;
            if !(0.0..=100.0).contains(&percent) {
                return Err("The slot mate's percent must be between 0 and 100.".to_owned());
            }
            Measure::Exactly(from_first(length * percent / 100.0))
        }
    };
    out.push(measure.on(proto(
        EqKind::Offset,
        slot_is_a,
        along,
        slot.first,
        DVec3::ZERO,
        pp,
    )));
    Ok(out)
}

/// A piece of a path mate's path, in its component's coordinates, running from its start
/// to its end.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Piece {
    Line {
        from: DVec3,
        to: DVec3,
    },
    /// Round `axis` (a unit vector) from `from`, by `sweep` radians (right-handed about
    /// the axis; negative the other way).
    Arc {
        center: DVec3,
        axis: DVec3,
        radius: f64,
        from: DVec3,
        sweep: f64,
    },
}

impl Piece {
    fn length(&self) -> f64 {
        match *self {
            Self::Line { from, to } => from.distance(to),
            Self::Arc { radius, sweep, .. } => radius * sweep.abs(),
        }
    }

    /// The point and the unit direction of travel a length `s` along it.
    fn at(&self, s: f64) -> (DVec3, DVec3) {
        match *self {
            Self::Line { from, to } => {
                let d = (to - from).normalize_or_zero();
                (from + d * s, d)
            }
            Self::Arc {
                center,
                axis,
                radius,
                from,
                sweep,
            } => {
                let angle = s / radius * sweep.signum();
                let turn = DQuat::from_axis_angle(axis, angle);
                let radial = turn * (from - center);
                let tangent = axis.cross(radial).normalize_or_zero() * sweep.signum();
                (center + radial, tangent)
            }
        }
    }

    fn start(&self) -> DVec3 {
        self.at(0.0).0
    }

    fn end(&self) -> DVec3 {
        self.at(self.length()).0
    }

    fn reversed(self) -> Self {
        match self {
            Self::Line { from, to } => Self::Line { from: to, to: from },
            Self::Arc {
                center,
                axis,
                radius,
                sweep,
                ..
            } => Self::Arc {
                center,
                axis,
                radius,
                from: self.end(),
                sweep: -sweep,
            },
        }
    }

    /// How far along it the point nearest `p` is.
    fn nearest(&self, p: DVec3) -> f64 {
        match *self {
            Self::Line { from, to } => {
                let d = to - from;
                let l2 = d.length_squared();
                if l2 < 1e-24 {
                    0.0
                } else {
                    ((p - from).dot(d) / l2).clamp(0.0, 1.0) * l2.sqrt()
                }
            }
            Self::Arc {
                center,
                axis,
                radius,
                from,
                sweep,
            } => {
                let r0 = from - center;
                let q = p - center;
                let q = q - axis * q.dot(axis);
                if q.length() < 1e-12 {
                    return 0.0;
                }
                // The angle from the start, the way the arc runs, in [0, 2π)
                let mut angle = r0.cross(q).dot(axis).atan2(r0.dot(q)) * sweep.signum();
                if angle < 0.0 {
                    angle += std::f64::consts::TAU;
                }
                let span = sweep.abs();
                let along = if angle <= span {
                    angle
                } else if angle - span < std::f64::consts::TAU - angle {
                    span
                } else {
                    0.0
                };
                along * radius
            }
        }
    }
}

/// A piece of a path from an end of a path mate: a straight or round edge, or a sketch
/// line, arc or circle; in the end's component's coordinates.
fn path_piece(end: &MateEnd, component: &Placed) -> Result<Piece, String> {
    let name = &component.name;
    let gone = |what: &str| {
        format!(
            "The {what} of {name} the path was on is gone: the part was changed. Edit the mate to pick it again, or delete the mate."
        )
    };
    let wrong = || {
        format!(
            "A path mate's path is made of straight and round edges, or sketch lines, arcs and circles, of {name}: select those."
        )
    };
    let inner = end.path.get(1..).unwrap_or_default();
    match &end.geom {
        Some(MateGeom::Edge(r)) => {
            let (bodies, frames): (Vec<Arc<Body>>, Vec<Frame>) = component
                .bodies
                .iter()
                .filter(|(path, ..)| path == inner)
                .map(|(_, body, rel)| (body.clone(), *rel))
                .unzip();
            let f = find_edge(&bodies, r).ok_or_else(|| gone("edge"))?;
            let rel = frames[f.body];
            let edge = bodies[f.body].solid.edge(f.id);
            let piece = match &edge.curve {
                Curve3::Line(_) => Piece::Line {
                    from: edge.point_at_fraction(0.0),
                    to: edge.point_at_fraction(1.0),
                },
                Curve3::Circle(c) => Piece::Arc {
                    center: c.frame.origin,
                    axis: c.frame.z_axis(),
                    radius: c.radius,
                    from: edge.point_at_fraction(0.0),
                    sweep: edge.t1 - edge.t0,
                },
                _ => return Err(wrong()),
            };
            Ok(moved_piece(piece, &rel))
        }
        Some(MateGeom::Sketch(reference)) => {
            let (_, model, rel) = component
                .models
                .iter()
                .find(|(path, ..)| path == inner)
                .ok_or_else(|| gone("part"))?;
            let sketch = model
                .feature(reference.feature)
                .and_then(crate::Feature::sketch)
                .ok_or_else(|| gone("sketch"))?;
            let frame = sketch.placement.frame;
            let at = |p: peet_math::DVec2| frame.to_world(DVec3::new(p.x, p.y, 0.0));
            let curve = sketch
                .sketch
                .curve(reference.entity)
                .ok_or_else(|| gone("sketch curve"))?;
            let axis = frame.z_axis();
            let piece = match curve {
                peet_sketch::Curve::Line { a, b } => Piece::Line {
                    from: at(a),
                    to: at(b),
                },
                peet_sketch::Curve::Circle { center, radius } => Piece::Arc {
                    center: at(center),
                    axis,
                    radius,
                    from: at(center + peet_math::DVec2::X * radius),
                    sweep: std::f64::consts::TAU,
                },
                peet_sketch::Curve::Arc {
                    center,
                    radius,
                    start_angle,
                    sweep,
                } => Piece::Arc {
                    center: at(center),
                    axis,
                    radius,
                    from: at(center
                        + peet_math::DVec2::new(start_angle.cos(), start_angle.sin()) * radius),
                    sweep,
                },
                peet_sketch::Curve::Spline(_) => return Err(wrong()),
            };
            Ok(moved_piece(piece, rel))
        }
        _ => Err(wrong()),
    }
}

fn moved_piece(piece: Piece, frame: &Frame) -> Piece {
    match piece {
        Piece::Line { from, to } => Piece::Line {
            from: frame.to_world(from),
            to: frame.to_world(to),
        },
        Piece::Arc {
            center,
            axis,
            radius,
            from,
            sweep,
        } => Piece::Arc {
            center: frame.to_world(center),
            axis: frame.vector_to_world(axis),
            radius,
            from: frame.to_world(from),
            sweep,
        },
    }
}

/// The pieces joined end to end into one path, in the order they join, the others turned
/// round as they must be. The path starts with the first piece, from its end away from
/// the rest; a path of one piece runs the way it does.
fn chain_pieces(pieces: Vec<Piece>) -> Result<Vec<Piece>, String> {
    let size = pieces.iter().map(Piece::length).fold(0.0, f64::max);
    let close = |p: DVec3, q: DVec3| p.distance(q) <= 1e-6 * size.max(1.0);
    let mut rest = pieces;
    let mut chain = vec![rest.remove(0)];
    // Where the first piece is in the chain
    let mut first = 0;
    while !rest.is_empty() {
        let (head, tail) = (chain[0].start(), chain[chain.len() - 1].end());
        let found = rest.iter().enumerate().find_map(|(i, p)| {
            if close(p.start(), tail) {
                Some((i, *p, true))
            } else if close(p.end(), tail) {
                Some((i, p.reversed(), true))
            } else if close(p.end(), head) {
                Some((i, *p, false))
            } else if close(p.start(), head) {
                Some((i, p.reversed(), false))
            } else {
                None
            }
        });
        let Some((i, piece, after)) = found else {
            return Err("The pieces of the path mate's path don't join end to end: select edges or sketch curves that make one path.".to_owned());
        };
        rest.remove(i);
        if after {
            chain.push(piece);
        } else {
            chain.insert(0, piece);
            first += 1;
        }
    }
    // The path starts with the first piece selected, where that is at one of its ends
    if first + 1 == chain.len() && chain.len() > 1 {
        chain.reverse();
        for piece in &mut chain {
            *piece = piece.reversed();
        }
    }
    Ok(chain)
}

/// The equations of a path mate: `point` (of the first end, in its component's
/// coordinates) on `chain` (in the second end's), `now` being where the point is in the
/// second's coordinates. `up` is the roll control's axis and its direction's element.
#[allow(clippy::too_many_arguments)]
fn path_equations(
    chain: &[Piece],
    point: DVec3,
    now: DVec3,
    along: &WidthMode,
    follow: Option<PathAxis>,
    up: Option<(PathAxis, Element)>,
    flip: bool,
    params: &Parameters,
) -> Result<Vec<Proto>, String> {
    let lengths: Vec<f64> = chain.iter().map(Piece::length).collect();
    let total: f64 = lengths.iter().sum();
    if total < 1e-9 {
        return Err("The path mate's path has no length.".to_owned());
    }
    // Where along the whole path, from its start (from its end with flip_dimension)
    let target = match along {
        WidthMode::Free => None,
        WidthMode::Centered => Some(total * 0.5),
        WidthMode::Dimension(s) => {
            let at = s.evaluate(ScalarKind::Length, params)?;
            if !(-1e-9..=total + 1e-9).contains(&at) {
                return Err(format!(
                    "The path mate's distance must be between 0 and {total:.3} mm, the path's length."
                ));
            }
            Some(at)
        }
        WidthMode::Percent(s) => {
            let percent = s.evaluate(ScalarKind::Number, params)?;
            if !(0.0..=100.0).contains(&percent) {
                return Err("The path mate's percent must be between 0 and 100.".to_owned());
            }
            Some(total * percent / 100.0)
        }
    }
    .map(|s| if flip { total - s } else { s });
    // The piece the point is on (or goes to), and how far along it
    let (k, s) = match target {
        Some(mut s) => {
            let mut k = 0;
            while k + 1 < chain.len() && s > lengths[k] {
                s -= lengths[k];
                k += 1;
            }
            (k, s.clamp(0.0, lengths[k]))
        }
        None => chain
            .iter()
            .enumerate()
            .map(|(k, p)| {
                let s = p.nearest(now);
                (k, s, p.at(s).0.distance(now))
            })
            .min_by(|x, y| x.2.total_cmp(&y.2))
            .map(|(k, s, _)| (k, s))
            .expect("a path has a piece"),
    };
    let piece = chain[k];
    let (q, tangent) = piece.at(s);
    let mut out = Vec::new();
    // On the piece's line or circle (the first side is the path's, END_B)
    match piece {
        Piece::Line { from, .. } => {
            on_line(&mut out, false, from, tangent, point);
            if target.is_none() {
                // Not past the piece's ends. The piece the point is on is chosen again at
                // every solve (the nearest), so a point held at the end of one piece is
                // on the next too; a drag stops at a joint.
                out.push(Proto {
                    range: Some((0.0, lengths[k])),
                    ..proto(EqKind::Offset, false, tangent, from, DVec3::ZERO, point)
                });
            }
        }
        Piece::Arc {
            center,
            axis,
            radius,
            ..
        } => {
            on_plane(&mut out, false, center, axis, point, 0.0);
            if target.is_some() {
                // At the place on it: the radius out to it (only that place on the
                // circle is that far out along it), then no way along the tangent
                let out_to = (q - center).normalize_or_zero();
                out.push(Proto {
                    target: radius,
                    ..proto(EqKind::Offset, false, out_to, center, DVec3::ZERO, point)
                });
            } else {
                out.push(Proto {
                    target: radius,
                    ..proto(EqKind::LineDistance, false, axis, center, DVec3::ZERO, point)
                });
                // Not past the arc's ends: on its circle, the arc is what is at least
                // r·cos(sweep / 2) out along the radius through its middle
                let span = lengths[k] / radius;
                if span < std::f64::consts::TAU - 1e-9 {
                    let middle = (piece.at(lengths[k] * 0.5).0 - center).normalize_or_zero();
                    out.push(Proto {
                        range: Some((radius * (span * 0.5).cos(), f64::INFINITY)),
                        ..proto(EqKind::Offset, false, middle, center, DVec3::ZERO, point)
                    });
                }
            }
        }
    }
    if target.is_some() {
        // At the place along it: no way along the path's direction there
        out.push(proto(EqKind::Offset, false, tangent, q, DVec3::ZERO, point));
    }
    if let Some(axis) = follow {
        let d = axis.vector();
        match piece {
            Piece::Line { .. } => parallel(&mut out, false, tangent, d),
            Piece::Arc {
                center, axis: n, ..
            } => {
                // In the arc's plane, and square to the radius through the point
                out.push(proto(EqKind::Dot, false, n, DVec3::ZERO, d, DVec3::ZERO));
                out.push(proto(EqKind::Offset, true, d, point, DVec3::ZERO, center));
            }
        }
    }
    if let Some((axis, element)) = up {
        let dir = match element.axis() {
            Element::Plane(_, n) | Element::Line(_, n) => n,
            _ => {
                return Err("A path mate's up vector is a direction: select a flat face, a plane, a straight edge or an axis.".to_owned());
            }
        };
        let mut rolled = Vec::new();
        parallel(&mut rolled, false, dir, axis.vector());
        out.extend(moved_to(rolled, END_B, END_UP));
    }
    Ok(out)
}

/// The index in [`mate_ends`] of a path mate's up vector.
const END_UP: usize = 2;

/// An element mirrored in the plane through `q` with unit normal `m` (all in one
/// component's coordinates).
fn mirrored(e: Element, q: DVec3, m: DVec3) -> Element {
    let point = |p: DVec3| p - 2.0 * m.dot(p - q) * m;
    match e {
        Element::Plane(p, n) => Element::Plane(point(p), reflect(n, m)),
        Element::Line(p, d) => Element::Line(point(p), reflect(d, m)),
        Element::Point(p) => Element::Point(point(p)),
        Element::Circle(p, d) => Element::Circle(point(p), reflect(d, m)),
    }
}

/// Sets each proto's side on `from` (an index of [`mate_ends`]) on `to` instead.
fn moved_to(mut protos: Vec<Proto>, from: usize, to: usize) -> Vec<Proto> {
    for p in &mut protos {
        for on in p.on.iter_mut().take(p.sides) {
            if *on == from {
                *on = to;
            }
        }
    }
    protos
}

/// The equations of a symmetric mate: `a` and `b` mirror images in the plane `s`. Each
/// element is in its own component's coordinates; `same` says which of them are on one
/// component (a and b, a and the plane, b and the plane); `plane_world` is the plane in
/// the assembly, for a plane that doesn't move.
fn symmetric_equations(
    a: Element,
    b: Element,
    s: Element,
    same: [bool; 3],
    plane_world: Option<(DVec3, DVec3)>,
    params: &Parameters,
) -> Result<Vec<Proto>, String> {
    let Element::Plane(q, m) = s else {
        return Err("A symmetric mate is symmetric about a plane: select a flat face or a reference plane for it.".to_owned());
    };
    let (a, b) = (a.axis(), b.axis());
    let [ab, a_plane, b_plane] = same;
    if ab {
        // Both on one component: their middle is in the plane, and what joins them
        // square to it.
        let middle = match (a, b) {
            (Element::Plane(..), Element::Plane(..)) => {
                width_middle(a, b, "symmetric").map(|(middle, _)| middle).map_err(|_| {
                    "Two planes symmetric about a plane, on one component, must be parallel and apart.".to_owned()
                })?
            }
            (Element::Point(pa), Element::Point(pb)) | (Element::Line(pa, _), Element::Line(pb, _)) => {
                let mut join = pb - pa;
                if let Element::Line(_, d) = a {
                    join -= d * d.dot(join);
                }
                let dir = join.try_normalize().ok_or_else(|| {
                    "The two ends of a symmetric mate on one component are in the same place: select two that are apart.".to_owned()
                })?;
                Element::Line((pa + pb) * 0.5, dir)
            }
            _ => {
                return Err("A symmetric mate joins two ends of one kind: two planes, two points or two lines.".to_owned());
            }
        };
        let protos = match middle {
            Element::Plane(..) => equations(&MateKind::Coincident, middle, s, params)?,
            Element::Line(p, d) => {
                let mut out = Vec::new();
                parallel(&mut out, false, m, d);
                on_plane(&mut out, false, q, m, p, 0.0);
                out
            }
            _ => unreachable!("a middle is a plane or a line"),
        };
        return Ok(moved_to(protos, END_B, END_PLANE));
    }
    if a_plane {
        return equations(&MateKind::Coincident, mirrored(a, q, m), b, params);
    }
    if b_plane {
        return equations(&MateKind::Coincident, a, mirrored(b, q, m), params);
    }
    let (q, m) = plane_world.ok_or_else(|| {
        "A symmetric mate's plane must be on one of its two components, or on one that doesn't move (a fixed component or the assembly). Fix the plane's component, or select another plane.".to_owned()
    })?;
    let mut protos = equations(&MateKind::Coincident, a, b, params)?;
    for p in &mut protos {
        p.mirror = Some((q, m));
    }
    Ok(protos)
}
// ---- Solving ----

/// A pull on a component: a point of it towards a place. The component goes as far as
/// its mates let it, turning only if it must.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub component: CompId,
    /// The point, in the component's coordinates.
    pub point: DVec3,
    /// Where it should go, in the assembly's.
    pub to: DVec3,
}

/// How much a drag's pull on its point weighs against a mate (1). Small, so that a
/// step keeps the mates even when the place is out of reach; not so small that the
/// solver's damping swallows it.
const PULL: f64 = 3e-3;

/// What solving the mates of an assembly came to.
pub(crate) struct Solved {
    /// Where each component is now (those the solve moved, and those it didn't).
    pub frames: HashMap<CompId, Frame>,
    pub statuses: HashMap<MateId, Status>,
    /// How many ways the components can still move: six for each that is not fixed,
    /// less what the mates hold.
    pub freedom: usize,
    /// How many ways each component can still move, on its own or along with others.
    pub component_freedom: HashMap<CompId, usize>,
}

/// What is kept between solves of the same mates on the same components, wherever the
/// components are: both are slower to work out than the solve itself.
#[derive(Default)]
pub(crate) struct Memo {
    key: u64,
    /// The freedom the mates leave (together, and for each component by its index), if
    /// they all hold (only then does it depend on which mates there are alone).
    freedom: Option<(usize, Vec<usize>)>,
    /// The solver's structure for each group.
    problems: Vec<Kept>,
}

/// A solver's structure, with the unknowns and equations it is for.
type Kept = (Vec<u32>, Problem);

const HARD: Options = Options {
    max_iter: 200,
    hard: true,
    dogleg: true,
};

/// The equations of every mate that can be set up, and the status of those that can't.
struct System {
    eqs: Vec<Eq>,
    /// The equations of each mate, as a range of `eqs`.
    ranges: Vec<(MateId, std::ops::Range<usize>)>,
}

fn slots_of(index: usize) -> [u32; 6] {
    let base = (index * 6) as u32;
    [base, base + 1, base + 2, base + 3, base + 4, base + 5]
}

/// The values of the unknowns for the components where they are: the origin, and no
/// turn from the base orientation.
fn start_values(components: &[Placed]) -> Vec<f64> {
    let mut vals = Vec::with_capacity(components.len() * 6);
    for c in components {
        vals.extend(c.frame.origin.to_array());
        vals.extend([0.0; 3]);
    }
    vals
}

/// Solves `eqs` (indices into `system.eqs`) for the unknowns of the free components they
/// touch. Returns whether every one of them holds.
///
/// With `kept`, the problem's structure is taken from it if it has one for exactly
/// these equations, and left in `used` for the next solve.
///
/// `exactly` solves them even if they already hold to within the tolerance (after a
/// drag's loose solve, which leaves them a little off).
fn solve_equations(
    system: &System,
    eqs: &[u32],
    components: &[Placed],
    vals: &mut [f64],
    kept: Option<(&mut Vec<Kept>, &mut Vec<Kept>)>,
    exactly: bool,
) -> bool {
    if eqs.is_empty() {
        return true;
    }
    let vars = unknowns(system, eqs, components);
    // The structure of the problem (which equation reads which unknown) is kept from
    // the last solve of the same equations: setting it up costs more than solving.
    let signature = signature(system, &vars, eqs, false);
    let new = |vars| Problem::new(vars, eqs.to_vec(), &system.eqs, Space::Equations);
    let Some((kept, used)) = kept else {
        return solve_problem(&mut new(vars), system, vals, exactly);
    };
    let mut problem = take_kept(kept, used, &signature).unwrap_or_else(|| new(vars));
    let solved = solve_problem(&mut problem, system, vals, exactly);
    used.push((signature, problem));
    solved
}

/// The slots of the free components that `eqs` read (a slot beyond the components is
/// a drag's target: a constant).
fn unknowns(system: &System, eqs: &[u32], components: &[Placed]) -> Vec<u32> {
    let mut vars: Vec<u32> = eqs
        .iter()
        .flat_map(|e| system.eqs[*e as usize].slots().iter().copied())
        .filter(|s| components.get(*s as usize / 6).is_some_and(|c| !c.fixed))
        .collect();
    vars.sort_unstable();
    vars.dedup();
    vars
}

fn solve_problem(problem: &mut Problem, system: &System, vals: &mut [f64], exactly: bool) -> bool {
    let mut residuals = Vec::new();
    problem.residuals(&system.eqs, vals, &mut residuals);
    // Already there: nothing is moved, so an assembly that is solved stays as it is.
    if !exactly && peet_solve::max_abs(&residuals) <= SKIP {
        return true;
    }
    problem.solve(&system.eqs, vals, HARD).converged
}

/// The signature a problem is kept under: its unknowns, its equations and their slots
/// (and which of the two kinds of problem it is).
fn signature(system: &System, vars: &[u32], eqs: &[u32], loose: bool) -> Vec<u32> {
    let mut out = vars.to_vec();
    out.push(if loose { u32::MAX - 1 } else { u32::MAX });
    for e in eqs {
        // Which equation it is, too: the problem keeps the equations' indices.
        out.push(*e);
        out.extend(system.eqs[*e as usize].slots());
    }
    out
}

/// The problem kept under `signature`, from the last solve or from earlier in this one.
fn take_kept(kept: &mut Vec<Kept>, used: &mut Vec<Kept>, signature: &[u32]) -> Option<Problem> {
    for list in [kept, used] {
        if let Some(found) = list.iter().position(|(s, _)| s == signature) {
            return Some(list.swap_remove(found).1);
        }
    }
    None
}

/// Most steps of a drag in one solve, and the most a component turns in one step.
const DRAG_STEPS: usize = 60;
/// The point is there when it is this near (mm): far below what a screen shows, and
/// the next frame of a drag carries on from here.
const DRAG_NEAR: f64 = 5e-4;
/// How far the point is pulled in one step, in mm.
const DRAG_REACH: f64 = 10.0;
const DRAG_TURN: f64 = 0.35;
/// How often a step that doesn't help is halved before the drag gives up.
const DRAG_HALVINGS: usize = 8;

/// A drag: the dragged component's group is moved towards where the pull wants it, in
/// steps. Each step is the least squares answer to the group's mates (`hard`),
/// linearised, with the pull (`soft`) at its small weight; then the mates are solved
/// exactly from there. The steps stop when one no longer brings the point (`point`
/// gives where it is) nearer to where it should go (`to`). `target` is the slot the
/// equations of the pull read the place to pull to from.
///
/// It is tried first without letting the dragged component turn (`turn` are the slots
/// of its rotation), and only then with: so a component that can slide to the place
/// slides, and one that can only get nearer by swinging round swings.
#[allow(clippy::too_many_arguments)]
fn pull(
    system: &System,
    hard: &[u32],
    soft: &[u32],
    turn: [u32; 3],
    scales: &[f64],
    components: &[Placed],
    vals: &mut [f64],
    kept: &mut Vec<Kept>,
    used: &mut Vec<Kept>,
    point: impl Fn(&[f64]) -> DVec3,
    to: DVec3,
    target: usize,
) {
    let miss = |vals: &[f64]| point(vals).distance(to);
    let all: Vec<u32> = hard.iter().chain(soft).copied().collect();
    let hard_vars = unknowns(system, hard, components);
    let exact_signature = signature(system, &hard_vars, hard, false);
    let mut exact = take_kept(kept, used, &exact_signature).unwrap_or_else(|| {
        Problem::new(
            hard_vars.clone(),
            hard.to_vec(),
            &system.eqs,
            Space::Equations,
        )
    });
    // The same, with the dragged component not allowed to turn.
    let mut still_vars = hard_vars;
    still_vars.retain(|v| !turn.contains(v));
    let still_signature = signature(system, &still_vars, hard, false);
    let mut still = take_kept(kept, used, &still_signature)
        .unwrap_or_else(|| Problem::new(still_vars, hard.to_vec(), &system.eqs, Space::Equations));

    let mut dx = Vec::new();
    let mut left = miss(vals);
    let start = left;
    for turning in [false, true] {
        // Sliding got it there, or as good as: it is not turned for the last hair.
        if turning && left < (1e-3 * start).max(1e-6) {
            break;
        }
        let mut vars = unknowns(system, &all, components);
        if !turning {
            vars.retain(|v| !turn.contains(v));
        }
        let loose_signature = signature(system, &vars, &all, true);
        let mut loose = take_kept(kept, used, &loose_signature)
            .unwrap_or_else(|| Problem::new(vars, all.clone(), &system.eqs, Space::Variables));
        for _ in 0..DRAG_STEPS {
            if left < DRAG_NEAR {
                break;
            }
            // Pulled a reach at a time: a pull that is long (and perhaps out of reach)
            // would bend the mates in the step it asks for.
            let from = point(vals);
            let aim = from + (to - from) * (DRAG_REACH / left).min(1.0);
            vals[target..target + 3].copy_from_slice(&aim.to_array());
            if !loose.gauss_newton_step(&system.eqs, vals, &mut dx) {
                break;
            }
            // No component turns far in one step: the step is a straight line, and a
            // turn is not.
            let most = loose
                .vars
                .iter()
                .zip(&dx)
                .filter(|(slot, _)| **slot % 6 >= 3)
                .map(|(slot, d)| d.abs() / scales[*slot as usize / 6])
                .fold(0.0, f64::max);
            let mut part = if most > DRAG_TURN {
                DRAG_TURN / most
            } else {
                1.0
            };
            // The step is halved while it doesn't bring the point nearer (it is a
            // straight line, and overshoots round a curve), or the mates can't be
            // solved from where it ends.
            let before = vals.to_vec();
            let mut nearer = None;
            for _ in 0..DRAG_HALVINGS {
                for (slot, d) in loose.vars.iter().zip(&dx) {
                    vals[*slot as usize] += d * part;
                }
                // The mates, exactly: while sliding, without turning the dragged
                // component if they can be had that way (the step leaves them a little
                // off, and putting that right must not turn it either).
                let stepped = vals.to_vec();
                let mut holds = !turning && still.solve(&system.eqs, vals, HARD).converged;
                if !holds {
                    vals.copy_from_slice(&stepped);
                    holds = exact.solve(&system.eqs, vals, HARD).converged;
                }
                let now = miss(vals);
                if holds && now < left - 1e-9 {
                    nearer = Some(now);
                    break;
                }
                vals.copy_from_slice(&before);
                part *= 0.5;
            }
            match nearer {
                Some(now) => left = now,
                None => break,
            }
        }
        used.push((loose_signature, loose));
    }
    used.push((exact_signature, exact));
    used.push((still_signature, still));
}

/// The group a component is in: the first of the components it is joined to.
fn root(group: &mut [usize], mut i: usize) -> usize {
    while group[i] != i {
        group[i] = group[group[i]];
        i = group[i];
    }
    i
}

/// Below this a pivot counts as nothing: the Jacobian's entries are of order one.
const PIVOT: f64 = 1e-7;

/// Brings `rows` to reduced row echelon form in place. Returns the pivot column of each
/// row that has one, in order.
fn reduce(rows: &mut [Vec<f64>], width: usize) -> Vec<usize> {
    let mut pivots = Vec::new();
    for col in 0..width {
        let rank = pivots.len();
        let Some(pivot) =
            (rank..rows.len()).max_by(|a, b| rows[*a][col].abs().total_cmp(&rows[*b][col].abs()))
        else {
            break;
        };
        if rows[pivot][col].abs() < PIVOT {
            continue;
        }
        rows.swap(rank, pivot);
        let scale = rows[rank][col];
        for x in &mut rows[rank][col..] {
            *x /= scale;
        }
        let lead = rows[rank].clone();
        for (r, row) in rows.iter_mut().enumerate() {
            let factor = row[col];
            if r != rank && factor != 0.0 {
                for (x, l) in row.iter_mut().zip(&lead).skip(col) {
                    *x -= factor * l;
                }
            }
        }
        pivots.push(col);
        if pivots.len() == rows.len() {
            break;
        }
    }
    pivots
}

/// How many ways the components can still move, with the mates that hold (`held`): of
/// them all together, and of each one (by its index).
///
/// Together: six for each free component, less the rank of the mates' Jacobian. Of one
/// component: in how many independent ways it moves among all the motions the mates
/// leave, which is the rank of its six rows of a basis of the Jacobian's null space. A
/// component that only moves along with others counts those motions as its own, so two
/// free components fastened to each other each have six. Each group of mated
/// components is worked out on its own.
fn freedoms(
    system: &System,
    held: &[u32],
    components: &[Placed],
    vals: &[f64],
    group: &mut [usize],
) -> (usize, Vec<usize>) {
    let mut each: Vec<usize> = components
        .iter()
        .map(|c| if c.fixed { 0 } else { 6 })
        .collect();
    let mut total: usize = each.iter().sum();
    // The equations of each group, by the group of a free component they read.
    let mut buckets: Vec<(usize, Vec<u32>)> = Vec::new();
    for e in held {
        // A one-sided travel limit bounds a freedom rather than removing it. Exact
        // zero-clearance width mates use an equality instead and do remove it.
        let eq = &system.eqs[*e as usize];
        if eq.range.is_some() {
            continue;
        }
        let free = eq
            .slots()
            .iter()
            .step_by(6)
            .map(|s| *s as usize / 6)
            .find(|i| !components[*i].fixed);
        let Some(free) = free else {
            continue;
        };
        let of = root(group, free);
        match buckets.iter_mut().find(|(g, _)| *g == of) {
            Some((_, eqs)) => eqs.push(*e),
            None => buckets.push((of, vec![*e])),
        }
    }
    let mut g = [0.0; MAX_SLOTS];
    for (_, eqs) in &buckets {
        // A column for each unknown of the group's free components.
        let mut members: Vec<usize> = eqs
            .iter()
            .flat_map(|e| {
                system.eqs[*e as usize]
                    .slots()
                    .iter()
                    .step_by(6)
                    .map(|s| *s as usize / 6)
            })
            .filter(|i| !components[*i].fixed)
            .collect();
        members.sort_unstable();
        members.dedup();
        let width = members.len() * 6;
        let column = |component: usize| members.binary_search(&component).ok().map(|k| k * 6);
        let mut rows: Vec<Vec<f64>> = eqs
            .iter()
            .map(|e| {
                let eq = &system.eqs[*e as usize];
                eq.eval(vals, &mut g);
                let mut row = vec![0.0; width];
                for (k, slot) in eq.slots().iter().enumerate() {
                    if let Some(start) = column(*slot as usize / 6) {
                        row[start + *slot as usize % 6] += g[k];
                    }
                }
                row
            })
            .collect();
        let pivots = reduce(&mut rows, width);
        total -= pivots.len();
        // The null space: one vector for each column without a pivot, with one in
        // that column and, in each pivot's column, minus that row's entry there.
        let loose: Vec<usize> = (0..width).filter(|c| !pivots.contains(c)).collect();
        for (k, member) in members.iter().enumerate() {
            let mut own: Vec<Vec<f64>> = (k * 6..k * 6 + 6)
                .map(|col| {
                    loose
                        .iter()
                        .map(|f| match pivots.iter().position(|p| *p == col) {
                            Some(row) => -rows[row][*f],
                            None => f64::from(u8::from(col == *f)),
                        })
                        .collect()
                })
                .collect();
            each[*member] = reduce(&mut own, loose.len()).len();
        }
    }
    (total, each)
}

/// A component turned half way round about `axis` through `through`.
fn turned_over(frame: Frame, through: DVec3, axis: DVec3) -> Frame {
    let q = DQuat::from_axis_angle(axis, std::f64::consts::PI);
    Frame {
        origin: through + q * (frame.origin - through),
        rotation: (q * frame.rotation).normalize(),
    }
}

/// Solves the mates. `components` are those that are not suppressed, where they are;
/// `mates` are in the assembly's order. `key` identifies the mates and the components
/// (not where they are): what `memo` remembers is used again while it stays the same.
/// `drag` pulls a component as the mates are solved.
pub(crate) fn solve(
    mates: &[&Mate],
    mut components: Vec<Placed>,
    params: &Parameters,
    key: u64,
    memo: &mut Memo,
    drag: Option<Drag>,
) -> Solved {
    if memo.key != key {
        *memo = Memo {
            key,
            ..Memo::default()
        };
    }
    // The assembly's own planes and origin, for the ends on them: a component that
    // doesn't move.
    let ground = (0..)
        .map(CompId)
        .find(|id| components.iter().all(|c| c.id != *id))
        .expect("an unused id");
    components.push(Placed::assembly(ground));
    let index: HashMap<CompId, usize> = components
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id, i))
        .collect();
    let mut statuses = HashMap::new();

    // ---- What each mate asks for ----
    // Each mate's id, the component each of its ends is on (`mate_ends`), and its
    // equations.
    let mut asked: Vec<(MateId, Vec<usize>, Vec<Proto>)> = Vec::new();
    for mate in mates {
        if mate.suppressed {
            statuses.insert(mate.id, Status::Suppressed);
            continue;
        }
        let find = |end: &MateEnd| match end.component() {
            None => index.get(&ground).copied(),
            Some(c) => index.get(&c).copied(),
        };
        let ends = mate_ends(mate);
        // On a component that is suppressed (or gone): the mate waits for it.
        let Some(on) = ends.iter().map(|e| find(e)).collect::<Option<Vec<usize>>>() else {
            statuses.insert(mate.id, Status::Suppressed);
            continue;
        };
        let (ia, ib) = (on[END_A], on[END_B]);
        let built = (|| {
            if ia == ib && !matches!(mate.kind, MateKind::Symmetric { .. }) {
                return Err(format!(
                    "Both ends are on {}: a mate joins two components.",
                    components[ia].name
                ));
            }
            let element_of = |k: usize| element(ends[k], &components[on[k]]);
            let mut a = element_of(END_A)?;
            let mut b = element_of(END_B)?;
            let protos = match &mate.kind {
                MateKind::Width { a2, b2, mode } => {
                    if a2.geom.is_none() || b2.as_ref().is_some_and(|e| e.geom.is_none()) {
                        return Err("A width mate needs geometry at every end.".to_owned());
                    }
                    let rigid = on[END_A2] == ia && on.get(END_B2).is_none_or(|i| *i == ib);
                    let radius = if b2.is_none() && matches!(b.axis(), Element::Line(..)) {
                        width_axis_radius(&mate.b, &components[ib])?
                    } else {
                        0.0
                    };
                    if rigid {
                        let (middle, width) = width_middle(a, element_of(END_A2)?, "width")?;
                        a = middle;
                        let tab_width;
                        if b2.is_some() {
                            let (middle, span) = width_middle(b, element_of(END_B2)?, "tab")?;
                            b = middle;
                            tab_width = span;
                        } else if !matches!(b.axis(), Element::Line(..)) {
                            return Err("A width mate's single tab end must be an axis: select a cylindrical face, a round edge or a straight edge, or give two tab planes.".to_owned());
                        } else {
                            b = b.axis();
                            tab_width = 2.0 * radius;
                        }
                        width_equations(mode, a, b, width - tab_width, params)?
                    } else {
                        // A pair on two components: weighed face by face.
                        let elements = (0..ends.len())
                            .map(element_of)
                            .collect::<Result<Vec<_>, _>>()?;
                        let world: Vec<Element> = elements
                            .iter()
                            .zip(&on)
                            .map(|(e, i)| e.moved(&components[*i].frame))
                            .collect();
                        width_spread(mode, &elements, &world, radius, params)?
                    }
                }
                MateKind::Slot { mode } => {
                    // Either end may be the slot: the one that is a face of one.
                    let (slot, slot_is_a, pin) = match slot_of(&mate.a, &components[ia]) {
                        Some(slot) => (slot, true, b),
                        None => match slot_of(&mate.b, &components[ib]) {
                            Some(slot) => (slot, false, a),
                            None => {
                                return Err("Neither end is a face of a straight slot: select one of the slot's round ends or flat sides, and the pin's round face or axis.".to_owned());
                            }
                        },
                    };
                    slot_equations(mode, slot, slot_is_a, pin, mate.flip_dimension, params)?
                }
                MateKind::Path {
                    more,
                    along,
                    follow,
                    roll,
                } => {
                    let first_piece = 2 + usize::from(roll.is_some());
                    if on[first_piece..].iter().any(|i| *i != ib) {
                        return Err("A path mate's path must be all on one component: select edges or sketch curves of one.".to_owned());
                    }
                    let mut pieces = vec![path_piece(&mate.b, &components[ib])?];
                    for end in more {
                        pieces.push(path_piece(end, &components[ib])?);
                    }
                    let chain = chain_pieces(pieces)?;
                    let point = match a {
                        Element::Point(p) | Element::Circle(p, _) => p,
                        _ => {
                            return Err("A path mate keeps a point on the path: select a vertex, a sketch point or an origin for its first end.".to_owned());
                        }
                    };
                    // Where the point is now, in the path's component
                    let now = components[ib]
                        .frame
                        .to_local(components[ia].frame.to_world(point));
                    let up = match roll {
                        Some(r) => Some((r.axis, element_of(2)?)),
                        None => None,
                    };
                    path_equations(
                        &chain,
                        point,
                        now,
                        along,
                        *follow,
                        up,
                        mate.flip_dimension,
                        params,
                    )?
                }
                MateKind::Symmetric { .. } => {
                    let is = on[END_PLANE];
                    let s = element_of(END_PLANE)?;
                    let fixed_plane = components[is]
                        .fixed
                        .then(|| match s.moved(&components[is].frame) {
                            Element::Plane(q, m) => Some((q, m)),
                            _ => None,
                        })
                        .flatten();
                    if ia == ib && ia == is {
                        return Err(format!(
                            "Both ends and the plane are on {}: a mate joins two components.",
                            components[ia].name
                        ));
                    }
                    symmetric_equations(
                        a,
                        b,
                        s,
                        [ia == ib, ia == is, ib == is],
                        fixed_plane,
                        params,
                    )?
                }
                _ => {
                    let mut protos = equations(&mate.kind, a, b, params)?;
                    if mate.flip_dimension {
                        for p in &mut protos {
                            let measured = match mate.kind {
                                MateKind::Distance(_) | MateKind::LimitDistance { .. } => {
                                    p.kind == EqKind::Offset
                                }
                                MateKind::Angle(_) | MateKind::LimitAngle { .. } => {
                                    p.kind == EqKind::Dot
                                }
                                _ => false,
                            };
                            if measured {
                                p.target = -p.target;
                                p.range = p.range.map(|(min, max)| (-max, -min));
                            }
                        }
                    }
                    protos
                }
            };
            Ok((a, b, protos))
        })();
        let (a, b, protos) = match built {
            Ok(built) => built,
            Err(message) => {
                statuses.insert(mate.id, Status::Failed(message));
                continue;
            }
        };
        // At exactly perpendicular planes (or an axis exactly normal to its plane),
        // a dot residual has zero rotational derivative. Seed a small turn so the
        // solver can reach either of the nearest equivalent width orientations.
        if matches!(mate.kind, MateKind::Width { .. }) {
            if let (Element::Plane(_, na), Element::Plane(_, nb) | Element::Line(_, nb)) = (a, b) {
                let (wa, wb) = (
                    components[ia].frame.vector_to_world(na),
                    components[ib].frame.vector_to_world(nb),
                );
                let singular = if matches!(b, Element::Plane(..)) {
                    wa.dot(wb).abs() < 1e-8
                } else {
                    wa.dot(wb).abs() > 1.0 - 1e-8
                };
                if singular {
                    let axis = wa
                        .cross(wb)
                        .try_normalize()
                        .unwrap_or_else(|| wa.any_orthonormal_vector());
                    let q = DQuat::from_axis_angle(axis, 1e-3);
                    let target = if !components[ib].fixed {
                        Some(ib)
                    } else if !components[ia].fixed {
                        Some(ia)
                    } else {
                        None
                    };
                    if let Some(i) = target {
                        components[i].frame.rotation =
                            (q * components[i].frame.rotation).normalize();
                    }
                }
            }
        }
        // An angle asked of two directions that are parallel now: the angle's residual
        // has no derivative there, so the solve can't start. Tilt one a little about a
        // slant axis (so that a hinge has some of it), as for a width mate above.
        let angled = match &mate.kind {
            MateKind::Angle(_) => true,
            MateKind::LimitAngle { min, .. } => min
                .evaluate(ScalarKind::Angle, params)
                .is_ok_and(|m| m > 1e-6),
            _ => false,
        };
        if angled
            && let (
                Element::Plane(_, na) | Element::Line(_, na),
                Element::Plane(_, nb) | Element::Line(_, nb),
            ) = (a.axis(), b.axis())
        {
            let (wa, wb) = (
                components[ia].frame.vector_to_world(na),
                components[ib].frame.vector_to_world(nb),
            );
            let target = [ib, ia].into_iter().find(|i| !components[*i].fixed);
            if wa.dot(wb).abs() > 1.0 - 1e-10
                && let Some(i) = target
            {
                let (u, v) = square_to(wa);
                let q = DQuat::from_axis_angle((u + v).normalize(), 1e-3);
                components[i].frame.rotation = (q * components[i].frame.rotation).normalize();
            }
        }
        // Two planes or two axes the wrong way round: turn one over first (see the
        // module docs). Planes go against each other unless asked otherwise; axes
        // whichever way is nearer. A round edge is its axis here.
        let (a, b) = (a.axis(), b.axis());
        let wanted = match (a, b) {
            (Element::Plane(..), Element::Plane(..)) => match mate.align {
                None | Some(Align::AntiAligned) => Some(-1.0),
                Some(Align::Aligned) => Some(1.0),
                Some(Align::Closest) => None,
            },
            (Element::Line(..), Element::Line(..)) => match mate.align {
                Some(Align::AntiAligned) => Some(-1.0),
                Some(Align::Aligned) => Some(1.0),
                None | Some(Align::Closest) => None,
            },
            _ => None,
        };
        if let (
            Element::Plane(pa, na) | Element::Line(pa, na),
            Element::Plane(pb, nb) | Element::Line(pb, nb),
            Some(wanted),
        ) = (a, b, wanted)
            && matches!(
                mate.kind,
                MateKind::Coincident
                    | MateKind::Concentric
                    | MateKind::Distance(_)
                    | MateKind::Parallel
            )
        {
            let (fa, fb) = (components[ia].frame, components[ib].frame);
            let (wa, wb) = (fa.vector_to_world(na), fb.vector_to_world(nb));
            if wa.dot(wb) * wanted < 0.0 {
                let axis = wa
                    .cross(wb)
                    .try_normalize()
                    .unwrap_or_else(|| wb.any_orthonormal_vector());
                if !components[ib].fixed {
                    components[ib].frame = turned_over(fb, fb.to_world(pb), axis);
                } else if !components[ia].fixed {
                    components[ia].frame = turned_over(fa, fa.to_world(pa), axis);
                }
            }
        }
        asked.push((mate.id, on, protos));
    }

    // ---- The equations, from where the components are now ----
    let sides: Vec<(DQuat, f64)> = components
        .iter()
        .map(|c| (c.frame.rotation, c.size()))
        .collect();
    let mut system = System {
        eqs: Vec::new(),
        ranges: Vec::new(),
    };
    for (id, on, protos) in &asked {
        let start = system.eqs.len();
        for p in protos {
            let mut eq = Eq {
                kind: p.kind,
                slots: [0; MAX_SLOTS],
                sides: [Side::FIXED; SIDES],
                count: p.sides,
                coef: p.coef,
                target: p.target,
                range: p.range,
                weight: 1.0,
            };
            for k in 0..p.sides {
                let i = on[p.on[k]];
                eq.slots[6 * k..6 * k + 6].copy_from_slice(&slots_of(i));
                eq.sides[k] = Side {
                    base: sides[i].0,
                    scale: sides[i].1,
                    dir: p.dirs[k],
                    point: p.points[k],
                    mirror: p.mirror.filter(|_| p.on[k] == END_A),
                };
            }
            system.eqs.push(eq);
        }
        system.ranges.push((*id, start..system.eqs.len()));
    }

    // ---- The groups: components joined by mates, a fixed one joining nothing ----
    let mut group: Vec<usize> = (0..components.len()).collect();
    for (_, on, _) in &asked {
        let mut free = on.iter().copied().filter(|i| !components[*i].fixed);
        if let Some(first) = free.next() {
            for other in free {
                let (a, b) = (root(&mut group, first), root(&mut group, other));
                group[a.max(b)] = a.min(b);
            }
        }
    }
    // The mates of each group, in the assembly's order. A mate between fixed
    // components is a group of its own: there is nothing to solve, only to check.
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (k, (_, on, _)) in asked.iter().enumerate() {
        let of = match on.iter().find(|i| !components[**i].fixed) {
            Some(i) => root(&mut group, *i),
            None => components.len() + k,
        };
        match groups.iter_mut().find(|(g, _)| *g == of) {
            Some((_, mates)) => mates.push(k),
            None => groups.push((of, vec![k])),
        }
    }

    // ---- Solve each group: all its mates together, or one at a time if they can't
    // all hold ----
    let mut vals = start_values(&components);
    let mut held: Vec<u32> = Vec::new();
    let mut kept = std::mem::take(&mut memo.problems);
    let mut used = Vec::new();
    let mates_equations = system.eqs.len();

    // ---- A drag: the component is pulled before its group is solved ----
    let mut dragged = None;
    if let Some(drag) = drag
        && let Some(&i) = index.get(&drag.component)
        && !components[i].fixed
    {
        let of = root(&mut group, i);
        let hard: Vec<u32> = groups
            .iter()
            .filter(|(g, _)| *g == of)
            .flat_map(|(_, mates)| mates.iter())
            .flat_map(|k| system.ranges[*k].1.clone().map(|e| e as u32))
            .collect();
        if hard.is_empty() {
            // Nothing holds it: it just goes there.
            let by = drag.to - components[i].frame.to_world(drag.point);
            for (v, d) in vals[i * 6..i * 6 + 3].iter_mut().zip(by.to_array()) {
                *v += d;
            }
        } else {
            // The place to go to, as a component that doesn't move, after the others.
            let ground = components.len();
            vals.extend(drag.to.to_array());
            vals.extend([0.0; 3]);
            let own = Side {
                base: sides[i].0,
                scale: sides[i].1,
                point: drag.point,
                ..Side::FIXED
            };
            let mut slots = [0; MAX_SLOTS];
            slots[..6].copy_from_slice(&slots_of(ground));
            slots[6..12].copy_from_slice(&slots_of(i));
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                let mut sides = [Side::FIXED; SIDES];
                sides[0].dir = axis;
                sides[1] = own;
                system.eqs.push(Eq {
                    kind: EqKind::Offset,
                    slots,
                    sides,
                    count: 2,
                    coef: [0.0; SIDES],
                    target: 0.0,
                    range: None,
                    weight: PULL,
                });
            }
            let soft: Vec<u32> = (mates_equations..system.eqs.len())
                .map(|e| e as u32)
                .collect();
            let (base, scale) = sides[i];
            let point = |vals: &[f64]| {
                let v = &vals[i * 6..i * 6 + 6];
                let turn = DQuat::from_scaled_axis(DVec3::new(v[3], v[4], v[5]) / scale);
                DVec3::new(v[0], v[1], v[2]) + turn * base * drag.point
            };
            let own_slots = slots_of(i);
            let scales: Vec<f64> = sides.iter().map(|s| s.1).collect();
            pull(
                &system,
                &hard,
                &soft,
                [own_slots[3], own_slots[4], own_slots[5]],
                &scales,
                &components,
                &mut vals,
                &mut kept,
                &mut used,
                point,
                drag.to,
                ground * 6,
            );
            dragged = Some(of);
        }
    }

    for (of, mates) in &groups {
        let eqs_of = |k: usize| system.ranges[k].1.clone().map(|e| e as u32);
        let all: Vec<u32> = mates.iter().flat_map(|k| eqs_of(*k)).collect();
        let before = vals.clone();
        if solve_equations(
            &system,
            &all,
            &components,
            &mut vals,
            Some((&mut kept, &mut used)),
            dragged == Some(*of),
        ) {
            for k in mates {
                statuses.insert(system.ranges[*k].0, Status::Ok);
            }
            held.extend(all);
            continue;
        }
        vals = before;
        let mut holding: Vec<u32> = Vec::new();
        for k in mates {
            let before = vals.clone();
            let mut with = holding.clone();
            with.extend(eqs_of(*k));
            if solve_equations(&system, &with, &components, &mut vals, None, false) {
                holding = with;
                statuses.insert(system.ranges[*k].0, Status::Ok);
            } else {
                vals = before;
                statuses.insert(
                    system.ranges[*k].0,
                    Status::Failed(
                        "It can't hold together with the mates above it: the components would have to be in two places at once. Change or delete this mate, or one of those."
                            .to_owned(),
                    ),
                );
            }
        }
        held.extend(holding);
    }
    memo.problems = used;

    // ---- Where the components are now ----
    let all_hold =
        held.len() == mates_equations && statuses.values().all(|s| !matches!(s, Status::Failed(_)));
    let known = memo
        .freedom
        .take()
        .filter(|(_, each)| all_hold && each.len() == components.len());
    let (freedom, each) =
        known.unwrap_or_else(|| freedoms(&system, &held, &components, &vals, &mut group));
    let component_freedom = components
        .iter()
        .zip(&each)
        .map(|(c, f)| (c.id, *f))
        .collect();
    if all_hold {
        memo.freedom = Some((freedom, each));
    }
    let mut frames = HashMap::with_capacity(components.len());
    for (i, c) in components.iter().enumerate() {
        let v = &vals[i * 6..i * 6 + 6];
        let origin = DVec3::new(v[0], v[1], v[2]);
        let w = DVec3::new(v[3], v[4], v[5]) / sides[i].1;
        // One the solve didn't touch is exactly where it was.
        let frame = if w == DVec3::ZERO && origin == c.frame.origin {
            c.frame
        } else {
            Frame {
                origin,
                rotation: (DQuat::from_scaled_axis(w) * c.frame.rotation).normalize(),
            }
        };
        frames.insert(c.id, frame);
    }
    // The assembly itself is no component.
    frames.remove(&ground);
    let mut component_freedom: HashMap<CompId, usize> = component_freedom;
    component_freedom.remove(&ground);
    Solved {
        frames,
        statuses,
        freedom,
        component_freedom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gradients of every kind of equation agree with finite differences, at
    /// rotations that are not small: with two sides, with four (a sum), with a range
    /// inside and outside, and with the first side mirrored.
    #[test]
    fn gradients_match_finite_differences() {
        let side = |seed: f64| Side {
            base: DQuat::from_euler(peet_math::EulerRot::XYZ, 0.3 * seed, -0.7, 1.1 * seed),
            scale: 40.0 + seed,
            dir: DVec3::new(0.3, -0.5 * seed, 0.8).normalize(),
            point: DVec3::new(12.0 * seed, -7.0, 30.0),
            mirror: None,
        };
        let vals = [
            5.0, -3.0, 8.0, 20.0, -35.0, 50.0, // a: moved, and turned well away from base
            -40.0, 22.0, 6.0, -60.0, 15.0, 33.0, //
            7.0, 1.0, -9.0, 12.0, 40.0, -25.0, //
            -3.0, 30.0, 2.0, -18.0, -6.0, 44.0,
        ];
        let mirror = Some((
            DVec3::new(3.0, -1.0, 2.0),
            DVec3::new(0.2, 0.9, -0.4).normalize(),
        ));
        let mut cases = Vec::new();
        for kind in [
            EqKind::Dot,
            EqKind::Offset,
            EqKind::Distance,
            EqKind::LineDistance,
        ] {
            for range in [
                None,
                Some((-1000.0, 1000.0)),
                Some((1000.0, 2000.0)),
                Some((-2000.0, -1000.0)),
            ] {
                for mirror in [None, mirror] {
                    cases.push((kind, 2, range, mirror));
                }
            }
        }
        for count in [2, 3, 4] {
            for mirror in [None, mirror] {
                cases.push((EqKind::Sum, count, None, mirror));
                cases.push((EqKind::Sum, count, Some((1000.0, 2000.0)), mirror));
            }
        }
        for (kind, count, range, mirror) in cases {
            let mut slots = [0; MAX_SLOTS];
            for (i, s) in slots.iter_mut().enumerate() {
                *s = i as u32;
            }
            let mut sides = [side(1.0), side(2.0), side(3.0), side(-1.5)];
            sides[0].mirror = mirror;
            let eq = Eq {
                kind,
                slots,
                sides,
                count,
                coef: [-0.5, 0.75, 1.25, -2.0],
                target: 0.25,
                range,
                weight: 0.5,
            };
            let mut g = [0.0; MAX_SLOTS];
            eq.eval(&vals, &mut g);
            for k in 0..6 * count {
                let h = 1e-5;
                let (mut up, mut down) = (vals, vals);
                up[k] += h;
                down[k] -= h;
                let numeric = (eq.residual(&up) - eq.residual(&down)) / (2.0 * h);
                assert!(
                    (numeric - g[k]).abs() < 1e-7,
                    "{kind:?} ({count} sides, {range:?}, {mirror:?}) slot {k}: {numeric} vs {}",
                    g[k]
                );
            }
        }
    }
}
