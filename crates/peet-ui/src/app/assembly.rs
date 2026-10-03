//! The application's side of assemblies: the component tree, a component's properties,
//! and the commands that start an assembly, insert a part and open a component's part.
//!
//! What these do to the assembly is done by operations (`peet_ops`), so a script does
//! the same; this file turns clicks into them and shows the results.

use egui::{RichText, Ui};
use peet_math::{DQuat, DVec3};
use peet_model::{CompId, Status};
use peet_ops::{CompSel, ComponentChange, InsertSource, Op, Placing, Source};

use super::PeetApp;
use super::scripting::{Coverage, coverage};
use crate::commands::CommandId;
use crate::features_ui::{ERROR, WARNING};

/// Undo keys of the drags in a component's properties (one step per drag).
const MOVE_KEY: u64 = 0x636f_6d70_6d6f_7665;

/// What a click in the component tree asks for.
enum TreeAction {
    Select(CompId),
    Open(CompId),
    Change(CompId, ComponentChange),
}

/// Whether a command works in an assembly. The others are about a part's features and
/// bodies, and are disabled there.
pub(super) fn works_in_assembly(cmd: CommandId) -> bool {
    match coverage(cmd) {
        Coverage::App(_) => true,
        Coverage::Op(word) => {
            matches!(
                word,
                "undo"
                    | "redo"
                    | "new"
                    | "open"
                    | "save"
                    | "open_sample"
                    | "parameters"
                    | "materials"
                    | "insert"
                    | "open_component"
                    | "delete"
                    | "suppress"
            ) || cmd == CommandId::ExportStep
        }
    }
}

/// Whether a command works only in an assembly.
pub(super) fn needs_assembly(cmd: CommandId) -> bool {
    matches!(cmd, CommandId::InsertComponent | CommandId::EditComponent)
}

impl PeetApp {
    /// The selected component, if the document is an assembly that still has it.
    pub(super) fn selected_component(&self) -> Option<CompId> {
        let id = self.selected_component?;
        self.doc.model.assembly()?.component(id).map(|c| c.id)
    }

    /// Asks for a part file to insert as a component.
    pub(super) fn start_insert_component(&mut self) {
        self.inserting = Some(peet_platform::open_file(crate::files::FILTER));
    }

    /// Inserts the file the user chose, beside what is already there.
    pub(super) fn poll_insert(&mut self) {
        let Some(result) = self
            .inserting
            .as_ref()
            .and_then(peet_platform::Pending::take)
        else {
            return;
        };
        self.inserting = None;
        let file = match result {
            Ok(Some(file)) => file,
            Ok(None) => return,
            Err(e) => return self.error(e),
        };
        // To the right of everything so far, so it is not put inside another component.
        let bounds = self.doc.visible_body_bounds();
        let at = if bounds.min.cmple(bounds.max).all() {
            DVec3::new(bounds.max.x + 0.25 * bounds.size().x.max(20.0), 0.0, 0.0)
        } else {
            DVec3::ZERO
        };
        let reply = self.perform(Op::Insert {
            from: InsertSource::File(Source::loaded(file.name, file.path, file.bytes)),
            name: None,
            placing: Some(Placing::Frame(peet_math::Frame {
                origin: at,
                ..peet_math::Frame::WORLD
            })),
            fixed: None,
        });
        if reply.ok {
            let id = reply.json["component"]["id"]
                .as_u64()
                .map(|i| CompId(i as u32));
            self.selected_component = id;
            self.initial_fit_done = false;
            if let Some(name) = reply.json["component"]["name"].as_str() {
                self.info(format!(
                    "Inserted {name}. Its position is in the properties; Edit Part opens the part."
                ));
            }
        }
    }

    /// Opens the selected component's part as a document of its own.
    pub(super) fn open_selected_component(&mut self) {
        if let Some(id) = self.selected_component() {
            self.open_component(id);
        }
    }

    fn open_component(&mut self, id: CompId) {
        let reply = self.perform(Op::OpenComponent {
            component: CompSel::Id(id),
        });
        if reply.ok {
            self.info(format!(
                "Editing {}. Save stores it back in the assembly; close its tab to return.",
                self.doc.title()
            ));
        }
    }

    pub(super) fn change_component(&mut self, id: CompId, change: ComponentChange) {
        let deleted = change == ComponentChange::Delete;
        let reply = self.perform(Op::Component {
            component: CompSel::Id(id),
            change,
        });
        if reply.ok && deleted && self.selected_component == Some(id) {
            self.selected_component = None;
        }
    }

    /// The tree of an assembly: its components, in the order they were inserted.
    pub(super) fn assembly_tree(&mut self, ui: &mut Ui) {
        let Some(assembly) = self.doc.model.assembly() else {
            return;
        };
        if assembly.components().len() == 0 {
            ui.weak(
                "No components yet. Insert Part (on the File tab) adds a part to the assembly.",
            );
            return;
        }
        let mut actions = Vec::new();
        for c in assembly.components() {
            let status = self.doc.evaluation().component_status(c.id);
            let mut text = RichText::new(format!(
                "{}{}{}",
                if c.fixed { "(f) " } else { "" },
                c.name,
                if c.visible { "" } else { "  (hidden)" }
            ));
            text = match status {
                Some(Status::Failed(_)) => text.color(ERROR),
                Some(Status::Warning(_)) => text.color(WARNING),
                Some(Status::Suppressed) => text.weak().strikethrough(),
                _ => text,
            };
            let selected = self.selected_component == Some(c.id);
            let mut r = ui.selectable_label(selected, text);
            if let Some(message) = status.and_then(Status::message) {
                r = r.on_hover_text(message);
            }
            if r.hovered() {
                self.hovered_component = Some(c.id);
            }
            if r.clicked() {
                actions.push(TreeAction::Select(c.id));
            }
            if r.double_clicked() {
                actions.push(TreeAction::Open(c.id));
            }
            r.context_menu(|ui| {
                let mut pick = |ui: &mut Ui, label: &str, action: TreeAction| {
                    if ui.button(label).clicked() {
                        actions.push(action);
                        ui.close();
                    }
                };
                pick(ui, "Edit Part", TreeAction::Open(c.id));
                ui.separator();
                let change = |change| TreeAction::Change(c.id, change);
                pick(
                    ui,
                    if c.fixed { "Float" } else { "Fix" },
                    change(ComponentChange::Fix(!c.fixed)),
                );
                pick(
                    ui,
                    if c.visible { "Hide" } else { "Show" },
                    change(ComponentChange::Show(!c.visible)),
                );
                pick(
                    ui,
                    if c.suppressed {
                        "Unsuppress"
                    } else {
                        "Suppress"
                    },
                    change(ComponentChange::Suppress(!c.suppressed)),
                );
                ui.separator();
                pick(ui, "Delete", change(ComponentChange::Delete));
            });
        }
        for action in actions {
            match action {
                TreeAction::Select(id) => {
                    self.selected_component = Some(id);
                    self.selected = None;
                    self.selected_geom.clear();
                }
                TreeAction::Open(id) => self.open_component(id),
                TreeAction::Change(id, change) => self.change_component(id, change),
            }
        }
    }

    /// The properties of a component: its name, where it is, and its part.
    pub(super) fn component_properties(&mut self, ui: &mut Ui, id: CompId) {
        let Some(assembly) = self.doc.model.assembly() else {
            return;
        };
        let Some(c) = assembly.component(id).cloned() else {
            return;
        };
        let part = assembly
            .definition(c.definition)
            .map_or_else(String::new, |d| d.name().to_owned());
        let units = self.doc.model.parameters.units;
        let status = self.doc.evaluation().component_status(id).cloned();

        // The name is typed into a copy and set when the field is left.
        let mut name = match &self.component_name {
            Some((editing, text)) if *editing == id => text.clone(),
            _ => c.name.clone(),
        };
        let mut origin = c.placement.origin.to_array().map(|v| units.from_mm(v));
        let mut fixed = c.fixed;
        let mut moved = false;
        let mut move_done = false;
        let mut turn = None;
        let mut rename = None;
        let mut open = false;
        egui::Grid::new("component_props")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                let r = ui.text_edit_singleline(&mut name);
                if r.changed() {
                    self.component_name = Some((id, name.clone()));
                }
                if r.lost_focus() {
                    self.component_name = None;
                    if name.trim() != c.name {
                        rename = Some(name.clone());
                    }
                }
                ui.end_row();
                ui.label("Part");
                ui.horizontal(|ui| {
                    ui.label(&part);
                    open = ui
                        .button("Edit Part")
                        .on_hover_text("Open the part as a document of its own. Saving it there stores it back in the assembly, for every component of the part.")
                        .clicked();
                });
                ui.end_row();
                for (axis, value) in ["X", "Y", "Z"].iter().zip(&mut origin) {
                    ui.label(*axis).on_hover_text(
                        "Where the part's origin is in the assembly.",
                    );
                    let r = ui.add(
                        egui::DragValue::new(value)
                            .speed(units.from_mm(1.0))
                            .max_decimals(4)
                            .suffix(format!(" {}", units.length.suffix())),
                    );
                    moved |= r.changed();
                    move_done |= r.drag_stopped() || r.lost_focus();
                    ui.end_row();
                }
                ui.label("Turn")
                    .on_hover_text("A quarter turn about an axis of the assembly, through the part's origin.");
                ui.horizontal(|ui| {
                    for (label, axis) in [("X 90°", DVec3::X), ("Y 90°", DVec3::Y), ("Z 90°", DVec3::Z)] {
                        if ui.small_button(label).clicked() {
                            turn = Some(axis);
                        }
                    }
                });
                ui.end_row();
                ui.label("Fixed").on_hover_text(
                    "A fixed component stays where it is; mates move the others to it.",
                );
                ui.checkbox(&mut fixed, "");
                ui.end_row();
            });
        if let Some(message) = status.as_ref().and_then(Status::message) {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(WARNING, "⚠");
                ui.label(message);
            });
        }

        if let Some(name) = rename {
            self.change_component(id, ComponentChange::Rename(name));
        }
        if moved {
            let origin = DVec3::from_array(origin.map(|v| units.to_mm(v)));
            self.change_model(
                &format!("Move {}", c.name),
                Some(MOVE_KEY ^ u64::from(id.0)),
                |m| {
                    if let Some(c) = m.assembly_mut().and_then(|a| a.component_mut(id)) {
                        c.placement.origin = origin;
                    }
                },
            );
        }
        if move_done {
            self.doc.seal_history();
        }
        if let Some(axis) = turn {
            let mut frame = c.placement;
            frame.rotation = (DQuat::from_axis_angle(axis, std::f64::consts::FRAC_PI_2)
                * frame.rotation)
                .normalize();
            self.change_component(id, ComponentChange::Place(Placing::Frame(frame)));
        }
        if fixed != c.fixed {
            self.change_component(id, ComponentChange::Fix(fixed));
        }
        if open {
            self.open_component(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::ItemId;
    use peet_ops::{Undo, parse};
    use serde_json::{Value, json};

    fn ok(app: &mut PeetApp, ctx: &egui::Context, op: Value) -> Value {
        let parsed = parse(&app.doc, &op).unwrap_or_else(|e| panic!("{op}: {e}"));
        let reply = app.apply_op(ctx, &parsed, Undo::Step);
        assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
        reply.json
    }

    /// Draws the tree, the properties and the toolbar for a few frames.
    fn draw(app: &mut PeetApp, _ctx: &egui::Context) {
        use super::super::RibbonTab;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui(|ui| {
                let mut pending = Vec::new();
                let mut tab = RibbonTab::File;
                app.toolbar(ui, &mut pending, &mut tab);
                app.feature_tree(ui);
                app.properties(ui, &mut pending);
                assert!(pending.is_empty());
            });
        harness.run_steps(3);
    }

    #[test]
    fn an_assembly_is_worked_on_in_the_application() {
        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        // A part: the assembly's commands are off, its own are on.
        assert!(!app.command_state(CommandId::InsertComponent).enabled);
        assert!(app.command_state(CommandId::NewSketch).enabled);

        app.execute(&ctx, CommandId::NewAssembly);
        assert!(app.doc.is_assembly());
        assert!(app.command_state(CommandId::InsertComponent).enabled);
        assert!(!app.command_state(CommandId::EditComponent).enabled);
        // The part's tools are off in an assembly, so none can add a feature to it.
        for cmd in [
            CommandId::NewSketch,
            CommandId::Extrude,
            CommandId::RefPlane,
            CommandId::ImportStep,
            CommandId::MassProperties,
            CommandId::Fillet,
        ] {
            assert!(!app.command_state(cmd).enabled, "{cmd:?}");
        }
        draw(&mut app, &ctx);

        ok(&mut app, &ctx, json!({"op": "insert", "sample": "bracket"}));
        ok(
            &mut app,
            &ctx,
            json!({"op": "insert", "component": "Bracket-1", "at": [0, 150, 0]}),
        );
        assert_eq!(app.doc.bodies.len(), 2);
        let second = CompId(2);
        app.selected_component = Some(second);
        draw(&mut app, &ctx);
        assert!(app.command_state(CommandId::EditComponent).enabled);

        // The properties' changes are operations, like everything else.
        app.change_component(second, ComponentChange::Fix(true));
        app.change_component(second, ComponentChange::Rename("Upper".to_owned()));
        let c = app.doc.model.assembly().unwrap().component(second).unwrap();
        assert!(c.fixed && c.name == "Upper");
        assert!(app.untranslated().is_empty(), "{:?}", app.untranslated());

        // Suppress and delete act on the selected component.
        app.execute(&ctx, CommandId::ToggleSuppress);
        assert_eq!(app.doc.bodies.len(), 1);
        assert_eq!(
            app.command_state(CommandId::ToggleSuppress).checked,
            Some(true)
        );
        app.execute(&ctx, CommandId::ToggleSuppress);
        assert_eq!(app.doc.bodies.len(), 2);

        // Edit Part opens the part beside the assembly; saving it stores it back.
        app.execute(&ctx, CommandId::EditComponent);
        assert_eq!(app.doc.count(), 2);
        assert!(!app.doc.is_assembly() && app.doc.embedded.is_some());
        assert!(app.selected_component().is_none());
        assert!(app.command_state(CommandId::NewSketch).enabled);
        let extrude = app
            .doc
            .model
            .features()
            .find(|f| f.name == "Extrude1")
            .map(|f| f.id)
            .unwrap();
        app.selected = Some(ItemId::Feature(extrude));
        app.execute(&ctx, CommandId::ToggleSuppress);
        assert!(app.doc.is_modified());
        app.execute(&ctx, CommandId::SaveDocument);
        assert!(!app.doc.is_modified(), "stored in the assembly");
        let part = app.doc.current_id();
        ok(&mut app, &ctx, json!({"op": "close"}));
        assert!(app.doc.is_assembly());
        assert!(app.doc.get(part).is_none());
        assert_eq!(app.doc.undo_label(), Some("Edit Bracket"));
        assert_eq!(app.doc.evaluation().component_problems().count(), 2);
        draw(&mut app, &ctx);

        app.selected_component = Some(second);
        app.execute(&ctx, CommandId::DeleteSelection);
        assert_eq!(app.doc.model.assembly().unwrap().components().count(), 1);
        assert!(app.selected_component().is_none());
        assert!(app.untranslated().is_empty(), "{:?}", app.untranslated());
    }
}
