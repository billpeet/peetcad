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
//! **Suppression.** A suppressed feature is skipped, as if it were not there, and so is
//! every feature that uses it: what is built on a suppressed feature is suppressed with
//! it, and comes back with it.
//!
//! **Assemblies.** A model that is an assembly has no features. Each of its definitions
//! is rebuilt by an engine of its own, kept here, and only if its model is another than
//! last time; the result is a list of [`Instance`]s, one per body of each component, with
//! the body shared between the instances of one part.
//!
//! **Write-back.** A sketch's solved positions and its resolved plane are stored back
//! into the model, so a saved file opens with every sketch where it was, the solver always
//! starts from the last solution, and a sketch whose plane is lost can still be shown.

use std::collections::HashMap;
use std::sync::Arc;

use peet_kernel::Curve3;
use peet_math::{DQuat, DVec3, Frame, Plane};
use peet_sketch::solver::Solver;
use peet_sketch::{Sketch, expr};
use web_time::Instant;

use crate::assembly::{CompId, DefId};
use crate::extrude::{EndCondition, ExtrudeInput, apply_extrude};
use crate::feature::{
    AxisDef, AxisRef, CoordSystemDef, Feature, FeatureId, FeatureKind, PatternDef, PlaneDef,
    PlaneRef, PointDef, PointRef, ScalarKind,
};
use crate::mate::{self, MateId};
use crate::naming::{Body, find_edge, find_face, find_vertex};
use crate::{Axis, Model, hash, pattern, sheet};

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
    /// Skipped because this feature, which it uses, is suppressed (or is itself skipped
    /// for that reason).
    SuppressedBy(FeatureId),
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

    /// Whether the feature is skipped: suppressed itself, or through a feature it uses.
    pub fn is_suppressed(&self) -> bool {
        matches!(self, Self::Suppressed | Self::SuppressedBy(_))
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

/// One body of one component of an assembly, where it is.
#[derive(Clone, Debug)]
pub struct Instance {
    /// The component, from the assembly's own down through sub-assemblies to the part's:
    /// the first is the component of the assembly that was rebuilt.
    pub path: Vec<CompId>,
    /// The body, in its part's coordinates. Instances of one part share it.
    pub body: Arc<Body>,
    /// Where the part's coordinates are in the assembly.
    pub frame: Frame,
    /// The colour of the part the body belongs to.
    pub color: Option<[u8; 3]>,
}

/// The result of a rebuild.
#[derive(Clone, Debug, Default)]
pub struct Evaluation {
    /// The bodies at the rollback bar. An assembly has none of its own: see
    /// [`Evaluation::instances`].
    pub bodies: Vec<Arc<Body>>,
    /// For an assembly: the bodies of its components, placed.
    pub instances: Vec<Instance>,
    states: HashMap<FeatureId, FeatureState>,
    components: HashMap<CompId, Status>,
    mates: HashMap<MateId, Status>,
    patterns: HashMap<crate::PatternId, Status>,
    /// For an assembly: how many ways its components can still move (six for each that
    /// is not fixed, less what the mates hold).
    pub freedom: usize,
    component_freedom: HashMap<CompId, usize>,
    pub stats: Stats,
}

impl Evaluation {
    /// How a component of an assembly came out.
    pub fn component_status(&self, id: CompId) -> Option<&Status> {
        self.components.get(&id)
    }

    /// How many ways a component of an assembly can still move (0 to 6), on its own or
    /// along with others it is mated to: 0 for one that is fixed or fully held.
    pub fn component_freedom(&self, id: CompId) -> Option<usize> {
        self.component_freedom.get(&id).copied()
    }

    /// How a mate of an assembly came out.
    pub fn mate_status(&self, id: MateId) -> Option<&Status> {
        self.mates.get(&id)
    }

    /// The mates that don't hold, with why.
    pub fn mate_failures(&self) -> impl Iterator<Item = (MateId, &str)> {
        self.mates.iter().filter_map(|(id, s)| match s {
            Status::Failed(m) => Some((*id, m.as_str())),
            _ => None,
        })
    }

    /// How a component pattern of an assembly came out.
    pub fn pattern_status(&self, id: crate::PatternId) -> Option<&Status> {
        self.patterns.get(&id)
    }

    /// The component patterns that can't be worked out, with why.
    pub fn pattern_failures(&self) -> impl Iterator<Item = (crate::PatternId, &str)> {
        self.patterns.iter().filter_map(|(id, s)| match s {
            Status::Failed(m) => Some((*id, m.as_str())),
            _ => None,
        })
    }

    /// The components that need attention, with why.
    pub fn component_problems(&self) -> impl Iterator<Item = (CompId, &str)> {
        self.components
            .iter()
            .filter_map(|(id, s)| s.message().map(|m| (*id, m)))
    }

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

/// What rebuilding an assembly gives.
#[derive(Default)]
struct Assembled {
    instances: Vec<Instance>,
    components: HashMap<CompId, Status>,
    mates: HashMap<MateId, Status>,
    patterns: HashMap<crate::PatternId, Status>,
    freedom: usize,
    component_freedom: HashMap<CompId, usize>,
}

/// A definition of an assembly as last rebuilt.
struct Part {
    /// The model that was rebuilt: nothing is done while the definition still has it.
    model: Arc<Model>,
    engine: Engine,
}

/// Rebuilds models, remembering each feature's last result.
#[derive(Default)]
pub struct Engine {
    cache: HashMap<FeatureId, Cached>,
    /// For an assembly: an engine per definition.
    parts: HashMap<DefId, Part>,
    /// For an assembly: what is kept between solves of the same mates.
    mates: mate::Memo,
    /// A pull on a component, for the next rebuild only.
    drag: Option<mate::Drag>,
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

    /// The bodies entering a feature in the most recent rebuild. Sketch editing uses
    /// this history state so downstream cuts cannot become references into their sketch.
    pub fn bodies_before(&self, model: &Model, id: FeatureId) -> Option<&[Arc<Body>]> {
        let mut bodies: &[Arc<Body>] = &[];
        for feature in model.features() {
            if feature.id == id {
                return Some(bodies);
            }
            if self
                .evaluation
                .state(feature.id)
                .is_some_and(|s| s.status.is_built())
                && let Some(built) = self
                    .cache
                    .get(&feature.id)
                    .and_then(|c| c.bodies.as_deref())
            {
                bodies = built;
            }
        }
        None
    }

    /// Pulls a component of an assembly at the next rebuild (and only that one): a point
    /// of it towards a place, as far as its mates let it go. See [`crate::Drag`].
    pub fn set_drag(&mut self, drag: Option<mate::Drag>) {
        self.drag = drag;
    }

    /// Forgets every remembered result, so the next rebuild recomputes everything.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.parts.clear();
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
                // What it uses, in tree order, so the message names the first.
                let mut parents = feature.kind.dependencies();
                parents.sort_by_key(|p| model.index_of(*p));
                parents
                    .into_iter()
                    .find(|p| run.states.get(p).is_some_and(|s| s.status.is_suppressed()))
                    .map(Status::SuppressedBy)
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
        let assembled = self.assemble(model, &mut run.stats);
        run.stats.ms = start.elapsed().as_secs_f64() * 1000.0;
        self.evaluation = Evaluation {
            bodies: run.bodies,
            instances: assembled.instances,
            states: run.states,
            components: assembled.components,
            mates: assembled.mates,
            patterns: assembled.patterns,
            freedom: assembled.freedom,
            component_freedom: assembled.component_freedom,
            stats: run.stats,
        };
        &self.evaluation
    }

    /// Rebuilds the parts of an assembly (those whose model changed), solves its mates
    /// and places the parts' bodies. A part's solved sketches are written back into its
    /// definition, and the placements the mates come to into the components.
    fn assemble(&mut self, model: &mut Model, stats: &mut Stats) -> Assembled {
        let Some(assembly) = model.assembly() else {
            self.parts.clear();
            self.drag = None;
            self.mates = mate::Memo::default();
            return Assembled::default();
        };
        // ---- The definitions ----
        let mut written = Vec::new();
        for def in assembly.definitions() {
            let part = self.parts.entry(def.id).or_insert_with(|| Part {
                // A model no definition has, so the first rebuild is not skipped.
                model: Arc::new(Model::new()),
                engine: Self::new(),
            });
            if Arc::ptr_eq(&part.model, &def.model) {
                continue;
            }
            let mut rebuilt = (*def.model).clone();
            let done = part.engine.regenerate(&mut rebuilt).stats;
            stats.rebuilt += done.rebuilt;
            stats.reused += done.reused;
            part.model = if rebuilt == *def.model {
                def.model.clone()
            } else {
                let rebuilt = Arc::new(rebuilt);
                written.push((def.id, rebuilt.clone()));
                rebuilt
            };
        }
        self.parts
            .retain(|id, _| assembly.definition(*id).is_some());

        // ---- The mates ----
        // Solved from where the components are, with the bodies of their parts (and, for
        // a sub-assembly, of the parts in it) to find the mates' faces in.
        let parts = &self.parts;
        let place = |c: &crate::Component, frame: Frame| -> Option<mate::Placed> {
            let built = parts.get(&c.definition)?.engine.evaluation();
            let bodies = built
                .bodies
                .iter()
                .map(|b| (Vec::new(), b.clone(), Frame::WORLD))
                .chain(
                    built
                        .instances
                        .iter()
                        .map(|i| (i.path.clone(), i.body.clone(), i.frame)),
                )
                .collect();
            Some(mate::Placed {
                id: c.id,
                name: c.name.clone(),
                frame,
                // A pattern's copy is where its pattern puts it: the mates don't move it.
                fixed: c.fixed || c.pattern.is_some(),
                bodies,
            })
        };
        let placed: Vec<mate::Placed> = assembly
            .components()
            .filter(|c| !c.suppressed)
            .filter_map(|c| place(c, c.placement))
            .collect();
        let mates: Vec<&crate::Mate> = assembly.mates().collect();
        let structure: Vec<(CompId, DefId, bool, bool)> = assembly
            .components()
            .map(|c| {
                (
                    c.id,
                    c.definition,
                    c.fixed || c.pattern.is_some(),
                    c.suppressed,
                )
            })
            .collect();
        let key = hash::of(&(&mates, structure, &model.parameters));
        let drag = self.drag.take();
        let solved = mate::solve(
            &mates,
            placed,
            &model.parameters,
            key,
            &mut self.mates,
            drag,
        );
        // ---- The patterns ----
        // After the mates: each copy goes where its original now is, moved by its place
        // in the pattern. A copy whose original is suppressed is left out with it.
        let mut frames = solved.frames;
        let mut patterns = HashMap::new();
        let mut left_out: Vec<CompId> = Vec::new();
        for pattern in assembly.patterns() {
            let located = |line: &crate::PatternLine| -> Result<(DVec3, DVec3), String> {
                let (origin, direction) = match line {
                    crate::PatternLine::Fixed { origin, direction } => (*origin, *direction),
                    crate::PatternLine::Geom(end) => {
                        let gone = || {
                            "The component its direction is taken from is gone or suppressed: edit the pattern to pick another."
                                .to_owned()
                        };
                        let c = end
                            .component()
                            .and_then(|id| assembly.component(id))
                            .filter(|c| !c.suppressed)
                            .ok_or_else(gone)?;
                        let frame = frames.get(&c.id).copied().unwrap_or(c.placement);
                        let on = place(c, frame).ok_or_else(gone)?;
                        match mate::locate(end, &on)? {
                            mate::Located::Line(p, d) | mate::Located::Plane(p, d) => (p, d),
                            mate::Located::Point(_) => {
                                return Err(
                                    "A corner has no direction: pick a straight edge, a round face or edge (its axis), or a flat face (its normal)."
                                        .to_owned(),
                                );
                            }
                        }
                    }
                };
                let direction = direction.try_normalize().ok_or_else(|| {
                    "The direction has no length: give one such as [1, 0, 0].".to_owned()
                })?;
                Ok((origin, direction))
            };
            let length = |s: &crate::Scalar, kind| s.evaluate(kind, &model.parameters);
            // How each place moves a component from where the original is.
            let moves = (|| -> Result<Vec<([u32; 2], Frame)>, String> {
                let places = pattern.kind.places();
                Ok(match &pattern.kind {
                    crate::PatternKind::Linear { first, second } => {
                        let step = |s: &crate::PatternStep| -> Result<DVec3, String> {
                            let (_, direction) = located(&s.direction)?;
                            let spacing = length(&s.spacing, crate::ScalarKind::Length)?;
                            Ok(direction * spacing * if s.flip { -1.0 } else { 1.0 })
                        };
                        let a = step(first)?;
                        let b = second
                            .as_ref()
                            .map(step)
                            .transpose()?
                            .unwrap_or(DVec3::ZERO);
                        places
                            .into_iter()
                            .map(|place| {
                                let origin = a * f64::from(place[0]) + b * f64::from(place[1]);
                                (
                                    place,
                                    Frame {
                                        origin,
                                        ..Frame::WORLD
                                    },
                                )
                            })
                            .collect()
                    }
                    crate::PatternKind::Circular {
                        axis,
                        angle,
                        count,
                        flip,
                    } => {
                        let (origin, direction) = located(axis)?;
                        let angle = length(angle, crate::ScalarKind::Angle)?;
                        // All the way round: evenly spaced. Else from the first to the
                        // last over the angle.
                        let step = if (angle.abs() - 360.0).abs() < 1e-9 {
                            angle / f64::from((*count).max(1))
                        } else {
                            angle / f64::from(count.saturating_sub(1).max(1))
                        }
                        .to_radians()
                            * if *flip { -1.0 } else { 1.0 };
                        places
                            .into_iter()
                            .map(|place| {
                                let rotation = peet_math::DQuat::from_axis_angle(
                                    direction,
                                    step * f64::from(place[0]),
                                );
                                (
                                    place,
                                    Frame {
                                        origin: origin - rotation * origin,
                                        rotation,
                                    },
                                )
                            })
                            .collect()
                    }
                })
            })();
            match moves {
                Ok(moves) => {
                    for instance in &pattern.instances {
                        let seed = assembly.component(instance.seed);
                        let Some(seed) = seed.filter(|s| !s.suppressed) else {
                            left_out.push(instance.component);
                            continue;
                        };
                        let from = frames.get(&seed.id).copied().unwrap_or(seed.placement);
                        if let Some((_, by)) = moves.iter().find(|(p, _)| *p == instance.place) {
                            let mut to = by.compose(&from);
                            to.rotation = to.rotation.normalize();
                            frames.insert(instance.component, to);
                        }
                    }
                    patterns.insert(pattern.id, Status::Ok);
                }
                Err(why) => {
                    patterns.insert(pattern.id, Status::Failed(why));
                }
            }
        }
        let moved: Vec<(CompId, Frame)> = assembly
            .components()
            .filter_map(|c| {
                let frame = *frames.get(&c.id)?;
                (frame != c.placement).then_some((c.id, frame))
            })
            .collect();

        // ---- The components ----
        let mut instances = Vec::new();
        let mut components = HashMap::new();
        for c in assembly.components() {
            if c.suppressed || left_out.contains(&c.id) {
                components.insert(c.id, Status::Suppressed);
                continue;
            }
            let Some(part) = self.parts.get(&c.definition) else {
                continue;
            };
            let placement = frames.get(&c.id).copied().unwrap_or(c.placement);
            let built = part.engine.evaluation();
            let before = instances.len();
            for body in &built.bodies {
                instances.push(Instance {
                    path: vec![c.id],
                    body: body.clone(),
                    frame: placement,
                    color: c.color.or(part.model.color),
                });
            }
            // A sub-assembly's components, as one rigid thing.
            for inner in &built.instances {
                let mut path = Vec::with_capacity(inner.path.len() + 1);
                path.push(c.id);
                path.extend(&inner.path);
                instances.push(Instance {
                    path,
                    body: inner.body.clone(),
                    frame: placement.compose(&inner.frame),
                    color: c.color.or(inner.color),
                });
            }
            let name = &part.model.name;
            let failed = built.failures().count()
                + built.component_problems().count()
                + built.mate_failures().count();
            let status = if failed > 0 {
                Status::Warning(if part.model.is_assembly() {
                    format!(
                        "{failed} of the components of {name} need attention: open it to see which."
                    )
                } else if failed == 1 {
                    format!("A feature of {name} can't be built: open the part to fix it.")
                } else {
                    format!(
                        "{failed} features of {name} can't be built: open the part to fix them."
                    )
                })
            } else if instances.len() == before {
                Status::Warning(format!(
                    "{name} has no bodies yet, so there is nothing to show."
                ))
            } else {
                Status::Ok
            };
            components.insert(c.id, status);
        }
        if (!written.is_empty() || !moved.is_empty())
            && let Some(assembly) = model.assembly_mut()
        {
            for (id, rebuilt) in written {
                assembly.set_model(id, rebuilt);
            }
            for (id, frame) in moved {
                if let Some(c) = assembly.component_mut(id) {
                    c.placement = frame;
                }
            }
        }
        Assembled {
            instances,
            components,
            mates: solved.statuses,
            patterns,
            freedom: solved.freedom,
            component_freedom: solved.component_freedom,
        }
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
            FeatureKind::Sketch(s) => match ctx.plane(&s.plane).and_then(|plane| {
                let mut sketch = s.sketch.clone();
                for projection in &s.projections {
                    use crate::projection::{Shape, Source, intersect_plane, project_edge};
                    let shape = match &projection.source {
                        Source::Point(p) => Shape::Point(plane.to_plane_coords(ctx.point(p)?)),
                        Source::Plane(p) => intersect_plane(plane, ctx.plane(p)?)?,
                        Source::Edge(e) => {
                            let found = find_edge(ctx.bodies, e).ok_or(
                                "The projected edge no longer exists. Replace its projection.",
                            )?;
                            project_edge(ctx.bodies[found.body].solid.edge(found.id), plane)?
                        }
                    };
                    shape.update(&mut sketch, projection.entity)?;
                }
                Ok((plane, sketch))
            }) {
                Err(e) => fail(
                    e,
                    Output::Sketch {
                        plane: s.placement,
                        definition: SketchStatus::Under,
                    },
                ),
                Ok((plane, projected)) => {
                    let key = hash::of(&(1u8, &projected, params_key, &plane));
                    match self.cache.get(&id).filter(|c| c.key == key) {
                        Some(c) => {
                            if projected != s.sketch || plane != s.placement {
                                write_back = Some((projected.clone(), plane));
                            }
                            run.out_keys.insert(id, c.out_key);
                            reused(&c.state)
                        }
                        None => {
                            let (state, sketch) = solve_sketch(&projected, plane, model, &s.sketch);
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
                    let (plane, sketch) = ctx.sketch_input(e.sketch)?;
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
                    Ok((plane, depth, up_to, sketch))
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
                        self.solid_feature(id, key, run, |bodies| {
                            let out = apply_extrude(&ExtrudeInput {
                                feature: id,
                                bodies,
                                plane: &plane,
                                sketch,
                                params: &e.params,
                                depth,
                                up_to,
                                stamp: key,
                                mirror: false,
                            })?;
                            let warning = lost_sheet(model, bodies, &out, &feature.name);
                            Ok((out, warning))
                        })
                    }
                }
            }
            FeatureKind::BaseFlange(b) => {
                let inputs = (|| {
                    let (plane, sketch) = ctx.sketch_input(b.sketch)?;
                    let settings = sheet::evaluate_settings(&b.settings, &model.parameters)?;
                    let depth = b
                        .depth
                        .evaluate(ScalarKind::Length, &model.parameters)
                        .map_err(|m| format!("Depth: {m}."))?;
                    Ok((plane, sketch, settings, depth))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((plane, sketch, settings, depth)) => {
                        let sketch_key = run.out_keys.get(&b.sketch).copied().unwrap_or(0);
                        let key =
                            hash::of(&(3u8, id, &**b, sketch_key, run.body_key, &settings, depth));
                        self.solid_feature(id, key, run, |bodies| {
                            sheet::apply_base_flange(
                                &sheet::BaseFlangeInput {
                                    feature: id,
                                    model,
                                    plane,
                                    sketch,
                                    def: b,
                                    settings,
                                    depth,
                                    stamp: key,
                                },
                                bodies,
                            )
                        })
                    }
                }
            }
            FeatureKind::EdgeFlange(e) => match sheet::edge_flange_spec(e, &model.parameters) {
                Err(m) => fail(m, Output::None),
                Ok(spec) => {
                    let values = (spec.length, spec.angle, spec.offsets, spec.radius);
                    let key = hash::of(&(4u8, id, &**e, run.body_key, values));
                    self.solid_feature(id, key, run, |bodies| {
                        sheet::apply_edge_flange(id, model, bodies, e, &spec, key)
                    })
                }
            },
            FeatureKind::SheetCut(c) => match ctx.sketch_input(c.sketch) {
                Err(m) => fail(m, Output::None),
                Ok((plane, sketch)) => {
                    let face = match model.sketch(c.sketch).map(|s| &s.plane) {
                        Some(PlaneRef::Face(f)) => Some(f.clone()),
                        _ => None,
                    };
                    let sketch_key = run.out_keys.get(&c.sketch).copied().unwrap_or(0);
                    let key = hash::of(&(5u8, id, sketch_key, run.body_key, &face));
                    self.solid_feature(id, key, run, |bodies| {
                        sheet::apply_sheet_cut(
                            id,
                            model,
                            bodies,
                            face.as_ref(),
                            &plane,
                            sketch,
                            key,
                        )
                    })
                }
            },
            FeatureKind::Hem(h) => match sheet::hem_spec(h, &model.parameters) {
                Err(m) => fail(m, Output::None),
                Ok(spec) => {
                    let values = (spec.length, spec.gap, spec.radius, spec.angle, spec.offsets);
                    let key = hash::of(&(6u8, id, &**h, run.body_key, values));
                    self.solid_feature(id, key, run, |bodies| {
                        sheet::apply_hem(id, model, bodies, h, &spec, key)
                    })
                }
            },
            FeatureKind::SketchedBend(b) => {
                let inputs = (|| {
                    let (plane, sketch) = ctx.sketch_input(b.sketch)?;
                    let angle = ctx.scalar(&b.angle, ScalarKind::Angle, "Angle")?;
                    let radius = b
                        .radius
                        .as_ref()
                        .map(|r| ctx.scalar(r, ScalarKind::Length, "Bend radius"))
                        .transpose()?;
                    Ok((plane, sketch, angle, radius))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((plane, sketch, angle, radius)) => {
                        let face = sheet::sketch_face_ref(model, b.sketch);
                        let sketch_key = run.out_keys.get(&b.sketch).copied().unwrap_or(0);
                        let key = hash::of(&(
                            7u8,
                            id,
                            &**b,
                            sketch_key,
                            run.body_key,
                            &face,
                            angle,
                            radius,
                        ));
                        self.solid_feature(id, key, run, |bodies| {
                            sheet::apply_sketched_bend(
                                id,
                                model,
                                bodies,
                                face.as_ref(),
                                &plane,
                                sketch,
                                b,
                                angle,
                                radius,
                                key,
                            )
                        })
                    }
                }
            }
            FeatureKind::Jog(j) => {
                let inputs = (|| {
                    let (plane, sketch) = ctx.sketch_input(j.sketch)?;
                    let offset = ctx.scalar(&j.offset, ScalarKind::Length, "Offset")?;
                    let angle = ctx.scalar(&j.angle, ScalarKind::Angle, "Angle")?;
                    let radius = j
                        .radius
                        .as_ref()
                        .map(|r| ctx.scalar(r, ScalarKind::Length, "Bend radius"))
                        .transpose()?;
                    Ok((plane, sketch, (offset, angle, radius)))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((plane, sketch, values)) => {
                        let face = sheet::sketch_face_ref(model, j.sketch);
                        let sketch_key = run.out_keys.get(&j.sketch).copied().unwrap_or(0);
                        let key =
                            hash::of(&(8u8, id, &**j, sketch_key, run.body_key, &face, values));
                        self.solid_feature(id, key, run, |bodies| {
                            sheet::apply_jog(
                                id,
                                model,
                                bodies,
                                face.as_ref(),
                                &plane,
                                sketch,
                                j,
                                values,
                                key,
                            )
                        })
                    }
                }
            }
            FeatureKind::MiterFlange(m) => {
                let inputs = (|| {
                    let (plane, sketch) = ctx.sketch_input(m.sketch)?;
                    let gap = ctx.scalar(&m.gap, ScalarKind::Length, "Gap")?;
                    let offsets = [
                        ctx.scalar(&m.offset_start, ScalarKind::Length, "Start offset")?,
                        ctx.scalar(&m.offset_end, ScalarKind::Length, "End offset")?,
                    ];
                    Ok((plane, sketch, gap, offsets))
                })();
                match inputs {
                    Err(e) => fail(e, Output::None),
                    Ok((plane, sketch, gap, offsets)) => {
                        let sketch_key = run.out_keys.get(&m.sketch).copied().unwrap_or(0);
                        let key =
                            hash::of(&(9u8, id, &**m, sketch_key, run.body_key, gap, offsets));
                        self.solid_feature(id, key, run, |bodies| {
                            sheet::apply_miter_flange(
                                id, model, bodies, m, &plane, sketch, gap, offsets, key,
                            )
                        })
                    }
                }
            }
            FeatureKind::Corner(c) => match sheet::corner_spec(c, &model.parameters) {
                Err(m) => fail(m, Output::None),
                Ok(spec) => {
                    let key = hash::of(&(10u8, id, &**c, run.body_key, spec.gap, spec.relief_size));
                    self.solid_feature(id, key, run, |bodies| {
                        sheet::apply_corner(model, bodies, c, spec, key)
                    })
                }
            },
            FeatureKind::Form(f) => {
                let inputs = (|| {
                    let (plane, sketch) = ctx.sketch_input(f.sketch)?;
                    let height = ctx.scalar(&f.height, ScalarKind::Length, "Height")?;
                    Ok((plane, sketch, height))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((plane, sketch, height)) => {
                        let face = sheet::sketch_face_ref(model, f.sketch);
                        let sketch_key = run.out_keys.get(&f.sketch).copied().unwrap_or(0);
                        let key =
                            hash::of(&(11u8, id, &**f, sketch_key, run.body_key, &face, height));
                        self.solid_feature(id, key, run, |bodies| {
                            sheet::apply_form(
                                id,
                                model,
                                bodies,
                                face.as_ref(),
                                &plane,
                                sketch,
                                f,
                                height,
                                key,
                            )
                        })
                    }
                }
            }
            FeatureKind::Pattern(p) => {
                let inputs = (|| {
                    let motions = ctx.pattern_motions(&p.def)?;
                    let seeds = ctx.seeds(&p.seeds)?;
                    Ok((motions, seeds))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((motions, seeds)) => {
                        let key = hash::of(&(
                            12u8,
                            id,
                            &**p,
                            run.body_key,
                            seed_key(model, &p.seeds, &run.out_keys),
                            format!("{motions:?}"),
                        ));
                        self.solid_feature(id, key, run, |bodies| {
                            pattern::apply_copies(id, model, bodies, &seeds, &motions, key)
                        })
                    }
                }
            }
            FeatureKind::Mirror(m) => {
                let inputs = (|| {
                    let plane = ctx
                        .plane(&m.plane)
                        .map_err(|e| format!("Mirror plane: {e}"))?;
                    let seeds = ctx.seeds(&m.seeds)?;
                    Ok((plane, seeds))
                })();
                match inputs {
                    Err(e) => fail(e, Output::None),
                    Ok((plane, seeds)) => {
                        let motions = [pattern::Motion::Mirror {
                            point: plane.origin(),
                            normal: plane.normal(),
                        }];
                        let key = hash::of(&(
                            13u8,
                            id,
                            &**m,
                            run.body_key,
                            seed_key(model, &m.seeds, &run.out_keys),
                            &plane,
                        ));
                        self.solid_feature(id, key, run, |bodies| {
                            pattern::apply_copies(id, model, bodies, &seeds, &motions, key)
                        })
                    }
                }
            }
            FeatureKind::Revolve(r) => {
                let inputs = (|| {
                    let (plane, sketch) = ctx.sketch_input(r.sketch)?;
                    let angle = ctx.scalar(&r.angle, ScalarKind::Angle, "Angle")?;
                    let axis = match &r.axis {
                        crate::RevolveAxisRef::Axis(a) => {
                            Some(ctx.axis(a).map_err(|m| format!("Axis: {m}"))?)
                        }
                        _ => None,
                    };
                    Ok((plane, sketch, angle, axis))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((plane, sketch, angle, axis)) => {
                        let sketch_key = run.out_keys.get(&r.sketch).copied().unwrap_or(0);
                        let key = hash::of(&(
                            14u8,
                            id,
                            &**r,
                            sketch_key,
                            run.body_key,
                            angle,
                            axis.map(|a| (a.origin, a.dir)),
                        ));
                        self.solid_feature(id, key, run, |bodies| {
                            let out = crate::apply_revolve(&crate::RevolveInput {
                                feature: id,
                                bodies,
                                plane: &plane,
                                sketch,
                                def: r,
                                angle,
                                axis,
                                stamp: key,
                                mirror: false,
                            })?;
                            let warning = lost_sheet(model, bodies, &out, &feature.name);
                            Ok((out, warning))
                        })
                    }
                }
            }
            FeatureKind::Blend(b) => match ctx.scalar(&b.size, ScalarKind::Length, "Size") {
                Err(m) => fail(m, Output::None),
                Ok(size) => {
                    let key = hash::of(&(15u8, id, &**b, run.body_key, size));
                    self.solid_feature(id, key, run, |bodies| {
                        let (out, _) =
                            crate::dressup::apply_blend(id, model, bodies, b, size, key)?;
                        let warning = lost_sheet(model, bodies, &out, &feature.name);
                        Ok((out, warning))
                    })
                }
            },
            FeatureKind::Shell(s) => {
                match ctx.scalar(&s.thickness, ScalarKind::Length, "Thickness") {
                    Err(m) => fail(m, Output::None),
                    Ok(thickness) => {
                        let key = hash::of(&(16u8, id, &**s, run.body_key, thickness));
                        self.solid_feature(id, key, run, |bodies| {
                            let (out, _) =
                                crate::dressup::apply_shell(id, model, bodies, s, thickness, key)?;
                            let warning = lost_sheet(model, bodies, &out, &feature.name);
                            Ok((out, warning))
                        })
                    }
                }
            }
            FeatureKind::Draft(d) => {
                let inputs = (|| {
                    let Some(p) = &d.neutral else {
                        return Err("Pick the neutral plane: the part keeps its size where the drafted faces cross it.".to_owned());
                    };
                    let neutral = ctx.plane(p).map_err(|m| format!("Neutral plane: {m}"))?;
                    let angle = ctx.scalar(&d.angle, ScalarKind::Angle, "Angle")?;
                    Ok((neutral, angle))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((neutral, angle)) => {
                        let key = hash::of(&(17u8, id, &**d, run.body_key, &neutral, angle));
                        self.solid_feature(id, key, run, |bodies| {
                            let (out, _) = crate::dressup::apply_draft(
                                model, bodies, d, &neutral, angle, key,
                            )?;
                            let warning = lost_sheet(model, bodies, &out, &feature.name);
                            Ok((out, warning))
                        })
                    }
                }
            }
            FeatureKind::Hole(h) => {
                let inputs = (|| {
                    let (plane, sketch) = ctx.sketch_input(h.sketch)?;
                    Ok((plane, sketch, ctx.hole_sizes(h)?))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((plane, sketch, sizes)) => {
                        let sketch_key = run.out_keys.get(&h.sketch).copied().unwrap_or(0);
                        let key = hash::of(&(18u8, id, &**h, sketch_key, run.body_key, &sizes));
                        self.solid_feature(id, key, run, |bodies| {
                            let (out, warning) =
                                crate::hole::apply_hole(&crate::hole::HoleInput {
                                    feature: id,
                                    bodies,
                                    plane: &plane,
                                    sketch,
                                    def: h,
                                    sizes,
                                    stamp: key,
                                    instance: 0,
                                    mirror: false,
                                })?;
                            let lost = lost_sheet(model, bodies, &out, &feature.name);
                            Ok((out, warning.or(lost)))
                        })
                    }
                }
            }
            FeatureKind::Sweep(s) => {
                let inputs = (|| {
                    let profile = ctx.sketch_input(s.profile)?;
                    let Some(path_id) = s.path else {
                        return Err("Pick the path: a sketch of lines and arcs that starts on the profile's plane.".to_owned());
                    };
                    let path = ctx
                        .sketch_input(path_id)
                        .map_err(|m| m.replace("Its sketch", "Its path"))?;
                    Ok((profile, path, path_id))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok(((plane, sketch), (path_plane, path), path_id)) => {
                        let keys = (
                            run.out_keys.get(&s.profile).copied().unwrap_or(0),
                            run.out_keys.get(&path_id).copied().unwrap_or(0),
                        );
                        let key = hash::of(&(20u8, id, &**s, keys, run.body_key));
                        self.solid_feature(id, key, run, |bodies| {
                            let out = crate::apply_sweep(&crate::SweepInput {
                                feature: id,
                                bodies,
                                profile_plane: &plane,
                                profile: sketch,
                                path_plane: &path_plane,
                                path,
                                def: s,
                                stamp: key,
                            })?;
                            let warning = lost_sheet(model, bodies, &out, &feature.name);
                            Ok((out, warning))
                        })
                    }
                }
            }
            FeatureKind::Loft(l) => {
                let inputs = (|| {
                    let mut sections = Vec::with_capacity(l.sections.len());
                    let mut keys = Vec::with_capacity(l.sections.len());
                    for (k, &s) in l.sections.iter().enumerate() {
                        let (plane, sketch) = ctx
                            .sketch_input(s)
                            .map_err(|m| m.replace("Its sketch", &format!("Profile {}", k + 1)))?;
                        sections.push((plane, sketch, model.name_of(s)));
                        keys.push(run.out_keys.get(&s).copied().unwrap_or(0));
                    }
                    Ok::<_, String>((sections, keys))
                })();
                match inputs {
                    Err(m) => fail(m, Output::None),
                    Ok((sections, keys)) => {
                        let key = hash::of(&(21u8, id, &**l, keys, run.body_key));
                        self.solid_feature(id, key, run, |bodies| {
                            let out = crate::apply_loft(&crate::LoftInput {
                                feature: id,
                                bodies,
                                sections: &sections,
                                def: l,
                                stamp: key,
                            })?;
                            let warning = lost_sheet(model, bodies, &out, &feature.name);
                            Ok((out, warning))
                        })
                    }
                }
            }
            FeatureKind::ConvertToSheet(c) => {
                match crate::convert::convert_settings(c, &model.parameters) {
                    Err(m) => fail(m, Output::None),
                    Ok(settings) => {
                        let key = hash::of(&(22u8, id, &**c, run.body_key, &settings));
                        self.solid_feature(id, key, run, |bodies| {
                            crate::convert::apply_convert(id, model, bodies, c, &settings, key)
                        })
                    }
                }
            }
            FeatureKind::Import(i) => {
                let key = hash::of(&(19u8, id, &**i, run.body_key));
                self.solid_feature(id, key, run, |bodies| {
                    crate::import::apply_import(id, bodies, i, key)
                })
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

impl Engine {
    /// Runs a solid feature with the given key, or reuses its remembered result. If it
    /// succeeds, the bodies it produced are what the next features build on.
    fn solid_feature(
        &mut self,
        id: FeatureId,
        key: u64,
        run: &mut Run,
        compute: impl FnOnce(&[Arc<Body>]) -> Result<sheet::Applied, crate::FeatureError>,
    ) -> FeatureState {
        let hit = self.cache.get(&id).filter(|c| c.key == key);
        let (state, bodies) = match hit {
            Some(c) => (reused(&c.state), c.bodies.clone()),
            None => {
                let (status, output, bodies) = match compute(&run.bodies) {
                    Ok((b, None)) => (Status::Ok, Output::Solid, Some(b)),
                    Ok((b, Some(w))) => (Status::Warning(w), Output::Solid, Some(b)),
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

/// A warning if a solid feature turned a sheet metal body into a plain solid (which loses
/// its flat pattern).
fn lost_sheet(
    model: &Model,
    before: &[Arc<Body>],
    after: &[Arc<Body>],
    name: &str,
) -> Option<String> {
    let lost = before.iter().find(|b| {
        b.sheet.is_some()
            && after
                .iter()
                .any(|a| a.origin == b.origin && a.sheet.is_none())
    })?;
    Some(format!(
        "{name} changed the sheet metal body of {} like a solid, so it no longer has a flat pattern. To keep it, sketch on its face and use a sheet metal cut.",
        model.name_of(lost.origin)
    ))
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
fn solve_sketch(
    sketch: &Sketch,
    plane: Plane,
    model: &Model,
    previous: &Sketch,
) -> (FeatureState, Sketch) {
    let mut sketch = sketch.clone();
    let failures = expr::apply_expressions(&mut sketch, &model.parameters);
    let mut solver = Solver::new();
    let report = solver.solve_with_previous(&mut sketch, previous);
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
struct Ctx<'m, 'r> {
    model: &'m Model,
    bodies: &'r [Arc<Body>],
    states: &'r HashMap<FeatureId, FeatureState>,
}

impl<'m> Ctx<'m, '_> {
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
                Status::Suppressed | Status::SuppressedBy(_) => {
                    Err(format!("{what} ({name}) is suppressed. Unsuppress it."))
                }
                Status::RolledBack => Err(format!("{what} ({name}) is rolled back.")),
            },
        }
    }

    /// A built sketch's plane and geometry, for a feature that uses it.
    fn sketch_input(&self, id: FeatureId) -> Result<(Plane, &'m Sketch), String> {
        let state = self.state_of(id, "Its sketch")?;
        let Output::Sketch { plane, .. } = state.output else {
            return Err(format!("{} is not a sketch.", self.model.name_of(id)));
        };
        let sketch = self
            .model
            .sketch(id)
            .ok_or_else(|| "Its sketch was deleted.".to_owned())?;
        Ok((plane, &sketch.sketch))
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
            AxisRef::Edge(e) => self.axis_def(&AxisDef::Edge(e.clone())),
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

    /// A hole feature's sizes, evaluated.
    fn hole_sizes(&self, h: &crate::HoleFeature) -> Result<crate::HoleSizes, String> {
        let length = |s: &crate::Scalar, label: &str| self.scalar(s, ScalarKind::Length, label);
        let angle = |s: &crate::Scalar, label: &str| self.scalar(s, ScalarKind::Angle, label);
        Ok(crate::HoleSizes {
            diameter: length(&h.diameter, "Diameter")?,
            depth: length(&h.depth, "Depth")?,
            tip_angle: angle(&h.tip_angle, "Drill point angle")?,
            counterbore_diameter: length(&h.counterbore_diameter, "Counterbore diameter")?,
            counterbore_depth: length(&h.counterbore_depth, "Counterbore depth")?,
            countersink_diameter: length(&h.countersink_diameter, "Countersink diameter")?,
            countersink_angle: angle(&h.countersink_angle, "Countersink angle")?,
        })
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
                Ok(match &e.curve {
                    Curve3::Nurbs(_) => {
                        return Err(
                            "The edge it refers to is a freeform curve now, which has no \
                             axis. Pick a straight or a round edge."
                                .to_owned(),
                        );
                    }
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
                // Every round face is turned about an axis (a ball's runs through its poles).
                match self.bodies[found.body]
                    .solid
                    .face(found.id)
                    .surface
                    .revolution_frame()
                {
                    Some(frame) => Ok(Axis {
                        origin: frame.origin,
                        dir: frame.z_axis(),
                    }),
                    None => Err(
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

/// Identifies the copied features' inputs: their definitions and their sketches.
fn seed_key(model: &Model, seeds: &[FeatureId], out_keys: &HashMap<FeatureId, u64>) -> u64 {
    let mut key = 0;
    for &s in seeds {
        let def = model.feature(s).map_or(0, |f| hash::of(&f.kind));
        let sketch = model
            .feature(s)
            .and_then(|f| f.kind.sketch())
            .and_then(|sk| out_keys.get(&sk).copied())
            .unwrap_or(0);
        key = hash::combine(key, hash::combine(def, sketch));
    }
    key
}

impl<'m> Ctx<'m, '_> {
    /// The copied features, resolved.
    fn seeds(&self, ids: &[FeatureId]) -> Result<Vec<(String, pattern::Seed<'m>)>, String> {
        if ids.is_empty() {
            return Err("Pick the features to copy.".to_owned());
        }
        let mut out = Vec::new();
        for &id in ids {
            self.state_of(id, "A copied feature")?;
            let feature = self.model.feature(id).expect("state_of checked it exists");
            let name = feature.name.clone();
            let seed = match &feature.kind {
                FeatureKind::Extrude(e) => {
                    let (plane, sketch) = self.sketch_input(e.sketch)?;
                    let depth = if e.params.end.uses_depth() {
                        self.scalar(&e.params.depth, ScalarKind::Length, "Depth")?
                    } else {
                        0.0
                    };
                    let up_to = match &e.params.end {
                        EndCondition::UpTo(r) => Some(self.plane(r)?),
                        _ => None,
                    };
                    pattern::Seed::Extrude {
                        plane,
                        sketch,
                        params: &e.params,
                        depth,
                        up_to,
                    }
                }
                FeatureKind::SheetCut(c) => {
                    let (plane, sketch) = self.sketch_input(c.sketch)?;
                    pattern::Seed::SheetCut {
                        plane,
                        sketch,
                        face: sheet::sketch_face_ref(self.model, c.sketch),
                    }
                }
                FeatureKind::Revolve(r) => {
                    let (plane, sketch) = self.sketch_input(r.sketch)?;
                    pattern::Seed::Revolve {
                        plane,
                        sketch,
                        def: r,
                        angle: self.scalar(&r.angle, ScalarKind::Angle, "Angle")?,
                        axis: match &r.axis {
                            crate::RevolveAxisRef::Axis(a) => Some(self.axis(a)?),
                            _ => None,
                        },
                    }
                }
                FeatureKind::Hole(h) => {
                    let (plane, sketch) = self.sketch_input(h.sketch)?;
                    pattern::Seed::Hole {
                        plane,
                        sketch,
                        def: h,
                        sizes: self.hole_sizes(h)?,
                    }
                }
                FeatureKind::Form(f) => {
                    let (plane, sketch) = self.sketch_input(f.sketch)?;
                    pattern::Seed::Form {
                        plane,
                        sketch,
                        face: sheet::sketch_face_ref(self.model, f.sketch),
                        def: f,
                        height: self.scalar(&f.height, ScalarKind::Length, "Height")?,
                    }
                }
                other => {
                    return Err(format!(
                        "{name} is a {}, which can't be copied: patterns and mirrors copy extrusions, cuts, revolves, holes, sheet metal cuts and forms.",
                        other.type_name().to_lowercase()
                    ));
                }
            };
            out.push((name, seed));
        }
        Ok(out)
    }

    /// Where a pattern's copies go (the original excluded).
    fn pattern_motions(&self, def: &PatternDef) -> Result<Vec<pattern::Motion>, String> {
        let count = |n: u32, what: &str| {
            if (2..=10_000).contains(&n) {
                Ok(n)
            } else {
                Err(format!(
                    "The {what} count must be between 2 and 10000 (it is {n})."
                ))
            }
        };
        match def {
            PatternDef::Linear { first, second } => {
                let step = |d: &crate::feature::LinearDirection, what: &str| {
                    let axis = self
                        .axis(&d.direction)
                        .map_err(|m| format!("{what}: {m}"))?;
                    let spacing = self.scalar(&d.spacing, ScalarKind::Length, "Spacing")?;
                    let sign = if d.flip { -1.0 } else { 1.0 };
                    Ok::<_, String>((axis.dir * spacing * sign, count(d.count, what)?))
                };
                let (s1, n1) = step(first, "direction")?;
                let (s2, n2) = match second {
                    Some(d) => step(d, "second direction")?,
                    None => (DVec3::ZERO, 1),
                };
                let mut out = Vec::new();
                for j in 0..n2 {
                    for i in 0..n1 {
                        if i == 0 && j == 0 {
                            continue;
                        }
                        out.push(pattern::Motion::translation(
                            s1 * f64::from(i) + s2 * f64::from(j),
                        ));
                    }
                }
                Ok(out)
            }
            PatternDef::Circular {
                axis,
                count: n,
                angle,
                flip,
            } => {
                let a = self.axis(axis).map_err(|m| format!("Axis: {m}"))?;
                let n = count(*n, "copy")?;
                let total = self.scalar(angle, ScalarKind::Angle, "Angle")?;
                if total.abs() < 1e-9 {
                    return Err("The angle must not be zero.".to_owned());
                }
                // All the way round: evenly spaced; otherwise the last copy at the angle.
                let full = (total.abs() - 360.0).abs() < 1e-9;
                let step = if full {
                    total / f64::from(n)
                } else {
                    total / f64::from(n - 1)
                };
                let sign = if *flip { -1.0 } else { 1.0 };
                Ok((1..n)
                    .map(|k| {
                        pattern::Motion::rotation(
                            a.origin,
                            a.dir,
                            (sign * step * f64::from(k)).to_radians(),
                        )
                    })
                    .collect())
            }
        }
    }
}
