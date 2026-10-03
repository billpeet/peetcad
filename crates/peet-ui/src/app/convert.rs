//! The application's side of Convert to Sheet Metal: the command turns the selection into
//! the fixed face and asks the document (`peet_document`) for the feature, so a script
//! does the same.

use peet_ops::{ConvertToSheet, FeatureArgs};

use super::PeetApp;
use crate::document::ItemId;
use crate::features_ui::Slot;

impl PeetApp {
    /// Converts a solid body to sheet metal: the body of the selected flat face, which
    /// stays fixed, or the only body. With several bodies and no face selected, the
    /// fixed face is picked next.
    pub(super) fn start_convert_to_sheet(&mut self) {
        let face = match self.doc.convert_fixed_face(&self.selected_geom) {
            Ok(face) => face,
            Err(m) => {
                self.error(m);
                return;
            }
        };
        let pick = face.is_none() && self.doc.bodies.len() > 1;
        let id = self.add_by_operation(FeatureArgs::ConvertToSheet(ConvertToSheet {
            face: Some(face.map(Into::into)),
            ..Default::default()
        }));
        // The rebuild replaced the bodies: what was selected on them is gone.
        self.restore_selection(&[]);
        self.selected = id.map(ItemId::Feature);
        if pick && let Some(id) = id {
            self.picking = Some((id, Slot::ConvertFace));
            self.status_message = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bodies::GeomRef;
    use crate::commands::CommandId;
    use crate::features_ui::{Picked, apply_pick};
    use peet_kernel::primitive::cuboid;
    use peet_math::DVec3;
    use peet_model::{FeatureKind, ImportedSolid, Status};

    /// The app with plates imported as plain solids, each 2 mm thick, side by side.
    fn app_with_plates(count: usize) -> PeetApp {
        let mut app = PeetApp::headless();
        app.doc.change("Import", |m| {
            let solids = (0..count)
                .map(|i| {
                    let x = 80.0 * i as f64;
                    ImportedSolid {
                        name: format!("plate {i}"),
                        solid: cuboid(DVec3::new(x, 0.0, 0.0), DVec3::new(x + 60.0, 40.0, 2.0)),
                    }
                })
                .collect();
            m.add_import("plates.step".to_owned(), solids);
        });
        app
    }

    fn face(app: &PeetApp, body: usize, normal: DVec3) -> GeomRef {
        let b = &app.doc.bodies[body];
        let face = b
            .solid
            .face_ids()
            .find(|f| b.solid.face_normal_at(*f, DVec3::ZERO).dot(normal) > 0.999)
            .expect("a face with that normal");
        GeomRef::Face { body, face }
    }

    fn run(app: &mut PeetApp, cmd: CommandId) {
        assert!(app.command_state(cmd).enabled, "{cmd:?} is disabled");
        app.execute(&egui::Context::default(), cmd);
    }

    #[test]
    fn needs_a_solid_body() {
        let app = PeetApp::headless();
        assert!(!app.command_state(CommandId::ConvertToSheet).enabled);
        let info = CommandId::ConvertToSheet.info();
        assert_eq!(info.category, "Sheet Metal");
    }

    #[test]
    fn converts_from_the_selected_face() {
        let mut app = app_with_plates(1);
        app.selected_geom = vec![face(&app, 0, DVec3::Z)];
        run(&mut app, CommandId::ConvertToSheet);
        let id = app.selected_feature().expect("the new feature is selected");
        let Some(FeatureKind::ConvertToSheet(c)) = app.doc.feature(id).map(|f| &f.kind) else {
            panic!("not a conversion");
        };
        assert!(c.face.is_some());
        assert_eq!(app.doc.status(id), Some(&Status::Ok));
        assert!(app.doc.has_sheet_metal());
        assert!(app.selected_geom.is_empty());
        assert!(app.picking.is_none());
        // Nothing is left to convert, and the sheet metal commands are there.
        assert!(!app.command_state(CommandId::ConvertToSheet).enabled);
        assert!(app.command_state(CommandId::FlatPattern).enabled);
        assert!(app.command_state(CommandId::EdgeFlange).enabled);
    }

    #[test]
    fn the_only_body_needs_no_selection() {
        let mut app = app_with_plates(1);
        run(&mut app, CommandId::ConvertToSheet);
        let id = app.selected_feature().unwrap();
        assert_eq!(app.doc.status(id), Some(&Status::Ok));
        assert!(app.picking.is_none());
    }

    #[test]
    fn with_several_bodies_the_fixed_face_is_picked_next() {
        let mut app = app_with_plates(2);
        run(&mut app, CommandId::ConvertToSheet);
        let id = app.selected_feature().unwrap();
        assert_eq!(app.picking, Some((id, Slot::ConvertFace)));
        assert!(matches!(app.doc.status(id), Some(Status::Failed(_))));

        // Clicking a flat face of the second plate fills the slot.
        let picked = face(&app, 1, DVec3::Z);
        let GeomRef::Face { body, face } = picked else {
            unreachable!()
        };
        let reference = app.doc.bodies[body].source.face_ref(face);
        let mut kind = app.doc.feature(id).unwrap().kind.clone();
        apply_pick(
            &mut kind,
            Slot::ConvertFace,
            Picked::Face {
                face: reference.clone(),
                planar: true,
                round: false,
            },
        )
        .unwrap();
        // A curved face is refused with a reason.
        let curved = apply_pick(
            &mut kind.clone(),
            Slot::ConvertFace,
            Picked::Face {
                face: reference,
                planar: false,
                round: true,
            },
        );
        assert!(curved.unwrap_err().contains("flat"));
        app.doc.change("Pick", |m| {
            if let Some(f) = m.feature_mut(id) {
                f.kind = kind;
            }
        });
        assert_eq!(app.doc.status(id), Some(&Status::Ok));
        assert!(app.doc.bodies[0].source.sheet.is_none());
        assert!(app.doc.bodies[1].source.sheet.is_some());
    }

    #[test]
    fn a_curved_selection_is_explained() {
        let mut app = PeetApp::headless();
        app.doc.change("Import", |m| {
            m.add_import(
                "ball.step".to_owned(),
                vec![ImportedSolid {
                    name: "ball".to_owned(),
                    solid: peet_kernel::primitive::ball(&peet_math::Frame::WORLD, 10.0)
                        .expect("a ball"),
                }],
            );
        });
        app.selected_geom = vec![GeomRef::Face {
            body: 0,
            face: peet_kernel::FaceId(0),
        }];
        let before = app.doc.model.len();
        run(&mut app, CommandId::ConvertToSheet);
        assert_eq!(app.doc.model.len(), before, "nothing was added");
        let (message, is_error) = app.status_message.clone().expect("a message");
        assert!(is_error && message.contains("must be flat"), "{message}");
    }
}
