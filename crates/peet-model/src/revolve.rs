//! Revolve features: a sketch's regions turned about an axis in the sketch plane, creating
//! a new body, adding to bodies or cutting from them.

use std::sync::Arc;

use peet_kernel::Solid;
use peet_kernel::revolve::{RevolveAxis, RevolveFace, revolve_traced};
use peet_math::{DVec2, Plane, tolerance};
use peet_sketch::region::Region;
use peet_sketch::{Curve, EntityId, Sketch};
use serde::{Deserialize, Serialize};

use crate::FeatureError;
use crate::extrude::{Combine, Operation, RegionSelection, combine, selected_regions};
use crate::feature::{Axis, AxisRef, FeatureId, Scalar};
use crate::naming::{Body, FaceName, FaceRole};

/// What a revolve turns about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RevolveAxisRef {
    /// A line of the feature's own sketch: usually a construction line (a centreline), or
    /// the straight side of a half profile.
    SketchLine(EntityId),
    /// The sketch's horizontal axis.
    SketchX,
    /// The sketch's vertical axis.
    SketchY,
    /// A reference axis, a standard axis or a straight edge lying in the sketch plane.
    Axis(AxisRef),
}

/// A revolution of a sketch's regions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevolveFeature {
    pub sketch: FeatureId,
    pub axis: RevolveAxisRef,
    /// The angle turned through, in degrees (360: all the way round).
    pub angle: Scalar,
    /// Turn half the angle to each side of the sketch plane.
    pub symmetric: bool,
    /// Turn the other way about the axis.
    pub reverse: bool,
    pub operation: Operation,
    pub regions: RegionSelection,
}

impl RevolveFeature {
    /// A full turn of `sketch` about its first construction line, or about its vertical
    /// axis if it has none.
    pub fn new(sketch: FeatureId, geometry: &Sketch, operation: Operation) -> Self {
        Self {
            sketch,
            axis: default_axis(geometry),
            angle: Scalar::new(360.0),
            symmetric: false,
            reverse: false,
            operation,
            regions: RegionSelection::Auto,
        }
    }
}

/// The axis a new revolve of `sketch` uses: its first construction line, else its
/// vertical axis.
pub fn default_axis(sketch: &Sketch) -> RevolveAxisRef {
    sketch
        .entities()
        .find(|(id, e)| e.construction && matches!(sketch.curve(*id), Some(Curve::Line { .. })))
        .map_or(RevolveAxisRef::SketchY, |(id, _)| {
            RevolveAxisRef::SketchLine(id)
        })
}

/// Everything a revolve is built from, with references already resolved.
pub struct RevolveInput<'a> {
    /// The feature being built: its faces are named after it.
    pub feature: FeatureId,
    /// The bodies built so far.
    pub bodies: &'a [Arc<Body>],
    pub plane: &'a Plane,
    pub sketch: &'a Sketch,
    pub def: &'a RevolveFeature,
    /// The evaluated angle in degrees.
    pub angle: f64,
    /// The resolved axis, for [`RevolveAxisRef::Axis`].
    pub axis: Option<Axis>,
    /// Seed for the stamps of the bodies this feature creates or changes.
    pub stamp: u64,
    /// Mirror the sketch (y to −y) before revolving: for mirrored copies.
    pub mirror: bool,
}

/// The axis in the sketch plane's coordinates.
pub fn sketch_axis(
    def: &RevolveAxisRef,
    sketch: &Sketch,
    plane: &Plane,
    resolved: Option<Axis>,
) -> Result<RevolveAxis, FeatureError> {
    let err = |m: &str| Err(FeatureError(m.to_owned()));
    match def {
        RevolveAxisRef::SketchX => Ok(RevolveAxis {
            origin: DVec2::ZERO,
            dir: DVec2::X,
        }),
        RevolveAxisRef::SketchY => Ok(RevolveAxis {
            origin: DVec2::ZERO,
            dir: DVec2::Y,
        }),
        RevolveAxisRef::SketchLine(id) => match sketch.curve(*id) {
            Some(Curve::Line { a, b }) if a.distance(b) > tolerance::LINEAR => Ok(RevolveAxis {
                origin: a,
                dir: b - a,
            }),
            Some(_) => err("The axis must be a straight line of the sketch. Pick another axis."),
            None => err(
                "The sketch line used as the axis was deleted. Pick another axis: a \
                 construction line in the sketch works well.",
            ),
        },
        RevolveAxisRef::Axis(_) => {
            let Some(axis) = resolved else {
                return err("Pick the axis to revolve about.");
            };
            // It must lie in the sketch plane: the profile turns about a line it shares
            // a plane with.
            let off = plane.signed_distance(axis.origin).abs();
            let tilt = axis.dir.dot(plane.normal()).abs();
            if off > 100.0 * tolerance::LINEAR || tilt > 1e-6 {
                return err(
                    "The axis must lie in the sketch's plane. Pick an axis or an edge in that \
                     plane, or draw a construction line in the sketch and use that.",
                );
            }
            let dir = plane.frame.vector_to_local(axis.dir).truncate();
            Ok(RevolveAxis {
                origin: plane.to_plane_coords(axis.origin),
                dir,
            })
        }
    }
}

/// The angles the revolve turns from and to, in radians about the axis.
fn range(input: &RevolveInput<'_>) -> Result<(f64, f64), FeatureError> {
    let angle = input.angle;
    if !angle.is_finite() || angle <= 1e-6 {
        return Err(FeatureError(
            "The angle must be greater than zero.".to_owned(),
        ));
    }
    if angle > 360.0 + 1e-9 {
        return Err(FeatureError(
            "The angle can't be more than 360° (a full turn).".to_owned(),
        ));
    }
    let a = angle.min(360.0).to_radians();
    Ok(if input.def.symmetric {
        (-a / 2.0, a / 2.0)
    } else if input.def.reverse {
        (-a, 0.0)
    } else {
        (0.0, a)
    })
}

/// The revolved tool solid with its face names.
fn tool(input: &RevolveInput<'_>) -> Result<(Solid, Vec<FaceName>), FeatureError> {
    let regions: Vec<Region> = selected_regions(input.sketch, &input.def.regions, input.mirror)?;
    let mut axis = sketch_axis(&input.def.axis, input.sketch, input.plane, input.axis)?;
    let (mut from, mut to) = range(input)?;
    // A mirrored copy: an axis drawn in the sketch is mirrored with it, and the turn
    // goes the other way round.
    if input.mirror {
        if !matches!(input.def.axis, RevolveAxisRef::Axis(_)) {
            axis.origin.y = -axis.origin.y;
            axis.dir.y = -axis.dir.y;
        }
        (from, to) = (-to, -from);
    }
    let (solid, faces) = revolve_traced(input.plane, &regions, &axis, from, to)?;
    // The cap in the sketch plane is the start, whichever way the revolve turns.
    let near_is_start = !(input.def.reverse && !input.def.symmetric);
    let names = faces
        .iter()
        .map(|f| {
            let role = match *f {
                RevolveFace::Start { .. } if near_is_start => FaceRole::NearCap,
                RevolveFace::Start { .. } => FaceRole::FarCap,
                RevolveFace::End { .. } if near_is_start => FaceRole::FarCap,
                RevolveFace::End { .. } => FaceRole::NearCap,
                RevolveFace::Side {
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

/// Applies a revolve feature to the bodies built so far and returns the new list of
/// bodies.
pub fn apply_revolve(input: &RevolveInput<'_>) -> Result<Vec<Arc<Body>>, FeatureError> {
    let (tool, tool_names) = tool(input)?;
    combine(&Combine {
        feature: input.feature,
        bodies: input.bodies,
        tool,
        tool_names,
        operation: input.def.operation,
        stamp: input.stamp,
        hint: "Check its axis and angle.",
    })
}
