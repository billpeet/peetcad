//! In-context references: a part's sketch following another component's geometry in an
//! assembly the part is used in, as SolidWorks' external references ("->" in its tree).
//!
//! A reference is a sketch projection ([`crate::projection::Source::External`]) that names
//! its *context* (the assembly it was made in, by file name), the component that is this
//! part there (the instance it was made in) and what it follows: a face, edge, vertex,
//! plane or sketch entity of another component, by path and persistent reference, as a
//! mate's end. It holds the geometry it last had, in the part's own coordinates, and that
//! is what the part is built from, in its context or not: a part opened on its own, or used
//! in another assembly, keeps its last values (SolidWorks' "out of context").
//!
//! When the context assembly rebuilds, it solves its mates, then works out where each
//! reference's geometry now is in the part's coordinates and, where that changed, writes it
//! into the part (the assembly's copy, or the linked part, whose file is written when the
//! assembly is saved); the part is then rebuilt and the mates solved again, as SolidWorks
//! repeats mates and in-context parts until nothing moves (see `regen`).
//!
//! A locked reference keeps its last values in its context too (SolidWorks' Lock); breaking
//! one turns the projected entity into plain fixed sketch geometry (Break).

use std::sync::Arc;

use peet_kernel::geom::Surface;
use peet_kernel::Curve3;
use peet_math::{DVec2, DVec3, Frame, Plane};
use peet_sketch::EntityId;
use serde::{Deserialize, Serialize};

use crate::assembly::CompId;
use crate::mate::{MateEnd, MateGeom};
use crate::naming::{Body, find_edge, find_face, find_vertex};
use crate::projection::Shape;
use crate::{FeatureId, Model, PlaneRef};

/// Geometry an in-context reference follows, in some coordinates (the part's, once stored).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Geom {
    Point(DVec3),
    /// A straight edge or sketch line, from one end to the other (a round face: its axis).
    Line(DVec3, DVec3),
    Circle {
        center: DVec3,
        normal: DVec3,
        radius: f64,
    },
    /// Counter-clockwise about `normal` from `start` to `end`.
    Arc {
        center: DVec3,
        normal: DVec3,
        start: DVec3,
        end: DVec3,
    },
    /// A flat face (its outward normal) or a reference plane.
    Plane(Plane),
}

impl Geom {
    /// The same geometry seen from the coordinates `frame` is in.
    pub fn moved(&self, frame: &Frame) -> Self {
        let p = |q: DVec3| frame.to_world(q);
        let v = |q: DVec3| frame.vector_to_world(q);
        match *self {
            Self::Point(a) => Self::Point(p(a)),
            Self::Line(a, b) => Self::Line(p(a), p(b)),
            Self::Circle {
                center,
                normal,
                radius,
            } => Self::Circle {
                center: p(center),
                normal: v(normal),
                radius,
            },
            Self::Arc {
                center,
                normal,
                start,
                end,
            } => Self::Arc {
                center: p(center),
                normal: v(normal),
                start: p(start),
                end: p(end),
            },
            Self::Plane(plane) => Self::Plane(Plane {
                frame: frame.compose(&plane.frame),
            }),
        }
    }

    /// Whether two are the same to `tol` (mm, and the same in directions).
    pub fn same(&self, other: &Self, tol: f64) -> bool {
        let p = |a: DVec3, b: DVec3| a.distance(b) <= tol;
        let n = |a: DVec3, b: DVec3| (a - b).length() <= tol * 1e-3;
        match (*self, *other) {
            (Self::Point(a), Self::Point(b)) => p(a, b),
            (Self::Line(a, b), Self::Line(c, d)) => p(a, c) && p(b, d),
            (
                Self::Circle {
                    center: a,
                    normal: m,
                    radius: r,
                },
                Self::Circle {
                    center: b,
                    normal: n2,
                    radius: s,
                },
            ) => p(a, b) && n(m, n2) && (r - s).abs() <= tol,
            (
                Self::Arc {
                    center: a,
                    normal: m,
                    start: s1,
                    end: e1,
                },
                Self::Arc {
                    center: b,
                    normal: n2,
                    start: s2,
                    end: e2,
                },
            ) => p(a, b) && n(m, n2) && p(s1, s2) && p(e1, e2),
            (Self::Plane(a), Self::Plane(b)) => {
                p(a.origin(), b.origin())
                    && n(a.frame.x_axis(), b.frame.x_axis())
                    && n(a.frame.z_axis(), b.frame.z_axis())
            }
            _ => false,
        }
    }

    /// A word for what it is: "point", "line", "circle", "arc" or "plane".
    pub fn word(&self) -> &'static str {
        match self {
            Self::Point(_) => "point",
            Self::Line(..) => "line",
            Self::Circle { .. } => "circle",
            Self::Arc { .. } => "arc",
            Self::Plane(_) => "plane",
        }
    }

    /// What it is seen as on a sketch's plane (both in the same coordinates): a point, a
    /// line, a circle or arc parallel to the sketch (within 1e-4 of a radian, as mated
    /// components are square to each other only as closely as the solve gets them), the
    /// line a circle or arc square to the sketch covers, or where a plane cuts the sketch.
    pub fn project(&self, plane: Plane) -> Result<Shape, String> {
        let uv = |p: DVec3| plane.to_plane_coords(p);
        let along = |center: DVec3, normal: DVec3, points: &[DVec3]| -> Result<Shape, String> {
            // Square to the sketch: the line it covers, along the direction in both planes.
            let w = normal.cross(plane.normal()).normalize_or_zero();
            let w2 = (uv(center + w) - uv(center)).normalize_or_zero();
            let (lo, hi) = points.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| {
                let t = (uv(*q) - uv(center)).dot(w2);
                (lo.min(t), hi.max(t))
            });
            if hi - lo <= 1e-9 || (hi - lo).is_nan() {
                return Err("What it follows is seen edge on and projects to a point.".to_owned());
            }
            Ok(Shape::Line(uv(center) + w2 * lo, uv(center) + w2 * hi))
        };
        let tilt = |normal: DVec3| normal.normalize().cross(plane.normal()).length();
        match *self {
            Self::Point(p) => Ok(Shape::Point(uv(p))),
            Self::Line(a, b) => {
                let (a, b) = (uv(a), uv(b));
                if a.distance(b) <= 1e-9 {
                    return Err(
                        "The line it follows is seen end on: it projects to a point.".to_owned(),
                    );
                }
                Ok(Shape::Line(a, b))
            }
            Self::Circle {
                center,
                normal,
                radius,
            } => {
                if tilt(normal) < 1e-4 {
                    Ok(Shape::Circle(uv(center), radius))
                } else if normal.normalize().dot(plane.normal()).abs() < 1e-4 {
                    let w = normal.cross(plane.normal()).normalize_or_zero();
                    along(center, normal, &[center + w * radius, center - w * radius])
                } else {
                    Err("A circle it follows is tilted to the sketch: it would project to an ellipse.".to_owned())
                }
            }
            Self::Arc {
                center,
                normal,
                start,
                end,
            } => {
                if tilt(normal) < 1e-4 {
                    let (c, a, b) = (uv(center), uv(start), uv(end));
                    Ok(if normal.dot(plane.normal()) > 0.0 {
                        Shape::Arc(c, a, b)
                    } else {
                        Shape::Arc(c, b, a)
                    })
                } else if normal.normalize().dot(plane.normal()).abs() < 1e-4 {
                    let points = arc_points(center, normal, start, end, 64);
                    along(center, normal, &points)
                } else {
                    Err("An arc it follows is tilted to the sketch: it would project to an ellipse.".to_owned())
                }
            }
            Self::Plane(other) => crate::projection::intersect_plane(plane, other),
        }
    }
}

/// Points along an arc, counter-clockwise about `normal` from `start` to `end`.
fn arc_points(center: DVec3, normal: DVec3, start: DVec3, end: DVec3, n: usize) -> Vec<DVec3> {
    let z = normal.normalize();
    let x = (start - center).normalize_or_zero();
    let y = z.cross(x);
    let r = start.distance(center);
    let e = end - center;
    let mut sweep = e.dot(y).atan2(e.dot(x));
    if sweep <= 1e-12 {
        sweep += std::f64::consts::TAU;
    }
    (0..=n)
        .map(|k| {
            let t = sweep * k as f64 / n as f64;
            center + (x * t.cos() + y * t.sin()) * r
        })
        .collect()
}

/// A reference of a part's sketch to another component of an assembly it is used in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct External {
    /// The assembly whose rebuild updates it, by its file name ("Door Assembly.peet"): the
    /// assembly it was made in. Any other assembly the part is in leaves it as it is.
    pub context: String,
    /// The component that is this part in the context (the instance it was made in), by
    /// its path from the context's own components.
    pub component: Vec<CompId>,
    /// What it follows: another component of the context and its face, edge, vertex,
    /// plane or sketch entity, as a mate's end names them (in that part's coordinates).
    pub target: MateEnd,
    /// The names along `component` and along `target`'s path when it was made: what it
    /// refers to, said when the context isn't there.
    pub component_names: Vec<String>,
    pub target_names: Vec<String>,
    /// The geometry as it was the last time its context worked it out, in this part's
    /// coordinates. The part is built from this.
    pub last: Geom,
    /// Kept at `last` in its context too (SolidWorks' locked references).
    #[serde(default)]
    pub locked: bool,
}

impl External {
    /// What it refers to, in words: "Hinge-1 / Pin-1 (edge) in Frame.peet".
    pub fn describe(&self) -> String {
        let what = match &self.target.geom {
            Some(MateGeom::Face(_)) => "face",
            Some(MateGeom::Edge(_)) => "edge",
            Some(MateGeom::Vertex(_)) => "vertex",
            Some(MateGeom::Plane(_)) => "plane",
            Some(MateGeom::Origin) => "origin",
            Some(MateGeom::Sketch(_)) => "sketch entity",
            None => "component",
        };
        format!(
            "{} ({what}) in {}",
            self.target_names.join(" / "),
            self.context
        )
    }
}

/// How an in-context reference stands, as SolidWorks' List External References says.
#[derive(Clone, Debug, PartialEq)]
pub enum RefStatus {
    /// Worked out in this rebuild of its context.
    InContext,
    /// Not in its context (another assembly, or another instance of the part, or the part
    /// on its own): kept as it last was.
    OutOfContext,
    /// Locked: kept as it last was, in its context too.
    Locked,
    /// In its context, but what it follows is gone or can't be projected: kept as it last
    /// was, with why.
    Dangling(String),
}

impl RefStatus {
    pub fn word(&self) -> &'static str {
        match self {
            Self::InContext => "in_context",
            Self::OutOfContext => "out_of_context",
            Self::Locked => "locked",
            Self::Dangling(_) => "dangling",
        }
    }
}

/// An in-context reference of a component's part, as an assembly's rebuild found it.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    /// The component of this assembly whose part has the reference.
    pub component: CompId,
    pub feature: FeatureId,
    pub entity: EntityId,
    pub status: RefStatus,
}

/// The name an assembly is known by as a context: its file name without folder or
/// extension, in lower case. `"C:/x/Door Assembly.peet"` and `"door assembly"` are one.
pub fn context_key(name: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = match file.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("peet") => stem,
        _ => file,
    };
    stem.trim().to_lowercase()
}

impl Model {
    /// Every in-context reference of the part: its sketch, the projected entity, and the
    /// reference.
    pub fn external_references(&self) -> Vec<(FeatureId, EntityId, &External)> {
        let mut out = Vec::new();
        for f in self.features() {
            if let Some(s) = f.sketch() {
                for p in &s.projections {
                    if let crate::projection::Source::External(x) = &p.source {
                        out.push((f.id, p.entity, &**x));
                    }
                }
            }
        }
        out
    }

    /// Locks (or unlocks) the part's in-context references, of `feature` only if given.
    /// Returns how many changed.
    pub fn lock_external(&mut self, feature: Option<FeatureId>, on: bool) -> usize {
        self.change_external(feature, &mut |x| {
            let changed = x.locked != on;
            x.locked = on;
            changed
        })
    }

    /// Breaks the part's in-context references (of `feature` only if given): each projected
    /// entity stays where it is, fixed, as plain sketch geometry. Returns how many.
    pub fn break_external(&mut self, feature: Option<FeatureId>) -> usize {
        let ids: Vec<FeatureId> = self
            .external_references()
            .into_iter()
            .map(|(f, ..)| f)
            .filter(|f| feature.is_none_or(|g| g == *f))
            .collect();
        let mut count = 0;
        for id in ids {
            if let Some(s) = self.feature_mut(id).and_then(crate::Feature::sketch_mut) {
                let before = s.projections.len();
                s.projections
                    .retain(|p| !matches!(p.source, crate::projection::Source::External(_)));
                count += before - s.projections.len();
            }
        }
        count
    }

    /// For an assembly saved under a new name (SolidWorks' Save As): the in-context
    /// references of its parts (and its sub-assemblies' parts) whose context was `old` name
    /// it as `new` from now on. Returns how many.
    pub fn rename_context(&mut self, old: &str, new: &str) -> usize {
        let (old_key, mut count) = (context_key(old), 0);
        let Some(assembly) = self.assembly() else {
            return 0;
        };
        let mut changed: Vec<(crate::DefId, Model)> = Vec::new();
        for d in assembly.definitions() {
            let mut part = (*d.model).clone();
            let n = if part.is_assembly() {
                part.rename_context(old, new)
            } else {
                part.change_external(None, &mut |x| {
                    let mine = context_key(&x.context) == old_key && x.context != new;
                    if mine {
                        x.context = new.to_owned();
                    }
                    mine
                })
            };
            if n > 0 {
                count += n;
                changed.push((d.id, part));
            }
        }
        if let Some(assembly) = self.assembly_mut() {
            for (id, part) in changed {
                assembly.set_model(id, Arc::new(part));
            }
        }
        count
    }

    /// Changes each in-context reference (of `feature` only if given) by `change`, which
    /// says whether it changed it. Returns how many did.
    fn change_external(
        &mut self,
        feature: Option<FeatureId>,
        change: &mut dyn FnMut(&mut External) -> bool,
    ) -> usize {
        let ids: Vec<FeatureId> = self
            .external_references()
            .into_iter()
            .map(|(f, ..)| f)
            .filter(|f| feature.is_none_or(|g| g == *f))
            .collect();
        let mut ids = ids;
        ids.dedup();
        let mut count = 0;
        for id in ids {
            // Only cloned (copy on write) where something changes.
            let wants = self.sketch(id).is_some_and(|s| {
                s.projections.iter().any(|p| match &p.source {
                    crate::projection::Source::External(x) => {
                        let mut copy = (**x).clone();
                        change(&mut copy)
                    }
                    _ => false,
                })
            });
            if !wants {
                continue;
            }
            if let Some(s) = self.feature_mut(id).and_then(crate::Feature::sketch_mut) {
                for p in &mut s.projections {
                    if let crate::projection::Source::External(x) = &mut p.source
                        && change(x)
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }
}

/// Where a face, edge, vertex, plane, origin or sketch entity of a part is, in the
/// coordinates the bodies (each with where it is, `frames`) and the model (at `model_at`)
/// are placed in. `plane_of` gives a reference or standard plane of the part, likewise.
pub(crate) fn geom_in(
    bodies: &[Arc<Body>],
    frames: &[Frame],
    model: Option<(&Model, Frame)>,
    plane_of: &dyn Fn(&PlaneRef) -> Option<Plane>,
    geom: &MateGeom,
) -> Result<Geom, String> {
    let gone = |what: &str| {
        format!("The {what} it follows is gone: the part it is on was changed.")
    };
    let face_geom = |r: &crate::FaceRef| -> Result<Geom, String> {
        let f = find_face(bodies, r).ok_or_else(|| gone("face"))?;
        let body = &bodies[f.body];
        let face = body.solid.face(f.id);
        let local = match &face.surface {
            Surface::Plane(plane) => {
                let mut frame = plane.frame;
                if face.reversed {
                    frame.rotation *= peet_math::DQuat::from_rotation_x(std::f64::consts::PI);
                }
                Geom::Plane(Plane { frame })
            }
            Surface::Cylinder(c) => Geom::Line(c.axis_origin(), c.axis_origin() + c.axis()),
            Surface::Cone(c) => {
                Geom::Line(c.axis_origin(), c.axis_origin() + c.frame.z_axis())
            }
            Surface::Sphere(s) => Geom::Point(s.frame.origin),
            _ => {
                return Err(
                    "The face it follows is neither flat nor round about an axis.".to_owned(),
                );
            }
        };
        Ok(local.moved(&frames[f.body]))
    };
    match geom {
        MateGeom::Face(r) | MateGeom::Plane(PlaneRef::Face(r)) => face_geom(r),
        MateGeom::Edge(r) => {
            let f = find_edge(bodies, r).ok_or_else(|| gone("edge"))?;
            let edge = bodies[f.body].solid.edge(f.id);
            let local = match &edge.curve {
                Curve3::Line(_) => {
                    Geom::Line(edge.point_at_fraction(0.0), edge.point_at_fraction(1.0))
                }
                Curve3::Circle(c) if edge.is_closed() => Geom::Circle {
                    center: c.frame.origin,
                    normal: c.frame.z_axis(),
                    radius: c.radius,
                },
                Curve3::Circle(c) => {
                    // The edge's own direction need not be counter-clockwise about its
                    // circle's axis: the middle says which way it goes.
                    let (a, b, m) = (
                        edge.point_at_fraction(0.0),
                        edge.point_at_fraction(1.0),
                        edge.point_at_fraction(0.5),
                    );
                    let z = c.frame.z_axis();
                    let ccw = (a - c.frame.origin).cross(m - c.frame.origin).dot(z) > 0.0;
                    let (start, end) = if ccw { (a, b) } else { (b, a) };
                    Geom::Arc {
                        center: c.frame.origin,
                        normal: z,
                        start,
                        end,
                    }
                }
                _ => {
                    return Err(
                        "The edge it follows is neither straight nor round.".to_owned(),
                    );
                }
            };
            Ok(local.moved(&frames[f.body]))
        }
        MateGeom::Vertex(r) => {
            let f = find_vertex(bodies, r).ok_or_else(|| gone("corner"))?;
            Ok(Geom::Point(frames[f.body].to_world(bodies[f.body].solid.vertex(f.id).point)))
        }
        MateGeom::Plane(reference) => plane_of(reference)
            .map(Geom::Plane)
            .ok_or_else(|| gone("plane")),
        MateGeom::Origin => plane_of(&PlaneRef::Standard(crate::StdPlane::Top))
            .map(|p| Geom::Point(p.origin()))
            .ok_or_else(|| gone("part")),
        MateGeom::Sketch(reference) => {
            use peet_sketch::Geometry;
            let (model, at) = model.ok_or_else(|| gone("part"))?;
            let sketch = model
                .sketch(reference.feature)
                .ok_or_else(|| gone("sketch"))?;
            let frame = at.compose(&sketch.placement.frame);
            let point = |id: EntityId| -> Result<DVec3, String> {
                sketch
                    .sketch
                    .try_point(id)
                    .map(|p: DVec2| frame.to_world(DVec3::new(p.x, p.y, 0.0)))
                    .ok_or_else(|| gone("sketch point"))
            };
            let entity = sketch
                .sketch
                .entity(reference.entity)
                .ok_or_else(|| gone("sketch entity"))?;
            Ok(match &entity.geometry {
                Geometry::Point { .. } => Geom::Point(point(reference.entity)?),
                Geometry::Line { start, end } => Geom::Line(point(*start)?, point(*end)?),
                Geometry::Circle { center, radius } => Geom::Circle {
                    center: point(*center)?,
                    normal: frame.z_axis(),
                    radius: *radius,
                },
                Geometry::Arc { center, start, end } => Geom::Arc {
                    center: point(*center)?,
                    normal: frame.z_axis(),
                    start: point(*start)?,
                    end: point(*end)?,
                },
                _ => {
                    return Err(
                        "A spline or text of another part's sketch can't be followed: pick a point, line, circle or arc."
                            .to_owned(),
                    );
                }
            })
        }
    }
}

/// Where `geom` of a part is, in that part's own coordinates, from the part as built:
/// `model` and its evaluation's `bodies`, with `plane_of` its reference planes. For
/// operations that make a reference.
pub fn geom_in_part(
    model: &Model,
    bodies: &[Arc<Body>],
    plane_of: &dyn Fn(&PlaneRef) -> Option<Plane>,
    geom: &MateGeom,
) -> Result<Geom, String> {
    let frames = vec![Frame::WORLD; bodies.len()];
    geom_in(bodies, &frames, Some((model, Frame::WORLD)), plane_of, geom)
}

/// Where a mate end on a component is in the assembly, as the solve placed it.
pub(crate) fn locate(end: &MateEnd, placed: &crate::mate::Placed) -> Result<Geom, String> {
    let inner = end.path.get(1..).unwrap_or_default();
    let geom = end
        .geom
        .as_ref()
        .ok_or("It follows a whole component: name a face, edge, vertex, plane or sketch entity.")?;
    let (bodies, frames): (Vec<Arc<Body>>, Vec<Frame>) = placed
        .bodies
        .iter()
        .filter(|(path, ..)| path == inner)
        .map(|(_, body, rel)| (body.clone(), *rel))
        .unzip();
    let model = placed
        .models
        .iter()
        .find(|(path, ..)| path == inner)
        .map(|(_, model, rel)| (&**model, *rel));
    let plane_of = |r: &PlaneRef| {
        placed
            .planes
            .iter()
            .find(|(path, p, _)| path == inner && p == r)
            .map(|(_, _, plane)| *plane)
    };
    Ok(geom_in(&bodies, &frames, model, &plane_of, geom)?.moved(&placed.frame))
}

/// The in-context references of the assembly's components whose context is this assembly
/// (`context`, its name as [`context_key`] has it): for each, where what it follows is now
/// in its part's coordinates. `model_of` gives a definition's model as it is now, `at` a
/// component of this assembly where the mates and patterns put it (none if suppressed), and
/// `frame_of` only where it is.
/// Returns the definitions whose references moved, changed, and every reference's status.
pub(crate) fn update(
    assembly: &crate::Assembly,
    context: Option<&str>,
    model_of: &dyn Fn(crate::DefId) -> Option<Arc<Model>>,
    at: &dyn Fn(CompId) -> Option<crate::mate::Placed>,
    frame_of: &dyn Fn(CompId) -> Option<Frame>,
) -> (Vec<(crate::DefId, Model)>, Vec<Resolved>) {
    let mut changed: Vec<(crate::DefId, Model)> = Vec::new();
    let mut statuses = Vec::new();
    let here = context.map(context_key);
    for c in assembly.components() {
        let Some(model) = model_of(c.definition) else {
            continue;
        };
        if model.is_assembly() {
            continue;
        }
        let references = model.external_references();
        if references.is_empty() {
            continue;
        }
        let mut moved: Vec<(FeatureId, EntityId, Geom)> = Vec::new();
        for (feature, entity, x) in references {
            let mine = here.as_deref() == Some(context_key(&x.context).as_str())
                && x.component.as_slice() == [c.id];
            let status = if !mine || c.suppressed {
                RefStatus::OutOfContext
            } else if x.locked {
                RefStatus::Locked
            } else {
                let found = (|| -> Result<Geom, String> {
                    let target = x.target.component().ok_or(
                        "It follows the assembly itself, which has no geometry of its own to follow.",
                    )?;
                    if target == c.id {
                        return Err("It follows its own component: refer to the part's own geometry instead.".to_owned());
                    }
                    let other = at(target).ok_or_else(|| {
                        format!(
                            "{} is gone or suppressed.",
                            x.target_names.first().map_or("The component it follows", |n| n.as_str())
                        )
                    })?;
                    let own = frame_of(c.id).ok_or("Its own component is not placed.")?;
                    let geom = locate(&x.target, &other)?;
                    let local = geom.moved(&own.inverse());
                    // It must still project into the sketch.
                    if let Some(s) = model.sketch(feature) {
                        local.project(s.placement)?;
                    }
                    Ok(local)
                })();
                match found {
                    Ok(geom) => {
                        if !geom.same(&x.last, 1e-9) {
                            moved.push((feature, entity, geom));
                        }
                        RefStatus::InContext
                    }
                    Err(why) => RefStatus::Dangling(why),
                }
            };
            statuses.push(Resolved {
                component: c.id,
                feature,
                entity,
                status,
            });
        }
        if moved.is_empty() {
            continue;
        }
        let def = c.definition;
        let index = match changed.iter().position(|(d, _)| *d == def) {
            Some(i) => i,
            None => {
                changed.push((def, (*model).clone()));
                changed.len() - 1
            }
        };
        let part = &mut changed[index].1;
        for (feature, entity, geom) in moved {
            if let Some(s) = part.feature_mut(feature).and_then(crate::Feature::sketch_mut) {
                for p in &mut s.projections {
                    if p.entity == entity
                        && let crate::projection::Source::External(x) = &mut p.source
                    {
                        x.last = geom;
                    }
                }
            }
        }
    }
    (changed, statuses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_names_compare_by_file_stem() {
        assert_eq!(context_key("C:\\jobs\\Door Assembly.peet"), "door assembly");
        assert_eq!(context_key("../Door Assembly.PEET"), "door assembly");
        assert_eq!(context_key("Door Assembly"), "door assembly");
    }

    #[test]
    fn a_circle_square_to_the_sketch_projects_to_the_line_it_covers() {
        let circle = Geom::Circle {
            center: DVec3::new(10.0, 0.0, 5.0),
            normal: DVec3::X,
            radius: 3.0,
        };
        let Shape::Line(a, b) = circle.project(Plane::TOP).unwrap() else {
            panic!("a line")
        };
        assert!((a.distance(b) - 6.0).abs() < 1e-9);
        assert!((a.x - 10.0).abs() < 1e-9 && (b.x - 10.0).abs() < 1e-9);
    }

    #[test]
    fn an_arc_seen_from_behind_runs_the_other_way() {
        let arc = Geom::Arc {
            center: DVec3::ZERO,
            normal: DVec3::Z,
            start: DVec3::X * 5.0,
            end: DVec3::Y * 5.0,
        };
        let below = Plane::from_origin_normal_x(DVec3::ZERO, -DVec3::Z, DVec3::X).unwrap();
        let Shape::Arc(_, a, b) = arc.project(below).unwrap() else {
            panic!("an arc")
        };
        assert!(a.abs_diff_eq(DVec2::new(0.0, -5.0), 1e-9), "{a}");
        assert!(b.abs_diff_eq(DVec2::new(5.0, 0.0), 1e-9), "{b}");
    }
}
