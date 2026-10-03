//! Converting a solid body to sheet metal on the document: which bodies can be converted,
//! which picked face stays fixed, and the feature itself as one undo step.
//!
//! As everywhere in this crate, nothing here knows about a user interface: the
//! application's command and a script call the same functions.

use peet_kernel::Surface;
use peet_model::{FaceRef, FeatureId};

use crate::body::GeomRef;
use crate::document::Document;

impl Document {
    /// Whether there is a body that is not sheet metal yet (one a conversion could take).
    pub fn has_plain_solid(&self) -> bool {
        self.evaluation().bodies.iter().any(|b| b.sheet.is_none())
    }

    /// The face a conversion to sheet metal keeps fixed, from what is selected: the
    /// first face among `picked`. `Ok(None)` if no face is selected (the feature then
    /// takes the largest flat face of the only body, or the face is picked afterwards).
    /// The error says why the selected face can't be used.
    pub fn convert_fixed_face(&self, picked: &[GeomRef]) -> Result<Option<FaceRef>, String> {
        let Some((body, face)) = picked.iter().find_map(|g| match *g {
            GeomRef::Face { body, face } => Some((body, face)),
            _ => None,
        }) else {
            return Ok(None);
        };
        let Some(view) = self.bodies.get(body) else {
            return Ok(None);
        };
        let source = &view.source;
        if source.sheet.is_some() {
            return Err(
                "That body is already sheet metal. Select a flat face of a solid body to convert."
                    .to_owned(),
            );
        }
        if !matches!(source.solid.face(face).surface, Surface::Plane(_)) {
            return Err(
                "The fixed face must be flat: select one of the part's flat walls (the one that stays in place when the part is unfolded)."
                    .to_owned(),
            );
        }
        Ok(Some(source.face_ref(face)))
    }

    /// Adds a conversion to sheet metal of the body `face` is on, unfolding from that
    /// face (none: the only body, from its largest flat face).
    pub fn add_convert_to_sheet(&mut self, face: Option<FaceRef>) -> Option<FeatureId> {
        let mut id = None;
        self.change("Add Convert to Sheet Metal", |m| {
            id = Some(m.add_convert_to_sheet(face));
        });
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_kernel::primitive::cuboid;
    use peet_math::DVec3;
    use peet_model::{ImportedSolid, Status};

    /// A document with a 60 × 40 × 2 plate imported as a plain solid.
    fn plate() -> Document {
        let mut doc = Document::default();
        doc.change("Import", |m| {
            m.add_import(
                "plate.step".to_owned(),
                vec![ImportedSolid {
                    name: "plate".to_owned(),
                    solid: cuboid(DVec3::ZERO, DVec3::new(60.0, 40.0, 2.0)),
                }],
            );
        });
        doc
    }

    fn face_with_normal(doc: &Document, n: DVec3) -> GeomRef {
        let body = &doc.bodies[0];
        let face = body
            .solid
            .face_ids()
            .find(|f| body.solid.face_normal_at(*f, DVec3::ZERO).dot(n) > 0.999)
            .expect("a face with that normal");
        GeomRef::Face { body: 0, face }
    }

    #[test]
    fn converts_the_selected_body_in_one_undo_step() {
        let mut doc = plate();
        assert!(doc.has_plain_solid());
        assert!(!doc.has_sheet_metal());
        assert_eq!(doc.convert_fixed_face(&[]), Ok(None));

        let top = face_with_normal(&doc, DVec3::Z);
        let face = doc.convert_fixed_face(&[top]).unwrap();
        assert!(face.is_some());
        let id = doc.add_convert_to_sheet(face).unwrap();
        assert_eq!(doc.status(id), Some(&Status::Ok));
        assert!(doc.has_sheet_metal());
        assert!(!doc.has_plain_solid());
        let sheet = doc.sheet_body(None).unwrap().sheet.clone().unwrap();
        assert!((sheet.report().thickness - 2.0).abs() < 1e-12);

        // A face of the converted body can't be converted again.
        let top = face_with_normal(&doc, DVec3::Z);
        let again = doc.convert_fixed_face(&[top]).unwrap_err();
        assert!(again.contains("already sheet metal"), "{again}");

        // One undo step takes the conversion away.
        assert!(doc.undo().is_some());
        assert!(!doc.has_sheet_metal());
        assert!(doc.feature(id).is_none());
    }

    #[test]
    fn with_nothing_selected_the_only_body_is_converted() {
        let mut doc = plate();
        let id = doc.add_convert_to_sheet(None).unwrap();
        assert_eq!(doc.status(id), Some(&Status::Ok));
        assert!(doc.has_sheet_metal());
    }

    #[test]
    fn curved_faces_are_refused() {
        let mut doc = Document::default();
        doc.change("Import", |m| {
            m.add_import(
                "ball.step".to_owned(),
                vec![ImportedSolid {
                    name: "ball".to_owned(),
                    solid: peet_kernel::primitive::ball(&peet_math::Frame::WORLD, 10.0)
                        .expect("a ball"),
                }],
            );
        });
        let face = GeomRef::Face {
            body: 0,
            face: peet_kernel::FaceId(0),
        };
        let m = doc.convert_fixed_face(&[face]).unwrap_err();
        assert!(m.contains("must be flat"), "{m}");
    }
}
