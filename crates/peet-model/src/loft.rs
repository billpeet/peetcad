//! Loft features: a solid through the profiles of several sketches, in order.
//!
//! Each sketch gives one profile: its closed outline. The profiles are joined piece to
//! piece ([`peet_kernel::loft`]), so they need the same number of edges; a circle adapts
//! to its neighbours. Two profiles are joined by straight rulings, more by a smooth
//! surface through all of them.

use std::sync::Arc;

use peet_kernel::Solid;
use peet_kernel::loft::{LoftFace, LoftSection, loft_traced};
use peet_math::Plane;
use peet_sketch::Sketch;
use peet_sketch::region::Region;
use serde::{Deserialize, Serialize};

use crate::FeatureError;
use crate::extrude::{Combine, Operation, RegionSelection, combine, selected_regions};
use crate::feature::FeatureId;
use crate::naming::{Body, FaceName, FaceRole};

/// A loft through the profiles of sketches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoftFeature {
    /// The profile sketches, in the order the loft passes through them.
    pub sections: Vec<FeatureId>,
    pub operation: Operation,
}

impl LoftFeature {
    pub fn new(sections: Vec<FeatureId>, operation: Operation) -> Self {
        Self {
            sections,
            operation,
        }
    }
}

/// Everything a loft is built from, with references already resolved.
pub struct LoftInput<'a> {
    pub feature: FeatureId,
    pub bodies: &'a [Arc<Body>],
    /// Each profile's plane, sketch and name (for messages), in order.
    pub sections: &'a [(Plane, &'a Sketch, &'a str)],
    pub def: &'a LoftFeature,
    pub stamp: u64,
}

/// The lofted tool solid with its face names.
fn tool(input: &LoftInput<'_>) -> Result<(Solid, Vec<FaceName>), FeatureError> {
    if input.sections.len() < 2 {
        return Err(FeatureError(
            "A loft needs at least two profile sketches. Pick another sketch on a different \
             plane."
                .to_owned(),
        ));
    }
    let mut regions: Vec<Region> = Vec::with_capacity(input.sections.len());
    for (_, sketch, name) in input.sections {
        let mut found = selected_regions(sketch, &RegionSelection::Auto, false)
            .map_err(|e| FeatureError(format!("{name}: {}", e.0)))?;
        if found.len() != 1 {
            return Err(FeatureError(format!(
                "{name} has {} separate shapes. A loft profile is one closed outline: remove \
                 the others or make them construction geometry.",
                found.len()
            )));
        }
        regions.push(found.remove(0));
    }
    let sections: Vec<LoftSection<'_>> = input
        .sections
        .iter()
        .zip(&regions)
        .map(|((plane, _, _), region)| LoftSection { plane, region })
        .collect();
    let (solid, faces) = loft_traced(&sections)?;
    let names = faces
        .iter()
        .map(|f| {
            let role = match *f {
                LoftFace::Start => FaceRole::NearCap,
                LoftFace::End => FaceRole::FarCap,
                // Named by the first profile's curve the side starts from.
                LoftFace::Side { edge } => FaceRole::Side(regions[0].outer.edges[edge].entity),
            };
            FaceName::new(input.feature, role)
        })
        .collect();
    Ok((solid, names))
}

/// Applies a loft feature to the bodies built so far and returns the new list of bodies.
pub fn apply_loft(input: &LoftInput<'_>) -> Result<Vec<Arc<Body>>, FeatureError> {
    let (tool, tool_names) = tool(input)?;
    combine(&Combine {
        feature: input.feature,
        bodies: input.bodies,
        tool,
        tool_names,
        operation: input.def.operation,
        stamp: input.stamp,
        hint: "Check its profiles.",
    })
}
