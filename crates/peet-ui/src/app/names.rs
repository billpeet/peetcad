//! Naming the selected face or edge from its properties.
use super::{PeetApp, Persistent};
use crate::bodies::GeomRef;
use egui::Ui;
use peet_model::naming::NamedGeometry;
use peet_ops::Op;

#[derive(Default)]
pub(super) struct NameEditor {
    target: Option<Persistent>,
    name: String,
    error: Option<String>,
}

impl PeetApp {
    /// With no selection, names can still be removed when their geometry is unavailable.
    pub(super) fn existing_geometry_names_ui(&mut self, ui: &mut Ui) {
        self.geometry_name_editor = NameEditor::default();
        if self.doc.model.geometry_names.is_empty() {
            return;
        }
        ui.separator();
        ui.strong("Named geometry");
        let names = self.doc.model.geometry_names.clone();
        let mut remove = None;
        for (name, target) in names {
            let (kind, reference) = match target {
                NamedGeometry::Face(r) => ("Face", Persistent::Face(r)),
                NamedGeometry::Edge(r) => ("Edge", Persistent::Edge(r)),
            };
            ui.label(&name);
            ui.horizontal(|ui| {
                if ui.small_button(format!("Remove name {name}")).clicked() {
                    remove = Some(name);
                }
                ui.weak(if self.doc.restore(&reference).is_some() {
                    kind.to_owned()
                } else {
                    format!("{kind} unavailable")
                });
            });
        }
        if let Some(name) = remove {
            self.perform(Op::DeleteName { name });
        }
    }

    pub(super) fn geometry_names_ui(&mut self, ui: &mut Ui) {
        let [geometry] = self.selected_geom[..] else {
            self.geometry_name_editor = NameEditor::default();
            if self
                .selected_geom
                .iter()
                .any(|g| !matches!(g, GeomRef::Vertex { .. }))
            {
                ui.weak("Select one face or edge to assign a name.");
            }
            return;
        };
        let target = match self.doc.persist(geometry) {
            Some(target @ (Persistent::Face(_) | Persistent::Edge(_))) => target,
            _ => {
                self.geometry_name_editor = NameEditor::default();
                return;
            }
        };
        if self.geometry_name_editor.target.as_ref() != Some(&target) {
            self.geometry_name_editor = NameEditor {
                target: Some(target.clone()),
                ..Default::default()
            };
        }
        let names: Vec<String> = self
            .doc
            .model
            .geometry_names
            .iter()
            .filter_map(|(name, target)| {
                let reference = match target {
                    NamedGeometry::Face(r) => Persistent::Face(r.clone()),
                    NamedGeometry::Edge(r) => Persistent::Edge(r.clone()),
                };
                (self.doc.restore(&reference) == Some(geometry)).then(|| name.clone())
            })
            .collect();
        ui.separator();
        ui.strong("Names");
        let mut action = None;
        for name in names {
            ui.horizontal(|ui| {
                ui.label(&name);
                if ui.small_button(format!("Remove name {name}"))
                    .on_hover_text("Remove this name. The geometry and existing sketches or mates stay in place.")
                    .clicked() {
                    action = Some(Op::DeleteName { name });
                }
            });
        }
        let editor = &mut self.geometry_name_editor;
        let label = ui.label("New name");
        let response = ui
            .add(
                egui::TextEdit::singleline(&mut editor.name)
                    .id_salt("geometry_name")
                    .desired_width(ui.available_width()),
            )
            .labelled_by(label.id);
        if response.changed() {
            editor.error = None;
        }
        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let add = ui
            .add_enabled(
                !editor.name.trim().is_empty(),
                egui::Button::new("Add name"),
            )
            .clicked();
        if (add || enter) && !editor.name.trim().is_empty() {
            let name = editor.name.trim().to_owned();
            action = Some(match target {
                Persistent::Face(face) => Op::NameFace {
                    face: face.into(),
                    name,
                },
                Persistent::Edge(edge) => Op::NameEdge {
                    edge: edge.into(),
                    name,
                },
                Persistent::Vertex(_) => unreachable!(),
            });
        }
        ui.weak("Names are case-sensitive and shared by all configurations.");
        if let Some(op) = action {
            let adding = matches!(op, Op::NameFace { .. } | Op::NameEdge { .. });
            let reply = self.perform_quietly(op);
            if reply.ok {
                self.geometry_name_editor.error = None;
                if adding {
                    self.geometry_name_editor.name.clear();
                }
            } else {
                self.geometry_name_editor.error = reply.json["error"].as_str().map(str::to_owned);
            }
        }
        if let Some(error) = &self.geometry_name_editor.error {
            ui.colored_label(crate::features_ui::ERROR, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    use peet_math::DVec2;
    use peet_model::{Operation, PlaneRef, StdPlane};

    fn block() -> PeetApp {
        let mut app = PeetApp::headless();
        app.doc.change("Block", |m| {
            let sketch = m.add_sketch(PlaneRef::Standard(StdPlane::Top), StdPlane::Top.plane());
            peet_sketch::shapes::rectangle(
                &mut m.feature_mut(sketch).unwrap().sketch_mut().unwrap().sketch,
                DVec2::ZERO,
                DVec2::new(60.0, 40.0),
            );
            m.add_extrude(sketch, Operation::Add);
        });
        app
    }

    #[test]
    fn properties_add_remove_names_through_operations_and_keep_selection() {
        let mut app = block();
        let face = app.doc.bodies[0].solid.face_ids().next().unwrap();
        app.selected_geom = vec![GeomRef::Face { body: 0, face }];
        let mut h = Harness::builder()
            .with_size(egui::vec2(380.0, 700.0))
            .build_ui_state(
                |ui, app: &mut PeetApp| {
                    app.properties(ui, &mut Vec::new());
                },
                app,
            );
        h.run();
        h.get_by_label("New name").click();
        h.run();
        h.get_by_label("New name").type_text("Front");
        h.run();
        h.get_by_label("Add name").click();
        h.run();
        assert!(
            matches!(h.state().journal().last(),Some(Op::NameFace { name, .. }) if name == "Front")
        );
        assert!(h.state().doc.model.geometry_names.contains_key("Front"));
        assert_eq!(h.state().selected_geom.len(), 1);
        h.get_by_label("Remove name Front").click();
        h.run();
        assert!(!h.state().doc.model.geometry_names.contains_key("Front"));
        assert!(
            matches!(h.state().journal().last(),Some(Op::DeleteName { name }) if name == "Front")
        );
        h.state_mut().perform(Op::Undo);
        h.run();
        assert!(h.query_by_label("Remove name Front").is_some());
        let edge = h.state().doc.bodies[0].solid.edge_ids().next().unwrap();
        h.state_mut().selected_geom = vec![GeomRef::Edge { body: 0, edge }];
        h.run();
        h.get_by_label("New name").click();
        h.run();
        h.get_by_label("New name").type_text("Front");
        h.run();
        h.get_by_label("Add name").click();
        h.run();
        assert!(
            h.state()
                .geometry_name_editor
                .error
                .as_deref()
                .unwrap()
                .contains("already named")
        );
        assert!(matches!(
            h.state().doc.model.geometry_names["Front"],
            NamedGeometry::Face(_)
        ));
        h.state_mut().geometry_name_editor.name = "Corner".into();
        h.run();
        h.get_by_label("Add name").click();
        h.run();
        assert!(
            matches!(h.state().journal().last(),Some(Op::NameEdge { name, .. }) if name == "Corner")
        );
        assert!(h.query_by_label("Remove name Corner").is_some());
    }

    #[test]
    fn names_of_deleted_geometry_can_be_removed_without_selection() {
        let mut app = block();
        let face = app.doc.bodies[0].solid.face_ids().next().unwrap();
        let reference = app.doc.bodies[0].source.face_ref(face);
        app.perform(Op::NameFace {
            face: reference.into(),
            name: "Front".into(),
        });
        let extrude = app.doc.model.features().last().unwrap().id;
        app.perform(Op::Delete {
            features: vec![extrude.into()],
        });
        app.selected_geom.clear();
        let mut h = Harness::builder()
            .with_size(egui::vec2(380.0, 700.0))
            .build_ui_state(
                |ui, app: &mut PeetApp| {
                    app.properties(ui, &mut Vec::new());
                },
                app,
            );
        h.run();
        assert!(h.query_by_label("Face unavailable").is_some());
        h.get_by_label("Remove name Front").click();
        h.run();
        assert!(h.state().doc.model.geometry_names.is_empty());
        assert!(matches!(
            h.state().journal().last(),
            Some(Op::DeleteName { .. })
        ));
    }

    #[test]
    fn vertices_and_multiple_selections_do_not_offer_naming() {
        let mut app = block();
        let vertex = app.doc.bodies[0].solid.vertex_ids().next().unwrap();
        app.selected_geom = vec![GeomRef::Vertex { body: 0, vertex }];
        let mut h = Harness::builder()
            .with_size(egui::vec2(380.0, 700.0))
            .build_ui_state(
                |ui, app: &mut PeetApp| {
                    app.properties(ui, &mut Vec::new());
                },
                app,
            );
        h.run();
        assert!(h.query_by_label("Add name").is_none());
        let face = h.state().doc.bodies[0].solid.face_ids().next().unwrap();
        h.state_mut()
            .selected_geom
            .push(GeomRef::Face { body: 0, face });
        h.run();
        assert!(h.query_by_label("Add name").is_none());
        assert!(
            h.query_by_label("Select one face or edge to assign a name.")
                .is_some()
        );
    }
}
