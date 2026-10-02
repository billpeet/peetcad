//! Regeneration: turning the feature list into bodies, rebuilding only what changed.
//!
//! **Keys.** Every feature's result is a pure function of its inputs: its own definition,
//! the values of the parameters, the outputs of the features it refers to, and (for solid
//! features) the bodies as they were when its turn came. The engine hashes those inputs
//! into a *key* and remembers each feature's last key and result. On the next rebuild a
//! feature whose key is unchanged takes its remembered result; everything else is
//! recomputed. Nothing needs to be told what changed: editing a dimension changes that
//! sketch's key, which changes the key of the extrusion using it, which changes the
//! key of the body state every later solid feature builds on. Features above the edit keep
//! their keys and cost nothing. So do features below it whose inputs come out the same
//! (a sketch on a face that didn't move).
//!
//! **Failures.** A feature that fails is flagged with a message that says what went wrong
//! and how to fix it, the bodies pass through it unchanged, and the features below still
//! build. Features that refer to a failed one fail too, naming it.
//!
//! **Write-back.** A sketch's solved positions and its resolved plane are stored back
//! into the model, so a saved file opens with every sketch where it was, the solver always
//! starts from the last solution, and a sketch whose plane is lost can still be shown.

use std::collections::HashMap;
use std::sync::Arc;

use peet_kernel::{Curve3, Surface};
use peet_math::{DQuat, DVec3, Frame, Plane};
use peet_sketch::solver::Solver;
use peet_sketch::{Sketch, expr};
use web_time::Instant;

use crate::extrude::{EndCondition, ExtrudeInput, apply_extrude};
use crate::feature::{
    AxisDef, AxisRef, CoordSystemDef, Feature, FeatureId, FeatureKind, PlaneDef, PlaneRef,
    PointDef, PointRef, ScalarKind,
};
use crate::naming::{Body, find_edge, find_face, find_vertex};
use crate::{Axis, Model, hash};

/// How well defined a sketch is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SketchStatus {
    #[default]
    Under,
    Fully,
    Over,
}

impl SketchStatus {
    /// Tree prefix, as in SolidWorks: "(-)" under defined, "(+)" over defined.
    pub fn marker(self) -> &'static str {
        match self {
            Self::Under => "(-) ",
            Self::Fully => "",
            Self::Over => "(+) ",
        }
    }
}

/// How a feature's last rebuild went.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Ok,
    /// Built, but something needs attention.
    Warning(String),
    /// Not built; the message says why and what to do.
    Failed(String),
    Suppressed,
    /// Below the rollback bar.
    RolledBack,
}

impl Status {
    pub fn is_built(&self) -> bool {
        matches!(self, Self::Ok | Self::Warning(_))
    }

    /// The warning or error message.
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Warning(m) | Self::Failed(m) => Some(m),
            _ => None,
        }
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

/// What a feature produced, for other features and the display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Output {
    None,
    /// A sketch on its resolved plane.
    Sketch {
        plane: Plane,
        definition: SketchStatus,
    },
    Plane(Plane),
    Axis(Axis),
    Point(DVec3),
    Frame(Frame),
    /// The feature changed the bodies.
    Solid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeatureState {
    pub status: Status,
    pub output: Output,
    /// Whether the last rebuild recomputed the feature (false: reused, or not built).
    pub rebuilt: bool,
}

/// What the last rebuild did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// Features recomputed.
    pub rebuilt: usize,
    /// Features whose remembered result was reused.
    pub reused: usize,
    pub ms: f64,
}

/// The result of a rebuild.
#[derive(Clone, Debug, Default)]
pub struct Evaluation {
    /// The bodies at the rollback bar.
    pub bodies: Vec<Arc<Body>>,
    states: HashMap<FeatureId, FeatureState>,
    pub stats: Stats,
}

impl Evaluation {
    pub fn state(&self, id: FeatureId) -> Option<&FeatureState> {
        self.states.get(&id)
    }

    pub fn status(&self, id: FeatureId) -> Option<&Status> {
        self.states.get(&id).map(|s| &s.status)
    }

    pub fn output(&self, id: FeatureId) -> Output {
        self.states.get(&id).map_or(Output::None, |s| s.output)
    }

    /// The first failure in tree order, with its feature.
    pub fn failures(&self) -> impl Iterator<Item = (FeatureId, &str)> {
        self.states.iter().filter_map(|(id, s)| match &s.status {
            Status::Failed(m) => Some((*id, m.as_str())),
            _ => None,
        })
    }
}

struct Cached {
    key: u64,
    state: FeatureState,
    /// Identifies the feature's output for the features that use it.
    out_key: u64,
    /// For solid features that succeeded: the bodies after the feature.
    bodies: Option<Vec<Arc<Body>>>,
}

/// Rebuilds models, remembering each feature's last result.
#[derive(Default)]
pub struct Engine {
    cache: HashMap<FeatureId, Cached>,
    evaluation: Evaluation,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// The result of the last [`Engine::regenerate`].
    pub fn evaluation(&self) -> &Evaluation {
        &self.evaluation
    }

    /// Forgets every remembered result, so the next rebuild recomputes everything.
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Rebuilds the model. Only features whose inputs changed since the last call are
    /// recomputed. Solved sketch positions and resolved sketch planes are written back
    /// into the model.
    pub fn regenerate(&mut self, model: &mut Model) -> &Evaluation {
        let start = Instant::now();
        let params_key = hash::of(&model.parameters);
        let built = model.rollback_index();
        let mut run = Run {
            bodies: Vec::new(),
            body_key: 0,
            states: HashMap::with_capacity(model.len()),
            out_keys: HashMap::new(),
            stats: Stats::default(),
        };
        for index in 0..model.len() {
            let feature = model.feature_arc(index);
            let id = feature.id;
            let skipped = if index >= built {
                Some(Status::RolledBack)
            } else if feature.suppressed {
                Some(Status::Suppressed)
            } else {
                None
            };
            if let Some(status) = skipped {
                let output = match &feature.kind {
                    // Still drawn (dimmed) so it can be found and edited.
                    FeatureKind::Sketch(s) => Output::Sketch {
                        plane: s.placement,
                        definition: SketchStatus::Under,
                    },
                    _ => Output::None,
                };
                run.states.insert(
                    id,
                    FeatureState {
                        status,
                        output,
                        rebuilt: false,
                    },
                );
                continue;
            }
            let write_back = self.build(model, &feature, params_key, &mut run);
            if let Some((sketch, placement)) = write_back
                && let Some(s) = model.feature_mut(id).and_then(Feature::sketch_mut)
            {
                s.sketch = sketch;
                s.placement = placement;
            }
        }
        self.cache.retain(|id, _| run.states.contains_key(id));
        run.stats.ms = start.elapsed().as_secs_f64() * 1000.0;
        self.evaluation = Evaluation {
            bodies: run.bodies,
            states: run.states,
            stats: run.stats,
        };
        &self.evaluation
    }

    /// Builds one feature (or reuses its remembered result) and records its state.
    /// Returns a sketch's solved geometry and plane if they must be stored in the model.
    fn build(
        &mut self,
        model: &Model,
        feature: &Feature,
        params_key: u64,
        run: &mut Run,
    ) -> Option<(Sketch, Plane)> {
        let id = feature.id;
        let ctx = Ctx {
            model,
            bodies: &run.bodies,
            states: &run.states,
        };
        let fail = |message: String, output: Output| FeatureState {
            status: Status::Failed(message),
            output,
            rebuilt: false,
        };
        let mut write_back = None;
        let state = match &feature.kind {
            FeatureKind::Sketch(s) => match ctx.plane(&s.plane) {
                Err(e) => fail(
                    e,
                    Output::Sketch {
                        plane: s.placement,
                        definition: SketchStatus::Under,
                    },
                ),
                Ok(plane) => {
                    let key = hash::of(&(1u8, &s.sketch, params_key, &plane));
                    match self.cache.get(&id).filter(|c| c.key == key) {
                        Some(c) => {
                            run.out_keys.insert(id, c.out_key);
                            reused(&c.state)
                        }
                        None => {
                            let (state, sketch) = solve_sketch(&s.sketch, plane, model);
                            let out_key = hash::of(&(&sketch, &plane));
                            // Keyed on the solved sketch: that is what the model holds
                            // from now on.
                            let key = hash::of(&(1u8, &sketch, params_key, &plane));
                            run.out_keys.insert(id, out_key);
                            self.cache.insert(
                                id,
                                Cached {
                                    key,
                                    state: state.clone(),
                                    out_key,
                                    bodies: None,
                                },
                            );
                            if sketch != s.sketch || plane != s.placement {
                                write_back = Some((sketch, plane));
                            }
                            state
                        }
                    }
                }
            },
            FeatureKind::Extrude(e) => {
                let inputs = (|| {
                    let sketch_state = ctx.state_of(e.sketch, "Its sketch")?;
                    let Output::Sketch { plane, .. } = sketch_state.output else {
                        return Err(format!("{} is not a sketch.", model.name_of(e.sketch)));
                    };
                    let depth = if e.params.end.uses_depth() {
                        e.params
                            .depth
                            .evaluate(ScalarKind::Length, &model.parameters)
                            .map_err(|m| format!("Depth: {m}."))?
                    } else {
                        0.0
                    };
                    let up_to = match &e.params.end {
                        EndCondition::UpTo(r) => {
                            Some(ctx.plane(r).map_err(|m| format!("Up to face: {m}"))?)
                        }
                        _ => None,
                    };
                    let sketch = model
                        .sketch(e.sketch)
                        .ok_or_else(|| "Its sketch was deleted.".to_owned())?;
                    Ok((plane, depth, up_to, &sketch.sketch))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((plane, depth, up_to, sketch)) => {
                        let sketch_key = run.out_keys.get(&e.sketch).copied().unwrap_or(0);
                        let key = hash::of(&(
                            2u8,
                            id,
                            &e.params,
                            sketch_key,
                            run.body_key,
                            depth,
                            &up_to,
                        ));
                        let hit = self.cache.get(&id).filter(|c| c.key == key);
                        let (state, bodies) = match hit {
                            Some(c) => (reused(&c.state), c.bodies.clone()),
                            None => {
                                let result = apply_extrude(&ExtrudeInput {
                                    feature: id,
                                    bodies: &run.bodies,
                                    plane: &plane,
                                    sketch,
                                    params: &e.params,
                                    depth,
                                    up_to,
                                    stamp: key,
                                });
                                let (status, output, bodies) = match result {
                                    Ok(b) => (Status::Ok, Output::Solid, Some(b)),
                                    Err(e) => (Status::Failed(e.0), Output::None, None),
                                };
                                let state = FeatureState {
                                    status,
                                    output,
                                    rebuilt: true,
                                };
                                self.cache.insert(
                                    id,
                                    Cached {
                                        key,
                                        state: state.clone(),
                                        out_key: key,
                                        bodies: bodies.clone(),
                                    },
                                );
                                (state, bodies)
                            }
                        };
                        if let Some(b) = bodies {
                            run.bodies = b;
                            run.body_key = key;
                        }
                        state
                    }
                }
            }
            // Reference geometry is a few vector operations: always recomputed.
            FeatureKind::Plane(def) => reference(ctx.plane_def(def).map(Output::Plane)),
            FeatureKind::Axis(def) => reference(ctx.axis_def(def).map(Output::Axis)),
            FeatureKind::Point(def) => reference(ctx.point_def(def).map(Output::Point)),
            FeatureKind::CoordSystem(def) => reference(ctx.coord_system(def).map(Output::Frame)),
        };
        if state.rebuilt {
            run.stats.rebuilt += 1;
        } else if state.status.is_built() {
            run.stats.reused += 1;
        }
        run.states.insert(id, state);
        write_back
    }
}

/// The state of a rebuild in progress.
struct Run {
    bodies: Vec<Arc<Body>>,
    /// Identifies `bodies`: the key of the last solid feature that changed them.
    body_key: u64,
    states: HashMap<FeatureId, FeatureState>,
    out_keys: HashMap<FeatureId, u64>,
    stats: Stats,
}

fn reused(state: &FeatureState) -> FeatureState {
    FeatureState {
        rebuilt: false,
        ..state.clone()
    }
}

fn reference(result: Result<Output, String>) -> FeatureState {
    match result {
        Ok(output) => FeatureState {
            status: Status::Ok,
            output,
            rebuilt: true,
        },
        Err(m) => FeatureState {
            status: Status::Failed(m),
            output: Output::None,
            rebuilt: false,
        },
    }
}

/// Evaluates a sketch's dimension expressions and solves it.
fn solve_sketch(sketch: &Sketch, plane: Plane, model: &Model) -> (FeatureState, Sketch) {
    let mut sketch = sketch.clone();
    let failures = expr::apply_expressions(&mut sketch, &model.parameters);
    let mut solver = Solver::new();
    let report = solver.solve(&mut sketch);
    let analysis = solver.analyze(&sketch);
    let definition = if analysis.is_over_defined() || !report.converged {
        SketchStatus::Over
    } else if analysis.dof == 0 {
        SketchStatus::Fully
    } else {
        SketchStatus::Under
    };
    let status = if let Some((id, error)) = failures.first() {
        let name = sketch
            .constraint(*id)
            .and_then(|c| c.dimension.as_ref())
            .map_or("a dimension", |d| d.name.as_str());
        Status::Warning(format!(
            "The expression of {name} can't be evaluated ({error}), so it keeps its last value. Edit the sketch and correct it."
        ))
    } else if !report.converged {
        Status::Warning(
            "The sketch can't be solved: its relations and dimensions contradict each other. Edit the sketch and remove the ones shown in red."
                .to_owned(),
        )
    } else {
        Status::Ok
    };
    (
        FeatureState {
            status,
            output: Output::Sketch { plane, definition },
            rebuilt: true,
        },
        sketch,
    )
}

/// Resolves references against the features built so far.
struct Ctx<'a> {
    model: &'a Model,
    bodies: &'a [Arc<Body>],
    states: &'a HashMap<FeatureId, FeatureState>,
}

impl Ctx<'_> {
    /// The state of a built feature, or why it can't be used. `what` starts the message
    /// ("Its sketch").
    fn state_of(&self, id: FeatureId, what: &str) -> Result<&FeatureState, String> {
        let Some(feature) = self.model.feature(id) else {
            return Err(format!(
                "{what} was deleted. Edit this feature and pick another."
            ));
        };
        let name = &feature.name;
        match self.states.get(&id) {
            None => Err(format!(
                "{what} ({name}) comes after it in the tree. Drag {name} above it."
            )),
            Some(s) => match &s.status {
                Status::Ok | Status::Warning(_) => Ok(s),
                Status::Failed(_) => {
                    Err(format!("{what} ({name}) failed to rebuild. Fix it first."))
                }
                Status::Suppressed => Err(format!("{what} ({name}) is suppressed. Unsuppress it.")),
                Status::RolledBack => Err(format!("{what} ({name}) is rolled back.")),
            },
        }
    }

    fn plane(&self, r: &PlaneRef) -> Result<Plane, String> {
        match r {
            PlaneRef::Standard(p) => Ok(p.plane()),
            PlaneRef::Feature(id) => match self.state_of(*id, "Its plane")?.output {
                Output::Plane(p) => Ok(p),
                Output::Frame(frame) => Ok(Plane { frame }),
                _ => Err(format!("{} is not a plane.", self.model.name_of(*id))),
            },
            PlaneRef::Face(face) => {
                let found = find_face(self.bodies, face).ok_or_else(|| {
                    format!(
                        "The face it refers to no longer exists ({}). Edit it and pick another face.",
                        self.model.describe_face(&face.name)
                    )
                })?;
                crate::face_sketch_plane(&self.bodies[found.body].solid, found.id).ok_or_else(
                    || {
                        format!(
                            "The face it refers to is no longer flat ({}). Pick a flat face.",
                            self.model.describe_face(&face.name)
                        )
                    },
                )
            }
        }
    }

    fn axis(&self, r: &AxisRef) -> Result<Axis, String> {
        match r {
            AxisRef::Standard(a) => Ok(a.axis()),
            AxisRef::Feature(id) => match self.state_of(*id, "Its axis")?.output {
                Output::Axis(a) => Ok(a),
                Output::Frame(f) => Ok(Axis {
                    origin: f.origin,
                    dir: f.z_axis(),
                }),
                _ => Err(format!("{} is not an axis.", self.model.name_of(*id))),
            },
        }
    }

    fn point(&self, r: &PointRef) -> Result<DVec3, String> {
        match r {
            PointRef::Origin => Ok(DVec3::ZERO),
            PointRef::Feature(id) => match self.state_of(*id, "Its point")?.output {
                Output::Point(p) => Ok(p),
                Output::Frame(f) => Ok(f.origin),
                _ => Err(format!("{} is not a point.", self.model.name_of(*id))),
            },
            PointRef::Vertex(v) => find_vertex(self.bodies, v)
                .map(|f| self.bodies[f.body].solid.vertex(f.id).point)
                .ok_or_else(|| {
                    "The vertex it refers to no longer exists. Edit it and pick another vertex."
                        .to_owned()
                }),
        }
    }

    fn scalar(&self, s: &crate::Scalar, kind: ScalarKind, label: &str) -> Result<f64, String> {
        s.evaluate(kind, &self.model.parameters)
            .map_err(|m| format!("{label}: {m}."))
    }

    fn plane_def(&self, def: &PlaneDef) -> Result<Plane, String> {
        match def {
            PlaneDef::Offset {
                from,
                distance,
                flip,
            } => {
                let base = self.plane(from)?;
                let d = self.scalar(distance, ScalarKind::Length, "Distance")?;
                let d = if *flip { -d } else { d };
                Ok(Plane {
                    frame: Frame {
                        origin: base.origin() + base.normal() * d,
                        ..base.frame
                    },
                })
            }
            PlaneDef::Angled { from, about, angle } => {
                let base = self.plane(from)?;
                let axis = self.axis(about)?;
                let angle = self.scalar(angle, ScalarKind::Angle, "Angle")?;
                let turn = DQuat::from_axis_angle(axis.dir, angle.to_radians());
                Ok(Plane {
                    frame: Frame {
                        origin: axis.origin + turn * (base.origin() - axis.origin),
                        rotation: (turn * base.frame.rotation).normalize(),
                    },
                })
            }
            PlaneDef::Midplane { a, b } => {
                let (a, b) = (self.plane(a)?, self.plane(b)?);
                if a.normal().cross(b.normal()).length() > 1e-9 {
                    return Err(
                        "A mid plane needs two parallel planes or faces; these are at an angle."
                            .to_owned(),
                    );
                }
                let across = b.signed_distance(a.origin());
                Ok(Plane {
                    frame: Frame {
                        origin: a.origin() - b.normal() * (across / 2.0),
                        ..a.frame
                    },
                })
            }
        }
    }

    fn axis_def(&self, def: &AxisDef) -> Result<Axis, String> {
        match def {
            AxisDef::Edge(edge) => {
                let found = find_edge(self.bodies, edge).ok_or_else(|| {
                    "The edge it refers to no longer exists. Edit it and pick another edge."
                        .to_owned()
                })?;
                let e = self.bodies[found.body].solid.edge(found.id);
                Ok(match e.curve {
                    Curve3::Line(l) => Axis {
                        origin: e.point_at_fraction(0.0),
                        dir: l.dir,
                    },
                    // A round edge gives the axis through its centre.
                    Curve3::Circle(c) => Axis {
                        origin: c.frame.origin,
                        dir: c.frame.z_axis(),
                    },
                    Curve3::Ellipse(c) => Axis {
                        origin: c.frame.origin,
                        dir: c.frame.z_axis(),
                    },
                })
            }
            AxisDef::Cylinder(face) => {
                let found = find_face(self.bodies, face).ok_or_else(|| {
                    format!(
                        "The face it refers to no longer exists ({}). Edit it and pick another.",
                        self.model.describe_face(&face.name)
                    )
                })?;
                match self.bodies[found.body].solid.face(found.id).surface {
                    Surface::Cylinder(c) => Ok(Axis {
                        origin: c.axis_origin(),
                        dir: c.axis(),
                    }),
                    Surface::Plane(_) => Err(
                        "The face it refers to is flat now: an axis needs a round face.".to_owned(),
                    ),
                }
            }
            AxisDef::TwoPlanes(a, b) => {
                let (a, b) = (self.plane(a)?, self.plane(b)?);
                let (na, nb) = (a.normal(), b.normal());
                let dir = na.cross(nb);
                let len2 = dir.length_squared();
                if len2 <= 1e-18 {
                    return Err(
                        "The two planes are parallel, so they don't meet in an axis.".to_owned(),
                    );
                }
                let (da, db) = (na.dot(a.origin()), nb.dot(b.origin()));
                Ok(Axis {
                    origin: (nb.cross(dir) * da + dir.cross(na) * db) / len2,
                    dir: dir / len2.sqrt(),
                })
            }
        }
    }

    fn point_def(&self, def: &PointDef) -> Result<DVec3, String> {
        match def {
            PointDef::Vertex(v) => self.point(&PointRef::Vertex(v.clone())),
            PointDef::Coordinates { x, y, z } => Ok(DVec3::new(
                self.scalar(x, ScalarKind::Length, "X")?,
                self.scalar(y, ScalarKind::Length, "Y")?,
                self.scalar(z, ScalarKind::Length, "Z")?,
            )),
        }
    }

    fn coord_system(&self, def: &CoordSystemDef) -> Result<Frame, String> {
        Ok(Frame {
            origin: self.point(&def.origin)?,
            rotation: self.plane(&def.orientation)?.frame.rotation,
        })
    }
}
