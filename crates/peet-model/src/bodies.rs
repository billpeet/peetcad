//! Features that work on whole bodies: moving or copying them, and inserting the bodies of
//! another part.
//!
//! **Move and copy.** SolidWorks' Move/Copy Body: the bodies picked (by a face of each)
//! are turned about the X, Y and Z axes through a centre, in that order, then moved
//! along X, Y and Z. A moved body keeps its faces' names, so what was picked on it before
//! is still found; with `copy` the bodies stay where they are and the copies are new
//! bodies, the k-th moved k times, whose faces carry [`FaceRole::Instance`] with their
//! number.
//!
//! **Inserted parts.** SolidWorks' Insert Part and Mirror Part: the bodies of another
//! part, read from its file when the feature is made (and again when asked to), kept in
//! the model as they were last read, so the part still opens if the file is gone. A
//! mirror plane makes the mirror image (a left-hand part from a right-hand one). The
//! faces are named by their index in the part's bodies, as an imported body's are.

use std::sync::Arc;

use peet_kernel::Solid;
use peet_kernel::transform::{Reflection, mirrored};
use peet_math::{DQuat, DVec3, Frame};
use serde::{Deserialize, Serialize};

use crate::FeatureError;
use crate::extrude::stamp;
use crate::feature::{FeatureId, PlaneRef, PointRef, Scalar};
use crate::import::ImportedSolid;
use crate::naming::{Body, FaceName, FaceRef, FaceRole, find_face};
use crate::sheet::Applied;

/// Bodies moved, turned or copied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoveBodyFeature {
    /// A face of each body to move. None: every body.
    pub bodies: Vec<FaceRef>,
    /// How far along X, Y and Z, after turning.
    pub translate: [Scalar; 3],
    /// Degrees about the X, Y and Z axes through `center`, turned in that order.
    pub rotate: [Scalar; 3],
    /// The point the bodies turn about.
    pub center: PointRef,
    /// Leave the bodies where they are and make moved copies of them.
    pub copy: bool,
    /// How many copies, each moved once more than the last.
    pub copies: Scalar,
}

impl MoveBodyFeature {
    pub fn new(bodies: Vec<FaceRef>) -> Self {
        Self {
            bodies,
            translate: [Scalar::new(0.0), Scalar::new(0.0), Scalar::new(0.0)],
            rotate: [Scalar::new(0.0), Scalar::new(0.0), Scalar::new(0.0)],
            center: PointRef::Origin,
            copy: false,
            copies: Scalar::new(1.0),
        }
    }
}

/// The motion of a move: turned about X, then Y, then Z (degrees) through `center`, then
/// moved by `by`.
pub fn body_motion(by: DVec3, degrees: [f64; 3], center: DVec3) -> Frame {
    let [x, y, z] = degrees.map(f64::to_radians);
    let rotation =
        DQuat::from_rotation_z(z) * DQuat::from_rotation_y(y) * DQuat::from_rotation_x(x);
    Frame {
        origin: center - rotation * center + by,
        rotation,
    }
}

/// The bodies a move picks, by index (every body if it picks none).
fn picked(bodies: &[Arc<Body>], faces: &[FaceRef], what: &str) -> Result<Vec<usize>, FeatureError> {
    if bodies.is_empty() {
        return Err(FeatureError(format!(
            "There is no body to {what}. Make a solid first."
        )));
    }
    if faces.is_empty() {
        return Ok((0..bodies.len()).collect());
    }
    let mut out = Vec::new();
    for r in faces {
        let found = find_face(bodies, r).ok_or_else(|| {
            FeatureError(format!(
                "A body to {what} is no longer there: none of its faces is. Pick a face of \
                 each body again."
            ))
        })?;
        if !out.contains(&found.body) {
            out.push(found.body);
        }
    }
    Ok(out)
}

pub(crate) fn apply_move_body(
    feature: FeatureId,
    bodies: &[Arc<Body>],
    def: &MoveBodyFeature,
    motion: &Frame,
    copies: usize,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let what = if def.copy { "copy" } else { "move" };
    let chosen = picked(bodies, &def.bodies, what)?;
    let mut out = bodies.to_vec();
    if !def.copy {
        for &bi in &chosen {
            let body = &bodies[bi];
            out[bi] = Arc::new(Body {
                solid: peet_kernel::transform::solid(&body.solid, motion),
                face_names: body.face_names.clone(),
                origin: body.origin,
                stamp: stamp(seed, bi),
                // The flat pattern is drawn where the body was.
                sheet: None,
            });
        }
        return Ok((out, None));
    }
    if copies == 0 {
        return Err(FeatureError(
            "There are no copies to make: give 1 or more.".to_owned(),
        ));
    }
    let mut step = *motion;
    for k in 1..=copies {
        for (n, &bi) in chosen.iter().enumerate() {
            let body = &bodies[bi];
            let instance =
                FaceName::new(feature, FaceRole::Instance(((n as u32) << 16) | k as u32));
            out.push(Arc::new(Body {
                solid: peet_kernel::transform::solid(&body.solid, &step),
                face_names: body
                    .face_names
                    .iter()
                    .map(|name| FaceName::merged([name, &instance]))
                    .collect(),
                origin: feature,
                stamp: stamp(seed, out.len()),
                sheet: None,
            }));
        }
        step = motion.compose(&step);
    }
    Ok((out, None))
}

/// Another part's file and its bodies as they were last read from it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PartSource {
    /// The part's file, as it was given.
    pub path: String,
    /// The part's bodies when it was last read.
    pub solids: Vec<ImportedSolid>,
}

/// The bodies of another part, as they were last read from its file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InsertPartFeature {
    pub source: PartSource,
    /// Mirrored in this plane: SolidWorks' mirror part. None: as the part has them.
    pub mirror: Option<PlaneRef>,
}

impl InsertPartFeature {
    pub fn new(source: PartSource) -> Self {
        Self {
            source,
            mirror: None,
        }
    }
}

pub(crate) fn apply_insert_part(
    feature: FeatureId,
    bodies: &[Arc<Body>],
    def: &InsertPartFeature,
    mirror: Option<&peet_math::Plane>,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let source = &def.source;
    if source.solids.is_empty() {
        return Err(FeatureError(format!(
            "{} has no bodies to insert. Read it again with update_links once it has.",
            if source.path.is_empty() {
                "The part"
            } else {
                &source.path
            }
        )));
    }
    let reflection = mirror.map(|p| Reflection {
        point: p.origin(),
        normal: p.normal(),
    });
    let solids: Vec<Solid> = source
        .solids
        .iter()
        .map(|s| match &reflection {
            Some(r) => mirrored(&s.solid, r),
            None => s.solid.clone(),
        })
        .collect();
    Ok((
        crate::import::named_bodies(feature, bodies, solids, seed),
        None,
    ))
}
