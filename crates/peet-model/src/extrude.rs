//! Extrude features: a sketch's regions pushed along the sketch normal, creating a new
//! body, adding to bodies or cutting from them.

use peet_kernel::Solid;
use peet_kernel::boolean::{BooleanOp, boolean};
use peet_math::{Aabb, DVec2, DVec3, Plane, tolerance};
use peet_sketch::Sketch;
use peet_sketch::region::{Profile, find_regions};
use peet_sketch::triangulate::triangulate_region;
use serde::{Deserialize, Serialize};

use crate::FeatureError;

/// How far an extrusion goes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum EndCondition {
    /// A given depth in one direction.
    Blind,
    /// A given total depth, split evenly on both sides of the sketch plane.
    Symmetric,
    /// Through every body on that side of the sketch.
    ThroughAll,
    /// Up to a plane (a planar face) parallel to the sketch.
    UpTo(Plane),
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
    /// Depth in mm (Blind and Mid-plane).
    pub depth: f64,
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
            depth: 10.0,
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
fn interior_point(region: &peet_sketch::region::Region) -> Option<DVec2> {
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

/// The extrusion range `(from, to)` along the plane normal.
fn range(plane: &Plane, ex: &Extrude, bodies: &[Solid]) -> Result<(f64, f64), FeatureError> {
    let s = ex.direction();
    let (a, b) = match ex.end {
        EndCondition::Blind => (0.0, s * ex.depth),
        EndCondition::Symmetric => (-ex.depth / 2.0, ex.depth / 2.0),
        EndCondition::ThroughAll => {
            let mut reach = f64::NEG_INFINITY;
            for body in bodies {
                let bb = body.bounds();
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
        EndCondition::UpTo(target) => {
            let parallel = target.normal().cross(plane.normal()).length() <= 1e-9;
            if !parallel {
                return Err(FeatureError(
                    "Up to face: the face must be parallel to the sketch plane.".to_owned(),
                ));
            }
            let d = (target.origin() - plane.origin()).dot(plane.normal());
            (0.0, d)
        }
    };
    let (from, to) = (a.min(b), a.max(b));
    if to - from <= tolerance::LINEAR {
        return Err(FeatureError(
            "The extrusion has no depth. Enter a depth greater than zero.".to_owned(),
        ));
    }
    Ok((from, to))
}

/// Applies an extrude feature to the bodies built so far.
pub fn apply_extrude(
    bodies: &mut Vec<Solid>,
    plane: &Plane,
    sketch: &Sketch,
    ex: &Extrude,
) -> Result<(), FeatureError> {
    let profile = find_regions(sketch);
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
    let indices: Vec<usize> = match &ex.regions {
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
    let regions: Vec<_> = indices
        .iter()
        .map(|&i| profile.regions[i].clone())
        .collect();
    let (from, to) = range(plane, ex, bodies)?;
    let tool = peet_kernel::extrude::extrude(plane, &regions, from, to)?;
    let tool_box = tool.bounds();

    match ex.operation {
        Operation::NewBody => bodies.push(tool),
        Operation::Add => {
            let touching: Vec<usize> = (0..bodies.len())
                .filter(|&i| overlaps(&bodies[i].bounds(), &tool_box))
                .collect();
            let mut merged = tool;
            for &i in &touching {
                merged = boolean(&bodies[i], &merged, BooleanOp::Union)?;
            }
            for &i in touching.iter().rev() {
                bodies.remove(i);
            }
            bodies.push(merged);
        }
        Operation::Cut => {
            let mut touched = false;
            let mut out = Vec::with_capacity(bodies.len());
            // Build the new list first so a failure leaves the bodies untouched.
            for body in bodies.iter() {
                if overlaps(&body.bounds(), &tool_box) {
                    touched = true;
                    let result = boolean(body, &tool, BooleanOp::Subtract)?;
                    if !result.faces.is_empty() {
                        out.push(result);
                    }
                } else {
                    out.push(body.clone());
                }
            }
            if !touched {
                return Err(FeatureError(
                    "The cut doesn't reach any body. Check its direction and depth.".to_owned(),
                ));
            }
            *bodies = out;
        }
    }
    Ok(())
}
