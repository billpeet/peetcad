//! Saving and opening PeetCAD parts: what goes into the sections of a `.peet` file.
//!
//! | Section     | Contents                                         | Required |
//! |-------------|--------------------------------------------------|----------|
//! | metadata    | [`Metadata`]: who saved it, units                | yes      |
//! | model       | the [`Model`]: features and parameters           | yes      |
//! | B-rep cache | the bodies as last rebuilt, with face names      | no       |
//! | mesh cache  | their display meshes                             | no       |
//!
//! The model is the source of truth. The caches are tagged with the hash of the model
//! they were built from and are ignored if it doesn't match, so a file edited by hand (or
//! by the text round trip) can never show stale geometry. Saving with "minimum size"
//! leaves them out; the model is then rebuilt on open, which takes milliseconds.

use std::sync::Arc;

use peet_kernel::tessellate::SolidMesh;
use peet_model::{Body, Model};
use serde::{Deserialize, Serialize};

use crate::peet::{PeetError, Reader, SectionKind, Writer};

/// Schema versions of the sections this version writes and reads.
///
/// - Model 2 (Phase 4): sheet metal features, a unitless scalar kind. Version 1 models
///   decode unchanged (the new variants are appended).
/// - B-rep 2 (Phase 4): sheet metal face roles in face names.
/// - Model 3 (Phase 5): hems, sketched bends, jogs, mitre flanges, corners, forms,
///   patterns and mirrors; axes along edges; the copy role in face names. Version 1 and 2
///   models decode unchanged (every addition is a new variant at the end of its enum).
/// - B-rep 3 (Phase 5): the copy role in face names.
/// - Model 4 (Phase 6): revolves, fillets and chamfers, shells, draft, holes and imported
///   bodies. Earlier models decode unchanged (new variants at the end of the feature enum).
/// - B-rep 4 (Phase 6): cones, spheres and tori as surfaces; blend, shell and import roles
///   in face names. Earlier caches decode unchanged.
/// - Model 5 (configurations): the model holds its configurations. Earlier models have
///   another layout: they are read as [`peet_model::ModelV4`] and converted, to a part
///   with one configuration. Their caches are tagged with the hash of the model as it was
///   stored, so they no longer match and the part is rebuilt once.
/// - Model 6 (configurations stage 2): feature values and sketch dimensions can differ
///   between configurations, which the configurations hold in one more table. Schema 5
///   models are read as [`peet_model::ModelV5`] and converted.
pub const METADATA_SCHEMA: u16 = 1;
pub const MODEL_SCHEMA: u16 = 6;
/// The last model schema without configurations.
const MODEL_SCHEMA_BEFORE_CONFIGURATIONS: u16 = 4;
pub const BREP_SCHEMA: u16 = 4;
pub const MESH_SCHEMA: u16 = 1;

/// The file extension, without the dot.
pub const EXTENSION: &str = "peet";

/// Facts about a file that don't affect the geometry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// "PeetCAD 0.1.0".
    pub saved_by: String,
    /// Seconds since 1970, when known.
    pub saved_at: Option<u64>,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            saved_by: format!("PeetCAD {}", env!("CARGO_PKG_VERSION")),
            saved_at: None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct BrepCache {
    model_hash: u64,
    bodies: Vec<Body>,
}

/// The display mesh of one body, identified by the body's stamp.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedMesh {
    pub stamp: u64,
    pub mesh: SolidMesh,
}

#[derive(Serialize, Deserialize)]
struct MeshCache {
    model_hash: u64,
    meshes: Vec<CachedMesh>,
}

/// What to save besides the model.
pub struct Caches<'a> {
    /// The bodies of the last rebuild of exactly this model.
    pub bodies: &'a [Arc<Body>],
    /// Their display meshes.
    pub meshes: Vec<CachedMesh>,
}

/// Encodes a part. Pass `caches` for a file that opens instantly, `None` for the smallest
/// file.
pub fn save(
    model: &Model,
    metadata: &Metadata,
    caches: Option<Caches<'_>>,
) -> Result<Vec<u8>, PeetError> {
    let mut w = Writer::new();
    w.section(SectionKind::METADATA, METADATA_SCHEMA, metadata)?;
    w.section(SectionKind::MODEL, MODEL_SCHEMA, model)?;
    if let Some(c) = caches {
        let model_hash = peet_model::hash::of(model);
        let bodies: Vec<Body> = c.bodies.iter().map(|b| (**b).clone()).collect();
        w.section(
            SectionKind::BREP_CACHE,
            BREP_SCHEMA,
            &BrepCache { model_hash, bodies },
        )?;
        w.section(
            SectionKind::MESH_CACHE,
            MESH_SCHEMA,
            &MeshCache {
                model_hash,
                meshes: c.meshes,
            },
        )?;
    }
    w.try_finish()
}

/// A part read from a file.
#[derive(Debug)]
pub struct Opened {
    pub model: Model,
    pub metadata: Metadata,
    /// The bodies from the B-rep cache, if the file has one that matches the model.
    pub bodies: Option<Vec<Arc<Body>>>,
    /// Display meshes from the mesh cache, if it matches the model.
    pub meshes: Vec<CachedMesh>,
    /// Things that were skipped but didn't stop the file from opening.
    pub warnings: Vec<String>,
}

fn check_schema(r: &Reader<'_>, kind: SectionKind, newest: u16) -> Result<(), PeetError> {
    match r.schema_version(kind) {
        Some(v) if v > newest => Err(PeetError::new(format!(
            "This file was saved by a newer version of PeetCAD: its {} is version {v}, and this version reads up to {newest}. Update PeetCAD to open it.",
            kind.name()
        ))),
        _ => Ok(()),
    }
}

/// Decodes a part. Damaged or unusable caches are dropped with a warning; a damaged model
/// is an error.
pub fn open(bytes: &[u8]) -> Result<Opened, PeetError> {
    let r = Reader::new(bytes)?;
    check_schema(&r, SectionKind::METADATA, METADATA_SCHEMA)?;
    check_schema(&r, SectionKind::MODEL, MODEL_SCHEMA)?;
    let metadata: Metadata = r.read(SectionKind::METADATA)?.unwrap_or_default();
    let model: Option<Model> = match r.schema_version(SectionKind::MODEL) {
        Some(v) if v <= MODEL_SCHEMA_BEFORE_CONFIGURATIONS => r
            .read::<peet_model::ModelV4>(SectionKind::MODEL)?
            .map(Model::from),
        Some(5) => r
            .read::<peet_model::ModelV5>(SectionKind::MODEL)?
            .map(Model::from),
        _ => r.read(SectionKind::MODEL)?,
    };
    let mut model =
        model.ok_or_else(|| PeetError::new("The file is damaged: it has no model in it."))?;
    model.validate().map_err(PeetError::new)?;
    let model_hash = peet_model::hash::of(&model);
    let mut warnings = Vec::new();

    let mut bodies = None;
    if r.schema_version(SectionKind::BREP_CACHE)
        .is_some_and(|v| v <= BREP_SCHEMA)
    {
        match r.read::<BrepCache>(SectionKind::BREP_CACHE) {
            Ok(Some(c)) if c.model_hash == model_hash => {
                let valid = c
                    .bodies
                    .iter()
                    .all(|b| b.face_names.len() == b.solid.faces.len());
                if valid {
                    bodies = Some(c.bodies.into_iter().map(Arc::new).collect());
                } else {
                    warnings.push("The saved geometry is damaged; it is rebuilt.".to_owned());
                }
            }
            Ok(_) => {}
            Err(e) => warnings.push(format!("The saved geometry was skipped: {e}")),
        }
    }
    let mut meshes = Vec::new();
    if r.schema_version(SectionKind::MESH_CACHE)
        .is_some_and(|v| v <= MESH_SCHEMA)
    {
        match r.read::<MeshCache>(SectionKind::MESH_CACHE) {
            Ok(Some(c)) if c.model_hash == model_hash => meshes = c.meshes,
            Ok(_) => {}
            Err(e) => warnings.push(format!("The saved display meshes were skipped: {e}")),
        }
    }
    Ok(Opened {
        model,
        metadata,
        bodies,
        meshes,
        warnings,
    })
}

/// The readable form of a file, for diffs and debugging (`peetcad dump`).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Serialize, Deserialize)]
struct Text {
    metadata: Metadata,
    model: Model,
}

/// A `.peet` file as RON text: the metadata and the model (caches are left out).
#[cfg(not(target_arch = "wasm32"))]
pub fn to_text(bytes: &[u8]) -> Result<String, PeetError> {
    let opened = open(bytes)?;
    let text = Text {
        metadata: opened.metadata,
        model: opened.model,
    };
    ron::ser::to_string_pretty(&text, ron::ser::PrettyConfig::default())
        .map_err(|e| PeetError::new(format!("Couldn't write the text form: {e}")))
}

/// The reverse of [`to_text`]: a `.peet` file (without caches) from RON text.
#[cfg(not(target_arch = "wasm32"))]
pub fn from_text(text: &str) -> Result<Vec<u8>, PeetError> {
    let t: Text = ron::from_str(text)
        .map_err(|e| PeetError::new(format!("The text is not a valid PeetCAD part: {e}")))?;
    let mut model = t.model;
    model.validate().map_err(PeetError::new)?;
    save(&model, &t.metadata, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_model::samples::bracket;

    fn meshes(bodies: &[Arc<Body>]) -> Vec<CachedMesh> {
        bodies
            .iter()
            .map(|b| CachedMesh {
                stamp: b.stamp,
                mesh: peet_kernel::tessellate::tessellate(&b.solid, 0.1).unwrap(),
            })
            .collect()
    }

    #[test]
    fn round_trip_with_and_without_caches() {
        let (model, engine) = bracket();
        let bodies = &engine.evaluation().bodies;
        let meta = Metadata::default();
        let small = save(&model, &meta, None).unwrap();
        let full = save(
            &model,
            &meta,
            Some(Caches {
                bodies,
                meshes: meshes(bodies),
            }),
        )
        .unwrap();
        assert!(small.len() < full.len());
        println!(
            "20-feature bracket: {} bytes minimum, {} with caches",
            small.len(),
            full.len()
        );

        let o = open(&small).unwrap();
        assert_eq!(o.model, model);
        assert_eq!(o.metadata, meta);
        assert!(o.bodies.is_none() && o.meshes.is_empty());

        let o = open(&full).unwrap();
        assert_eq!(o.model, model);
        let cached = o.bodies.expect("the cache matches");
        assert_eq!(cached.len(), bodies.len());
        assert_eq!(cached[0].solid, bodies[0].solid);
        assert_eq!(cached[0].face_names, bodies[0].face_names);
        assert_eq!(o.meshes.len(), 1);
        assert_eq!(o.meshes[0].stamp, bodies[0].stamp);
        assert!(o.warnings.is_empty());
    }

    #[test]
    fn caches_of_another_model_are_ignored() {
        let (model, engine) = bracket();
        let bodies = &engine.evaluation().bodies;
        let mut other = model.clone();
        other.name = "Changed".to_owned();
        // Caches tagged with `model`, stored next to `other`.
        let mut w = Writer::new();
        w.section(SectionKind::METADATA, 1, &Metadata::default())
            .unwrap();
        w.section(SectionKind::MODEL, MODEL_SCHEMA, &other).unwrap();
        w.section(
            SectionKind::BREP_CACHE,
            1,
            &BrepCache {
                model_hash: peet_model::hash::of(&model),
                bodies: bodies.iter().map(|b| (**b).clone()).collect(),
            },
        )
        .unwrap();
        let o = open(&w.finish()).unwrap();
        assert_eq!(o.model, other);
        assert!(o.bodies.is_none());
    }

    #[test]
    fn errors_say_what_is_wrong() {
        assert!(
            open(b"hello")
                .unwrap_err()
                .message
                .contains("not a PeetCAD file")
        );
        let mut w = Writer::new();
        w.section(SectionKind::METADATA, 1, &Metadata::default())
            .unwrap();
        let e = open(&w.finish()).unwrap_err();
        assert!(e.message.contains("no model"), "{e}");
        let mut w = Writer::new();
        w.section(SectionKind::MODEL, MODEL_SCHEMA + 1, &Model::new())
            .unwrap();
        let e = open(&w.finish()).unwrap_err();
        assert!(e.message.contains("newer version"), "{e}");
    }

    #[test]
    fn text_round_trip() {
        let (model, _) = bracket();
        let bytes = save(&model, &Metadata::default(), None).unwrap();
        let text = to_text(&bytes).unwrap();
        assert!(text.contains("Extrude1"));
        let back = from_text(&text).unwrap();
        assert_eq!(open(&back).unwrap().model, model);
    }
}
