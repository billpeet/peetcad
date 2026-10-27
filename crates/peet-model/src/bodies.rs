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
//! **By mates.** SolidWorks' Move/Copy Body can instead place the bodies by constraints:
//! mates between their faces, edges and corners and the rest of the part's
//! ([`BodyMate`]). Then the move's distances and angles are only where the solve starts:
//! the bodies go to the nearest place the mates allow, at every rebuild, so they follow
//! the part when it changes.
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
use crate::mate::BodyMate;
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
    /// Mates that place the moved bodies, solved from where `translate` and `rotate`
    /// put them. None: they go where those put them.
    pub mates: Vec<BodyMate>,
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
            mates: Vec::new(),
        }
    }
}

/// The fields of a move's mate values: `mate1` is the first mate's distance or angle,
/// `mate2` the second's, and so on (a mate with no value has its field unused). They are
/// the move's values like `dx`, so each configuration can have its own; as in SolidWorks,
/// where a mate is a feature, a mate's value is called `D1@` the mate's name
/// (`D1@Distance2`) unless it was given another name.
pub const MATE_FIELDS: [&str; 32] = [
    "mate1", "mate2", "mate3", "mate4", "mate5", "mate6", "mate7", "mate8", "mate9", "mate10",
    "mate11", "mate12", "mate13", "mate14", "mate15", "mate16", "mate17", "mate18", "mate19",
    "mate20", "mate21", "mate22", "mate23", "mate24", "mate25", "mate26", "mate27", "mate28",
    "mate29", "mate30", "mate31", "mate32",
];

impl MoveBodyFeature {
    /// The mate whose value is the field `field` (`mate2`), if there is one.
    pub fn mate_of_field(&self, field: &str) -> Option<&BodyMate> {
        let i = MATE_FIELDS.iter().position(|f| *f == field)?;
        self.mates.get(i)
    }
}

/// The motion that puts the bodies a move picks where its mates hold, nearest to
/// `start`, with a warning naming the mates that can't. `planes` are the reference
/// planes the mates name, where they are.
pub(crate) fn mated_motion(
    bodies: &[Arc<Body>],
    def: &MoveBodyFeature,
    start: &Frame,
    planes: Vec<(PlaneRef, peet_math::Plane)>,
    params: &peet_sketch::expr::Parameters,
) -> Result<(Frame, Option<String>), FeatureError> {
    if def.copy {
        return Err(FeatureError(
            "Mates place moved bodies, not copies: leave copy off, or take the mates out."
                .to_owned(),
        ));
    }
    let chosen = picked(bodies, &def.bodies, "move")?;
    let (moved, rest): (Vec<_>, Vec<_>) = bodies
        .iter()
        .enumerate()
        .partition(|(i, _)| chosen.contains(i));
    let strip = |v: Vec<(usize, &Arc<Body>)>| v.into_iter().map(|(_, b)| b.clone()).collect();
    let (motion, failed) = crate::mate::solve_body(
        &def.mates,
        strip(moved),
        strip(rest),
        planes,
        *start,
        params,
    );
    let warning = (!failed.is_empty()).then(|| {
        let each: Vec<String> = failed
            .iter()
            .map(|(name, why)| format!("{name}: {why}"))
            .collect();
        format!(
            "Not every mate holds, so the bodies are where the others put them. {}",
            each.join(" ")
        )
    });
    Ok((motion, warning))
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
