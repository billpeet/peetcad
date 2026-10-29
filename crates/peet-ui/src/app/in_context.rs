//! Editing a part in the context of its assembly, as SolidWorks' Edit Part: the part's
//! component stays highlighted among the others, whose faces, edges and corners can be
//! picked to make the part follow them (`in_context`): a sketch converts what was picked,
//! a sketch is put on a picked face, an extrusion runs up to one, a reference plane is made
//! from one. The part's external references are listed, locked and broken from its
//! context menu and its properties (`external_references`, `lock_references`,
//! `break_references`).
//!
//! What these do is done by operations (`peet_ops`), so a script does the same.

use egui::{RichText, Ui};
use peet_model::in_context::{Holder, RefStatus};
use peet_model::{CompId, FeatureKind, MateEnd, MateGeom, PlaneDef};
use peet_ops::{CompSel, FeatureSel, InContextOp, MadeIn, MateEndSel, Op};

use super::PeetApp;
use crate::features_ui::{ERROR, WARNING};

/// One of a part's in-context references, as the properties list it.
pub(super) struct ReferenceRow {
    pub feature: String,
    /// What follows: a sketch entity, or the feature's plane.
    pub held_by: String,
    pub follows: String,
    pub status: RefStatus,
}

/// What a feature of the part being edited in context can do with a picked face, edge or
/// corner of another component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Use {
    /// The sketch converts it: a fixed entity that follows it.
    Convert,
    /// The feature's plane is the picked flat face.
    Plane,
}

impl PeetApp {
    /// The component whose part is edited in context, if it is still there.
    pub(super) fn in_context_component(&self) -> Option<CompId> {
        let id = self.in_context?;
        let c = self.doc.model.assembly()?.component(id)?;
        Some(c.id)
    }

    /// Starts (or ends) editing the selected component's part in the context of the
    /// assembly.
    pub(super) fn edit_in_context(&mut self) {
        if self.in_context_component().is_some() {
            self.in_context = None;
            self.info("Back to the assembly.");
            return;
        }
        let Some(id) = self.selected_component() else {
            return;
        };
        let Some(assembly) = self.doc.model.assembly() else {
            return;
        };
        if assembly
            .definition_of(id)
            .is_some_and(|d| d.model.is_assembly())
        {
            return self.error(
                "That is a sub-assembly: edit one of its parts in context from its own assembly, or with 'in_context' and a component path.",
            );
        }
        let name = assembly.name_of(id).to_owned();
        self.in_context = Some(id);
        self.selected_geom.clear();
        self.info(format!(
            "Editing {name} in context: pick a face, edge or corner of another component, then what of {name} follows it, in the properties."
        ));
    }

    /// The face, edge or corner picked on another component than `own`, as an end.
    fn picked_in_context(&self, own: CompId) -> Result<MateEnd, String> {
        let [geom] = self.selected_geom[..] else {
            return Err(if self.selected_geom.is_empty() {
                "Pick a face, edge or corner of another component.".to_owned()
            } else {
                "Pick one face, edge or corner.".to_owned()
            });
        };
        let end = self
            .doc
            .placed
            .get(geom.body())
            .filter(|p| !p.path.is_empty())
            .and_then(|_| self.mate_end(geom))
            .ok_or("Pick a face, edge or corner of a component.")?;
        if end.component() == Some(own) {
            return Err("That is the part's own: pick another component's geometry.".to_owned());
        }
        Ok(end)
    }

    /// Makes the part edited in context follow what was picked.
    fn follow_picked(
        &mut self,
        own: CompId,
        feature: peet_model::FeatureId,
        how: Use,
        end: MateEnd,
    ) {
        let target = match how {
            Use::Convert => MadeIn::Sketch(FeatureSel::Id(feature)),
            Use::Plane => MadeIn::Plane(FeatureSel::Id(feature)),
        };
        let reply = self.perform(Op::InContext(InContextOp::Make {
            component: vec![CompSel::Id(own)],
            target,
            from: Box::new(MateEndSel::Ref(end)),
            entity: None,
            construction: true,
        }));
        if reply.ok {
            let follows = reply.json["in_context"]["follows"]
                .as_array()
                .map(|n| {
                    n.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default();
            self.info(format!(
                "{} follows {follows} in this assembly.",
                reply.json["in_context"]["feature"].as_str().unwrap_or("It")
            ));
            self.selected_geom.clear();
        }
    }

    /// Locks, unlocks (`Some`) or breaks (`None`) the in-context references of a
    /// component's part.
    pub(super) fn change_references(&mut self, id: CompId, lock: Option<bool>) {
        let component = Some(vec![CompSel::Id(id)]);
        let reply = self.perform(Op::InContext(match lock {
            Some(on) => InContextOp::Lock {
                component,
                feature: None,
                on,
            },
            None => InContextOp::Break {
                component,
                feature: None,
            },
        }));
        if reply.ok {
            let n = reply.json["changed"].as_u64().unwrap_or(0);
            self.info(match (lock, n) {
                (_, 0) => "Nothing to change.".to_owned(),
                (Some(true), n) => format!("Locked {n} reference(s): they stay where they are."),
                (Some(false), n) => format!("Unlocked {n} reference(s): they follow again."),
                (None, n) => format!("Broke {n} reference(s): they follow nothing from now on."),
            });
        }
    }

    /// The in-context references of a component's part, with how each stands here.
    pub(super) fn reference_rows(&self, id: CompId) -> Vec<ReferenceRow> {
        let Some(part) = self
            .doc
            .model
            .assembly()
            .and_then(|a| a.definition_of(id))
            .map(|d| d.model.clone())
        else {
            return Vec::new();
        };
        let evaluation = self.doc.evaluation();
        part.external_references()
            .into_iter()
            .map(|r| {
                let status = evaluation
                    .external
                    .iter()
                    .find(|s| s.path == [id] && s.feature == r.feature && s.holder == r.holder)
                    .map_or_else(|| RefStatus::kept(r.external), |s| s.status.clone());
                ReferenceRow {
                    feature: part.name_of(r.feature).to_owned(),
                    held_by: match r.holder {
                        Holder::Entity(e) => format!("entity {}", e.0),
                        Holder::Plane(_) => r.role.replace('_', " "),
                    },
                    follows: r.external.describe(),
                    status,
                }
            })
            .collect()
    }

    /// A component's part's in-context references, with buttons to lock, unlock and
    /// break them (SolidWorks' List External References).
    pub(super) fn external_references_ui(&mut self, ui: &mut Ui, id: CompId) {
        let rows = self.reference_rows(id);
        if rows.is_empty() {
            return;
        }
        let mut change = None;
        egui::CollapsingHeader::new(format!("External references ({})", rows.len()))
            .id_salt(("external_references", id.0))
            .default_open(true)
            .show(ui, |ui| {
                for row in &rows {
                    let text = format!(
                        "{} ({}) -> {}: {}",
                        row.feature,
                        row.held_by,
                        row.follows,
                        row.status.word().replace('_', " ")
                    );
                    let label = match &row.status {
                        RefStatus::Dangling(_) => ui.colored_label(ERROR, text),
                        RefStatus::Locked | RefStatus::Broken => ui.colored_label(WARNING, text),
                        _ => ui.label(text),
                    };
                    if let RefStatus::Dangling(why) = &row.status {
                        label.on_hover_text(why);
                    }
                }
                ui.horizontal(|ui| {
                    if ui
                        .button("Lock")
                        .on_hover_text(
                            "Keep them where they are, in this assembly too, until unlocked.",
                        )
                        .clicked()
                    {
                        change = Some(Some(true));
                    }
                    if ui
                        .button("Unlock")
                        .on_hover_text("Let them follow again.")
                        .clicked()
                    {
                        change = Some(Some(false));
                    }
                    if ui
                        .button("Break")
                        .on_hover_text("Leave them where they are for good, following nothing.")
                        .clicked()
                    {
                        change = Some(None);
                    }
                });
            });
        if let Some(lock) = change {
            self.change_references(id, lock);
        }
    }

    /// The properties while a part is edited in context: what was picked, and what of the
    /// part can follow it.
    pub(super) fn in_context_properties(&mut self, ui: &mut Ui, own: CompId) {
        let Some(assembly) = self.doc.model.assembly() else {
            return;
        };
        let name = assembly.name_of(own).to_owned();
        let Some(part) = assembly.definition_of(own).map(|d| d.model.clone()) else {
            return;
        };
        ui.horizontal(|ui| {
            ui.strong(format!("Editing {name} in context"));
            if ui.button("Done").clicked() {
                self.in_context = None;
            }
        });
        if self.in_context.is_none() {
            return;
        }
        let picked = self.picked_in_context(own);
        let flat = matches!(
            &picked,
            Ok(MateEnd {
                geom: Some(MateGeom::Face(_)),
                ..
            })
        );
        match &picked {
            Ok(end) => {
                let names = end
                    .path
                    .iter()
                    .scan(Some(self.doc.model.clone()), |at, id| {
                        let model = at.take()?;
                        let a = model.assembly()?;
                        let name = a.name_of(*id).to_owned();
                        *at = a.component_model(*id).map(|m| (*m).clone());
                        Some(name)
                    })
                    .collect::<Vec<_>>()
                    .join(" / ");
                let what = match &end.geom {
                    Some(MateGeom::Face(_)) => "a face",
                    Some(MateGeom::Edge(_)) => "an edge",
                    Some(MateGeom::Vertex(_)) => "a corner",
                    _ => "geometry",
                };
                ui.label(format!("Picked {what} of {names}."));
            }
            Err(why) => {
                ui.weak(why);
            }
        }
        ui.add_space(4.0);
        let mut chosen = None;
        egui::Grid::new(("in_context_features", own.0))
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for f in part.features() {
                    let uses: Vec<(Use, &str, &str)> = match &f.kind {
                        FeatureKind::Sketch(_) => vec![
                            (Use::Convert, "Convert", "Add what was picked to the sketch, seen on its plane, as a fixed construction entity that follows it (SolidWorks' Convert Entities in context)."),
                            (Use::Plane, "On it", "Put the sketch on the picked flat face: it follows the face wherever it goes."),
                        ],
                        FeatureKind::Extrude(_) => vec![(
                            Use::Plane,
                            "Up to it",
                            "End the extrusion at the picked flat face: it follows the face wherever it goes.",
                        )],
                        FeatureKind::Plane(PlaneDef::Offset { .. } | PlaneDef::Angled { .. }) => vec![(
                            Use::Plane,
                            "From it",
                            "Make the plane from the picked flat face: it follows the face wherever it goes.",
                        )],
                        _ => Vec::new(),
                    };
                    if uses.is_empty() {
                        continue;
                    }
                    ui.label(RichText::new(&f.name));
                    ui.horizontal(|ui| {
                        for (how, label, hint) in uses {
                            let enabled = picked.is_ok() && (how == Use::Convert || flat);
                            if ui
                                .add_enabled(enabled, egui::Button::new(label))
                                .on_hover_text(hint)
                                .clicked()
                            {
                                chosen = Some((f.id, how));
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        if let (Some((feature, how)), Ok(end)) = (chosen, picked) {
            self.follow_picked(own, feature, how, end);
        }
        ui.add_space(6.0);
        self.external_references_ui(ui, own);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bodies::GeomRef;
    use crate::commands::CommandId;
    use peet_math::DVec3;
    use peet_ops::{Undo, parse};
    use serde_json::{Value, json};

    fn ok(app: &mut PeetApp, ctx: &egui::Context, op: Value) -> Value {
        let parsed = parse(&app.doc, &op).unwrap_or_else(|e| panic!("{op}: {e}"));
        let reply = app.apply_op(ctx, &parsed, Undo::Step);
        assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
        reply.json
    }

    /// A flat face of a shown body whose outward normal (in its part) is `normal`.
    fn flat_face(app: &PeetApp, body: usize, normal: DVec3) -> GeomRef {
        let solid = &app.doc.bodies[body].solid;
        let face = solid
            .face_ids()
            .find(|f| {
                let face = solid.face(*f);
                match &face.surface {
                    peet_kernel::Surface::Plane(p) => {
                        let n = if face.reversed {
                            -p.normal()
                        } else {
                            p.normal()
                        };
                        n.abs_diff_eq(normal, 1e-9)
                    }
                    _ => false,
                }
            })
            .expect("a flat face that way");
        GeomRef::Face { body, face }
    }

    /// Draws the tree and the properties for a few frames.
    fn draw(app: &mut PeetApp) {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui(|ui| {
                let mut pending = Vec::new();
                app.feature_tree(ui);
                app.properties(ui, &mut pending);
                assert!(pending.is_empty());
            });
        harness.run_steps(3);
    }

    #[test]
    fn a_part_is_edited_in_the_context_of_its_assembly() {
        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        app.execute(&ctx, CommandId::NewAssembly);
        ok(
            &mut app,
            &ctx,
            json!({"op": "insert", "sample": "bracket", "fixed": true}),
        );
        ok(
            &mut app,
            &ctx,
            json!({"op": "insert", "component": "Bracket-1", "at": [200, 0, 0], "fixed": true}),
        );
        let (first, second) = (CompId(1), CompId(2));
        // Nothing selected: nothing to edit in context.
        assert!(!app.command_state(CommandId::EditInContext).enabled);
        app.selected_component = Some(second);
        assert!(app.command_state(CommandId::EditInContext).enabled);
        app.execute(&ctx, CommandId::EditInContext);
        assert_eq!(app.in_context_component(), Some(second));

        // Its own face is refused; another component's is what it can follow.
        app.selected_geom = vec![flat_face(&app, 1, DVec3::NEG_Z)];
        assert!(app.picked_in_context(second).is_err());
        app.selected_geom = vec![flat_face(&app, 0, DVec3::NEG_Y)];
        let end = app
            .picked_in_context(second)
            .expect("the first bracket's face");
        assert_eq!(end.component(), Some(first));
        let part = app
            .doc
            .model
            .assembly()
            .unwrap()
            .definition_of(second)
            .unwrap()
            .model
            .clone();
        let sketch = part.features().find(|f| f.sketch().is_some()).unwrap().id;
        draw(&mut app);
        app.follow_picked(second, sketch, Use::Convert, end);
        draw(&mut app);
        assert!(app.selected_geom.is_empty(), "{:?}", app.status_message);
        let rows = app.reference_rows(second);
        assert_eq!(rows.len(), 1, "one reference");
        assert_eq!(rows[0].status, RefStatus::InContext);
        // The other instance of the same part has it too, out of context.
        assert_eq!(app.reference_rows(first)[0].status, RefStatus::OutOfContext);

        // Locked, then broken, from the part's context menu.
        app.change_references(second, Some(true));
        assert_eq!(app.reference_rows(second)[0].status, RefStatus::Locked);
        app.change_references(second, None);
        assert!(app.reference_rows(second).is_empty());

        // Again to stop.
        app.execute(&ctx, CommandId::EditInContext);
        assert_eq!(app.in_context_component(), None);
    }
}
