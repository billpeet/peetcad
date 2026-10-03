//! Imported bodies: solids read from another CAD system's file (STEP), kept in the model
//! as they came.
//!
//! An imported body has no history: it is a finished solid. It takes its place in the
//! feature tree like any other body-making feature, and later features (cuts, holes,
//! fillets) work on it. Its faces are named by their index in the imported solid, which
//! is as stable as the file it came from.

use std::sync::Arc;

use peet_kernel::Solid;
use serde::{Deserialize, Serialize};

use crate::FeatureError;
use crate::extrude::stamp;
use crate::feature::FeatureId;
use crate::naming::{Body, FaceName, FaceRole};
use crate::sheet::Applied;

/// One solid of an imported file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedSolid {
    /// Its name in the file.
    pub name: String,
    pub solid: Solid,
}

/// Bodies imported from a file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportFeature {
    /// The file they came from (its name, for display).
    pub source: String,
    pub solids: Vec<ImportedSolid>,
}

pub(crate) fn apply_import(
    feature: FeatureId,
    bodies: &[Arc<Body>],
    def: &ImportFeature,
    seed: u64,
) -> Result<Applied, FeatureError> {
    if def.solids.is_empty() {
        return Err(FeatureError(format!(
            "{} has no solid bodies to import.",
            if def.source.is_empty() {
                "The file"
            } else {
                &def.source
            }
        )));
    }
    let mut out = bodies.to_vec();
    for (i, imported) in def.solids.iter().enumerate() {
        // The first solid's faces are named plainly; the others say which solid they
        // belong to.
        let instance = FaceName::new(feature, FaceRole::Instance(i as u32));
        let face_names = (0..imported.solid.faces.len() as u32)
            .map(|f| {
                let name = FaceName::new(feature, FaceRole::Imported(f));
                if i == 0 {
                    name
                } else {
                    FaceName::merged([&name, &instance])
                }
            })
            .collect();
        out.push(Arc::new(Body {
            solid: imported.solid.clone(),
            face_names,
            origin: feature,
            stamp: stamp(seed, i),
            sheet: None,
        }));
    }
    Ok((out, None))
}
