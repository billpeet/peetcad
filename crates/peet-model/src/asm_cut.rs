//! Assembly cuts: SolidWorks' assembly feature cut (an extruded cut made in the assembly,
//! with its own sketch).
//!
//! As in SolidWorks, an assembly cut removes material from the components *in the
//! assembly* only: their parts (and every other assembly that uses them) are unchanged.
//! Its feature scope is components: every component it reaches, or the ones listed.
//! The sketch lies on a plane of the assembly or on a flat face or plane of a component
//! (it moves with the component), and some of its entities may be converted from a
//! component's edges, following them as SolidWorks' converted entities do.
//!
//! The cut is worked out after the mates and patterns: each component instance it reaches
//! gets a body of its own, the part's body less the cut, which is what is drawn, weighed,
//! checked for interference and exported. Mates are solved on the parts' own bodies.

use std::sync::Arc;

use peet_math::Plane;
use peet_sketch::{EntityId, Sketch};
use serde::{Deserialize, Serialize};

use crate::assembly::CompId;
use crate::extrude::{Extrude, Operation};
use crate::mate::MateEnd;

/// Identifies an assembly cut within its assembly. Ids are not reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CutId(pub u32);

/// What an assembly cut's sketch lies on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CutPlane {
    /// A plane fixed in the assembly's coordinates.
    Fixed(Plane),
    /// One of the assembly's standard planes (an end with no component), or a flat face or
    /// reference plane of a component, which the sketch then follows.
    Geom(MateEnd),
}

/// An entity of the sketch converted from a component's edge: kept where the edge is,
/// seen on the sketch's plane, at every rebuild.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CutConversion {
    /// The sketch's entity; `None` until the first rebuild adds it (where the edge is then).
    pub entity: Option<EntityId>,
    /// The component and the edge of its part.
    pub edge: MateEnd,
}

/// An extruded cut made in an assembly, with its sketch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssemblyCut {
    pub id: CutId,
    /// Unique in the assembly: "Cut-Extrude1".
    pub name: String,
    /// The name of its sketch: "Sketch1".
    pub sketch_name: String,
    pub plane: CutPlane,
    /// Where the plane was the last time it could be resolved.
    pub placement: Plane,
    /// In the plane's coordinates.
    pub sketch: Sketch,
    pub conversions: Vec<CutConversion>,
    /// How far it goes and which way: an extrusion whose operation is a cut.
    pub params: Extrude,
    /// The components it may cut (SolidWorks' feature scope); `None`: every one it reaches.
    pub scope: Option<Vec<CompId>>,
    pub suppressed: bool,
}

impl AssemblyCut {
    pub fn new(id: CutId, plane: CutPlane, placement: Plane) -> Self {
        Self {
            id,
            name: String::new(),
            sketch_name: String::new(),
            plane,
            placement,
            sketch: Sketch::new(),
            conversions: Vec::new(),
            params: Extrude::new(Operation::Cut),
            scope: None,
            suppressed: false,
        }
    }

    /// The components it names: those of its scope, its plane's and its conversions'.
    pub fn components(&self) -> Vec<CompId> {
        let mut out: Vec<CompId> = self.scope.clone().unwrap_or_default();
        if let CutPlane::Geom(end) = &self.plane {
            out.extend(end.component());
        }
        out.extend(self.conversions.iter().filter_map(|c| c.edge.component()));
        out
    }
}

/// An edge seen on a sketch's plane (both in the edge's body's coordinates). Components
/// placed by mates are square to each other only as closely as the solve gets them: a
/// circle within 1e-4 of a radian of parallel to the sketch is taken as parallel (its
/// centre and radius), as SolidWorks converts it.
fn convert_edge(
    edge: &peet_kernel::topo::Edge,
    plane: Plane,
) -> Result<crate::projection::Shape, String> {
    use crate::projection::Shape;
    if let peet_kernel::Curve3::Circle(c) = &edge.curve {
        let tilt = c.frame.z_axis().normalize().cross(plane.normal().normalize()).length();
        if tilt < 1e-4 {
            let uv = |p| plane.to_plane_coords(p);
            let center = uv(c.frame.origin);
            if edge.is_closed() {
                return Ok(Shape::Circle(center, c.radius));
            }
            let (a, b, mid) = (
                uv(edge.point_at_fraction(0.0)),
                uv(edge.point_at_fraction(1.0)),
                uv(edge.point_at_fraction(0.5)),
            );
            let angle = |p: peet_math::DVec2| (p - center).y.atan2((p - center).x);
            let sweep = (angle(b) - angle(a)).rem_euclid(std::f64::consts::TAU);
            return Ok(
                if (angle(mid) - angle(a)).rem_euclid(std::f64::consts::TAU) < sweep {
                    Shape::Arc(center, a, b)
                } else {
                    Shape::Arc(center, b, a)
                },
            );
        }
    }
    crate::projection::project_edge(edge, plane)
}

/// Where the sketch of a cut is in the assembly, and the sketch with its converted
/// entities where their edges are now, solved.
fn resolve(
    cut: &AssemblyCut,
    params: &peet_sketch::expr::Parameters,
    locate: &dyn Fn(CompId) -> Option<crate::mate::Placed>,
) -> Result<(Plane, Sketch, Vec<CutConversion>), String> {
    let gone = |what: &str| {
        format!("The component {what} is gone or suppressed: edit the cut to pick another.")
    };
    let placed = |end: &MateEnd, what: &str| -> Result<crate::mate::Placed, String> {
        match end.component() {
            None => Ok(crate::mate::Placed::assembly(CompId(u32::MAX))),
            Some(c) => locate(c).ok_or_else(|| gone(what)),
        }
    };
    let plane = match &cut.plane {
        CutPlane::Fixed(p) => *p,
        CutPlane::Geom(end) => crate::mate::plane_at(end, &placed(end, "its sketch is on")?)?,
    };
    let mut sketch = cut.sketch.clone();
    let mut conversions = cut.conversions.clone();
    for c in &mut conversions {
        let (edge, frame) =
            crate::mate::edge_at(&c.edge, &placed(&c.edge, "an edge it converts is on")?)?;
        // The sketch's plane in the edge's body's coordinates: the same plane coordinates.
        let local = Plane {
            frame: frame.inverse().compose(&plane.frame),
        };
        // Components placed by mates are square to each other only as closely as the
        // solve gets them (to about 1e-5 of a radian): a circle within 1e-4 of a radian of
        // parallel to the sketch is taken as parallel, as SolidWorks converts it.
        let shape = convert_edge(&edge, local)?;
        match c.entity.filter(|e| sketch.entity(*e).is_some()) {
            Some(e) => shape.update(&mut sketch, e)?,
            None => c.entity = Some(shape.add(&mut sketch, false)),
        }
    }
    if sketch.constraints().next().is_some() {
        let failures = peet_sketch::expr::apply_expressions(&mut sketch, params);
        if let Some((_, e)) = failures.first() {
            return Err(format!("A dimension of its sketch can't be worked out: {e}"));
        }
        let previous = sketch.clone();
        let report =
            peet_sketch::solver::Solver::new().solve_with_previous(&mut sketch, &previous);
        if !report.converged {
            return Err(
                "Its sketch can't be solved: its relations and dimensions contradict each other."
                    .to_owned(),
            );
        }
    }
    Ok((plane, sketch, conversions))
}

/// Works out the assembly's cuts on its component instances, in order: each instance in a
/// cut's scope that it reaches gets the body less the cut (in the instance's own
/// coordinates). `locate` gives a component where it is now, for the planes and edges the
/// cuts follow; `cache` keeps cut bodies between rebuilds. Returns each cut's status.
pub(crate) fn apply(
    assembly: &crate::Assembly,
    params: &peet_sketch::expr::Parameters,
    instances: &mut Vec<crate::regen::Instance>,
    locate: &dyn Fn(CompId) -> Option<crate::mate::Placed>,
    cache: &mut std::collections::HashMap<u64, Vec<Arc<crate::naming::Body>>>,
    writes: &mut Vec<(CutId, Plane, Sketch, Vec<CutConversion>)>,
) -> std::collections::HashMap<CutId, crate::regen::Status> {
    use crate::regen::Status;
    let mut out = std::collections::HashMap::new();
    let mut used = std::collections::HashSet::new();
    for cut in assembly.cuts() {
        if cut.suppressed {
            out.insert(cut.id, Status::Suppressed);
            continue;
        }
        let status = (|| -> Result<Status, String> {
            if !matches!(
                cut.params.end,
                crate::EndCondition::Blind
                    | crate::EndCondition::Symmetric
                    | crate::EndCondition::ThroughAll
            ) {
                return Err(
                    "An assembly cut goes a depth (blind or mid-plane) or through all.".to_owned(),
                );
            }
            let (plane, sketch, conversions) = resolve(cut, params, locate)?;
            // Kept in the model, as a part's sketch keeps its solved geometry.
            if sketch != cut.sketch || conversions != cut.conversions || plane != cut.placement {
                writes.push((cut.id, plane, sketch.clone(), conversions));
            }
            let depth = if cut.params.end.uses_depth() {
                cut.params
                    .depth
                    .evaluate(crate::ScalarKind::Length, params)
                    .map_err(|m| format!("Depth: {m}."))?
            } else {
                0.0
            };
            let draft = cut
                .params
                .draft
                .evaluate(crate::ScalarKind::Angle, params)
                .map_err(|m| format!("Draft: {m}."))?;
            let feature = crate::FeatureId(u32::MAX - cut.id.0);
            let key = crate::hash::of(&(&cut.params, &sketch, depth, draft, cut.id.0));
            let mut cuts = 0usize;
            let mut next = Vec::with_capacity(instances.len());
            let mut failed = None;
            for inst in instances.drain(..) {
                let in_scope = cut
                    .scope
                    .as_ref()
                    .is_none_or(|s| inst.path.first().is_some_and(|c| s.contains(c)));
                if !in_scope || failed.is_some() {
                    next.push(inst);
                    continue;
                }
                let local = Plane {
                    frame: inst.frame.inverse().compose(&plane.frame),
                };
                let ikey = crate::hash::combine(
                    key,
                    crate::hash::of(&(&local, inst.body.stamp)),
                );
                let bodies = match cache.get(&ikey) {
                    Some(b) => b.clone(),
                    None => {
                        let input = crate::extrude::ExtrudeInput {
                            feature,
                            bodies: std::slice::from_ref(&inst.body),
                            plane: &local,
                            sketch: &sketch,
                            params: &cut.params,
                            depth,
                            draft,
                            up_to: None,
                            stamp: ikey,
                            mirror: false,
                        };
                        let result = match crate::extrude::tool(&input) {
                            // Through all: this body is not on that side of the sketch.
                            Err(_) if cut.params.end == crate::EndCondition::ThroughAll => {
                                vec![inst.body.clone()]
                            }
                            Err(e) => {
                                failed = Some(e.0);
                                next.push(inst);
                                continue;
                            }
                            Ok((tool, tool_names)) => {
                                match crate::extrude::combine(&crate::extrude::Combine {
                                    feature,
                                    bodies: std::slice::from_ref(&inst.body),
                                    tool,
                                    tool_names,
                                    operation: Operation::Cut,
                                    stamp: ikey,
                                    hint: "",
                                }) {
                                    Ok(b) => b,
                                    // It doesn't reach this body, or removes nothing of it.
                                    Err(_) => vec![inst.body.clone()],
                                }
                            }
                        };
                        cache.insert(ikey, result.clone());
                        result
                    }
                };
                used.insert(ikey);
                if bodies.len() == 1 && Arc::ptr_eq(&bodies[0], &inst.body) {
                    next.push(inst);
                    continue;
                }
                cuts += 1;
                for body in bodies {
                    next.push(crate::regen::Instance {
                        body,
                        ..inst.clone()
                    });
                }
            }
            *instances = next;
            if let Some(e) = failed {
                return Err(e);
            }
            Ok(if cuts == 0 {
                Status::Warning(
                    "It cuts nothing: no component in its scope is where it cuts. Check its sketch, direction and depth, and its components."
                        .to_owned(),
                )
            } else {
                Status::Ok
            })
        })();
        out.insert(cut.id, status.unwrap_or_else(Status::Failed));
    }
    cache.retain(|k, _| used.contains(k));
    out
}
