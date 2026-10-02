//! Sketch mode: drawing tools, selection, dragging, relations and dimensions.
//!
//! While a sketch is open for editing, the [`SketchEditor`] owns the viewport's left mouse
//! button (navigation keeps the other buttons and modifier combinations), draws the sketch
//! on top of the 3D view with `egui`, and fills the properties panel. All geometry edits
//! go through the `peet-sketch` crate; after each edit the sketch is re-solved and
//! re-analysed, so colours (under / fully / over defined) are always current.

mod draw;
mod hit;
mod panel;
mod tools;
pub mod view;

use egui::{Pos2, Rect, Response, Ui};
use peet_math::DVec2;
use peet_platform::Instant;
use peet_render::Camera;
use peet_sketch::expr::Parameters;
use peet_sketch::infer::Inference;
use peet_sketch::region::Profile;
use peet_sketch::solver::{Analysis, ConstraintStatus, DofStatus, SolveReport, Solver};
use peet_sketch::{ConstraintId, ConstraintKind, EntityId, EntityKind, Sketch};

use crate::commands::CommandId;
use crate::document::{ItemId, SketchItem, SketchStatus};
pub use view::SketchView;

/// Something selectable in a sketch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sel {
    Entity(EntityId),
    Constraint(ConstraintId),
}

/// The active sketch tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Line,
    Rectangle,
    CenterRectangle,
    Circle,
    Arc,
    Slot,
    Polygon,
    Point,
    Trim,
    Extend,
    Fillet,
    Dimension,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Line => "Line",
            Self::Rectangle => "Rectangle",
            Self::CenterRectangle => "Center Rectangle",
            Self::Circle => "Circle",
            Self::Arc => "Arc",
            Self::Slot => "Slot",
            Self::Polygon => "Polygon",
            Self::Point => "Point",
            Self::Trim => "Trim",
            Self::Extend => "Extend",
            Self::Fillet => "Fillet",
            Self::Dimension => "Smart Dimension",
        }
    }

    /// Number of clicks that complete one shape (drawing tools only).
    fn clicks_needed(self) -> usize {
        match self {
            Self::Point => 1,
            Self::Line | Self::Rectangle | Self::CenterRectangle | Self::Circle | Self::Polygon => {
                2
            }
            Self::Arc | Self::Slot => 3,
            _ => 0,
        }
    }

    fn is_drawing(self) -> bool {
        self.clicks_needed() > 0
    }
}

/// Settings for the drawing tools, shown in the properties panel.
#[derive(Clone, Debug)]
pub struct ToolOptions {
    /// New geometry is construction geometry.
    pub construction: bool,
    pub polygon_sides: usize,
    pub fillet_radius: f64,
    pub offset_distance: f64,
    /// Show relation glyphs next to the geometry.
    pub show_relations: bool,
    /// Automatic relations while drawing.
    pub inference: bool,
}

impl Default for ToolOptions {
    fn default() -> Self {
        Self {
            construction: false,
            polygon_sides: 6,
            fillet_radius: 5.0,
            offset_distance: 5.0,
            show_relations: true,
            inference: true,
        }
    }
}

/// A status message for the user (errors explain what failed and how to fix it).
#[derive(Clone, Debug)]
pub struct Message {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// One placed point of a shape being drawn.
#[derive(Clone, Debug)]
struct Click {
    pos: DVec2,
    inference: Inference,
}

/// A line chain in progress: the next segment starts at the previous one's end.
#[derive(Clone, Copy, Debug)]
struct Chain {
    last_line: EntityId,
    last_end: EntityId,
    first_point: EntityId,
}

enum DragKind {
    Geometry(peet_sketch::Drag),
    Label {
        dimension: ConstraintId,
        start_offset: DVec2,
        grab: DVec2,
    },
    Box {
        start: Pos2,
    },
}

struct DragState {
    kind: DragKind,
    before: Sketch,
    moved: bool,
}

/// Inline editor for a dimension value.
struct DimEdit {
    dimension: ConstraintId,
    text: String,
    focus: bool,
    /// Undo snapshot to restore if a freshly placed dimension is cancelled... we keep the
    /// dimension (like SolidWorks), so this is only used to record the undo step.
    before: Option<Sketch>,
}

const UNDO_LIMIT: usize = 200;

pub struct SketchEditor {
    pub item: ItemId,
    solver: Solver,
    pub analysis: Analysis,
    pub last_report: SolveReport,
    /// Time of the last solve, in milliseconds.
    pub solve_ms: f64,
    pub tool: Tool,
    pub options: ToolOptions,
    clicks: Vec<Click>,
    chain: Option<Chain>,
    /// Accumulated angle swept by the cursor while placing an arc's end (sign = direction).
    arc_sweep: f64,
    arc_last_angle: Option<f64>,
    dim_picks: Vec<EntityId>,
    pub selection: Vec<Sel>,
    hover: Option<Sel>,
    drag: Option<DragState>,
    undo: Vec<Sketch>,
    redo: Vec<Sketch>,
    edit: Option<DimEdit>,
    pub message: Option<Message>,
    /// Cursor position in sketch coordinates.
    pub cursor: Option<DVec2>,
    inference: Option<Inference>,
    /// Screen rectangles of dimension labels and relation glyphs from the last paint.
    hit_rects: Vec<(Sel, Rect)>,
    profile: Profile,
    /// Triangulated closed regions, shaded so the user can see which profiles are closed.
    fills: Vec<peet_sketch::triangulate::Triangles>,
    profile_dirty: bool,
}

impl SketchEditor {
    pub fn new(item: ItemId, sketch_item: &mut SketchItem) -> Self {
        let mut editor = Self {
            item,
            solver: Solver::new(),
            analysis: Analysis::default(),
            last_report: SolveReport::default(),
            solve_ms: 0.0,
            tool: Tool::Select,
            options: ToolOptions::default(),
            clicks: Vec::new(),
            chain: None,
            arc_sweep: 0.0,
            arc_last_angle: None,
            dim_picks: Vec::new(),
            selection: Vec::new(),
            hover: None,
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            edit: None,
            message: None,
            cursor: None,
            inference: None,
            hit_rects: Vec::new(),
            profile: Profile::default(),
            fills: Vec::new(),
            profile_dirty: true,
        };
        editor.resolve(sketch_item);
        editor
    }

    // ---- Status ----

    pub fn status(&self) -> SketchStatus {
        if self.analysis.is_over_defined() || !self.last_report.converged {
            SketchStatus::Over
        } else if self.analysis.dof == 0 {
            SketchStatus::Fully
        } else {
            SketchStatus::Under
        }
    }

    /// One-line summary for the status bar.
    pub fn status_text(&self) -> String {
        match self.status() {
            SketchStatus::Fully => "Fully defined".to_owned(),
            SketchStatus::Under => format!("Under defined ({} DOF)", self.analysis.dof),
            SketchStatus::Over => {
                if self.last_report.converged {
                    "Over defined".to_owned()
                } else {
                    "Over defined: can't solve".to_owned()
                }
            }
        }
    }

    /// Instructions for the current tool and step.
    pub fn hint(&self) -> &'static str {
        let step = self.clicks.len();
        match self.tool {
            Tool::Select => {
                "Click to select, Shift/Ctrl+click to add. Drag geometry to move it, drag empty space to box select. Double-click a dimension to edit it."
            }
            Tool::Line if self.chain.is_some() => {
                "Click the next point. Double-click or Esc to finish the chain."
            }
            Tool::Line => "Click the start point of the line.",
            Tool::Rectangle if step == 0 => "Click the first corner.",
            Tool::Rectangle => "Click the opposite corner.",
            Tool::CenterRectangle if step == 0 => "Click the centre.",
            Tool::CenterRectangle => "Click a corner.",
            Tool::Circle if step == 0 => "Click the centre.",
            Tool::Circle => "Click to set the radius.",
            Tool::Arc if step == 0 => "Click the arc centre.",
            Tool::Arc if step == 1 => "Click the arc start point.",
            Tool::Arc => {
                "Click the end point. Move the cursor around the centre to choose the direction."
            }
            Tool::Slot if step == 0 => "Click the first end centre.",
            Tool::Slot if step == 1 => "Click the second end centre.",
            Tool::Slot => "Click to set the slot width.",
            Tool::Polygon if step == 0 => "Click the centre.",
            Tool::Polygon => "Click to place a vertex.",
            Tool::Point => "Click to place a point.",
            Tool::Trim => "Click the part of a curve to remove.",
            Tool::Extend => "Click near the end of a line or arc to extend it.",
            Tool::Fillet => "Click a corner point to round it (radius in the properties panel).",
            Tool::Dimension if self.dim_picks.is_empty() => {
                "Click a line, circle, arc or point to dimension."
            }
            Tool::Dimension => {
                "Click a second entity, or click empty space to place the dimension."
            }
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    // ---- Solving and undo ----

    /// Solves and analyses after any change, and refreshes derived state.
    fn resolve(&mut self, item: &mut SketchItem) {
        let start = Instant::now();
        let report = self.solver.solve(&mut item.sketch);
        let analysis = self.solver.analyze(&item.sketch);
        self.accept(item, report, analysis, peet_platform::elapsed_ms(start));
    }

    /// Takes a solve result as current and refreshes everything derived from it.
    fn accept(&mut self, item: &mut SketchItem, report: SolveReport, analysis: Analysis, ms: f64) {
        self.last_report = report;
        self.analysis = analysis;
        self.solve_ms = ms;
        self.profile_dirty = true;
        item.status = self.status();
        self.prune_selection(&item.sketch);
    }

    /// Records `before` as an undo step.
    fn push_undo(&mut self, before: Sketch) {
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, item: &mut SketchItem) {
        if let Some(prev) = self.undo.pop() {
            let current = std::mem::replace(&mut item.sketch, prev);
            self.redo.push(current);
            self.cancel_tool();
            self.resolve(item);
        }
    }

    pub fn redo(&mut self, item: &mut SketchItem) {
        if let Some(next) = self.redo.pop() {
            let current = std::mem::replace(&mut item.sketch, next);
            self.undo.push(current);
            self.cancel_tool();
            self.resolve(item);
        }
    }

    fn prune_selection(&mut self, sketch: &Sketch) {
        self.selection.retain(|s| match *s {
            Sel::Entity(id) => sketch.entity(id).is_some(),
            Sel::Constraint(id) => sketch.constraint(id).is_some(),
        });
        if let Some(edit) = &self.edit
            && sketch.constraint(edit.dimension).is_none()
        {
            self.edit = None;
        }
    }

    /// Applies an edit, then solves. If the result is over defined or can't be solved,
    /// the edit is rolled back and the user is told which constraints clash.
    ///
    /// `what` names the change for messages ("Horizontal", "Trim").
    fn checked_edit<T>(
        &mut self,
        item: &mut SketchItem,
        what: &str,
        edit: impl FnOnce(&mut Sketch) -> Result<T, String>,
    ) -> Option<T> {
        let before = item.sketch.clone();
        let previous_diagnoses = self.analysis.diagnoses.len();
        let degenerate_before = degenerate_curves(&before);
        let result = match edit(&mut item.sketch) {
            Ok(r) => r,
            Err(e) => {
                item.sketch = before;
                self.error(format!("{what}: {e}"));
                return None;
            }
        };
        let start = Instant::now();
        let report = self.solver.solve(&mut item.sketch);
        let analysis = self.solver.analyze(&item.sketch);
        let ms = peet_platform::elapsed_ms(start);
        if !report.converged || analysis.diagnoses.len() > previous_diagnoses {
            let explanation = self.explain_problem(&item.sketch, &analysis, &report);
            item.sketch = before;
            self.resolve(item);
            self.error(format!(
                "{what} would over-define the sketch. {explanation}"
            ));
            return None;
        }
        // Relations can be consistent only by collapsing geometry (horizontal + vertical on
        // one line): that is never what the user wants.
        let collapsed: Vec<EntityId> = degenerate_curves(&item.sketch)
            .into_iter()
            .filter(|e| !degenerate_before.contains(e) && before.entity(*e).is_some())
            .collect();
        if let Some(&e) = collapsed.first() {
            let name = describe_entity(&item.sketch, e);
            let mut related: Vec<String> = item
                .sketch
                .constraints_on(e)
                .filter(|c| before.constraint(*c).is_some())
                .map(|c| describe_constraint(&item.sketch, c))
                .collect();
            related.truncate(4);
            item.sketch = before;
            self.resolve(item);
            let with = if related.is_empty() {
                String::new()
            } else {
                format!(" It conflicts with: {}.", related.join(", "))
            };
            self.error(format!("{what} would shrink {name} to nothing.{with}"));
            return None;
        }
        self.push_undo(before);
        self.accept(item, report, analysis, ms);
        Some(result)
    }

    /// Adds relations to the sketch as one undoable step, rejecting clashes.
    fn add_relations(&mut self, item: &mut SketchItem, kinds: Vec<ConstraintKind>) {
        let Some(first) = kinds.first() else {
            return;
        };
        let label = first.label();
        let added = self.checked_edit(item, label, |sketch| {
            kinds
                .iter()
                .map(|k| sketch.add_constraint(*k).map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()
        });
        if let Some(added) = added {
            self.selection = added.into_iter().map(Sel::Constraint).collect();
        }
    }

    /// Describes the first new problem in an analysis for an error message.
    fn explain_problem(
        &self,
        sketch: &Sketch,
        analysis: &Analysis,
        report: &SolveReport,
    ) -> String {
        let old: Vec<ConstraintId> = self
            .analysis
            .diagnoses
            .iter()
            .map(|d| d.constraint)
            .collect();
        if let Some(d) = analysis
            .diagnoses
            .iter()
            .find(|d| !old.contains(&d.constraint))
            .or(analysis.diagnoses.first())
        {
            let mut all = vec![d.constraint];
            all.extend(d.involved.iter().copied());
            let names: Vec<String> = all
                .iter()
                .map(|c| describe_constraint(sketch, *c))
                .collect();
            let verb = if d.status == ConstraintStatus::Conflicting {
                "These conflict"
            } else {
                "These already fix the same thing"
            };
            return format!("{verb}: {}.", names.join(", "));
        }
        if !report.unsatisfied.is_empty() {
            let names: Vec<String> = report
                .unsatisfied
                .iter()
                .take(4)
                .map(|c| describe_constraint(sketch, *c))
                .collect();
            // Name the other relations on the same geometry: those are what it clashes with.
            let mut others: Vec<ConstraintId> = Vec::new();
            for c in &report.unsatisfied {
                let Some(con) = sketch.constraint(*c) else {
                    continue;
                };
                for e in con.kind.entities() {
                    let mut ids = vec![e];
                    if let Some(entity) = sketch.entity(e) {
                        ids.extend(entity.geometry.points());
                    }
                    for id in ids {
                        for o in sketch.constraints_on(id) {
                            if !report.unsatisfied.contains(&o) && !others.contains(&o) {
                                others.push(o);
                            }
                        }
                    }
                }
            }
            let others: Vec<String> = others
                .into_iter()
                .take(4)
                .map(|c| describe_constraint(sketch, c))
                .collect();
            if others.is_empty() {
                return format!("Can't satisfy: {}.", names.join(", "));
            }
            return format!(
                "Can't satisfy {} together with {}.",
                names.join(", "),
                others.join(", ")
            );
        }
        "Delete or change a relation or dimension first.".to_owned()
    }

    fn cancel_tool(&mut self) {
        self.clicks.clear();
        self.chain = None;
        self.dim_picks.clear();
        self.arc_sweep = 0.0;
        self.arc_last_angle = None;
        self.drag = None;
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.cancel_tool();
        self.edit = None;
        self.tool = tool;
        if tool != Tool::Select && tool != Tool::Dimension {
            self.selection.clear();
        }
    }

    // ---- Commands ----

    /// Whether a sketch command can run now.
    pub fn command_enabled(&self, cmd: CommandId, sketch: &Sketch) -> bool {
        let entities = self.selected_entities();
        match cmd {
            CommandId::Undo => self.can_undo(),
            CommandId::Redo => self.can_redo(),
            CommandId::DeleteSelection => !self.selection.is_empty(),
            CommandId::SketchOffset => entities.iter().any(|e| is_curve(sketch, *e)),
            CommandId::SketchMirror => {
                entities.len() >= 2
                    && entities
                        .last()
                        .is_some_and(|a| sketch.kind(*a) == Some(EntityKind::Line))
            }
            cmd if relation_command(cmd) => !relations_for(cmd, sketch, &entities).is_empty(),
            _ => true,
        }
    }

    /// Runs a sketch command. Returns false if the command isn't a sketch command.
    pub fn command(&mut self, cmd: CommandId, item: &mut SketchItem) -> bool {
        if let Some(tool) = tool_for_command(cmd) {
            self.set_tool(tool);
            return true;
        }
        match cmd {
            CommandId::Undo => self.undo(item),
            CommandId::Redo => self.redo(item),
            CommandId::DeleteSelection => self.delete_selection(item),
            CommandId::ToggleConstruction => self.toggle_construction(item),
            CommandId::ToggleRelations => self.options.show_relations ^= true,
            CommandId::SketchOffset => self.offset_selection(item),
            CommandId::SketchMirror => self.mirror_selection(item),
            cmd if relation_command(cmd) => {
                let kinds = relations_for(cmd, &item.sketch, &self.selected_entities());
                if kinds.is_empty() {
                    self.error(format!(
                        "{}: select {}.",
                        cmd.info().label,
                        relation_needs(cmd)
                    ));
                } else if cmd == CommandId::RelFix {
                    self.fix_selection(item);
                } else {
                    self.add_relations(item, kinds);
                }
            }
            _ => return false,
        }
        true
    }

    pub fn selected_entities(&self) -> Vec<EntityId> {
        self.selection
            .iter()
            .filter_map(|s| match s {
                Sel::Entity(e) => Some(*e),
                Sel::Constraint(_) => None,
            })
            .collect()
    }

    fn delete_selection(&mut self, item: &mut SketchItem) {
        if self.selection.is_empty() {
            return;
        }
        let selection = self.selection.clone();
        let before = item.sketch.clone();
        let mut skipped_owned = false;
        for s in selection {
            match s {
                Sel::Constraint(c) => {
                    item.sketch.remove_constraint(c);
                }
                Sel::Entity(e) => {
                    let Some(entity) = item.sketch.entity(e) else {
                        continue;
                    };
                    if entity.locked {
                        continue;
                    }
                    // A curve's own endpoints can't be deleted on their own.
                    if entity.owner.is_some() && item.sketch.curves_using(e).next().is_some() {
                        skipped_owned = true;
                        continue;
                    }
                    let _ = item.sketch.remove_entity(e);
                }
            }
        }
        if item.sketch != before {
            self.push_undo(before);
        }
        self.selection.clear();
        self.resolve(item);
        if skipped_owned {
            self.info("Endpoints and centres are deleted together with their curve.");
        }
    }

    fn toggle_construction(&mut self, item: &mut SketchItem) {
        let curves: Vec<EntityId> = self
            .selected_entities()
            .into_iter()
            .filter(|e| item.sketch.entity(*e).is_some_and(|x| !x.locked))
            .collect();
        if curves.is_empty() {
            self.options.construction ^= true;
            let state = if self.options.construction {
                "on"
            } else {
                "off"
            };
            self.info(format!("Construction mode {state} for new geometry."));
            return;
        }
        let before = item.sketch.clone();
        let make = !curves
            .iter()
            .all(|e| item.sketch.entity(*e).is_some_and(|x| x.construction));
        for e in curves {
            item.sketch.set_construction(e, make);
        }
        self.push_undo(before);
        self.resolve(item);
    }

    fn fix_selection(&mut self, item: &mut SketchItem) {
        let entities = self.selected_entities();
        self.checked_edit(item, "Fix", |sketch| {
            for e in entities {
                sketch.fix(e).map_err(|e| e.to_string())?;
            }
            Ok(())
        });
    }

    fn offset_selection(&mut self, item: &mut SketchItem) {
        let curves: Vec<EntityId> = self
            .selected_entities()
            .into_iter()
            .filter(|e| is_curve(&item.sketch, *e))
            .collect();
        let distance = self.options.offset_distance;
        if let Some(new) = self.checked_edit(item, "Offset", |sketch| {
            peet_sketch::ops::offset(sketch, &curves, distance).map_err(|e| e.to_string())
        }) {
            self.selection = new.into_iter().map(Sel::Entity).collect();
        }
    }

    fn mirror_selection(&mut self, item: &mut SketchItem) {
        let mut entities = self.selected_entities();
        let Some(axis) = entities.pop() else {
            return;
        };
        if let Some(new) = self.checked_edit(item, "Mirror", |sketch| {
            peet_sketch::ops::mirror(sketch, &entities, axis).map_err(|e| e.to_string())
        }) {
            self.selection = new.into_iter().map(Sel::Entity).collect();
        }
    }

    /// Re-solves after an outside change (parameters edited, for example).
    pub fn refresh(&mut self, item: &mut SketchItem, params: &Parameters) {
        let errors = peet_sketch::expr::apply_expressions(&mut item.sketch, params);
        if let Some((id, e)) = errors.first() {
            let name = describe_constraint(&item.sketch, *id);
            self.error(format!("{name}: {e}"));
        }
        self.resolve(item);
    }

    /// Handles input and draws the sketch over the viewport.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        response: &Response,
        camera: &Camera,
        item: &mut SketchItem,
        params: &Parameters,
        dark: bool,
    ) {
        set_display_units(params);
        let view = SketchView::new(camera, response.rect, item.plane);
        self.cursor = response
            .hover_pos()
            .or_else(|| ui.input(|i| i.pointer.latest_pos()))
            .and_then(|p| view.to_sketch(p));
        self.handle_keys(ui, item);
        self.handle_input(ui, response, &view, item);
        if self.profile_dirty {
            self.profile = peet_sketch::region::find_regions(&item.sketch);
            self.fills = self
                .profile
                .regions
                .iter()
                .map(|r| {
                    let (lo, hi) = r.outer.bounds();
                    let tol = (lo.distance(hi) * 2e-3).max(1e-4);
                    peet_sketch::triangulate::triangulate_region(r, tol)
                })
                .collect();
            self.profile_dirty = false;
        }
        let painter = ui.painter_at(response.rect);
        self.paint(&painter, &view, &item.sketch, dark);
        self.dimension_editor(ui, &view, item, params);
        self.toast(ui, response.rect);
    }

    fn handle_keys(&mut self, ui: &Ui, item: &mut SketchItem) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (escape, enter) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Enter) && i.modifiers.is_none(),
            )
        });
        if escape {
            if self.drag.is_some() {
                // Abort the drag, restoring the geometry.
                if let Some(drag) = self.drag.take() {
                    item.sketch = drag.before;
                    self.resolve(item);
                }
            } else if !self.clicks.is_empty() || self.chain.is_some() || !self.dim_picks.is_empty()
            {
                self.cancel_tool();
            } else if self.tool != Tool::Select {
                self.set_tool(Tool::Select);
            } else {
                self.selection.clear();
            }
        }
        if enter && self.chain.is_some() {
            self.cancel_tool();
        }
    }

    /// A short-lived message at the bottom of the viewport.
    fn toast(&mut self, ui: &Ui, rect: Rect) {
        let Some(msg) = &self.message else {
            return;
        };
        let age = peet_platform::elapsed_ms(msg.at) / 1000.0;
        let lifetime = if msg.error { 8.0 } else { 4.0 };
        if age > lifetime {
            self.message = None;
            return;
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
        let color = if msg.error {
            egui::Color32::from_rgb(229, 72, 77)
        } else {
            ui.visuals().text_color()
        };
        egui::Area::new(egui::Id::new("sketch_toast"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.center_bottom() + egui::vec2(-260.0, -64.0))
            .interactable(false)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(520.0);
                    ui.colored_label(color, &msg.text);
                });
            });
    }
}

/// The tool a command switches to, if it's a tool command.
pub fn tool_for_command(cmd: CommandId) -> Option<Tool> {
    Some(match cmd {
        CommandId::SketchSelect => Tool::Select,
        CommandId::SketchLine => Tool::Line,
        CommandId::SketchRectangle => Tool::Rectangle,
        CommandId::SketchCenterRectangle => Tool::CenterRectangle,
        CommandId::SketchCircle => Tool::Circle,
        CommandId::SketchArc => Tool::Arc,
        CommandId::SketchSlot => Tool::Slot,
        CommandId::SketchPolygon => Tool::Polygon,
        CommandId::SketchPoint => Tool::Point,
        CommandId::SketchTrim => Tool::Trim,
        CommandId::SketchExtend => Tool::Extend,
        CommandId::SketchFillet => Tool::Fillet,
        CommandId::SmartDimension => Tool::Dimension,
        _ => return None,
    })
}

/// Commands handled by the sketch editor while a sketch is open.
pub fn is_sketch_command(cmd: CommandId) -> bool {
    tool_for_command(cmd).is_some()
        || relation_command(cmd)
        || matches!(
            cmd,
            CommandId::Undo
                | CommandId::Redo
                | CommandId::DeleteSelection
                | CommandId::ToggleConstruction
                | CommandId::ToggleRelations
                | CommandId::SketchOffset
                | CommandId::SketchMirror
        )
}

/// Sets a dimension from user input and re-evaluates the expressions that depend on it.
fn set_dimension(
    sketch: &mut Sketch,
    params: &Parameters,
    id: ConstraintId,
    input: &str,
) -> Result<(), String> {
    peet_sketch::expr::set_dimension_input(sketch, params, id, input).map_err(|e| e.message)?;
    if let Some((failed, e)) = peet_sketch::expr::apply_expressions(sketch, params).first() {
        return Err(format!("{}: {e}", describe_constraint(sketch, *failed)));
    }
    Ok(())
}

/// Lines of zero length and circles/arcs of zero radius.
fn degenerate_curves(sketch: &Sketch) -> Vec<EntityId> {
    sketch
        .entities()
        .filter(|(id, _)| {
            sketch.curve(*id).is_some_and(|c| {
                let size = c.radius().unwrap_or_else(|| c.length());
                size <= peet_math::tolerance::LINEAR * 10.0
            })
        })
        .map(|(id, _)| id)
        .collect()
}

fn is_curve(sketch: &Sketch, id: EntityId) -> bool {
    sketch.kind(id).is_some_and(EntityKind::is_curve)
}

/// Whether a command adds a relation to the selection.
pub fn relation_command(cmd: CommandId) -> bool {
    matches!(
        cmd,
        CommandId::RelCoincident
            | CommandId::RelHorizontal
            | CommandId::RelVertical
            | CommandId::RelParallel
            | CommandId::RelPerpendicular
            | CommandId::RelTangent
            | CommandId::RelEqual
            | CommandId::RelConcentric
            | CommandId::RelMidpoint
            | CommandId::RelSymmetric
            | CommandId::RelFix
    )
}

fn relation_needs(cmd: CommandId) -> &'static str {
    match cmd {
        CommandId::RelCoincident => "two points, or a point and a curve",
        CommandId::RelHorizontal | CommandId::RelVertical => "lines, or two points",
        CommandId::RelParallel | CommandId::RelEqual => {
            "two or more lines (or circles/arcs for Equal)"
        }
        CommandId::RelPerpendicular => "two lines",
        CommandId::RelTangent => "a line and a circle/arc, or two circles/arcs",
        CommandId::RelConcentric => "two or more circles/arcs",
        CommandId::RelMidpoint => "a point and a line",
        CommandId::RelSymmetric => "two points and a line",
        _ => "geometry",
    }
}

/// The constraints a relation command would add for the selected entities. Empty if the
/// selection doesn't fit.
pub fn relations_for(cmd: CommandId, sketch: &Sketch, sel: &[EntityId]) -> Vec<ConstraintKind> {
    use ConstraintKind as C;
    let of = |k: EntityKind| -> Vec<EntityId> {
        sel.iter()
            .copied()
            .filter(|e| sketch.kind(*e) == Some(k))
            .collect()
    };
    let points = of(EntityKind::Point);
    let lines = of(EntityKind::Line);
    let circular: Vec<EntityId> = sel
        .iter()
        .copied()
        .filter(|e| sketch.kind(*e).is_some_and(EntityKind::is_circular))
        .collect();
    let n = sel.len();
    let pairs_with_first = |ids: &[EntityId], f: fn(EntityId, EntityId) -> C| -> Vec<C> {
        ids.iter().skip(1).map(|b| f(ids[0], *b)).collect()
    };
    match cmd {
        CommandId::RelHorizontal if !lines.is_empty() && lines.len() == n => {
            lines.into_iter().map(C::Horizontal).collect()
        }
        CommandId::RelHorizontal if points.len() == 2 && n == 2 => {
            vec![C::HorizontalPoints(points[0], points[1])]
        }
        CommandId::RelVertical if !lines.is_empty() && lines.len() == n => {
            lines.into_iter().map(C::Vertical).collect()
        }
        CommandId::RelVertical if points.len() == 2 && n == 2 => {
            vec![C::VerticalPoints(points[0], points[1])]
        }
        CommandId::RelCoincident if points.len() == 2 && n == 2 => {
            vec![C::Coincident(points[0], points[1])]
        }
        CommandId::RelCoincident if points.len() == 1 && n == 2 => {
            let curve = sel
                .iter()
                .copied()
                .find(|e| *e != points[0])
                .expect("n == 2");
            vec![C::PointOnCurve {
                point: points[0],
                curve,
            }]
        }
        CommandId::RelParallel if lines.len() >= 2 && lines.len() == n => {
            pairs_with_first(&lines, C::Parallel)
        }
        CommandId::RelPerpendicular if lines.len() == 2 && n == 2 => {
            vec![C::Perpendicular(lines[0], lines[1])]
        }
        CommandId::RelEqual if lines.len() >= 2 && lines.len() == n => {
            pairs_with_first(&lines, C::Equal)
        }
        CommandId::RelEqual if circular.len() >= 2 && circular.len() == n => {
            pairs_with_first(&circular, C::Equal)
        }
        CommandId::RelConcentric if circular.len() >= 2 && circular.len() == n => {
            pairs_with_first(&circular, C::Concentric)
        }
        CommandId::RelTangent
            if n == 2 && lines.len() + circular.len() == 2 && !circular.is_empty() =>
        {
            vec![C::Tangent(sel[0], sel[1])]
        }
        CommandId::RelMidpoint if points.len() == 1 && lines.len() == 1 && n == 2 => {
            vec![C::Midpoint {
                point: points[0],
                line: lines[0],
            }]
        }
        CommandId::RelSymmetric if points.len() == 2 && lines.len() == 1 && n == 3 => {
            vec![C::Symmetric {
                a: points[0],
                b: points[1],
                axis: lines[0],
            }]
        }
        CommandId::RelFix if n > 0 => {
            // Placeholder kinds so the command shows as applicable; `fix_selection` does the work.
            sel.iter()
                .filter_map(|e| {
                    sketch
                        .try_point(*e)
                        .map(|at| C::Fix { point: *e, at })
                        .or_else(|| {
                            let p = *sketch.entity(*e)?.geometry.points().first()?;
                            Some(C::Fix {
                                point: p,
                                at: sketch.point(p),
                            })
                        })
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Human readable name of an entity: "Line 4", "Line 4 start", "Origin".
pub fn describe_entity(sketch: &Sketch, id: EntityId) -> String {
    if id == Sketch::ORIGIN {
        return "Origin".to_owned();
    }
    let Some(entity) = sketch.entity(id) else {
        return format!("#{}", id.0);
    };
    if let Some(owner) = entity.owner
        && let Some(o) = sketch.entity(owner)
    {
        let role = match o.geometry {
            peet_sketch::Geometry::Line { start, .. }
            | peet_sketch::Geometry::Arc { start, .. }
                if start == id =>
            {
                "start"
            }
            peet_sketch::Geometry::Line { end, .. } | peet_sketch::Geometry::Arc { end, .. }
                if end == id =>
            {
                "end"
            }
            _ => "centre",
        };
        return format!("{} {} {role}", o.kind().label(), owner.0);
    }
    format!("{} {}", entity.kind().label(), id.0)
}

/// Human readable description of a constraint: "Horizontal (Line 4)", "d2 = 25 (Distance)".
pub fn describe_constraint(sketch: &Sketch, id: ConstraintId) -> String {
    let Some(c) = sketch.constraint(id) else {
        return format!("constraint {}", id.0);
    };
    if let Some(d) = &c.dimension {
        let unit = if c.kind.is_angular() { "°" } else { "" };
        return format!(
            "{} = {}{unit} ({})",
            d.name,
            dim_value_text(&c.kind, d.value),
            c.kind.label()
        );
    }
    let names: Vec<String> = c
        .kind
        .entities()
        .into_iter()
        .map(|e| describe_entity(sketch, e))
        .collect();
    format!("{} ({})", c.kind.label(), names.join(", "))
}

thread_local! {
    /// The document's units, for showing lengths. Set from the parameter table whenever
    /// the editor is shown, so every label and field in it agrees.
    static UNITS: std::cell::Cell<peet_sketch::expr::Units> =
        std::cell::Cell::new(peet_sketch::expr::Units::default());
}

fn set_display_units(params: &Parameters) {
    UNITS.with(|u| u.set(params.units));
}

/// A length (mm) in document units, without the unit: what an edit field shows.
pub fn length_text(mm: f64) -> String {
    UNITS.with(|u| u.get().format_length_value(mm))
}

/// A length (mm) in document units, with the unit ("12.5 mm", "0.5 in").
pub fn length_with_unit(mm: f64) -> String {
    UNITS.with(|u| u.get().format_length(mm))
}

/// The document's length unit ("mm", "in").
pub fn length_suffix() -> &'static str {
    UNITS.with(|u| u.get().length.suffix())
}

/// A dimension's value as typed: degrees for angles, document units for lengths.
pub fn dim_value_text(kind: &ConstraintKind, value: f64) -> String {
    if kind.is_angular() {
        format_value(value)
    } else {
        length_text(value)
    }
}

/// Formats a dimension value compactly: up to three decimals, no trailing zeros.
pub fn format_value(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

/// Colour of an entity for its constraint state.
pub fn dof_color(status: DofStatus, dark: bool) -> egui::Color32 {
    match (status, dark) {
        (DofStatus::Under, true) => egui::Color32::from_rgb(110, 168, 255),
        (DofStatus::Under, false) => egui::Color32::from_rgb(28, 104, 226),
        (DofStatus::Fully, true) => egui::Color32::from_rgb(236, 238, 242),
        (DofStatus::Fully, false) => egui::Color32::from_rgb(24, 26, 30),
        (DofStatus::Over, _) => egui::Color32::from_rgb(229, 72, 77),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_formatting() {
        assert_eq!(format_value(25.0), "25");
        assert_eq!(format_value(12.5), "12.5");
        assert_eq!(format_value(1.0 / 3.0), "0.333");
        assert_eq!(format_value(-0.0001), "0");
    }

    #[test]
    fn relation_selection_rules() {
        let mut s = Sketch::new();
        let l1 = s.add_line(DVec2::ZERO, DVec2::X);
        let l2 = s.add_line(DVec2::Y, DVec2::ONE);
        let c = s.add_circle(DVec2::ZERO, 1.0);
        let (p, _) = s.endpoints(l1).unwrap();
        assert_eq!(
            relations_for(CommandId::RelHorizontal, &s, &[l1, l2]).len(),
            2
        );
        assert_eq!(
            relations_for(CommandId::RelParallel, &s, &[l1, l2]).len(),
            1
        );
        assert!(relations_for(CommandId::RelParallel, &s, &[l1]).is_empty());
        assert!(relations_for(CommandId::RelParallel, &s, &[l1, c]).is_empty());
        assert_eq!(relations_for(CommandId::RelTangent, &s, &[l2, c]).len(), 1);
        assert_eq!(
            relations_for(CommandId::RelCoincident, &s, &[p, c]),
            vec![ConstraintKind::PointOnCurve { point: p, curve: c }]
        );
        assert_eq!(describe_entity(&s, p), format!("Line {} start", l1.0));
    }
}

#[cfg(test)]
mod ui_tests;
