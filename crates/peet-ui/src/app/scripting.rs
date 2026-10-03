//! The application's side of scripting: every command has an operation (`peet_ops`), and
//! an operation applied to the running application does what the command does.
//!
//! [`coverage`] names the operation that covers each command. It matches every command, so
//! a new command doesn't compile until it has one. Commands about the part (extrude, save)
//! are covered by operations that work with or without the application; commands about
//! the application itself (turn the view, open a window) are covered by
//! `peet_ops::AppCommand`, which [`PeetApp::apply_op`] carries out.

use peet_ops::{
    AppCommand, Host, Op, Query, Reply, SketchTool, Toggle, Undo, View, Window, apply_in,
    apply_model_scoped,
};
use peet_sheetmetal::{CheckRules, MaterialLibrary};
use serde_json::{Map, Value, json};

use super::PeetApp;
use crate::commands::CommandId;
use crate::document::Persistent;
use crate::sketch_ui;

/// The operation that does what a command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// An operation on the part or its files, by name: it works headless too.
    Op(&'static str),
    /// An operation on the application itself, by name: it needs the application.
    App(&'static str),
}

/// The operation that covers `cmd`. Where a command works on the selection (Extrude, a
/// sketch relation), its operation takes what was selected as fields.
pub fn coverage(cmd: CommandId) -> Coverage {
    use CommandId as C;
    use Coverage::{App, Op};
    match cmd {
        C::Undo => Op("undo"),
        C::Redo => Op("redo"),
        C::NewDocument => Op("new"),
        C::OpenDocument => Op("open"),
        C::SaveDocument | C::SaveDocumentAs => Op("save"),
        C::OpenSample | C::OpenSampleEnclosure | C::OpenSampleChassis | C::OpenSampleHousing => {
            Op("open_sample")
        }
        C::ExportStl | C::ExportDxf | C::ExportStep => Op("export"),
        C::ImportDxf => Op("import_dxf"),
        C::ImportStep => Op("import_step"),

        C::NewSketch => Op("sketch"),
        // The drawing tools, relations and edits of the sketch editor: items of a draw list.
        C::SketchLine
        | C::SketchRectangle
        | C::SketchCenterRectangle
        | C::SketchCircle
        | C::SketchArc
        | C::SketchSpline
        | C::SketchSlot
        | C::SketchPolygon
        | C::SketchPoint
        | C::SketchTrim
        | C::SketchExtend
        | C::SketchFillet
        | C::SketchOffset
        | C::SketchMirror
        | C::SmartDimension
        | C::ToggleConstruction
        | C::RelCoincident
        | C::RelHorizontal
        | C::RelVertical
        | C::RelParallel
        | C::RelPerpendicular
        | C::RelTangent
        | C::RelEqual
        | C::RelConcentric
        | C::RelMidpoint
        | C::RelSymmetric
        | C::RelFix => Op("draw"),

        C::Extrude => Op("extrude"),
        C::CutExtrude => Op("cut"),
        C::Revolve => Op("revolve"),
        C::CutRevolve => Op("cut_revolve"),
        C::Sweep => Op("sweep"),
        C::CutSweep => Op("cut_sweep"),
        C::Loft => Op("loft"),
        C::CutLoft => Op("cut_loft"),
        C::ConvertToSheet => Op("convert_to_sheet"),
        C::Hole => Op("hole"),
        C::Fillet => Op("fillet"),
        C::Chamfer => Op("chamfer"),
        C::Shell => Op("shell"),
        C::Draft => Op("draft"),
        C::RefPlane => Op("plane"),
        C::RefAxis => Op("axis"),
        C::RefPoint => Op("point"),
        C::RefCoordSystem => Op("coordinate_system"),
        C::BaseFlange => Op("base_flange"),
        C::EdgeFlange => Op("edge_flange"),
        C::SheetCut => Op("sheet_cut"),
        C::Hem => Op("hem"),
        C::SketchedBend => Op("sketched_bend"),
        C::Jog => Op("jog"),
        C::MiterFlange => Op("miter_flange"),
        C::CornerTreatment => Op("corner"),
        C::Dimple => Op("dimple"),
        C::Emboss => Op("emboss"),
        C::Louver => Op("louver"),
        C::LinearPattern => Op("linear_pattern"),
        C::CircularPattern => Op("circular_pattern"),
        C::MirrorFeature => Op("mirror"),

        C::DeleteSelection => Op("delete"),
        C::ToggleSuppress => Op("suppress"),
        C::RollToEnd => Op("rollback"),
        C::ToggleReferencePlanes => Op("show"),
        C::FlatPattern => Op("flat_pattern"),
        // Windows whose contents are data: the operation gives the data.
        C::Parameters => Op("parameters"),
        C::ConfigurationTable => Op("configurations"),
        C::BendTable => Op("bend_table"),
        C::SheetChecks => Op("checks"),
        C::GaugeTables => Op("materials"),
        C::MassProperties => Op("mass"),

        C::ViewIsometric
        | C::ViewFront
        | C::ViewBack
        | C::ViewLeft
        | C::ViewRight
        | C::ViewTop
        | C::ViewBottom => App("view"),
        C::ZoomToFit => App("zoom_to_fit"),
        C::ToggleProjection
        | C::ToggleGrid
        | C::ToggleViewCube
        | C::ToggleFeatureTree
        | C::ToggleProperties
        | C::TogglePerfOverlay
        | C::ToggleRelations => App("toggle"),
        C::CommandPalette | C::Settings | C::KeyboardShortcuts | C::About => App("window"),
        C::EditSketch => App("edit_sketch"),
        C::ExitSketch => App("exit_sketch"),
        C::SketchSelect => App("tool"),
        C::CheckForUpdates => App("check_for_updates"),
        C::InstallUpdate => App("install_update"),
        C::ToggleAutoUpdates => App("toggle"),
        C::Quit => App("quit"),
    }
}

/// The command an application operation runs (a sketch is opened by more than a command:
/// see [`PeetApp::apply_op`]).
pub fn command_for(command: &AppCommand) -> CommandId {
    use CommandId as C;
    match command {
        AppCommand::View(view) => match view {
            View::Isometric => C::ViewIsometric,
            View::Front => C::ViewFront,
            View::Back => C::ViewBack,
            View::Left => C::ViewLeft,
            View::Right => C::ViewRight,
            View::Top => C::ViewTop,
            View::Bottom => C::ViewBottom,
        },
        AppCommand::ZoomToFit => C::ZoomToFit,
        AppCommand::Toggle { what, .. } => match what {
            Toggle::Perspective => C::ToggleProjection,
            Toggle::Grid => C::ToggleGrid,
            Toggle::ViewCube => C::ToggleViewCube,
            Toggle::FeatureTree => C::ToggleFeatureTree,
            Toggle::Properties => C::ToggleProperties,
            Toggle::PerformanceOverlay => C::TogglePerfOverlay,
            Toggle::Relations => C::ToggleRelations,
            Toggle::Construction => C::ToggleConstruction,
            Toggle::AutomaticUpdates => C::ToggleAutoUpdates,
        },
        AppCommand::Window(window) => match window {
            Window::CommandPalette => C::CommandPalette,
            Window::Settings => C::Settings,
            Window::KeyboardShortcuts => C::KeyboardShortcuts,
            Window::About => C::About,
            Window::Parameters => C::Parameters,
            Window::Configurations => C::ConfigurationTable,
            Window::BendTable => C::BendTable,
            Window::Checks => C::SheetChecks,
            Window::Materials => C::GaugeTables,
            Window::MassProperties => C::MassProperties,
        },
        AppCommand::EditSketch(_) => C::EditSketch,
        AppCommand::ExitSketch => C::ExitSketch,
        AppCommand::Tool(tool) => match tool {
            SketchTool::Select => C::SketchSelect,
            SketchTool::Line => C::SketchLine,
            SketchTool::Rectangle => C::SketchRectangle,
            SketchTool::CenterRectangle => C::SketchCenterRectangle,
            SketchTool::Circle => C::SketchCircle,
            SketchTool::Arc => C::SketchArc,
            SketchTool::Slot => C::SketchSlot,
            SketchTool::Polygon => C::SketchPolygon,
            SketchTool::Point => C::SketchPoint,
            SketchTool::Trim => C::SketchTrim,
            SketchTool::Extend => C::SketchExtend,
            SketchTool::Fillet => C::SketchFillet,
            SketchTool::Offset => C::SketchOffset,
            SketchTool::Mirror => C::SketchMirror,
            SketchTool::Dimension => C::SmartDimension,
            SketchTool::Spline => C::SketchSpline,
        },
        AppCommand::CheckForUpdates => C::CheckForUpdates,
        AppCommand::InstallUpdate => C::InstallUpdate,
        AppCommand::Quit => C::Quit,
    }
}

/// The operations the application has applied to the part.
#[derive(Default)]
pub struct Journal {
    ops: Vec<Op>,
    untranslated: Vec<String>,
    keys: u64,
}

impl Journal {
    /// How many operations are kept: the oldest are dropped beyond this.
    const LIMIT: usize = 10_000;

    fn record(&mut self, ops: impl IntoIterator<Item = Op>) {
        self.ops.extend(ops);
        if self.ops.len() > Self::LIMIT {
            let extra = self.ops.len() - Self::LIMIT;
            self.ops.drain(..extra);
        }
    }

    /// An undo key no other change uses (drags use small keys of their own).
    fn fresh_key(&mut self) -> u64 {
        self.keys += 1;
        (1 << 63) | self.keys
    }
}

/// The user's material tables and check limits, for operations on the open document.
struct SettingsHost<'a> {
    materials: &'a mut MaterialLibrary,
    check_rules: &'a mut CheckRules,
}

impl Host for SettingsHost<'_> {
    fn materials(&mut self) -> &mut MaterialLibrary {
        self.materials
    }

    fn check_rules(&mut self) -> &mut CheckRules {
        self.check_rules
    }
}

impl PeetApp {
    /// Applies a scripted operation to the running application, as if the user had done
    /// it: the part changes on screen, and a change is an undo step.
    ///
    /// While a sketch is open for editing, its working copy is not in the part yet, so
    /// operations that change the part are refused until the sketch is finished
    /// (`exit_sketch`). Queries and application commands are not.
    pub fn apply_op(&mut self, ctx: &egui::Context, op: &Op, undo: Undo) -> Reply {
        let reply = match op {
            Op::App(command) => match self.app_command(ctx, command) {
                Ok(data) => {
                    let mut out = Map::new();
                    out.insert("ok".to_owned(), json!(true));
                    out.insert("op".to_owned(), json!(command.word()));
                    out.extend(data);
                    Reply {
                        ok: true,
                        changed: false,
                        created: Vec::new(),
                        replaced: false,
                        json: Value::Object(out),
                    }
                }
                Err(e) => Reply::error(command.word(), e),
            },
            Op::Query(Query::Help | Query::Status | Query::Features | Query::Parameters)
            | Op::Query(Query::Materials { .. }) => self.apply_to_document(op, undo),
            _ if self.sketch.is_some() && !matches!(op, Op::Query(_)) => Reply::error(
                op.word(),
                format!(
                    "{} is open for editing in PeetCAD, so the part can't be changed from outside: finish the sketch first (exit_sketch).",
                    self.sketch
                        .as_ref()
                        .map_or_else(String::new, |e| self.doc.item_name(e.item))
                ),
            ),
            _ => self.apply_to_document(op, undo),
        };
        if reply.ok && !matches!(op, Op::Query(_)) {
            self.journal.record([op.clone()]);
        }
        ctx.request_repaint();
        reply
    }

    /// Applies an operation the user asked for through the interface, and shows why if
    /// it couldn't be applied.
    pub(super) fn perform(&mut self, op: Op) -> Reply {
        let reply = self.perform_quietly(op);
        if let Some(e) = reply.json["error"].as_str().filter(|_| !reply.ok) {
            self.error(e.to_owned());
        }
        reply
    }

    /// [`PeetApp::perform`], leaving it to the caller to say what went wrong.
    pub(super) fn perform_quietly(&mut self, op: Op) -> Reply {
        let reply = self.apply_to_document(&op, Undo::Step);
        if reply.ok {
            self.journal.record([op]);
        }
        reply
    }

    /// Makes a change to the model that one of the application's tools worked out (it
    /// edits a copy of the model), as one undo step called `label`. The change is applied
    /// as the operations that express it, so everything the application does to a part
    /// is something a script can do, and is in the journal.
    ///
    /// Changes with the same `merge` key, one after the other, are one undo step (a
    /// drag): seal the history when they end. Returns whether the part changed.
    pub(super) fn change_model(
        &mut self,
        label: &str,
        merge: Option<u64>,
        f: impl FnOnce(&mut peet_model::Model),
    ) -> bool {
        let mut new = self.doc.model.clone();
        f(&mut new);
        if new == self.doc.model {
            return false;
        }
        let kept: Vec<Persistent> = self
            .selected_geom
            .iter()
            .filter_map(|g| self.doc.persist(*g))
            .collect();
        let key = merge.unwrap_or_else(|| self.journal.fresh_key());
        let mut host = SettingsHost {
            materials: &mut self.settings.materials,
            check_rules: &mut self.settings.check_rules,
        };
        // Values the tool changed go to this configuration only if that is asked for.
        let values = self.values_this_only.then_some(peet_ops::Configs::This);
        let done = apply_model_scoped(&mut host, &mut self.doc, new, label, key, values.as_ref());
        if merge.is_none() {
            self.doc.seal_history();
        }
        if let Some(reason) = &done.untranslated {
            // The part is as the tool asked, but the operations can't express it yet.
            log::error!("\"{label}\" was not made by operations: {reason}");
            self.journal.untranslated.push(format!("{label}: {reason}"));
        }
        self.journal.record(done.ops);
        if done.changed {
            self.restore_selection(&kept);
        }
        done.changed
    }

    /// The operations applied to the part since the application started, oldest first:
    /// what the user did, as a script would do it.
    pub fn journal(&self) -> &[Op] {
        &self.journal.ops
    }

    /// Changes the application made that the operations could not express (each with the
    /// reason): gaps in the operations. Empty when all is well.
    pub fn untranslated(&self) -> &[String] {
        &self.journal.untranslated
    }

    fn apply_to_document(&mut self, op: &Op, undo: Undo) -> Reply {
        let kept: Vec<Persistent> = self
            .selected_geom
            .iter()
            .filter_map(|g| self.doc.persist(*g))
            .collect();
        let mut host = SettingsHost {
            materials: &mut self.settings.materials,
            check_rules: &mut self.settings.check_rules,
        };
        let reply = apply_in(&mut host, &mut self.doc, op, undo);
        if reply.replaced {
            self.document_replaced();
            self.files.discard_autosave();
        } else if reply.changed {
            self.restore_selection(&kept);
        } else if matches!(op, Op::FlatPattern { .. }) {
            // The picked faces and edges are numbered the same in both views.
            self.hovered_geom = None;
        }
        reply
    }

    /// Carries out an operation on the application itself.
    fn app_command(
        &mut self,
        ctx: &egui::Context,
        command: &AppCommand,
    ) -> Result<Map<String, Value>, String> {
        let mut out = Map::new();
        if let AppCommand::EditSketch(sketch) = command {
            let id = sketch.resolve(&self.doc)?;
            if self.doc.model.sketch(id).is_none() {
                return Err(format!("{} is not a sketch.", self.doc.model.name_of(id)));
            }
            if let Some(editor) = &self.sketch {
                return Err(format!(
                    "{} is already open for editing: finish it first (exit_sketch).",
                    self.doc.item_name(editor.item)
                ));
            }
            self.open_sketch(id);
            out.insert("editing".to_owned(), json!(self.doc.model.name_of(id)));
            return Ok(out);
        }
        // These say how far the update is, and why there is none to install.
        match command {
            AppCommand::CheckForUpdates => {
                self.check_for_updates(true);
                out.insert("update".to_owned(), self.update_json());
                return Ok(out);
            }
            AppCommand::InstallUpdate => {
                self.install_update()?;
                out.insert("update".to_owned(), self.update_json());
                return Ok(out);
            }
            _ => {}
        }
        let cmd = command_for(command);
        let state = self.command_state(cmd);
        if !state.enabled {
            let hint = if sketch_ui::is_sketch_command(cmd) || cmd == CommandId::ExitSketch {
                " It works on a sketch that is open for editing (edit_sketch)."
            } else {
                ""
            };
            return Err(format!(
                "'{}' can't be done right now.{hint}",
                command.word()
            ));
        }
        match command {
            AppCommand::Toggle { what, on } => {
                // A toggle that is already as asked is left alone.
                let is = state.checked.unwrap_or(false);
                let want = on.unwrap_or(!is);
                if state.checked.is_none() || is != want {
                    self.execute(ctx, cmd);
                }
                let now = self.command_state(cmd).checked.unwrap_or(want);
                out.insert(what.word().to_owned(), json!(now));
            }
            _ => self.execute(ctx, cmd),
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_ops::apply_json;
    use peet_ops::{FeatureArgs, FeatureSel, parse};

    fn every_app_command() -> Vec<AppCommand> {
        let mut all = vec![
            AppCommand::ZoomToFit,
            AppCommand::ExitSketch,
            AppCommand::CheckForUpdates,
            AppCommand::InstallUpdate,
            AppCommand::Quit,
            AppCommand::EditSketch(FeatureSel::Name("Sketch1".to_owned())),
        ];
        all.extend(View::ALL.iter().map(|v| AppCommand::View(*v)));
        all.extend(Window::ALL.iter().map(|w| AppCommand::Window(*w)));
        all.extend(SketchTool::ALL.iter().map(|t| AppCommand::Tool(*t)));
        all.extend(
            Toggle::ALL
                .iter()
                .map(|t| AppCommand::Toggle { what: *t, on: None }),
        );
        all
    }

    #[test]
    fn every_command_is_covered_by_an_operation_that_exists() {
        let mut doc = crate::document::Document::default();
        let help = apply_json(&mut doc, &json!({"op": "help"}), Undo::Step).json;
        let ops = help["operations"].as_object().expect("the operations");
        for cmd in CommandId::ALL {
            let (word, app) = match coverage(cmd) {
                Coverage::Op(word) => (word, false),
                Coverage::App(word) => (word, true),
            };
            let entry = ops
                .get(word)
                .unwrap_or_else(|| panic!("{cmd:?} is covered by '{word}', which is no operation"));
            assert_eq!(
                entry.get("needs").is_some(),
                app,
                "{cmd:?}: '{word}' is on the wrong side of the application"
            );
        }
    }

    #[test]
    fn application_operations_and_commands_agree() {
        for command in every_app_command() {
            let cmd = command_for(&command);
            // A command covered by an application operation is covered by this one.
            if let Coverage::App(word) = coverage(cmd) {
                assert_eq!(word, command.word(), "{cmd:?}");
            }
        }
        // And every command covered by an application operation is reached by one.
        let reached: Vec<CommandId> = every_app_command().iter().map(command_for).collect();
        for cmd in CommandId::ALL {
            if matches!(coverage(cmd), Coverage::App(_)) {
                assert!(reached.contains(&cmd), "{cmd:?} is not reached");
            }
        }
    }

    fn run(app: &mut PeetApp, ctx: &egui::Context, op: Value) -> Reply {
        let op = parse(&app.doc, &op).unwrap_or_else(|e| panic!("{op}: {e}"));
        app.apply_op(ctx, &op, Undo::Step)
    }

    fn ok(app: &mut PeetApp, ctx: &egui::Context, op: Value) -> Value {
        let reply = run(app, ctx, op.clone());
        assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
        reply.json
    }

    #[test]
    fn operations_drive_the_application() {
        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();

        // The part changes in the application's document, one undo step each.
        ok(
            &mut app,
            &ctx,
            json!({"op": "sketch", "on": "top", "draw": [
                {"type": "rectangle", "from": [0, 0], "to": [60, 40]},
            ]}),
        );
        let made = ok(
            &mut app,
            &ctx,
            json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}),
        );
        assert_eq!(made["created"][0]["status"], "ok");
        assert_eq!(app.doc.bodies.len(), 1);
        assert_eq!(app.doc.undo_label(), Some("Add Extrude1"));

        // Application commands change the application, not the part.
        let grid = app.settings.show_grid;
        let reply = ok(&mut app, &ctx, json!({"op": "toggle", "what": "grid"}));
        assert_eq!(reply["grid"], !grid);
        assert_eq!(app.settings.show_grid, !grid);
        ok(
            &mut app,
            &ctx,
            json!({"op": "toggle", "what": "grid", "on": !grid}),
        );
        assert_eq!(app.settings.show_grid, !grid, "already as asked");
        ok(&mut app, &ctx, json!({"op": "window", "open": "settings"}));
        assert!(app.windows.settings);
        assert_eq!(app.doc.undo_label(), Some("Add Extrude1"));

        // The tables are the user's settings.
        ok(
            &mut app,
            &ctx,
            json!({"op": "set_gauge", "material": "Brass", "gauge": "1 mm", "thickness": 1}),
        );
        assert!(app.settings.materials.find("Brass", "1 mm").is_some());

        // A sketch open for editing blocks changes from outside, not queries.
        let e = run(&mut app, &ctx, json!({"op": "tool", "tool": "line"}));
        assert!(!e.ok);
        assert!(e.json["error"].as_str().unwrap().contains("edit_sketch"));
        let editing = ok(
            &mut app,
            &ctx,
            json!({"op": "edit_sketch", "sketch": "Sketch1"}),
        );
        assert_eq!(editing["editing"], "Sketch1");
        assert!(app.sketch.is_some());
        ok(&mut app, &ctx, json!({"op": "tool", "tool": "line"}));
        let busy = run(
            &mut app,
            &ctx,
            json!({"op": "edit", "feature": "Extrude1", "depth": 9}),
        );
        assert!(!busy.ok);
        assert!(
            busy.json["error"]
                .as_str()
                .unwrap()
                .contains("Sketch1 is open")
        );
        ok(&mut app, &ctx, json!({"op": "features"}));
        ok(&mut app, &ctx, json!({"op": "exit_sketch"}));
        assert!(app.sketch.is_none());
        ok(
            &mut app,
            &ctx,
            json!({"op": "edit", "feature": "Extrude1", "depth": 9}),
        );

        // A new document resets what the application knew about the old one.
        let refused = run(&mut app, &ctx, json!({"op": "new"}));
        assert!(!refused.ok, "unsaved changes");
        let fresh = run(
            &mut app,
            &ctx,
            json!({"op": "open_sample", "sample": "chassis", "discard": true}),
        );
        assert!(fresh.ok && fresh.replaced);
        assert!(app.selected.is_none());
        assert!(app.doc.has_sheet_metal());
        ok(&mut app, &ctx, json!({"op": "flat_pattern", "on": true}));
        assert!(app.doc.is_flat());

        // A typed operation works the same way.
        let typed = app.apply_op(
            &ctx,
            &Op::Edit {
                feature: "Base-Flange1".into(),
                fields: FeatureArgs::BaseFlange(peet_ops::BaseFlange {
                    thickness: Some(2.into()),
                    ..Default::default()
                }),
                configurations: None,
            },
            Undo::Step,
        );
        assert!(typed.ok && typed.changed, "{}", typed.json);
    }

    #[test]
    fn the_applications_own_commands_change_the_part_by_operations() {
        use crate::document::ItemId;
        use crate::files::AfterDiscard;
        use peet_document::GeomRef;
        use peet_math::DVec2;
        use peet_model::{Datum, StdPlane};

        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        let kinds =
            |app: &PeetApp| -> Vec<&'static str> { app.journal().iter().map(Op::word).collect() };

        // New Sketch on the top plane, a rectangle drawn in the editor, then Extrude,
        // which finishes the sketch first.
        app.selected = Some(ItemId::Datum(Datum::Plane(StdPlane::Top)));
        app.execute(&ctx, CommandId::NewSketch);
        assert!(app.sketch.is_some());
        let work = app.sketch_work.as_mut().expect("the sketch being edited");
        peet_sketch::shapes::rectangle(&mut work.sketch, DVec2::ZERO, DVec2::new(60.0, 40.0));
        app.execute(&ctx, CommandId::Extrude);
        assert!(app.sketch.is_none());
        assert_eq!(app.doc.bodies.len(), 1);
        assert_eq!(kinds(&app), ["sketch", "set_sketch", "extrude"]);
        assert_eq!(app.doc.undo_label(), Some("Add Extrude"));

        // The properties panel edits a copy of the feature; the change is an edit.
        let extrude = app
            .selected
            .and_then(ItemId::feature)
            .expect("the extrusion");
        app.change("Edit Extrude1", |m| {
            if let Some(e) = m.feature_mut(extrude).and_then(|f| f.extrude_mut()) {
                e.params.depth = peet_model::Scalar::new(25.0);
            }
        });
        assert!(matches!(app.journal().last(), Some(Op::Edit { .. })));

        // Tree and menu commands on the selection.
        app.execute(&ctx, CommandId::ToggleSuppress);
        app.execute(&ctx, CommandId::ToggleSuppress);
        app.execute(&ctx, CommandId::ToggleReferencePlanes);
        app.execute(&ctx, CommandId::Undo);
        app.execute(&ctx, CommandId::Redo);
        app.selected = None;
        app.execute(&ctx, CommandId::RefPlane);
        let plane = app.selected.and_then(ItemId::feature).expect("the plane");
        assert_eq!(app.doc.model.name_of(plane), "Plane1");

        // A fillet on a picked edge: the commands of general solid modelling build
        // their operations themselves.
        let edge = app.doc.bodies[0].solid.edge_ids().next().expect("an edge");
        app.picking = None;
        app.selected = None;
        app.selected_geom = vec![GeomRef::Edge { body: 0, edge }];
        app.execute(&ctx, CommandId::Fillet);
        let fillet = app.selected.and_then(ItemId::feature).expect("the fillet");
        assert_eq!(app.doc.model.name_of(fillet), "Fillet1");
        assert_eq!(app.doc.undo_label(), Some("Add Fillet1"));
        app.execute(&ctx, CommandId::DeleteSelection);
        assert!(app.doc.feature(fillet).is_none());

        // A sample, and its flat pattern.
        app.after_discard(AfterDiscard::SampleEnclosure);
        assert!(app.doc.has_sheet_metal());
        app.execute(&ctx, CommandId::FlatPattern);
        assert!(app.doc.is_flat());

        let all = kinds(&app);
        for word in [
            "edit",
            "suppress",
            "show",
            "undo",
            "redo",
            "plane",
            "fillet",
            "delete",
            "open_sample",
            "flat_pattern",
        ] {
            assert!(all.contains(&word), "{word} is not in {all:?}");
        }
        // Nothing the application did was beyond the operations.
        assert_eq!(app.untranslated(), &[] as &[String]);
    }

    #[test]
    fn configurations_are_made_and_used_through_the_interface() {
        use crate::configs_ui::ConfigAction;
        use crate::document::ItemId;
        use crate::tree::TreeAction;
        use peet_ops::Configs;

        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        ok(
            &mut app,
            &ctx,
            json!({"op": "open_sample", "sample": "enclosure"}),
        );
        let id = |app: &PeetApp, name: &str| {
            let f = app.doc.model.features().find(|f| f.name == name);
            f.expect("the feature").id
        };
        let cut = id(&app, "Sheet-Cut1");
        let flange = id(&app, "Edge-Flange1");
        app.update_title(&ctx);
        assert_eq!(app.title, "Enclosure Panel - PeetCAD");

        // The list: a new configuration is a copy and becomes the active one.
        app.apply_configs(vec![ConfigAction::Add]);
        assert_eq!(app.doc.model.active_configuration().name, "Configuration2");
        app.apply_configs(vec![ConfigAction::Rename {
            from: "Configuration2".to_owned(),
            to: "Blank".to_owned(),
        }]);
        app.update_title(&ctx);
        assert_eq!(app.title, "Enclosure Panel [Blank] * - PeetCAD");

        // The tree: suppressing is for this configuration, or for all of them.
        app.apply_tree(vec![TreeAction::SetSuppressed {
            feature: cut,
            on: true,
            all: false,
        }]);
        app.apply_tree(vec![TreeAction::SetSuppressed {
            feature: flange,
            on: true,
            all: true,
        }]);
        // The toolbar's Suppress is for this configuration too.
        app.selected = Some(ItemId::Feature(id(&app, "Sheet-Cut2")));
        app.execute(&ctx, CommandId::ToggleSuppress);
        let default = app.doc.model.configuration_named("Default").unwrap().id;
        assert_eq!(app.doc.model.suppressed_in(cut, default), Some(false));
        assert_eq!(app.doc.model.suppressed_in(flange, default), Some(true));
        assert!(app.doc.model.suppression_differs(id(&app, "Sheet-Cut2")));

        // Back to the first one, a copy of it, and the copy deleted again.
        app.apply_configs(vec![ConfigAction::Activate("Default".to_owned())]);
        assert!(!app.doc.model.feature(cut).unwrap().suppressed);
        assert!(app.doc.model.feature(flange).unwrap().suppressed);
        app.apply_configs(vec![ConfigAction::Copy("Blank".to_owned())]);
        assert_eq!(app.doc.model.active_configuration().name, "Configuration3");
        assert!(app.doc.model.feature(cut).unwrap().suppressed);
        app.apply_configs(vec![ConfigAction::Delete("Configuration3".to_owned())]);
        assert_eq!(app.doc.model.active_configuration().name, "Blank");
        // What can't be done is said, and changes nothing.
        app.apply_configs(vec![ConfigAction::Rename {
            from: "Blank".to_owned(),
            to: "Default".to_owned(),
        }]);
        assert!(matches!(&app.status_message, Some((m, true)) if m.contains("already called")));
        assert_eq!(app.doc.model.configurations().len(), 2);

        // All of it was operations, with their scopes.
        let journal = app.journal();
        assert!(journal.iter().any(|op| matches!(
            op,
            Op::Suppress {
                configurations: Configs::This,
                ..
            }
        )));
        assert!(journal.iter().any(|op| matches!(
            op,
            Op::Suppress {
                configurations: Configs::All,
                ..
            }
        )));
        let words: Vec<&str> = journal.iter().map(Op::word).collect();
        for word in [
            "add_configuration",
            "edit_configuration",
            "configuration",
            "delete_configuration",
        ] {
            assert!(words.contains(&word), "{word} is not in {words:?}");
        }
        assert_eq!(app.untranslated(), &[] as &[String]);
    }

    #[test]
    fn values_differ_per_configuration_through_the_panels_and_the_table() {
        use crate::config_table::set_value_op;
        use peet_model::{Scalar, Slot};
        use peet_ops::Configs;

        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        ok(
            &mut app,
            &ctx,
            json!({"op": "sketch", "on": "top", "draw": [
                {"type": "rectangle", "from": [0, 0], "to": [80, 50], "as": "r"},
                {"type": "length", "of": ["r.bottom"], "value": 80, "name": "width"},
            ]}),
        );
        ok(
            &mut app,
            &ctx,
            json!({"op": "extrude", "sketch": "Sketch1", "depth": 8}),
        );
        ok(
            &mut app,
            &ctx,
            json!({"op": "add_configuration", "name": "Other"}),
        );
        let sketch = app.doc.model.features().next().unwrap().id;
        let extrude = app.doc.model.features().nth(1).unwrap().id;
        let default = app.doc.model.configuration_named("Default").unwrap().id;
        let depth = Slot::Field("depth".to_owned());
        let in_default = |app: &PeetApp, feature, slot: &Slot| {
            let v = app.doc.model.value_in(feature, slot, default);
            v.unwrap().value
        };
        let set_depth = |app: &mut PeetApp, v: f64| {
            app.change("Edit Extrude1", |m| {
                let e = m.feature_mut(extrude).unwrap().extrude_mut().unwrap();
                e.params.depth = Scalar::new(v);
            });
        };

        // The panel, as it starts: a value that is the same everywhere changes everywhere.
        set_depth(&mut app, 9.0);
        assert_eq!(in_default(&app, extrude, &depth), 9.0);
        // "This configuration": the others keep theirs.
        app.values_this_only = true;
        set_depth(&mut app, 12.0);
        assert_eq!(in_default(&app, extrude, &depth), 9.0);
        assert!(matches!(
            app.journal().last(),
            Some(Op::Edit {
                configurations: Some(Configs::This),
                ..
            })
        ));
        app.values_this_only = false;

        // The table: a value typed in a configuration's column changes that one.
        let value = app.doc.model.slot_named(extrude, "depth").unwrap();
        let named = Configs::Named(vec!["Default".to_owned()]);
        let op = set_value_op(&app.doc, extrude, &value, "2 * 7", &named).unwrap();
        assert!(app.perform(op).ok);
        assert_eq!(in_default(&app, extrude, &depth), 14.0);
        assert_eq!(app.doc.model.value(extrude, &depth).unwrap().1.value, 12.0);
        let width = app.doc.model.slot_named(sketch, "width").unwrap();
        let op = set_value_op(&app.doc, sketch, &width, "100", &named).unwrap();
        assert!(matches!(op, Op::SetDimension { .. }));
        assert!(app.perform(op).ok);
        assert_eq!(in_default(&app, sketch, &width.slot), 100.0);
        assert_eq!(
            app.doc.model.value(sketch, &width.slot).unwrap().1.value,
            80.0
        );
        // And "=" makes it the same everywhere again.
        let op = set_value_op(&app.doc, sketch, &width, "80", &Configs::All).unwrap();
        assert!(app.perform(op).ok);
        assert!(!app.doc.model.value_differs(sketch, &width.slot));
        // What can't be evaluated is refused, with the reason.
        let op = set_value_op(&app.doc, extrude, &value, "2 * nothing", &named).unwrap();
        assert!(!app.perform_quietly(op).ok);

        // A bend model's value is a field of `bend`.
        ok(
            &mut app,
            &ctx,
            json!({"op": "open_sample", "sample": "enclosure", "discard": true}),
        );
        ok(
            &mut app,
            &ctx,
            json!({"op": "add_configuration", "name": "Soft"}),
        );
        let flange = app.doc.model.features().nth(1).unwrap().id;
        let k = app.doc.model.slot_named(flange, "k_factor").unwrap();
        let op = set_value_op(&app.doc, flange, &k, "0.38", &Configs::This).unwrap();
        assert!(app.perform(op).ok);
        assert!(app.doc.model.value_differs(flange, &k.slot));

        // The table is a window like the others.
        ok(
            &mut app,
            &ctx,
            json!({"op": "window", "open": "configurations"}),
        );
        assert!(app.windows.configurations);
        assert_eq!(app.untranslated(), &[] as &[String]);
    }
}
