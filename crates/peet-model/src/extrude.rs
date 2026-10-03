//! Extrude features: a sketch's regions pushed along the sketch normal, creating a new
//! body, adding to bodies or cutting from them.

use std::sync::Arc;

use peet_kernel::Solid;
use peet_kernel::boolean::{BooleanOp, boolean_traced};
use peet_kernel::extrude::{ExtrudeFace, extrude_traced};
use peet_math::{Aabb, DVec2, DVec3, Plane, tolerance};
use peet_sketch::Sketch;
use peet_sketch::region::{Profile, Region, find_regions};
use peet_sketch::triangulate::triangulate_region;
use serde::{Deserialize, Serialize};

use crate::FeatureError;
use crate::feature::{FeatureId, PlaneRef, Scalar};
use crate::naming::{Body, FaceName, FaceRole};

/// How far an extrusion goes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EndCondition {
    /// A given depth in one direction.
    Blind,
    /// A given total depth, split evenly on both sides of the sketch plane.
    Symmetric,
    /// Through every body on that side of the sketch.
    ThroughAll,
    /// Up to a plane or planar face parallel to the sketch.
    UpTo(PlaneRef),
}

impl EndCondition {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Blind => "Blind",
            Self::Symmetric => "Mid-plane",
            Self::ThroughAll => "Through all",
            Self::UpTo(_) => "Up to face",
        }
    }

    /// Whether the depth value is used.
    pub fn uses_depth(&self) -> bool {
        matches!(self, Self::Blind | Self::Symmetric)
    }
}

/// What the extruded material does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    /// Always a separate body.
    NewBody,
    /// Merged with the bodies it touches (a new body if it touches none).
    Add,
    /// Removed from the bodies it touches.
    Cut,
}

impl Operation {
    pub fn label(self) -> &'static str {
        match self {
            Self::NewBody => "New body",
            Self::Add => "Add",
            Self::Cut => "Cut",
        }
    }
}

/// Which of the sketch's closed regions to extrude.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RegionSelection {
    /// The outermost regions and islands inside holes (alternate nesting levels), which is
    /// what you want for a plate with holes.
    Auto,
    /// The regions containing these sketch points. Points survive sketch edits better than
    /// region indices.
    Points(Vec<DVec2>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extrude {
    pub end: EndCondition,
    /// Depth (Blind and Mid-plane).
    pub depth: Scalar,
    /// Flip the direction. The default direction is along the sketch normal, except for
    /// cuts, which go against it (into the face the sketch sits on).
    pub reverse: bool,
    pub operation: Operation,
    pub regions: RegionSelection,
}

impl Extrude {
    pub fn new(operation: Operation) -> Self {
        Self {
            end: EndCondition::Blind,
            depth: Scalar::new(10.0),
            reverse: false,
            operation,
            regions: RegionSelection::Auto,
        }
    }

    /// +1 or −1: which way along the sketch normal the extrusion goes.
    pub fn direction(&self) -> f64 {
        let base = if self.operation == Operation::Cut {
            -1.0
        } else {
            1.0
        };
        if self.reverse { -base } else { base }
    }
}

/// Indices of the regions extruded by [`RegionSelection::Auto`]: those at even nesting depth.
pub fn default_regions(profile: &Profile) -> Vec<usize> {
    let samples: Vec<Option<DVec2>> = profile.regions.iter().map(interior_point).collect();
    (0..profile.regions.len())
        .filter(|&i| {
            let Some(p) = samples[i] else {
                return false;
            };
            let depth = profile
                .regions
                .iter()
                .enumerate()
                .filter(|(j, r)| *j != i && r.outer.contains(p))
                .count();
            depth % 2 == 0
        })
        .collect()
}

/// A point strictly inside a region (the centroid of its largest triangle).
fn interior_point(region: &Region) -> Option<DVec2> {
    let (lo, hi) = region.outer.bounds();
    let tris = triangulate_region(region, (lo.distance(hi) * 1e-3).max(1e-6));
    tris.triangles
        .iter()
        .map(|t| t.map(|i| tris.points[i as usize]))
        .max_by(|a, b| tri_area(a).total_cmp(&tri_area(b)))
        .map(|[a, b, c]| (a + b + c) / 3.0)
}

fn tri_area([a, b, c]: &[DVec2; 3]) -> f64 {
    (b - *a).perp_dot(*c - *a).abs()
}

fn overlaps(a: &Aabb, b: &Aabb) -> bool {
    let m = tolerance::LINEAR * 10.0;
    !a.is_empty()
        && !b.is_empty()
        && a.min.x <= b.max.x + m
        && b.min.x <= a.max.x + m
        && a.min.y <= b.max.y + m
        && b.min.y <= a.max.y + m
        && a.min.z <= b.max.z + m
        && b.min.z <= a.max.z + m
}

/// Everything an extrusion is built from, with references already resolved.
pub struct ExtrudeInput<'a> {
    /// The feature being built: its faces are named after it.
    pub feature: FeatureId,
    /// The bodies built so far.
    pub bodies: &'a [Arc<Body>],
    pub plane: &'a Plane,
    pub sketch: &'a Sketch,
    pub params: &'a Extrude,
    /// The evaluated depth in mm.
    pub depth: f64,
    /// The resolved "up to" plane, for [`EndCondition::UpTo`].
    pub up_to: Option<Plane>,
    /// Seed for the stamps of the bodies this feature creates or changes.
    pub stamp: u64,
    /// Mirror the sketch's regions (y to −y) before extruding: for mirrored copies.
    pub mirror: bool,
}

/// The extrusion's offsets along the plane normal: where it starts (the near cap) and
/// where it ends (the far cap). Either may be the larger.
fn range(input: &ExtrudeInput<'_>) -> Result<(f64, f64), FeatureError> {
    let (plane, ex) = (input.plane, input.params);
    let s = ex.direction();
    let (near, far) = match &ex.end {
        EndCondition::Blind => (0.0, s * input.depth),
        EndCondition::Symmetric => (-input.depth / 2.0, input.depth / 2.0),
        EndCondition::ThroughAll => {
            let mut reach = f64::NEG_INFINITY;
            for body in input.bodies {
                let bb = body.solid.bounds();
                for i in 0..8 {
                    let corner = DVec3::new(
                        if i & 1 == 0 { bb.min.x } else { bb.max.x },
                        if i & 2 == 0 { bb.min.y } else { bb.max.y },
                        if i & 4 == 0 { bb.min.z } else { bb.max.z },
                    );
                    reach = reach.max(s * plane.signed_distance(corner));
                }
            }
            if reach <= tolerance::LINEAR {
                return Err(FeatureError(
                    "Through all: there is no body on that side of the sketch. Try reversing the direction.".to_owned(),
                ));
            }
            (0.0, s * (reach + 1.0))
        }
        EndCondition::UpTo(_) => {
            let target = input.up_to.ok_or_else(|| {
                FeatureError("Up to face: pick the face or plane to extrude up to.".to_owned())
            })?;
            let parallel = target.normal().cross(plane.normal()).length() <= 1e-9;
            if !parallel {
                return Err(FeatureError(
                    "Up to face: the face must be parallel to the sketch plane.".to_owned(),
                ));
            }
            (0.0, (target.origin() - plane.origin()).dot(plane.normal()))
        }
    };
    if !near.is_finite() || !far.is_finite() {
        return Err(FeatureError("The depth is not a finite number.".to_owned()));
    }
    if (far - near).abs() <= tolerance::LINEAR {
        return Err(FeatureError(
            "The extrusion has no depth. Enter a depth greater than zero.".to_owned(),
        ));
    }
    Ok((near, far))
}

/// The extruded tool solid with its face names.
fn tool(input: &ExtrudeInput<'_>) -> Result<(Solid, Vec<FaceName>), FeatureError> {
    let profile = find_regions(input.sketch);
    if profile.regions.is_empty() {
        let hint = if profile.open_ends.is_empty() {
            "Draw a closed shape."
        } else {
            "Some curve ends aren't connected: close the profile."
        };
        return Err(FeatureError(format!(
            "The sketch has no closed region. {hint}"
        )));
    }
    let indices: Vec<usize> = match &input.params.regions {
        RegionSelection::Auto => default_regions(&profile),
        RegionSelection::Points(points) => {
            let mut v: Vec<usize> = points
                .iter()
                .filter_map(|p| profile.region_at(*p))
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        }
    };
    if indices.is_empty() {
        return Err(FeatureError("No regions are selected.".to_owned()));
    }
    let regions: Vec<Region> = indices
        .iter()
        .map(|&i| profile.regions[i].clone())
        .map(|r| if input.mirror { mirrored(&r) } else { r })
        .collect();
    let (near, far) = range(input)?;
    let (from, to) = (near.min(far), near.max(far));
    let (solid, faces) = extrude_traced(input.plane, &regions, from, to)?;
    let names = faces
        .iter()
        .map(|f| {
            let role = match *f {
                // The kernel's start cap is the lower one along the normal.
                ExtrudeFace::Start { .. } if near <= far => FaceRole::NearCap,
                ExtrudeFace::Start { .. } => FaceRole::FarCap,
                ExtrudeFace::End { .. } if near <= far => FaceRole::FarCap,
                ExtrudeFace::End { .. } => FaceRole::NearCap,
                ExtrudeFace::Side {
                    region,
                    loop_index,
                    edge,
                } => {
                    let r = &regions[region];
                    let l = if loop_index == 0 {
                        &r.outer
                    } else {
                        &r.holes[loop_index - 1]
                    };
                    FaceRole::Side(l.edges[edge].entity)
                }
            };
            FaceName::new(input.feature, role)
        })
        .collect();
    Ok((solid, names))
}

/// A region mirrored in the sketch's x axis (y to −y), its loops still running the
/// same way round (outer counter-clockwise).
fn mirrored(region: &Region) -> Region {
    use peet_sketch::Curve;
    use peet_sketch::region::{Loop, LoopEdge};
    let m = |p: DVec2| DVec2::new(p.x, -p.y);
    let edge = |e: &LoopEdge| -> LoopEdge {
        let curve = match e.curve {
            Curve::Line { a, b } => Curve::Line { a: m(a), b: m(b) },
            Curve::Circle { center, radius } => Curve::Circle {
                center: m(center),
                radius,
            },
            Curve::Arc {
                center,
                radius,
                start_angle,
                sweep,
            } => Curve::Arc {
                // Mirrored, the arc runs clockwise: start it at the mirrored end instead.
                center: m(center),
                radius,
                start_angle: -(start_angle + sweep),
                sweep,
            },
        };
        // A mirrored arc is traversed the other way round its new (counter-clockwise)
        // direction; lines and circles keep their direction.
        let reversed = match e.curve {
            Curve::Arc { .. } => !e.reversed,
            _ => e.reversed,
        };
        LoopEdge {
            entity: e.entity,
            curve,
            reversed,
        }
    };
    // Mirroring turns every loop round: walk it backwards to turn it back.
    let fix = |l: &Loop| -> Loop {
        let edges = l
            .edges
            .iter()
            .rev()
            .map(|e| {
                let mut e = edge(e);
                e.reversed = !e.reversed;
                e
            })
            .collect();
        Loop {
            edges,
            signed_area: l.signed_area,
        }
    };
    Region {
        outer: fix(&region.outer),
        holes: region.holes.iter().map(fix).collect(),
    }
}

/// The names of a boolean's result faces: each face takes the names of the input faces it
/// is made of.
fn result_names(
    sources: &[Vec<peet_kernel::boolean::FaceSource>],
    a: &[FaceName],
    b: &[FaceName],
) -> Vec<FaceName> {
    sources
        .iter()
        .map(|list| {
            FaceName::merged(list.iter().map(|s| {
                let names = if s.solid == 0 { a } else { b };
                &names[s.face.index()]
            }))
        })
        .collect()
}

fn volume(solid: &Solid) -> f64 {
    peet_kernel::validate::measure::volume(solid)
}

/// A stamp for the `index`-th body an operation produces.
pub(crate) fn stamp(seed: u64, index: usize) -> u64 {
    crate::hash::combine(seed, index as u64 + 1)
}

/// Applies an extrude feature to the bodies built so far and returns the new list of
/// bodies. Bodies the feature doesn't touch are shared with the input, not copied.
pub fn apply_extrude(input: &ExtrudeInput<'_>) -> Result<Vec<Arc<Body>>, FeatureError> {
    let (tool, tool_names) = tool(input)?;
    let tool_box = tool.bounds();
    let bodies = input.bodies;
    let touching: Vec<usize> = (0..bodies.len())
        .filter(|&i| overlaps(&bodies[i].solid.bounds(), &tool_box))
        .collect();

    match input.params.operation {
        Operation::NewBody => {
            let mut out = bodies.to_vec();
            out.push(Arc::new(Body {
                solid: tool,
                face_names: tool_names,
                origin: input.feature,
                stamp: stamp(input.stamp, 0),
                sheet: None,
            }));
            Ok(out)
        }
        Operation::Add => {
            let (mut solid, mut names) = (tool, tool_names);
            for &i in &touching {
                let traced = boolean_traced(&bodies[i].solid, &solid, BooleanOp::Union)?;
                names = result_names(&traced.sources, &bodies[i].face_names, &names);
                solid = traced.solid;
            }
            // The merged body takes the place (and identity) of the first body it joined.
            let merged = Arc::new(Body {
                solid,
                face_names: names,
                origin: touching
                    .first()
                    .map_or(input.feature, |&i| bodies[i].origin),
                stamp: stamp(input.stamp, 0),
                sheet: None,
            });
            let mut out = Vec::with_capacity(bodies.len() + 1);
            let mut merged = Some(merged);
            for (i, body) in bodies.iter().enumerate() {
                if !touching.contains(&i) {
                    out.push(body.clone());
                } else if let Some(m) = merged.take() {
                    out.push(m);
                }
            }
            out.extend(merged);
            Ok(out)
        }
        Operation::Cut => {
            if touching.is_empty() {
                return Err(FeatureError(
                    "The cut doesn't reach any body. Check its direction and depth.".to_owned(),
                ));
            }
            // Build the new list first so a failure leaves the bodies untouched.
            let mut out = Vec::with_capacity(bodies.len());
            let mut removed = false;
            for (i, body) in bodies.iter().enumerate() {
                if !touching.contains(&i) {
                    out.push(body.clone());
                    continue;
                }
                let traced = boolean_traced(&body.solid, &tool, BooleanOp::Subtract)?;
                if traced.solid.faces.is_empty() {
                    removed = true;
                    continue; // cut away completely
                }
                let before = volume(&body.solid);
                if (before - volume(&traced.solid)).abs() <= 1e-9 * before.abs().max(1.0) {
                    out.push(body.clone()); // the boxes overlap, the material doesn't
                    continue;
                }
                removed = true;
                out.push(Arc::new(Body {
                    face_names: result_names(&traced.sources, &body.face_names, &tool_names),
                    solid: traced.solid,
                    origin: body.origin,
                    stamp: stamp(input.stamp, i),
                    sheet: None,
                }));
            }
            if !removed {
                return Err(FeatureError(
                    "The cut doesn't remove any material. Check its direction and depth."
                        .to_owned(),
                ));
            }
            Ok(out)
        }
    }
}
