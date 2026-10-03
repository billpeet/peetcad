//! Features that change bodies in place, picked on their edges and faces: fillets and
//! chamfers, shells, and draft.
//!
//! **Names.** Faces that were there before keep their names. A fillet or a chamfer names
//! its new faces by the edge they are on ([`FaceRole::Blend`]); a shell's inside walls take
//! the name of the face they stand behind plus [`FaceRole::Inner`], so "the inside of the
//! end face of Extrude1" stays that through upstream edits.

use std::sync::Arc;

use peet_kernel::blend::{Blend, BlendFace, blend, tangent_chain};
use peet_kernel::reshape::{ShellFace, draft, shell};
use peet_kernel::{EdgeId, FaceId, KernelError};
use peet_math::Plane;
use serde::{Deserialize, Serialize};

use crate::extrude::stamp;
use crate::feature::{FeatureId, PlaneRef, Scalar};
use crate::naming::{Body, EdgeRef, FaceName, FaceRef, FaceRole, find_edge, find_face};
use crate::sheet::Applied;
use crate::{FeatureError, Model};

/// What a blend puts on its edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendKind {
    Fillet,
    Chamfer,
}

/// A fillet or a chamfer on edges of the bodies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlendFeature {
    pub kind: BlendKind,
    pub edges: Vec<EdgeRef>,
    /// The fillet's radius, or how far the chamfer is set back on both faces.
    pub size: Scalar,
    /// Carry on along the edges that continue each picked edge smoothly.
    pub chain: bool,
}

impl BlendFeature {
    pub fn new(kind: BlendKind, edges: Vec<EdgeRef>) -> Self {
        Self {
            kind,
            edges,
            size: Scalar::new(match kind {
                BlendKind::Fillet => 2.0,
                BlendKind::Chamfer => 1.0,
            }),
            chain: true,
        }
    }
}

/// Hollows bodies, leaving walls of one thickness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellFeature {
    /// The faces to remove, opening the hollow. None: every body becomes a closed hollow.
    pub open: Vec<FaceRef>,
    pub thickness: Scalar,
}

impl ShellFeature {
    pub fn new(open: Vec<FaceRef>) -> Self {
        Self {
            open,
            thickness: Scalar::new(2.0),
        }
    }
}

/// Tapers faces about the lines where they cross a neutral plane: flat faces, and round
/// faces whose axis is along the direction of pull (they become cones).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraftFeature {
    pub faces: Vec<FaceRef>,
    /// The size of the part stays as it is in this plane; its normal is the direction of
    /// pull. `None` until picked.
    pub neutral: Option<PlaneRef>,
    /// Degrees. Positive: narrower along the neutral plane's normal.
    pub angle: Scalar,
    /// Taper the other way.
    pub flip: bool,
}

impl DraftFeature {
    pub fn new(faces: Vec<FaceRef>, neutral: Option<PlaneRef>) -> Self {
        Self {
            faces,
            neutral,
            angle: Scalar::new(3.0),
            flip: false,
        }
    }
}

/// A kernel message as a sentence for the feature tree.
pub(crate) fn sentence(e: &KernelError) -> FeatureError {
    let text = e.to_string();
    let mut chars = text.chars();
    let mut out: String = match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    if !out.ends_with(['.', ')']) {
        out.push('.');
    }
    FeatureError(out)
}

/// A body with a new solid and face names, in place of `body`.
fn changed(
    body: &Body,
    solid: peet_kernel::Solid,
    face_names: Vec<FaceName>,
    seed: u64,
    index: usize,
) -> Arc<Body> {
    Arc::new(Body {
        solid,
        face_names,
        origin: body.origin,
        stamp: stamp(seed, index),
        sheet: None,
    })
}

pub(crate) fn apply_blend(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    def: &BlendFeature,
    size: f64,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let (what, noun) = match def.kind {
        BlendKind::Fillet => ("fillet", "Fillet"),
        BlendKind::Chamfer => ("chamfer", "Chamfer"),
    };
    if def.edges.is_empty() {
        return Err(FeatureError(format!("Pick the edges to {what}.")));
    }
    // Per body: the edges to blend, each with the number its faces are named by.
    let mut picked: Vec<Vec<(EdgeId, u32)>> = vec![Vec::new(); bodies.len()];
    for (i, r) in def.edges.iter().enumerate() {
        let found = find_edge(bodies, r).ok_or_else(|| {
            FeatureError(format!(
                "An edge it refers to no longer exists (between {} and {}). Edit the \
                 {what} and pick the edge again, or remove it from the list.",
                model.describe_face(&r.faces[0]),
                model.describe_face(&r.faces[1])
            ))
        })?;
        let solid = &bodies[found.body].solid;
        let edges = if def.chain {
            tangent_chain(solid, found.id)
        } else {
            vec![found.id]
        };
        for (k, e) in edges.into_iter().enumerate() {
            if !picked[found.body].iter().any(|(known, _)| *known == e) {
                picked[found.body].push((e, ((i as u32) << 12) | (k as u32 & 0xfff)));
            }
        }
    }
    let shape = match def.kind {
        BlendKind::Fillet => Blend::Fillet { radius: size },
        BlendKind::Chamfer => Blend::Chamfer { distance: size },
    };
    let mut out = Vec::with_capacity(bodies.len());
    for (bi, body) in bodies.iter().enumerate() {
        let list = &picked[bi];
        if list.is_empty() {
            out.push(body.clone());
            continue;
        }
        let ids: Vec<EdgeId> = list.iter().map(|(e, _)| *e).collect();
        let blended = blend(&body.solid, &ids, shape).map_err(|e| {
            let FeatureError(m) = sentence(&e);
            FeatureError(format!("{noun}: {m}"))
        })?;
        let names = blended
            .faces
            .iter()
            .map(|labels| {
                let parts: Vec<FaceName> = labels
                    .iter()
                    .map(|l| match *l {
                        BlendFace::Original(f) => body.face_name(f).clone(),
                        BlendFace::Blend(j) => FaceName::new(feature, FaceRole::Blend(list[j].1)),
                        BlendFace::End(j) => FaceName::new(feature, FaceRole::BlendEnd(list[j].1)),
                        BlendFace::Corner(j) => {
                            FaceName::new(feature, FaceRole::BlendCorner(list[j].1))
                        }
                    })
                    .collect();
                FaceName::merged(&parts)
            })
            .collect();
        out.push(changed(body, blended.solid, names, seed, bi));
    }
    Ok((out, None))
}

pub(crate) fn apply_shell(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    def: &ShellFeature,
    thickness: f64,
    seed: u64,
) -> Result<Applied, FeatureError> {
    if bodies.is_empty() {
        return Err(FeatureError(
            "There is no body to shell. Make a solid first.".to_owned(),
        ));
    }
    let mut open: Vec<Vec<FaceId>> = vec![Vec::new(); bodies.len()];
    for r in &def.open {
        let found = find_face(bodies, r).ok_or_else(|| {
            FeatureError(format!(
                "A face to open no longer exists ({}). Edit the shell and pick the face \
                 again, or remove it from the list.",
                model.describe_face(&r.name)
            ))
        })?;
        if !open[found.body].contains(&found.id) {
            open[found.body].push(found.id);
        }
    }
    // With faces picked, the bodies they are on are shelled; with none, every body.
    let all = def.open.is_empty();
    let inner = FaceName::new(feature, FaceRole::Inner);
    let mut out = Vec::with_capacity(bodies.len());
    for (bi, body) in bodies.iter().enumerate() {
        if !all && open[bi].is_empty() {
            out.push(body.clone());
            continue;
        }
        let shelled = shell(&body.solid, &open[bi], thickness).map_err(|e| sentence(&e))?;
        let names = shelled
            .faces
            .iter()
            .map(|labels| {
                let parts: Vec<FaceName> = labels
                    .iter()
                    .map(|l| match *l {
                        ShellFace::Outer(f) => body.face_name(f).clone(),
                        ShellFace::Inner(f) => FaceName::merged([body.face_name(f), &inner]),
                    })
                    .collect();
                FaceName::merged(&parts)
            })
            .collect();
        out.push(changed(body, shelled.solid, names, seed, bi));
    }
    Ok((out, None))
}

pub(crate) fn apply_draft(
    model: &Model,
    bodies: &[Arc<Body>],
    def: &DraftFeature,
    neutral: &Plane,
    angle: f64,
    seed: u64,
) -> Result<Applied, FeatureError> {
    if def.faces.is_empty() {
        return Err(FeatureError("Pick the faces to draft.".to_owned()));
    }
    let mut faces: Vec<Vec<FaceId>> = vec![Vec::new(); bodies.len()];
    for r in &def.faces {
        let found = find_face(bodies, r).ok_or_else(|| {
            FeatureError(format!(
                "A face to draft no longer exists ({}). Edit the draft and pick the face \
                 again, or remove it from the list.",
                model.describe_face(&r.name)
            ))
        })?;
        if !faces[found.body].contains(&found.id) {
            faces[found.body].push(found.id);
        }
    }
    let angle = if def.flip { -angle } else { angle };
    let mut out = Vec::with_capacity(bodies.len());
    for (bi, body) in bodies.iter().enumerate() {
        if faces[bi].is_empty() {
            out.push(body.clone());
            continue;
        }
        let solid = draft(&body.solid, &faces[bi], neutral, angle.to_radians())
            .map_err(|e| sentence(&e))?;
        // The same faces, tilted: their names carry over one to one.
        out.push(changed(body, solid, body.face_names.clone(), seed, bi));
    }
    Ok((out, None))
}
