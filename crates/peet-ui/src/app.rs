//! The PeetCAD application shell: menus, toolbar, panels, windows and command dispatch.

use eframe::egui_wgpu::RenderState;
use egui::{Align, Layout, Ui};
use peet_kernel::Surface;
use peet_math::DVec3;
use peet_model::{
    AxisDef, CoordSystemDef, Datum, EdgeRef, FeatureId, FeatureKind, PlaneDef, PlaneRef, PointDef,
    PointRef, Scalar, ScalarKind, StdPlane,
};
use peet_platform::Instant;
use peet_render::{Projection, StandardView};

use peet_ops::Op;

use crate::bodies::GeomRef;
use crate::commands::{CommandId, CommandState};
use crate::document::{Document, FileLocation, ItemId, Persistent, SketchItem};
use crate::features_ui::{self, ERROR, PICKING, Picked, Slot};
use crate::files::{self, AfterDiscard, FileState};
use crate::icons::{self, Icon};
use crate::palette::CommandPalette;
use crate::perf::{PerfInfo, PerfMonitor};
use crate::ribbon;
use crate::settings::{MousePreset, OrbitChoice, STORAGE_KEY, Settings, ThemeChoice};
use crate::sketch_ui::{self, SketchEditor};
use crate::solid_ui;
use crate::tree::{TreeAction, TreeView, tree_ui};
use crate::viewport::{Viewport, ViewportParams};

mod convert;
pub mod scripting;
mod solids;

pub const APP_NAME: &str = "PeetCAD";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The ribbon's tabs. Sketch only shows while a sketch is open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RibbonTab {
    File,
    #[default]
    Model,
    Sketch,
    SheetMetal,
    View,
    Help,
}

#[derive(Default)]
struct OpenWindows {
    settings: bool,
    shortcuts: bool,
    about: bool,
    parameters: bool,
    bend_table: bool,
    sheet_checks: bool,
    gauges: bool,
    mass_properties: bool,
}

/// What dragging an edge flange's length handle asks for this frame.
enum FlangeEdit {
    /// The flange's length, in mm.
    Length { id: FeatureId, length: f64 },
    /// The drag ended: what it changed is one undo step.
    Done,
}

/// A drag of an edge flange's length handle in progress.
#[derive(Clone, Copy, Debug)]
struct FlangeDrag {
    id: FeatureId,
    /// The flange's far end and direction when the drag started.
    origin: DVec3,
    dir: DVec3,
    /// Its length then (mm), and where along its line the pointer was.
    length: f64,
    start: f64,
}

pub struct PeetApp {
    settings: Settings,
    applied_theme: Option<ThemeChoice>,
    doc: Document,
    selected: Option<ItemId>,
    /// Item under the cursor in the feature tree this frame (highlighted in the viewport).
    hovered: Option<ItemId>,
    render_state: Option<RenderState>,
    viewport: Option<Viewport>,
    initial_fit_done: bool,
    perf: PerfMonitor,
    palette: CommandPalette,
    windows: OpenWindows,
    adapter_name: String,
    backend_name: String,
    /// The sketch being edited, if any (sketch mode), and its working copy.
    sketch: Option<SketchEditor>,
    sketch_work: Option<SketchItem>,
    /// Inputs of the "add parameter" row in the parameters window.
    new_param: (String, String),
    param_error: Option<String>,
    /// Body face, edge or vertex under the cursor (from GPU picking).
    hovered_geom: Option<GeomRef>,
    /// Selected body faces, edges and vertices.
    selected_geom: Vec<GeomRef>,
    /// A feature reference waiting to be clicked in the viewport.
    picking: Option<(FeatureId, Slot)>,
    /// New Sketch is waiting for a face or plane to be clicked.
    picking_sketch_plane: bool,
    /// Last message for the status bar, and whether it is an error.
    status_message: Option<(String, bool)>,
    files: FileState,
    /// The user chose to quit (after deciding about unsaved changes).
    quit_requested: bool,
    /// The window title last set, so it is only sent when it changes.
    title: String,
    flange_drag: Option<FlangeDrag>,
    /// The operations applied to the part so far.
    journal: scripting::Journal,
    ribbon_tab: RibbonTab,
    /// The gauge window: the table shown, and what went wrong with the last import.
    gauge_table: usize,
    gauge_message: Option<String>,
    /// File dialogs waiting for an answer.
    gauge_import: Option<peet_platform::Pending<Result<Option<peet_platform::OpenedFile>, String>>>,
    dxf_import: Option<peet_platform::Pending<Result<Option<peet_platform::OpenedFile>, String>>>,
    step_import: Option<peet_platform::Pending<Result<Option<peet_platform::OpenedFile>, String>>>,
    /// A message to read and dismiss (what an import left out, or why it failed).
    notice: Option<solids::Notice>,
}

impl PeetApp {
    /// Creates the app. `process_start` is used to report startup time.
    pub fn new(cc: &eframe::CreationContext<'_>, process_start: Instant) -> Self {
        log::info!(
            "Window and GPU ready {:.0} ms after start",
            peet_platform::elapsed_ms(process_start)
        );
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();

        Self::with_renderer(settings, cc.wgpu_render_state.clone(), process_start)
    }

    /// The app without a GPU or a window: commands work, the viewport shows nothing.
    #[cfg(test)]
    pub(crate) fn headless() -> Self {
        Self::with_renderer(Settings::default(), None, Instant::now())
    }

    fn with_renderer(
        settings: Settings,
        render_state: Option<RenderState>,
        process_start: Instant,
    ) -> Self {
        let (adapter_name, backend_name) = render_state.as_ref().map_or_else(
            || ("none".to_owned(), "none".to_owned()),
            |rs| {
                let info = rs.adapter.get_info();
                // Browsers hide the GPU model from WebGPU pages for privacy.
                let name = if info.name.is_empty() {
                    "Browser GPU".to_owned()
                } else {
                    info.name
                };
                (name, format!("{:?}", info.backend))
            },
        );
        log::info!("GPU: {adapter_name} ({backend_name})");

        let pipelines_start = Instant::now();
        let mut viewport = render_state.as_ref().map(Viewport::new);
        log::info!(
            "Viewport renderer created in {:.0} ms",
            peet_platform::elapsed_ms(pipelines_start)
        );
        if let Some(vp) = &mut viewport {
            vp.camera.projection = if settings.perspective {
                Projection::Perspective
            } else {
                Projection::Orthographic
            };
        }

        Self {
            settings,
            applied_theme: None,
            doc: Document::default(),
            selected: None,
            hovered: None,
            render_state,
            viewport,
            initial_fit_done: false,
            perf: PerfMonitor::new(process_start),
            palette: CommandPalette::default(),
            windows: OpenWindows::default(),
            adapter_name,
            backend_name,
            sketch: None,
            sketch_work: None,
            new_param: (String::new(), String::new()),
            param_error: None,
            hovered_geom: None,
            selected_geom: Vec::new(),
            picking: None,
            picking_sketch_plane: false,
            status_message: None,
            files: FileState::default(),
            quit_requested: false,
            title: String::new(),
            flange_drag: None,
            journal: scripting::Journal::default(),
            ribbon_tab: RibbonTab::default(),
            gauge_table: 0,
            gauge_message: None,
            gauge_import: None,
            dxf_import: None,
            step_import: None,
            notice: None,
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.status_message = Some((text.into(), false));
    }

    fn error(&mut self, text: impl Into<String>) {
        self.status_message = Some((text.into(), true));
    }

    // ---- Document changes ----

    /// Applies a change to the model as one undo step, keeping the face/edge selection
    /// through the rebuild. The change is made by the operations that express it (see
    /// `scripting`), like every change to the part.
    fn change(&mut self, label: &str, f: impl FnOnce(&mut peet_model::Model)) -> bool {
        self.change_model(label, None, f)
    }

    fn restore_selection(&mut self, kept: &[Persistent]) {
        self.selected_geom = kept.iter().filter_map(|p| self.doc.restore(p)).collect();
        self.hovered_geom = None;
        if let Some(ItemId::Feature(id)) = self.selected
            && self.doc.feature(id).is_none()
        {
            self.selected = None;
        }
        if let Some((id, _)) = self.picking
            && self.doc.feature(id).is_none()
        {
            self.picking = None;
        }
    }

    fn undo_redo(&mut self, redo: bool) {
        let (op, word, done) = if redo {
            (Op::Redo, "redo", "Redid")
        } else {
            (Op::Undo, "undo", "Undid")
        };
        let reply = self.perform(op);
        if let Some(label) = reply.json[word].as_str() {
            self.info(format!("{done} {label}"));
        }
    }

    /// Replaces the document (new, opened, recovered).
    fn set_document(&mut self, doc: Document) {
        self.doc = doc;
        self.document_replaced();
    }

    /// Forgets what was known about the document that was open before: the open sketch,
    /// the selection, what was being picked.
    fn document_replaced(&mut self) {
        self.close_sketch_discarding();
        self.selected = None;
        self.hovered = None;
        self.selected_geom.clear();
        self.hovered_geom = None;
        self.picking = None;
        self.picking_sketch_plane = false;
        self.initial_fit_done = false;
    }

    // ---- Selection helpers ----

    /// The selected planar face, if exactly one face is selected.
    fn selected_face(&self) -> Option<(PlaneRef, peet_math::Plane)> {
        match self.selected_geom[..] {
            [GeomRef::Face { body, face }] => {
                let b = &self.doc.bodies.get(body)?.source;
                let plane = peet_model::face_sketch_plane(&b.solid, face)?;
                Some((PlaneRef::Face(b.face_ref(face)), plane))
            }
            _ => None,
        }
    }

    /// The selected flat thing to build on: a face, a standard plane or a reference plane.
    fn selected_plane(&self) -> Option<(PlaneRef, peet_math::Plane)> {
        self.selected_face().or_else(|| {
            let item = self.selected?;
            Some((
                self.doc.plane_ref_of_item(item)?,
                self.doc.resolve_plane(item)?,
            ))
        })
    }

    fn selected_feature(&self) -> Option<FeatureId> {
        self.selected.and_then(ItemId::feature)
    }

    fn selected_sketch(&self) -> Option<FeatureId> {
        self.selected_feature()
            .filter(|id| self.doc.model.sketch(*id).is_some())
    }

    /// The sketch an extrude command applies to: the open one, or the selected one.
    fn extrude_source(&self) -> Option<FeatureId> {
        self.sketch
            .as_ref()
            .and_then(|e| e.item.feature())
            .or_else(|| self.selected_sketch())
    }

    /// What was clicked, as a reference.
    fn picked(&self, geom: Option<GeomRef>, plane: Option<ItemId>) -> Option<Picked> {
        match geom {
            Some(GeomRef::Face { body, face }) => {
                let b = &self.doc.bodies.get(body)?.source;
                let surface = &b.solid.face(face).surface;
                Some(Picked::Face {
                    face: b.face_ref(face),
                    planar: matches!(surface, Surface::Plane(_)),
                    round: !matches!(surface, Surface::Plane(_)),
                })
            }
            Some(GeomRef::Edge { body, edge }) => Some(Picked::Edge(
                self.doc.bodies.get(body)?.source.edge_ref(edge)?,
            )),
            Some(GeomRef::Vertex { body, vertex }) => Some(Picked::Vertex(
                self.doc.bodies.get(body)?.source.vertex_ref(vertex),
            )),
            None => Some(Picked::Plane(self.doc.plane_ref_of_item(plane?)?)),
        }
    }

    /// Fills the reference being picked with what was clicked.
    fn finish_pick(&mut self, picked: Option<Picked>) {
        let Some((id, slot)) = self.picking.take() else {
            return;
        };
        self.selected = Some(ItemId::Feature(id));
        let Some(picked) = picked else {
            self.error(slot.prompt());
            self.picking = Some((id, slot));
            return;
        };
        let Some(feature) = self.doc.feature(id) else {
            return;
        };
        let name = feature.name.clone();
        let mut kind = feature.kind.clone();
        // What the feature itself made can't define it: the reference would go in a circle.
        let own = match &picked {
            Picked::Edge(e) => e.features().any(|f| f == id),
            Picked::Vertex(v) => v.features().any(|f| f == id),
            Picked::Face { face, .. } => face.name.features().any(|f| f == id),
            _ => false,
        };
        if own {
            self.error(format!(
                "That was made by {name} itself, so {name} can't be built on it. Pick something that was there before."
            ));
            self.picking = Some((id, slot));
            return;
        }
        // A loft's profile must be built before the loft, and is hidden once it is used.
        let profile = match (slot, &picked) {
            (Slot::LoftProfile, Picked::Sketch(s)) => Some(*s),
            _ => None,
        };
        if let Some(s) = profile
            && self.doc.model.index_of(s) > self.doc.model.index_of(id)
        {
            self.error(format!(
                "{} comes after {name} in the feature tree. Drag it above {name} to use it as a profile.",
                self.doc.model.name_of(s)
            ));
            self.picking = Some((id, slot));
            return;
        }
        match features_ui::apply_pick(&mut kind, slot, picked) {
            Ok(()) => {
                self.change(&format!("Edit {name}"), |m| {
                    if let Some(f) = m.feature_mut(id) {
                        f.kind = kind;
                    }
                    if let Some(f) = profile.and_then(|s| m.feature_mut(s)) {
                        f.visible = false;
                    }
                });
                self.status_message = None;
                // Lists of edges and faces take one click after another.
                if slot.repeats() && self.doc.feature(id).is_some() {
                    self.picking = Some((id, slot));
                }
            }
            Err(e) => {
                self.error(e);
                self.picking = Some((id, slot));
            }
        }
    }

    /// Starts a sketch on what was clicked while New Sketch was waiting for a plane.
    fn finish_sketch_plane_pick(&mut self, geom: Option<GeomRef>, plane: Option<ItemId>) {
        let target = match (geom, plane) {
            (Some(GeomRef::Face { body, face }), _) => {
                let Some(b) = self.doc.bodies.get(body).map(|b| &b.source) else {
                    return;
                };
                match peet_model::face_sketch_plane(&b.solid, face) {
                    Some(placement) => Some((PlaneRef::Face(b.face_ref(face)), placement)),
                    None => {
                        self.error("That face is curved: click a flat face or a plane.");
                        return;
                    }
                }
            }
            (Some(_), _) => {
                self.error("Click a flat face or a plane, not an edge or vertex.");
                return;
            }
            (None, Some(item)) => self
                .doc
                .plane_ref_of_item(item)
                .zip(self.doc.resolve_plane(item)),
            (None, None) => None,
        };
        if let Some((plane, placement)) = target {
            self.picking_sketch_plane = false;
            self.status_message = None;
            self.new_sketch(plane, placement);
        }
    }

    // ---- Sketches ----

    /// Opens a sketch for editing and turns the view to look straight at it.
    fn open_sketch(&mut self, id: FeatureId) {
        self.close_sketch();
        // Sketches are drawn on the folded part.
        if self.doc.is_flat() {
            self.perform(Op::FlatPattern { on: Some(false) });
        }
        let Some(f) = self.doc.model.sketch(id) else {
            return;
        };
        let Some((plane, status)) = self.doc.sketch_placement(id) else {
            return;
        };
        let mut work = SketchItem {
            plane,
            plane_name: self.doc.plane_name(&f.plane),
            sketch: f.sketch.clone(),
            status,
        };
        self.sketch = Some(SketchEditor::new(ItemId::Feature(id), &mut work));
        self.ribbon_tab = RibbonTab::Sketch;
        self.sketch_work = Some(work);
        self.selected = Some(ItemId::Feature(id));
        self.selected_geom.clear();
        self.picking = None;
        if let Some(vp) = &mut self.viewport {
            let rotation =
                peet_render::camera::rotation_from_back_up(plane.normal(), plane.frame.y_axis());
            vp.set_rotation(rotation, self.settings.animate_views);
        }
    }

    /// Ends sketch editing, writing the sketch back to the model as one undo step.
    fn close_sketch(&mut self) {
        if self.ribbon_tab == RibbonTab::Sketch {
            self.ribbon_tab = RibbonTab::Model;
        }
        let (Some(editor), Some(work)) = (self.sketch.take(), self.sketch_work.take()) else {
            return;
        };
        let Some(id) = editor.item.feature() else {
            return;
        };
        let name = self.doc.model.name_of(id).to_owned();
        self.change(&format!("Edit {name}"), |m| {
            if let Some(f) = m.feature_mut(id) {
                if let Some(s) = f.sketch_mut() {
                    s.sketch = work.sketch;
                }
                f.visible = true;
            }
        });
    }

    fn close_sketch_discarding(&mut self) {
        if self.ribbon_tab == RibbonTab::Sketch {
            self.ribbon_tab = RibbonTab::Model;
        }
        self.sketch = None;
        self.sketch_work = None;
    }

    fn new_sketch(&mut self, plane: PlaneRef, placement: peet_math::Plane) {
        self.close_sketch();
        let mut id = None;
        self.change("New Sketch", |m| id = Some(m.add_sketch(plane, placement)));
        if let Some(id) = id {
            self.open_sketch(id);
        }
    }

    fn start_extrude(&mut self, operation: peet_model::Operation) {
        let Some(sketch) = self.extrude_source() else {
            return;
        };
        self.close_sketch();
        let first = self.doc.bodies.is_empty();
        let mut id = None;
        let label = if operation == peet_model::Operation::Cut {
            "Cut-Extrude"
        } else {
            "Extrude"
        };
        self.change(&format!("Add {label}"), |m| {
            let e = m.add_extrude(sketch, operation);
            if first
                && operation == peet_model::Operation::Add
                && let Some(x) = m.feature_mut(e).and_then(|f| f.extrude_mut())
            {
                x.params.operation = peet_model::Operation::NewBody;
            }
            id = Some(e);
        });
        self.selected = id.map(ItemId::Feature);
        self.selected_geom.clear();
        if let Some(e) = id
            .and_then(|id| self.doc.feature(id))
            .and_then(|f| f.extrude())
        {
            let (sketch, dir) = (e.sketch, e.params.direction());
            self.show_in_3d(sketch, dir);
        }
    }

    /// Turns the view to a 3D one that shows a sketch-based feature from the side its
    /// material goes to (`dir`: +1 along the sketch normal, −1 against it), framing
    /// everything.
    fn show_in_3d(&mut self, sketch: FeatureId, dir: f64) {
        let Some((plane, _)) = self.doc.sketch_placement(sketch) else {
            return;
        };
        // The isometric view, mirrored to the side the material goes to, so the view looks
        // at the new feature rather than from behind the sketch.
        let iso = DVec3::new(1.0, -1.0, 1.0);
        let out = plane.normal() * dir;
        let from = if iso.dot(out) >= 0.0 {
            iso
        } else {
            let n = out.normalize();
            iso - n * (2.0 * iso.dot(n))
        };
        let bounds = self.doc.visible_bounds();
        let animate = self.settings.animate_views;
        if let Some(vp) = &mut self.viewport {
            vp.set_rotation(peet_render::camera::rotation_looking_from(from), animate);
            vp.zoom_to_fit(&bounds, animate);
        }
    }

    /// Whether a sketch lies on a face of a sheet metal body.
    fn sketch_on_sheet(&self, sketch: FeatureId) -> bool {
        let bodies = &self.doc.evaluation().bodies;
        match self.doc.model.sketch(sketch).map(|s| &s.plane) {
            Some(PlaneRef::Face(f)) => peet_model::naming::find_face(bodies, f)
                .is_some_and(|found| bodies[found.body].sheet.is_some()),
            _ => false,
        }
    }

    /// Starts a sheet metal part from the open or selected sketch.
    fn start_base_flange(&mut self) {
        let Some(sketch) = self.extrude_source() else {
            return;
        };
        self.close_sketch();
        // A new sheet metal body starts with the settings of the last one.
        let previous = self.doc.model.features().rev().find_map(|f| match &f.kind {
            FeatureKind::BaseFlange(b) => Some(b.settings.clone()),
            _ => None,
        });
        let mut id = None;
        self.change("Add Base Flange", |m| {
            let b = m.add_base_flange(sketch);
            if let (Some(settings), Some(f)) = (previous, m.feature_mut(b))
                && let FeatureKind::BaseFlange(def) = &mut f.kind
            {
                def.settings = settings;
            }
            id = Some(b);
        });
        self.selected = id.map(ItemId::Feature);
        self.selected_geom.clear();
        if let Some(id) = id {
            // Look from the side the sheet ended up on: thickness or depth, depending on
            // whether the profile is closed or open.
            let side = self.doc.sketch_placement(sketch).and_then(|(plane, _)| {
                let body = self
                    .doc
                    .evaluation()
                    .bodies
                    .iter()
                    .find(|b| b.origin == id)?;
                let off = plane
                    .normal()
                    .dot(body.solid.bounds().center() - plane.origin());
                (off.abs() > 1e-6).then(|| off.signum())
            });
            self.show_in_3d(sketch, side.unwrap_or(1.0));
        }
    }

    /// Adds an edge flange on each selected sheet metal edge, or one waiting for a pick.
    fn start_edge_flange(&mut self) {
        let edges: Vec<EdgeRef> = self
            .selected_geom
            .iter()
            .filter_map(|g| match *g {
                GeomRef::Edge { body, edge } => {
                    let b = &self.doc.bodies.get(body)?.source;
                    b.sheet.as_ref()?;
                    b.edge_ref(edge)
                }
                _ => None,
            })
            .collect();
        let pick = edges.is_empty();
        let label = if edges.len() > 1 {
            "Add Edge Flanges"
        } else {
            "Add Edge Flange"
        };
        let mut ids = Vec::new();
        self.change(label, |m| {
            if edges.is_empty() {
                ids.push(m.add_edge_flange(None));
            }
            for e in edges {
                ids.push(m.add_edge_flange(Some(e)));
            }
        });
        let last = ids.last().copied();
        self.selected = last.map(ItemId::Feature);
        self.selected_geom.clear();
        if pick && let Some(id) = last {
            self.picking = Some((id, Slot::FlangeEdge));
            self.status_message = None;
        }
    }

    /// Cuts the open or selected sketch through the sheet it is drawn on.
    fn start_sheet_cut(&mut self) {
        let Some(sketch) = self.extrude_source() else {
            return;
        };
        self.close_sketch();
        let mut id = None;
        self.change("Add Sheet Metal Cut", |m| {
            id = Some(m.add_sheet_cut(sketch))
        });
        self.selected = id.map(ItemId::Feature);
        self.selected_geom.clear();
    }

    fn toggle_flat(&mut self) {
        let flat = !self.doc.is_flat();
        self.perform(Op::FlatPattern { on: Some(flat) });
        if flat {
            self.info("Showing the flat pattern (U to fold it again). Bend lines are dashed.");
        } else {
            self.status_message = None;
        }
    }

    fn export_dxf(&mut self) {
        let preferred = self.selected_geom.first().map(|g| g.body());
        let Some(body) = self.doc.sheet_body(preferred).cloned() else {
            self.error("There is no sheet metal part to export: start one with Base Flange.");
            return;
        };
        let Some(sheet) = &body.sheet else {
            return;
        };
        let index = self
            .doc
            .evaluation()
            .bodies
            .iter()
            .position(|b| std::sync::Arc::ptr_eq(b, &body));
        let text = match peet_ops::export_bytes(
            &self.doc,
            peet_ops::Format::Dxf,
            index,
            peet_io::step::StepSchema::default(),
        ) {
            Ok((bytes, _)) => bytes,
            Err(e) => return self.error(e),
        };
        let title = self.doc.title();
        let stem = title.strip_suffix(".peet").unwrap_or(&title).to_owned();
        let several = self
            .doc
            .evaluation()
            .bodies
            .iter()
            .filter(|b| b.sheet.is_some())
            .count()
            > 1;
        let name = if several {
            format!("{stem}-{}.dxf", self.doc.model.name_of(body.origin))
        } else {
            format!("{stem}.dxf")
        };
        let size = sheet.report().flat_size;
        let units = self.doc.model.parameters.units;
        match peet_platform::save_file(&name, ("DXF drawing", &["dxf"]), &text) {
            Ok(peet_platform::SaveOutcome::Saved(to)) => self.info(format!(
                "Exported the flat pattern ({} × {}) to {to}",
                units.format_length(size.x),
                units.format_length(size.y)
            )),
            Ok(peet_platform::SaveOutcome::Cancelled) => {}
            Err(e) => self.error(e),
        }
    }

    /// Adds reference geometry built from the selection.
    fn add_reference(&mut self, cmd: CommandId) {
        let geom = self.selected_geom.first().copied();
        let picked = geom.and_then(|g| self.picked(Some(g), None));
        let top = PlaneRef::Standard(StdPlane::Top);
        let mut pick = None;
        let kind = match cmd {
            CommandId::RefPlane => {
                let from = match self.selected_plane() {
                    Some((p, _)) => p,
                    None => {
                        pick = Some(Slot::PlaneFirst);
                        top
                    }
                };
                FeatureKind::Plane(PlaneDef::Offset {
                    from,
                    distance: Scalar::new(10.0),
                    flip: false,
                })
            }
            CommandId::RefAxis => match picked {
                Some(Picked::Edge(e)) => FeatureKind::Axis(AxisDef::Edge(e)),
                Some(Picked::Face {
                    face, round: true, ..
                }) => FeatureKind::Axis(AxisDef::Cylinder(face)),
                _ => {
                    pick = Some(Slot::AxisFirst);
                    FeatureKind::Axis(AxisDef::TwoPlanes(top, PlaneRef::Standard(StdPlane::Front)))
                }
            },
            CommandId::RefPoint => match picked {
                Some(Picked::Vertex(v)) => FeatureKind::Point(PointDef::Vertex(v)),
                _ => FeatureKind::Point(PointDef::Coordinates {
                    x: Scalar::new(0.0),
                    y: Scalar::new(0.0),
                    z: Scalar::new(0.0),
                }),
            },
            _ => {
                let origin = match picked {
                    Some(Picked::Vertex(v)) => PointRef::Vertex(v),
                    _ => PointRef::Origin,
                };
                FeatureKind::CoordSystem(CoordSystemDef {
                    origin,
                    orientation: top,
                })
            }
        };
        let label = format!("Add {}", kind.type_name());
        let mut id = None;
        self.change(&label, |m| id = Some(m.add(kind)));
        if let Some(id) = id {
            self.selected = Some(ItemId::Feature(id));
            self.selected_geom.clear();
            self.picking = pick.map(|s| (id, s));
        }
    }

    // ---- Files ----

    /// Runs `then` now, or after asking whether to discard unsaved changes.
    fn guard_unsaved(&mut self, then: AfterDiscard) {
        if self.doc.is_modified() {
            self.files.confirm = Some(then);
        } else {
            self.after_discard(then);
        }
    }

    fn after_discard(&mut self, then: AfterDiscard) {
        match then {
            AfterDiscard::New => {
                self.perform(Op::New { discard: true });
            }
            AfterDiscard::Open => {
                self.files.opening = Some(peet_platform::open_file(files::FILTER));
            }
            AfterDiscard::Sample => {
                self.perform(Op::OpenSample {
                    sample: peet_ops::Sample::Bracket,
                    discard: true,
                });
                self.info("Opened the sample bracket. Try changing Sketch1's width (d1), or drag the rollback bar.");
            }
            AfterDiscard::SampleEnclosure => {
                self.perform(Op::OpenSample {
                    sample: peet_ops::Sample::Enclosure,
                    discard: true,
                });
                self.info("Opened the sample enclosure panel. Press U for its flat pattern, or change the thickness and flange parameters (Tools > Parameters).");
            }
            AfterDiscard::SampleChassis => {
                self.perform(Op::OpenSample {
                    sample: peet_ops::Sample::Chassis,
                    discard: true,
                });
                self.info("Opened the sample chassis. Press U for its flat pattern; Sheet Metal > Check runs the manufacturing checks.");
            }
            AfterDiscard::SampleHousing => {
                self.perform(Op::OpenSample {
                    sample: peet_ops::Sample::Housing,
                    discard: true,
                });
                self.info("Opened the sample housing: a revolve with a fillet, chamfers and a bolt circle of counterbored holes. Mass on the Model tab weighs it.");
            }
            AfterDiscard::Quit => {
                self.files.discard_autosave();
                self.doc.mark_saved(None);
                self.quit_requested = true;
            }
        }
    }

    fn save(&mut self, save_as: bool) {
        self.close_sketch();
        let bytes = match self.doc.save_bytes(self.settings.save_caches) {
            Ok(b) => b,
            Err(e) => return self.error(format!("Couldn't save: {e}")),
        };
        let path = self.doc.file.as_ref().and_then(|f| f.path.clone());
        let result = match path {
            Some(path) if !save_as => peet_platform::write_file(&path, &bytes).map(|()| {
                Some(FileLocation {
                    name: self.doc.title(),
                    path: Some(path),
                })
            }),
            _ => peet_platform::save_file_as(&files::file_name(&self.doc), files::FILTER, &bytes)
                .map(|saved| {
                    saved.map(|s| FileLocation {
                        name: s.name,
                        path: s.path,
                    })
                }),
        };
        match result {
            Ok(Some(location)) => {
                let shown = location
                    .path
                    .as_ref()
                    .map_or_else(|| location.name.clone(), |p| p.display().to_string());
                self.doc.mark_saved(Some(location));
                self.files.discard_autosave();
                self.info(format!("Saved {shown} ({} KB)", bytes.len().div_ceil(1024)));
            }
            Ok(None) => {}
            Err(e) => self.error(e),
        }
    }

    fn poll_files(&mut self, ctx: &egui::Context) {
        self.files.poll();
        if let Some(p) = &self.files.opening
            && let Some(result) = p.take()
        {
            self.files.opening = None;
            match result {
                Ok(Some(file)) => {
                    let name = file.name.clone();
                    let reply = self.perform(Op::Open {
                        file: peet_ops::Source::loaded(file.name, file.path, file.bytes),
                        discard: true,
                    });
                    if reply.ok {
                        match reply.json["warnings"][0].as_str() {
                            Some(w) => self.error(w.to_owned()),
                            None => self.info(format!("Opened {name}")),
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => self.error(e),
            }
        }
        if self.files.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.files.autosave(&self.doc, false);
    }

    // ---- Commands ----

    fn command_state(&self, cmd: CommandId) -> CommandState {
        let has_viewport = self.viewport.is_some();
        let on = |checked: bool| CommandState {
            enabled: true,
            checked: Some(checked),
        };
        let enabled = |enabled: bool| CommandState {
            enabled,
            checked: None,
        };
        if let (Some(editor), Some(work)) = (&self.sketch, &self.sketch_work)
            && sketch_ui::is_sketch_command(cmd)
        {
            let checked = match cmd {
                CommandId::ToggleConstruction => Some(editor.options.construction),
                CommandId::ToggleRelations => Some(editor.options.show_relations),
                _ => sketch_ui::tool_for_command(cmd).map(|t| t == editor.tool),
            };
            return CommandState {
                enabled: editor.command_enabled(cmd, &work.sketch),
                checked,
            };
        }
        let in_sketch = self.sketch.is_some();
        match cmd {
            CommandId::Undo => enabled(self.doc.can_undo()),
            CommandId::Redo => enabled(self.doc.can_redo()),
            CommandId::NewSketch => enabled(!in_sketch),
            CommandId::Extrude | CommandId::CutExtrude => enabled(self.extrude_source().is_some()),
            CommandId::ExportStl => enabled(!self.doc.bodies.is_empty()),
            CommandId::BaseFlange => enabled(self.extrude_source().is_some()),
            CommandId::EdgeFlange => enabled(!in_sketch && self.doc.has_sheet_metal()),
            CommandId::SheetCut => {
                enabled(self.extrude_source().is_some() && self.doc.has_sheet_metal())
            }
            CommandId::FlatPattern => CommandState {
                enabled: !in_sketch && self.doc.has_sheet_metal(),
                checked: Some(self.doc.is_flat()),
            },
            CommandId::BendTable | CommandId::ExportDxf | CommandId::SheetChecks => {
                enabled(self.doc.has_sheet_metal())
            }
            CommandId::Hem | CommandId::CornerTreatment => {
                enabled(!in_sketch && self.doc.has_sheet_metal())
            }
            CommandId::ConvertToSheet => enabled(!in_sketch && self.doc.has_plain_solid()),
            CommandId::SketchedBend
            | CommandId::Jog
            | CommandId::MiterFlange
            | CommandId::Dimple
            | CommandId::Emboss
            | CommandId::Louver => {
                enabled(self.extrude_source().is_some() && self.doc.has_sheet_metal())
            }
            CommandId::LinearPattern | CommandId::CircularPattern | CommandId::MirrorFeature => {
                enabled(!in_sketch && self.copy_source().is_some())
            }
            CommandId::GaugeTables
            | CommandId::ImportDxf
            | CommandId::ImportStep
            | CommandId::MassProperties => enabled(true),
            CommandId::Revolve
            | CommandId::CutRevolve
            | CommandId::Sweep
            | CommandId::CutSweep
            | CommandId::Loft
            | CommandId::CutLoft => enabled(self.extrude_source().is_some()),
            CommandId::Hole => {
                enabled(self.extrude_source().is_some() && !self.doc.bodies.is_empty())
            }
            CommandId::Fillet | CommandId::Chamfer | CommandId::Shell | CommandId::Draft => {
                enabled(!in_sketch && !self.doc.bodies.is_empty())
            }
            CommandId::ExportStep => enabled(!self.doc.bodies.is_empty()),
            CommandId::EditSketch => enabled(!in_sketch && self.selected_sketch().is_some()),
            CommandId::ExitSketch => enabled(in_sketch),
            CommandId::Parameters => enabled(true),
            CommandId::DeleteSelection => enabled(!in_sketch && self.selected_feature().is_some()),
            CommandId::ToggleSuppress => CommandState {
                enabled: !in_sketch && self.selected_feature().is_some(),
                checked: Some(
                    self.selected_feature()
                        .and_then(|id| self.doc.feature(id))
                        .is_some_and(|f| f.suppressed),
                ),
            },
            CommandId::RollToEnd => enabled(!in_sketch && self.doc.model.is_rolled_back()),
            CommandId::RefPlane
            | CommandId::RefAxis
            | CommandId::RefPoint
            | CommandId::RefCoordSystem => enabled(!in_sketch),
            CommandId::NewDocument
            | CommandId::OpenDocument
            | CommandId::OpenSample
            | CommandId::OpenSampleEnclosure
            | CommandId::OpenSampleChassis
            | CommandId::OpenSampleHousing
            | CommandId::SaveDocument
            | CommandId::SaveDocumentAs => enabled(true),
            CommandId::ViewIsometric
            | CommandId::ViewFront
            | CommandId::ViewBack
            | CommandId::ViewLeft
            | CommandId::ViewRight
            | CommandId::ViewTop
            | CommandId::ViewBottom
            | CommandId::ZoomToFit => enabled(has_viewport),
            CommandId::ToggleProjection => on(self.settings.perspective),
            CommandId::ToggleGrid => on(self.settings.show_grid),
            CommandId::ToggleReferencePlanes => on(self.doc.planes_visible()),
            CommandId::ToggleViewCube => on(self.settings.show_view_cube),
            CommandId::ToggleFeatureTree => on(self.settings.show_feature_tree),
            CommandId::ToggleProperties => on(self.settings.show_properties),
            CommandId::TogglePerfOverlay => on(self.settings.show_perf_overlay),
            CommandId::CommandPalette
            | CommandId::Settings
            | CommandId::KeyboardShortcuts
            | CommandId::About
            | CommandId::Quit => enabled(true),
            // Sketch commands outside sketch mode.
            _ => enabled(false),
        }
    }

    fn execute(&mut self, ctx: &egui::Context, cmd: CommandId) {
        if !self.command_state(cmd).enabled {
            return;
        }
        log::debug!("Command: {cmd:?}");
        let animate = self.settings.animate_views;
        let view = |vp: &mut Option<Viewport>, v: StandardView| {
            if let Some(vp) = vp {
                vp.set_view(v, animate);
            }
        };
        if sketch_ui::is_sketch_command(cmd)
            && let (Some(editor), Some(work)) = (&mut self.sketch, &mut self.sketch_work)
        {
            editor.command(cmd, work);
            ctx.request_repaint();
            return;
        }
        match cmd {
            CommandId::Undo => self.undo_redo(false),
            CommandId::Redo => self.undo_redo(true),
            CommandId::Extrude => self.start_extrude(peet_model::Operation::Add),
            CommandId::CutExtrude => {
                // A cut sketched on sheet metal is a sheet metal cut: it keeps the flat pattern.
                if self
                    .extrude_source()
                    .is_some_and(|s| self.sketch_on_sheet(s))
                {
                    self.start_sheet_cut();
                } else {
                    self.start_extrude(peet_model::Operation::Cut);
                }
            }
            CommandId::ExportStl => self.export_stl(),
            CommandId::BaseFlange => self.start_base_flange(),
            CommandId::EdgeFlange => self.start_edge_flange(),
            CommandId::SheetCut => self.start_sheet_cut(),
            CommandId::FlatPattern => self.toggle_flat(),
            CommandId::BendTable => self.windows.bend_table = true,
            CommandId::ExportDxf => self.export_dxf(),
            CommandId::Hem => self.start_hem(),
            CommandId::SketchedBend => {
                self.start_from_sheet_sketch("Sketched Bend", |m, s| m.add_sketched_bend(s));
            }
            CommandId::Jog => self.start_from_sheet_sketch("Jog", |m, s| m.add_jog(s)),
            CommandId::MiterFlange => self.start_miter_flange(),
            CommandId::CornerTreatment => self.start_corner(),
            CommandId::ConvertToSheet => self.start_convert_to_sheet(),
            CommandId::Dimple => self.start_from_sheet_sketch("Dimple", |m, s| {
                m.add_form(s, peet_sheetmetal::FormKind::Dimple)
            }),
            CommandId::Emboss => self.start_from_sheet_sketch("Emboss", |m, s| {
                m.add_form(s, peet_sheetmetal::FormKind::Emboss)
            }),
            CommandId::Louver => self.start_from_sheet_sketch("Louver", |m, s| {
                m.add_form(s, peet_sheetmetal::FormKind::Louver)
            }),
            CommandId::LinearPattern => self.start_copy(CopyKind::Linear),
            CommandId::CircularPattern => self.start_copy(CopyKind::Circular),
            CommandId::MirrorFeature => self.start_copy(CopyKind::Mirror),
            CommandId::SheetChecks => self.windows.sheet_checks = true,
            CommandId::GaugeTables => self.windows.gauges = true,
            CommandId::ExportStep => self.export_step(),
            CommandId::ImportDxf => {
                self.dxf_import = Some(peet_platform::open_file(("DXF drawing", &["dxf"])));
            }
            CommandId::ImportStep => {
                self.step_import = Some(peet_platform::open_file(("STEP file", &["step", "stp"])));
            }
            CommandId::Revolve => self.start_revolve(peet_model::Operation::Add),
            CommandId::CutRevolve => self.start_revolve(peet_model::Operation::Cut),
            CommandId::Sweep => self.start_sweep(peet_model::Operation::Add),
            CommandId::CutSweep => self.start_sweep(peet_model::Operation::Cut),
            CommandId::Loft => self.start_loft(peet_model::Operation::Add),
            CommandId::CutLoft => self.start_loft(peet_model::Operation::Cut),
            CommandId::Fillet => self.start_blend(peet_model::BlendKind::Fillet),
            CommandId::Chamfer => self.start_blend(peet_model::BlendKind::Chamfer),
            CommandId::Shell => self.start_shell(),
            CommandId::Draft => self.start_draft(),
            CommandId::Hole => self.start_hole(),
            CommandId::MassProperties => self.windows.mass_properties = true,
            CommandId::NewSketch => match self.selected_plane() {
                Some((plane, placement)) => self.new_sketch(plane, placement),
                None => {
                    self.close_sketch();
                    self.picking = None;
                    self.picking_sketch_plane = true;
                    self.selected = None;
                    self.selected_geom.clear();
                    self.status_message = None;
                }
            },
            CommandId::EditSketch => {
                if let Some(id) = self.selected_sketch() {
                    self.open_sketch(id);
                }
            }
            CommandId::ExitSketch => self.close_sketch(),
            CommandId::Parameters => self.windows.parameters = true,
            CommandId::DeleteSelection => {
                if let Some(id) = self.selected_feature() {
                    let name = self.doc.model.name_of(id).to_owned();
                    self.change(&format!("Delete {name}"), |m| {
                        m.remove(id);
                    });
                    self.selected = None;
                }
            }
            CommandId::ToggleSuppress => {
                if let Some(id) = self.selected_feature() {
                    self.set_suppressed(id, None);
                }
            }
            CommandId::RollToEnd => {
                self.change("Roll to End", |m| m.set_rollback(None));
            }
            CommandId::RefPlane
            | CommandId::RefAxis
            | CommandId::RefPoint
            | CommandId::RefCoordSystem => self.add_reference(cmd),
            CommandId::NewDocument => self.guard_unsaved(AfterDiscard::New),
            CommandId::OpenDocument => self.guard_unsaved(AfterDiscard::Open),
            CommandId::OpenSample => self.guard_unsaved(AfterDiscard::Sample),
            CommandId::OpenSampleEnclosure => self.guard_unsaved(AfterDiscard::SampleEnclosure),
            CommandId::OpenSampleChassis => self.guard_unsaved(AfterDiscard::SampleChassis),
            CommandId::OpenSampleHousing => self.guard_unsaved(AfterDiscard::SampleHousing),
            CommandId::SaveDocument => self.save(false),
            CommandId::SaveDocumentAs => self.save(true),
            CommandId::CommandPalette => self.palette.toggle(),
            CommandId::ViewIsometric => view(&mut self.viewport, StandardView::Isometric),
            CommandId::ViewFront => view(&mut self.viewport, StandardView::Front),
            CommandId::ViewBack => view(&mut self.viewport, StandardView::Back),
            CommandId::ViewLeft => view(&mut self.viewport, StandardView::Left),
            CommandId::ViewRight => view(&mut self.viewport, StandardView::Right),
            CommandId::ViewTop => view(&mut self.viewport, StandardView::Top),
            CommandId::ViewBottom => view(&mut self.viewport, StandardView::Bottom),
            CommandId::ZoomToFit => {
                let bounds = self.doc.visible_bounds();
                if let Some(vp) = &mut self.viewport {
                    vp.zoom_to_fit(&bounds, animate);
                }
            }
            CommandId::ToggleProjection => {
                self.settings.perspective = !self.settings.perspective;
                if let Some(vp) = &mut self.viewport {
                    vp.set_projection(if self.settings.perspective {
                        Projection::Perspective
                    } else {
                        Projection::Orthographic
                    });
                }
            }
            CommandId::ToggleGrid => self.settings.show_grid ^= true,
            CommandId::ToggleReferencePlanes => {
                let visible = !self.doc.planes_visible();
                let label = if visible {
                    "Show Planes"
                } else {
                    "Hide Planes"
                };
                self.change(label, |m| {
                    for p in StdPlane::ALL {
                        m.set_datum_visible(Datum::Plane(p), visible);
                    }
                });
            }
            CommandId::ToggleViewCube => self.settings.show_view_cube ^= true,
            CommandId::ToggleFeatureTree => self.settings.show_feature_tree ^= true,
            CommandId::ToggleProperties => self.settings.show_properties ^= true,
            CommandId::TogglePerfOverlay => self.settings.show_perf_overlay ^= true,
            CommandId::Settings => self.windows.settings = true,
            CommandId::KeyboardShortcuts => self.windows.shortcuts = true,
            CommandId::About => self.windows.about = true,
            CommandId::Quit => self.guard_unsaved(AfterDiscard::Quit),
            // Sketch commands outside sketch mode.
            _ => {}
        }
        ctx.request_repaint();
    }

    fn set_suppressed(&mut self, id: FeatureId, suppressed: Option<bool>) {
        let Some(f) = self.doc.feature(id) else {
            return;
        };
        let value = suppressed.unwrap_or(!f.suppressed);
        let label = format!(
            "{} {}",
            if value { "Suppress" } else { "Unsuppress" },
            f.name
        );
        self.change(&label, |m| {
            if let Some(f) = m.feature_mut(id) {
                f.suppressed = value;
            }
        });
    }

    fn apply_tree(&mut self, actions: Vec<TreeAction>) {
        for action in actions {
            match action {
                TreeAction::Select(item) => {
                    if self.picking_sketch_plane
                        && let Some(item) = item
                        && self.doc.plane_ref_of_item(item).is_some()
                    {
                        self.finish_sketch_plane_pick(None, Some(item));
                        continue;
                    }
                    // While picking an axis or a path, clicking a reference axis or a
                    // sketch in the tree picks it.
                    if let Some((_, slot)) = self.picking
                        && let Some(ItemId::Feature(id)) = item
                    {
                        let picked = match (slot, self.doc.feature(id).map(|f| &f.kind)) {
                            (Slot::RevolveAxis, Some(FeatureKind::Axis(_))) => {
                                Some(Picked::Axis(peet_model::AxisRef::Feature(id)))
                            }
                            (Slot::SweepPath | Slot::LoftProfile, Some(FeatureKind::Sketch(_))) => {
                                Some(Picked::Sketch(id))
                            }
                            _ => None,
                        };
                        if picked.is_some() {
                            self.finish_pick(picked);
                            continue;
                        }
                    }
                    // While picking, clicking a plane in the tree picks it.
                    if self.picking.is_some()
                        && let Some(item) = item
                        && self.doc.plane_ref_of_item(item).is_some()
                    {
                        let picked = self.picked(None, Some(item));
                        self.finish_pick(picked);
                        continue;
                    }
                    self.selected = item;
                    self.selected_geom.clear();
                    self.picking = None;
                }
                TreeAction::Edit(item) => match item.feature() {
                    Some(id) if self.doc.model.sketch(id).is_some() => self.open_sketch(id),
                    _ => {
                        self.selected = Some(item);
                        self.settings.show_properties = true;
                    }
                },
                TreeAction::SetVisible(item, visible) => {
                    let name = self.doc.item_name(item);
                    let label = format!("{} {name}", if visible { "Show" } else { "Hide" });
                    self.change(&label, |m| match item {
                        ItemId::Datum(d) => m.set_datum_visible(d, visible),
                        ItemId::Feature(id) => {
                            if let Some(f) = m.feature_mut(id) {
                                f.visible = visible;
                            }
                        }
                    });
                }
                TreeAction::SetSuppressed(id, s) => self.set_suppressed(id, Some(s)),
                TreeAction::Delete(id) => {
                    self.selected = Some(ItemId::Feature(id));
                    self.execute_delete(id);
                }
                TreeAction::Rollback(at) => {
                    let label = if at.is_some() {
                        "Roll Back"
                    } else {
                        "Roll to End"
                    };
                    self.change(label, |m| m.set_rollback(at));
                }
                TreeAction::Move(id, to) => {
                    let name = self.doc.model.name_of(id).to_owned();
                    let mut result = Ok(());
                    self.change(&format!("Move {name}"), |m| result = m.move_to(id, to));
                    if let Err(e) = result {
                        self.error(e);
                    }
                }
            }
        }
    }

    fn execute_delete(&mut self, id: FeatureId) {
        let name = self.doc.model.name_of(id).to_owned();
        self.change(&format!("Delete {name}"), |m| {
            m.remove(id);
        });
        self.selected = None;
    }

    fn export_stl(&mut self) {
        let mut triangles = Vec::new();
        for body in &self.doc.bodies {
            for f in &body.tess().faces {
                for t in &f.triangles {
                    triangles.push(t.map(|i| f.positions[i as usize]));
                }
            }
        }
        if triangles.is_empty() {
            self.error("There are no bodies to export.");
            return;
        }
        let title = self.doc.title();
        let stem = title.strip_suffix(".peet").unwrap_or(&title).to_owned();
        let bytes = peet_io::stl::write_binary(&stem, &triangles);
        match peet_platform::save_file(&format!("{stem}.stl"), ("STL mesh", &["stl"]), &bytes) {
            Ok(peet_platform::SaveOutcome::Saved(to)) => {
                self.info(format!("Exported {} triangles to {to}", triangles.len()));
            }
            Ok(peet_platform::SaveOutcome::Cancelled) => {}
            Err(e) => self.error(e),
        }
    }

    /// Commands triggered by keyboard shortcuts this frame.
    fn shortcut_commands(&self, ctx: &egui::Context) -> Vec<CommandId> {
        // While typing in a text field, only the palette shortcut stays active.
        let typing = ctx.egui_wants_keyboard_input();
        CommandId::ALL
            .into_iter()
            .filter(|c| c.available())
            .filter(|c| !typing || *c == CommandId::CommandPalette)
            .filter_map(|cmd| {
                let shortcut = cmd.info().shortcut?;
                if !self.command_state(cmd).enabled {
                    return None;
                }
                ctx.input_mut(|i| i.consume_shortcut(&shortcut))
                    .then_some(cmd)
            })
            .collect()
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied_theme != Some(self.settings.theme) {
            // egui already follows the system theme by default. Skipping the initial call
            // avoids an "unhandled viewport command" warning from the web backend.
            let initial_system =
                self.applied_theme.is_none() && self.settings.theme == ThemeChoice::System;
            if !initial_system {
                ctx.set_theme(self.settings.theme.preference());
            }
            self.applied_theme = Some(self.settings.theme);
        }
    }

    /// Keeps the window title in step with the document name and modified state.
    fn update_title(&mut self, ctx: &egui::Context) {
        let title = format!(
            "{}{} - {APP_NAME}",
            self.doc.title(),
            if self.doc.is_modified() { " *" } else { "" }
        );
        if title != self.title {
            if !peet_platform::is_web() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            }
            self.title = title;
        }
    }

    // ---- Panels ----

    fn toolbar(&self, ui: &mut Ui, pending: &mut Vec<CommandId>, tab: &mut RibbonTab) {
        use ribbon::Size::{Icon as IconOnly, Large, Small};
        let tool =
            |ui: &mut Ui, pending: &mut Vec<CommandId>, cmd: CommandId, label: &str, size| {
                if cmd.available() && ribbon::command(ui, cmd, label, self.command_state(cmd), size)
                {
                    pending.push(cmd);
                }
            };
        let menu = |ui: &mut Ui, pending: &mut Vec<CommandId>, cmds: &[CommandId]| {
            for &cmd in cmds {
                if menu_button(ui, cmd, self.command_state(cmd)) {
                    pending.push(cmd);
                    ui.close();
                }
            }
        };
        let sketching = self.sketch.is_some();
        if *tab == RibbonTab::Sketch && !sketching {
            *tab = RibbonTab::Model;
        }

        // Quick-access buttons, the tabs, and the command search.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            tool(ui, pending, CommandId::SaveDocument, "Save", IconOnly);
            // In sketch mode undo works on the sketch, whose steps have no labels here.
            let history = |label: Option<&str>, verb: &str| match label {
                Some(l) if !sketching => format!("{verb} {l}"),
                _ => verb.to_owned(),
            };
            let undo = history(self.doc.undo_label(), "Undo");
            let redo = history(self.doc.redo_label(), "Redo");
            tool(ui, pending, CommandId::Undo, &undo, IconOnly);
            tool(ui, pending, CommandId::Redo, &redo, IconOnly);
            ui.add_space(10.0);
            let mut tabs = vec![
                ribbon::Tab {
                    id: RibbonTab::File,
                    label: "File",
                    contextual: None,
                },
                ribbon::Tab {
                    id: RibbonTab::Model,
                    label: "Model",
                    contextual: None,
                },
            ];
            if sketching {
                tabs.push(ribbon::Tab {
                    id: RibbonTab::Sketch,
                    label: "Sketch",
                    contextual: Some(icons::Category::Sketch),
                });
            }
            tabs.extend([
                ribbon::Tab {
                    id: RibbonTab::SheetMetal,
                    label: "Sheet Metal",
                    contextual: None,
                },
                ribbon::Tab {
                    id: RibbonTab::View,
                    label: "View",
                    contextual: None,
                },
                ribbon::Tab {
                    id: RibbonTab::Help,
                    label: "Help",
                    contextual: None,
                },
            ]);
            ribbon::tab_bar(ui, &tabs, tab);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let palette = CommandId::CommandPalette;
                let label = format!(
                    "Search commands  {}",
                    ui.ctx()
                        .format_shortcut(&palette.info().shortcut.expect("has shortcut"))
                );
                tool(ui, pending, palette, &label, Small);
            });
        });

        // The current tab's commands.
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    match *tab {
                        RibbonTab::File => {
                            ribbon::group(ui, "Document", |ui| {
                                tool(ui, pending, CommandId::NewDocument, "New", Large);
                                tool(ui, pending, CommandId::OpenDocument, "Open", Large);
                                tool(ui, pending, CommandId::SaveDocument, "Save", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::SaveDocumentAs, "Save As", Small);
                                });
                            });
                            ribbon::group(ui, "Samples", |ui| {
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::OpenSample, "Bracket", Small);
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::OpenSampleEnclosure,
                                        "Enclosure Panel",
                                        Small,
                                    );
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::OpenSampleChassis,
                                        "Chassis",
                                        Small,
                                    );
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::OpenSampleHousing,
                                        "Housing",
                                        Small,
                                    );
                                });
                            });
                            ribbon::group(ui, "Import", |ui| {
                                tool(ui, pending, CommandId::ImportStep, "STEP", Large);
                                tool(ui, pending, CommandId::ImportDxf, "DXF", Large);
                            });
                            ribbon::group(ui, "Export", |ui| {
                                tool(ui, pending, CommandId::ExportStep, "STEP", Large);
                                tool(ui, pending, CommandId::ExportStl, "STL", Large);
                                tool(ui, pending, CommandId::ExportDxf, "DXF", Large);
                            });
                            ribbon::group(ui, "Application", |ui| {
                                tool(ui, pending, CommandId::Settings, "Settings", Large);
                                tool(ui, pending, CommandId::Quit, "Quit", Large);
                            });
                        }
                        RibbonTab::Model => {
                            ribbon::group(ui, "Sketch", |ui| {
                                tool(ui, pending, CommandId::NewSketch, "New Sketch", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::EditSketch, "Edit Sketch", Small);
                                    tool(ui, pending, CommandId::Parameters, "Parameters", Small);
                                });
                            });
                            ribbon::group(ui, "Features", |ui| {
                                tool(ui, pending, CommandId::Extrude, "Extrude", Large);
                                tool(ui, pending, CommandId::CutExtrude, "Cut", Large);
                                tool(ui, pending, CommandId::Revolve, "Revolve", Large);
                                tool(ui, pending, CommandId::Hole, "Hole", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::CutRevolve, "Cut-Revolve", Small);
                                    tool(ui, pending, CommandId::Sweep, "Sweep", Small);
                                    tool(ui, pending, CommandId::CutSweep, "Cut-Sweep", Small);
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::Loft, "Loft", Small);
                                    tool(ui, pending, CommandId::CutLoft, "Cut-Loft", Small);
                                });
                                ribbon::dropdown(
                                    ui,
                                    Icon::Reference,
                                    "Reference",
                                    "Add a reference plane, axis, point or coordinate system.",
                                    Large,
                                    |ui| menu(ui, pending, &REFERENCES),
                                );
                            });
                            ribbon::group(ui, "Modify", |ui| {
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::Fillet, "Fillet", Small);
                                    tool(ui, pending, CommandId::Chamfer, "Chamfer", Small);
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::Shell, "Shell", Small);
                                    tool(ui, pending, CommandId::Draft, "Draft", Small);
                                });
                            });
                            ribbon::group(ui, "Copy", |ui| {
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::LinearPattern,
                                        "Linear Pattern",
                                        Small,
                                    );
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::CircularPattern,
                                        "Circular Pattern",
                                        Small,
                                    );
                                    tool(ui, pending, CommandId::MirrorFeature, "Mirror", Small);
                                });
                            });
                            ribbon::group(ui, "History", |ui| {
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::DeleteSelection, "Delete", Small);
                                    tool(ui, pending, CommandId::ToggleSuppress, "Suppress", Small);
                                    tool(ui, pending, CommandId::RollToEnd, "Roll to End", Small);
                                });
                            });
                            ribbon::group(ui, "Evaluate", |ui| {
                                tool(ui, pending, CommandId::MassProperties, "Mass", Large);
                            });
                            ribbon::group(ui, "View", |ui| {
                                tool(ui, pending, CommandId::ZoomToFit, "Fit", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::ViewIsometric, "Iso", Small);
                                    tool(ui, pending, CommandId::ViewFront, "Front", Small);
                                    tool(ui, pending, CommandId::ViewTop, "Top", Small);
                                });
                            });
                        }
                        RibbonTab::Sketch => {
                            ribbon::group(ui, "Sketch", |ui| {
                                tool(ui, pending, CommandId::ExitSketch, "Exit Sketch", Large);
                            });
                            ribbon::group(ui, "Features", |ui| {
                                tool(ui, pending, CommandId::Extrude, "Extrude", Large);
                                tool(ui, pending, CommandId::CutExtrude, "Cut", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::Revolve, "Revolve", Small);
                                    tool(ui, pending, CommandId::Sweep, "Sweep", Small);
                                    tool(ui, pending, CommandId::Hole, "Hole", Small);
                                });
                                tool(ui, pending, CommandId::BaseFlange, "Base Flange", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::SheetCut, "Sheet Cut", Small);
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::SketchedBend,
                                        "Sketched Bend",
                                        Small,
                                    );
                                    tool(ui, pending, CommandId::Jog, "Jog", Small);
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::MiterFlange,
                                        "Miter Flange",
                                        Small,
                                    );
                                    tool(ui, pending, CommandId::Dimple, "Dimple", Small);
                                    tool(ui, pending, CommandId::Louver, "Louver", Small);
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::ImportDxf, "Import DXF", Small);
                                });
                            });
                            ribbon::group(ui, "Draw", |ui| {
                                tool(ui, pending, CommandId::SketchSelect, "Select", Large);
                                tool(ui, pending, CommandId::SketchLine, "Line", Large);
                                tool(ui, pending, CommandId::SketchRectangle, "Rectangle", Large);
                                tool(ui, pending, CommandId::SketchCircle, "Circle", Large);
                                tool(ui, pending, CommandId::SketchArc, "Arc", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::SketchCenterRectangle,
                                        "Center Rect",
                                        Small,
                                    );
                                    tool(ui, pending, CommandId::SketchSlot, "Slot", Small);
                                    tool(ui, pending, CommandId::SketchPolygon, "Polygon", Small);
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::SketchPoint, "Point", Small);
                                });
                            });
                            ribbon::group(ui, "Constrain", |ui| {
                                tool(ui, pending, CommandId::SmartDimension, "Dimension", Large);
                                ribbon::dropdown(
                                    ui,
                                    Icon::Relations,
                                    "Relations",
                                    "Add a geometric relation between the selected sketch items.",
                                    Large,
                                    |ui| menu(ui, pending, &RELATIONS),
                                );
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::ToggleConstruction,
                                        "Construction",
                                        Small,
                                    );
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::ToggleRelations,
                                        "Show Relations",
                                        Small,
                                    );
                                });
                            });
                            ribbon::group(ui, "Modify", |ui| {
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::SketchTrim, "Trim", Small);
                                    tool(ui, pending, CommandId::SketchExtend, "Extend", Small);
                                    tool(ui, pending, CommandId::SketchFillet, "Fillet", Small);
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::SketchOffset, "Offset", Small);
                                    tool(ui, pending, CommandId::SketchMirror, "Mirror", Small);
                                    tool(ui, pending, CommandId::ZoomToFit, "Fit", Small);
                                });
                            });
                        }
                        RibbonTab::SheetMetal => {
                            ribbon::group(ui, "Flanges", |ui| {
                                tool(ui, pending, CommandId::BaseFlange, "Base Flange", Large);
                                tool(ui, pending, CommandId::EdgeFlange, "Edge Flange", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::MiterFlange,
                                        "Miter Flange",
                                        Small,
                                    );
                                    tool(ui, pending, CommandId::Hem, "Hem", Small);
                                    tool(ui, pending, CommandId::CornerTreatment, "Corner", Small);
                                });
                            });
                            ribbon::group(ui, "Bends", |ui| {
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::SketchedBend,
                                        "Sketched Bend",
                                        Small,
                                    );
                                    tool(ui, pending, CommandId::Jog, "Jog", Small);
                                });
                            });
                            ribbon::group(ui, "Cut and Form", |ui| {
                                tool(ui, pending, CommandId::SheetCut, "Sheet Cut", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::Dimple, "Dimple", Small);
                                    tool(ui, pending, CommandId::Emboss, "Emboss", Small);
                                    tool(ui, pending, CommandId::Louver, "Louver", Small);
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::LinearPattern,
                                        "Linear Pattern",
                                        Small,
                                    );
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::CircularPattern,
                                        "Circular Pattern",
                                        Small,
                                    );
                                    tool(ui, pending, CommandId::MirrorFeature, "Mirror", Small);
                                });
                            });
                            ribbon::group(ui, "Flat Pattern", |ui| {
                                tool(ui, pending, CommandId::ConvertToSheet, "Convert", Large);
                                tool(ui, pending, CommandId::FlatPattern, "Flatten", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::BendTable, "Bend Table", Small);
                                    tool(ui, pending, CommandId::SheetChecks, "Check", Small);
                                    tool(ui, pending, CommandId::GaugeTables, "Materials", Small);
                                });
                            });
                            ribbon::group(ui, "Export", |ui| {
                                tool(ui, pending, CommandId::ExportDxf, "DXF", Large);
                                tool(ui, pending, CommandId::ExportStep, "STEP", Large);
                            });
                        }
                        RibbonTab::View => {
                            ribbon::group(ui, "Orientation", |ui| {
                                tool(ui, pending, CommandId::ZoomToFit, "Fit", Large);
                                tool(ui, pending, CommandId::ViewIsometric, "Isometric", Large);
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::ViewFront, "Front", Small);
                                    tool(ui, pending, CommandId::ViewTop, "Top", Small);
                                    tool(ui, pending, CommandId::ViewRight, "Right", Small);
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(ui, pending, CommandId::ViewBack, "Back", Small);
                                    tool(ui, pending, CommandId::ViewBottom, "Bottom", Small);
                                    tool(ui, pending, CommandId::ViewLeft, "Left", Small);
                                });
                            });
                            ribbon::group(ui, "Display", |ui| {
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::ToggleProjection,
                                        "Perspective",
                                        Small,
                                    );
                                    tool(ui, pending, CommandId::ToggleGrid, "Grid", Small);
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::ToggleReferencePlanes,
                                        "Planes",
                                        Small,
                                    );
                                });
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::ToggleViewCube,
                                        "View Cube",
                                        Small,
                                    );
                                });
                            });
                            ribbon::group(ui, "Panels", |ui| {
                                ribbon::stack(ui, |ui| {
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::ToggleFeatureTree,
                                        "Feature Tree",
                                        Small,
                                    );
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::ToggleProperties,
                                        "Properties",
                                        Small,
                                    );
                                    tool(
                                        ui,
                                        pending,
                                        CommandId::TogglePerfOverlay,
                                        "Performance",
                                        Small,
                                    );
                                });
                            });
                        }
                        RibbonTab::Help => {
                            ribbon::group(ui, "Help", |ui| {
                                tool(ui, pending, CommandId::CommandPalette, "Commands", Large);
                                tool(
                                    ui,
                                    pending,
                                    CommandId::KeyboardShortcuts,
                                    "Shortcuts",
                                    Large,
                                );
                                tool(ui, pending, CommandId::About, "About", Large);
                            });
                        }
                    }
                });
            });
    }

    fn status_bar(&self, ui: &mut Ui) {
        let units = self.doc.model.parameters.units;
        if let Some(editor) = &self.sketch {
            ui.horizontal(|ui| {
                ui.weak(editor.hint());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(units.length.suffix());
                    if let Some(p) = editor.cursor {
                        ui.separator();
                        ui.monospace(format!(
                            "x {:>9}  y {:>9}",
                            units.format_length_value(p.x),
                            units.format_length_value(p.y)
                        ));
                    }
                    ui.separator();
                    ui.weak(format!("solve {:.2} ms", editor.solve_ms));
                    ui.separator();
                    let dark = ui.visuals().dark_mode;
                    let color = match editor.status() {
                        crate::document::SketchStatus::Under => {
                            sketch_ui::dof_color(peet_sketch::solver::DofStatus::Under, dark)
                        }
                        crate::document::SketchStatus::Fully => ui.visuals().text_color(),
                        crate::document::SketchStatus::Over => {
                            sketch_ui::dof_color(peet_sketch::solver::DofStatus::Over, dark)
                        }
                    };
                    ui.colored_label(color, editor.status_text());
                });
            });
            return;
        }
        ui.horizontal(|ui| {
            // A body that can't be displayed is a bug, but must not go unnoticed.
            let display_error = self.doc.bodies.iter().find_map(|b| b.error());
            if self.picking_sketch_plane {
                ui.colored_label(
                    PICKING,
                    "New Sketch: click a flat face or a plane to sketch on. Esc to cancel.",
                );
            } else if let Some((_, slot)) = self.picking {
                ui.colored_label(PICKING, format!("{} Esc to cancel.", slot.prompt()));
            } else if let Some(e) = display_error {
                ui.colored_label(ERROR, format!("A body can't be displayed: {e}"));
            } else if let Some((msg, error)) = &self.status_message {
                if *error {
                    ui.colored_label(ERROR, msg);
                } else {
                    ui.label(msg);
                }
            } else {
                ui.weak(self.settings.mouse_preset.status_hint());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(units.length.suffix());
                ui.separator();
                ui.label(if self.settings.perspective {
                    "Perspective"
                } else {
                    "Orthographic"
                });
                let stats = self.doc.evaluation().stats;
                ui.separator();
                ui.weak(format!(
                    "rebuild {:.1} ms ({} of {})",
                    stats.ms,
                    stats.rebuilt,
                    stats.rebuilt + stats.reused
                ))
                .on_hover_text(
                    "Last rebuild: features recomputed, of those built. The rest were reused.",
                );
                if let Some(p) = self.viewport.as_ref().and_then(|v| v.cursor_on_ground) {
                    ui.separator();
                    ui.monospace(format!(
                        "X {:>9}  Y {:>9}",
                        units.format_length_value(p.x),
                        units.format_length_value(p.y)
                    ));
                }
            });
        });
    }

    fn feature_tree(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong(self.doc.title());
            if self.doc.is_modified() {
                ui.weak("(modified)");
            }
        });
        ui.separator();
        let view = TreeView {
            selected: self.selected,
            editing: self.sketch.as_ref().map(|e| e.item),
        };
        let (actions, hovered) = egui::ScrollArea::vertical()
            .show(ui, |ui| tree_ui(ui, &self.doc, &view))
            .inner;
        if hovered.is_some() {
            self.hovered = hovered;
        }
        if !actions.is_empty() {
            if self.sketch.is_some() {
                // Selecting or editing other features ends sketch editing first.
                let only_selecting = actions
                    .iter()
                    .all(|a| matches!(a, TreeAction::Select(_) | TreeAction::SetVisible(..)));
                if !only_selecting {
                    self.close_sketch();
                }
            }
            self.apply_tree(actions);
        }
    }

    fn properties(&mut self, ui: &mut Ui, pending: &mut Vec<CommandId>) {
        ui.add_space(4.0);
        if let (Some(editor), Some(work)) = (&mut self.sketch, &mut self.sketch_work) {
            let name = editor
                .item
                .feature()
                .map(|id| self.doc.model.name_of(id).to_owned())
                .unwrap_or_default();
            ui.strong(format!("Editing {name}"));
            ui.separator();
            if let Some(cmd) = editor.properties_ui(ui, work, &self.doc.model.parameters) {
                pending.push(cmd);
            }
            return;
        }
        ui.strong("Properties");
        ui.separator();
        if let Some(ItemId::Feature(id)) = self.selected {
            egui::ScrollArea::vertical().show(ui, |ui| self.feature_properties(ui, id, pending));
            return;
        }
        if !self.selected_geom.is_empty() {
            self.geometry_properties(ui);
            return;
        }
        match self.selected {
            Some(ItemId::Datum(d)) => {
                egui::Grid::new("datum_props")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.label(d.label());
                        ui.end_row();
                        match d {
                            Datum::Origin => {
                                ui.label("Position");
                                ui.monospace("0, 0, 0");
                            }
                            Datum::Plane(p) => {
                                let n = p.plane().normal();
                                ui.label("Normal");
                                ui.monospace(format!("{:.0}, {:.0}, {:.0}", n.x, n.y, n.z));
                            }
                        }
                        ui.end_row();
                    });
                if matches!(d, Datum::Plane(_)) {
                    ui.add_space(6.0);
                    ui.weak("Press S to sketch on this plane.");
                }
            }
            _ => {
                ui.weak("Select an item in the feature tree, or a face, edge or vertex in the view, to see its properties.");
            }
        }
    }

    fn feature_properties(&mut self, ui: &mut Ui, id: FeatureId, pending: &mut Vec<CommandId>) {
        let Some(feature) = self.doc.feature(id).cloned() else {
            return;
        };
        let picking = self.picking.filter(|(p, _)| *p == id).map(|(_, s)| s);
        features_ui::status_line(ui, self.doc.status(id));

        // The name, applied when the field loses focus.
        let key = ui.make_persistent_id(("feature_name", id.0));
        let mut name: String = ui
            .data(|m| m.get_temp(key))
            .unwrap_or_else(|| feature.name.clone());
        ui.horizontal(|ui| {
            ui.label("Name");
            let r = ui.text_edit_singleline(&mut name);
            if r.changed() {
                ui.data_mut(|m| m.insert_temp(key, name.clone()));
            }
            if r.lost_focus() {
                ui.data_mut(|m| m.remove::<String>(key));
                let name = name.trim().to_owned();
                if !name.is_empty() && name != feature.name {
                    let label = format!("Rename {}", feature.name);
                    self.change(&label, |m| {
                        if let Some(f) = m.feature_mut(id) {
                            f.name = name;
                        }
                    });
                }
            }
        });
        ui.weak(feature.kind.type_name());
        ui.add_space(6.0);

        let mut kind = feature.kind.clone();
        let result = match &mut kind {
            FeatureKind::Extrude(e) => {
                let r = features_ui::extrude_panel(ui, &self.doc, e, picking);
                ui.add_space(8.0);
                if ui.button("Edit Sketch").clicked() {
                    self.selected = Some(ItemId::Feature(e.sketch));
                    pending.push(CommandId::EditSketch);
                }
                r
            }
            FeatureKind::Sketch(s) => {
                let mut out = features_ui::PanelResult::default();
                let definition = self.doc.sketch_placement(id).map(|(_, d)| d);
                egui::Grid::new("sketch_props")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("On");
                        ui.horizontal(|ui| {
                            if picking == Some(Slot::SketchPlane) {
                                ui.colored_label(PICKING, "click a face or plane…");
                            } else {
                                ui.label(self.doc.plane_name(&s.plane));
                            }
                            if ui
                                .small_button("Change")
                                .on_hover_text("Move the sketch to another face or plane.")
                                .clicked()
                            {
                                out.pick = Some(Slot::SketchPlane);
                            }
                        });
                        ui.end_row();
                        ui.label("Status");
                        ui.label(match definition {
                            Some(crate::document::SketchStatus::Fully) => "Fully defined",
                            Some(crate::document::SketchStatus::Over) => "Over defined",
                            _ => "Under defined",
                        });
                        ui.end_row();
                        let curves = s
                            .sketch
                            .entities()
                            .filter(|(_, e)| e.kind().is_curve())
                            .count();
                        ui.label("Curves");
                        ui.label(curves.to_string());
                        ui.end_row();
                        ui.label("Relations");
                        ui.label(s.sketch.constraints().count().to_string());
                        ui.end_row();
                    });
                ui.add_space(8.0);
                if ui.button("Edit Sketch").clicked() {
                    pending.push(CommandId::EditSketch);
                }
                out
            }
            FeatureKind::BaseFlange(b) => {
                let r = features_ui::base_flange_panel(ui, &self.doc, b);
                ui.add_space(8.0);
                if ui.button("Edit Sketch").clicked() {
                    self.selected = Some(ItemId::Feature(b.sketch));
                    pending.push(CommandId::EditSketch);
                }
                r
            }
            FeatureKind::EdgeFlange(e) => {
                let radius = self.flange_default_radius(id);
                features_ui::edge_flange_panel(ui, &self.doc, e, picking, radius)
            }
            FeatureKind::SheetCut(c) => {
                ui.label(format!("Sketch: {}", self.doc.model.name_of(c.sketch)));
                ui.add_space(4.0);
                ui.weak("Cuts the sketch's regions square through the sheet. The cut is made in the flat pattern from the face the sketch is on, so straight edges can cross bends.");
                ui.add_space(8.0);
                if ui.button("Edit Sketch").clicked() {
                    self.selected = Some(ItemId::Feature(c.sketch));
                    pending.push(CommandId::EditSketch);
                }
                features_ui::PanelResult::default()
            }
            FeatureKind::Hem(h) => features_ui::hem_panel(ui, &self.doc, h, picking),
            FeatureKind::SketchedBend(b) => {
                let radius = self.flange_default_radius(id);
                let r = features_ui::sketched_bend_panel(ui, &self.doc, b, radius);
                self.edit_sketch_button(ui, b.sketch, pending);
                r
            }
            FeatureKind::Jog(j) => {
                let radius = self.flange_default_radius(id);
                let r = features_ui::jog_panel(ui, &self.doc, j, radius);
                self.edit_sketch_button(ui, j.sketch, pending);
                r
            }
            FeatureKind::MiterFlange(m) => {
                let r = features_ui::miter_flange_panel(ui, &self.doc, m, picking);
                self.edit_sketch_button(ui, m.sketch, pending);
                r
            }
            FeatureKind::Corner(c) => features_ui::corner_panel(ui, &self.doc, c, picking),
            FeatureKind::ConvertToSheet(c) => features_ui::convert_panel(ui, &self.doc, c, picking),
            FeatureKind::Form(f) => {
                let r = features_ui::form_panel(ui, &self.doc, f);
                self.edit_sketch_button(ui, f.sketch, pending);
                r
            }
            FeatureKind::Pattern(p) => features_ui::pattern_panel(ui, &self.doc, id, p, picking),
            FeatureKind::Mirror(m) => features_ui::mirror_panel(ui, &self.doc, id, m, picking),
            FeatureKind::Revolve(r) => {
                let out = solid_ui::revolve_panel(ui, &self.doc, r, picking);
                self.edit_sketch_button(ui, r.sketch, pending);
                out
            }
            FeatureKind::Sweep(s) => {
                let out = solid_ui::sweep_panel(ui, &self.doc, id, s, picking);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Edit Profile").clicked() {
                        self.selected = Some(ItemId::Feature(s.profile));
                        pending.push(CommandId::EditSketch);
                    }
                    if let Some(path) = s.path
                        && ui.button("Edit Path").clicked()
                    {
                        self.selected = Some(ItemId::Feature(path));
                        pending.push(CommandId::EditSketch);
                    }
                });
                out
            }
            FeatureKind::Loft(l) => solid_ui::loft_panel(ui, &self.doc, id, l, picking),
            FeatureKind::Blend(b) => solid_ui::blend_panel(ui, &self.doc, b, picking),
            FeatureKind::Shell(s) => solid_ui::shell_panel(ui, &self.doc, s, picking),
            FeatureKind::Draft(d) => solid_ui::draft_panel(ui, &self.doc, d, picking),
            FeatureKind::Hole(h) => {
                let out = solid_ui::hole_panel(ui, &self.doc, h);
                self.edit_sketch_button(ui, h.sketch, pending);
                out
            }
            FeatureKind::Import(i) => solid_ui::import_panel(ui, i),
            other => features_ui::reference_panel(ui, &self.doc, other, picking),
        };
        if kind != feature.kind {
            let label = format!("Edit {}", feature.name);
            self.change(&label, |m| {
                if let Some(f) = m.feature_mut(id) {
                    f.kind = kind;
                }
            });
        }
        if let Some(slot) = result.pick {
            self.picking = Some((id, slot));
            self.status_message = None;
        }
        if result.stop_pick {
            self.picking = None;
        }

        // What depends on it.
        let graph = peet_model::DependencyGraph::new(&self.doc.model);
        let users: Vec<&str> = graph
            .dependents(id)
            .iter()
            .map(|d| self.doc.model.name_of(*d))
            .collect();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let suppress = if feature.suppressed {
                "Unsuppress"
            } else {
                "Suppress"
            };
            if ui.button(suppress).clicked() {
                pending.push(CommandId::ToggleSuppress);
            }
            if ui.button("Delete").clicked() {
                pending.push(CommandId::DeleteSelection);
            }
        });
        if !users.is_empty() {
            ui.add_space(4.0);
            ui.weak(format!("Used by {}", users.join(", ")));
        }
    }

    /// A button that opens a feature's sketch.
    fn edit_sketch_button(&mut self, ui: &mut Ui, sketch: FeatureId, pending: &mut Vec<CommandId>) {
        ui.add_space(8.0);
        if ui.button("Edit Sketch").clicked() {
            self.selected = Some(ItemId::Feature(sketch));
            pending.push(CommandId::EditSketch);
        }
    }

    fn geometry_properties(&self, ui: &mut Ui) {
        let units = self.doc.model.parameters.units;
        for (index, g) in self.selected_geom.iter().enumerate() {
            let Some(body) = self.doc.bodies.get(g.body()) else {
                continue;
            };
            match *g {
                GeomRef::Face { face, .. } => {
                    let f = body.solid.face(face);
                    match &f.surface {
                        Surface::Nurbs(_) => ui.label("Freeform face"),
                        Surface::Plane(_) => ui.label("Planar face"),
                        Surface::Cylinder(c) => ui.label(format!(
                            "Cylindrical face, R{}",
                            units.format_length(c.radius)
                        )),
                        Surface::Cone(c) => ui.label(format!(
                            "Conical face, {}° to its axis",
                            peet_model::feature::format_number(c.half_angle.abs().to_degrees())
                        )),
                        Surface::Sphere(s) => ui.label(format!(
                            "Spherical face, R{}",
                            units.format_length(s.radius)
                        )),
                        Surface::Torus(t) => ui.label(format!(
                            "Toroidal face, R{} around R{}",
                            units.format_length(t.minor),
                            units.format_length(t.major)
                        )),
                    };
                    ui.weak(capitalized(
                        &self.doc.model.describe_face(body.face_name(face)),
                    ));
                }
                GeomRef::Vertex { .. } => {
                    ui.label("Vertex");
                }
                GeomRef::Edge { edge, .. } => {
                    ui.label(match &body.solid.edge(edge).curve {
                        peet_kernel::Curve3::Nurbs(_) => "Freeform edge",
                        peet_kernel::Curve3::Line(_) => "Line edge",
                        peet_kernel::Curve3::Circle(_) => "Circular edge",
                        peet_kernel::Curve3::Ellipse(_) => "Elliptical edge",
                    });
                }
            }
            // Its exact measurements: length, area, radius, centre.
            self.measurement_rows(ui, index, *g);
            ui.add_space(4.0);
        }
        self.between_rows(ui);
        ui.add_space(6.0);
        if self.selected_geom.len() == 1 {
            ui.weak(
                "Select a second face, edge or vertex with Shift held to measure between the two.",
            );
        }
        if self.selected_face().is_some() {
            ui.weak("Press S to sketch on this face, or add a reference plane from it.");
        }
    }

    /// The default bend radius of the sheet metal body an edge flange is on.
    fn flange_default_radius(&self, id: FeatureId) -> Option<f64> {
        let bodies = &self.doc.evaluation().bodies;
        let on = bodies.iter().find(|b| {
            b.sheet
                .as_ref()
                .is_some_and(|s| s.layout.pieces.iter().any(|p| p.origin.owner == id.0))
        });
        on.or_else(|| bodies.iter().find(|b| b.sheet.is_some()))
            .and_then(|b| b.sheet.as_ref())
            .map(|s| s.layout.settings.radius)
    }

    // ---- Windows ----

    /// The flat pattern report of every sheet metal body. Returns a command to run (the
    /// window's export button).
    fn bend_table_window(&mut self, ctx: &egui::Context) -> Option<CommandId> {
        let mut open = self.windows.bend_table;
        let mut command = None;
        let units = self.doc.model.parameters.units;
        egui::Window::new("Bend Table")
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .show(ctx, |ui| {
                let bodies: Vec<_> = self
                    .doc
                    .evaluation()
                    .bodies
                    .iter()
                    .filter(|b| b.sheet.is_some())
                    .cloned()
                    .collect();
                if bodies.is_empty() {
                    ui.weak("There is no sheet metal part. Start one with Base Flange.");
                    return;
                }
                for (bi, body) in bodies.iter().enumerate() {
                    let Some(sheet) = &body.sheet else { continue };
                    let report = sheet.report();
                    let name = |o: peet_sheetmetal::Origin| {
                        self.doc.model.name_of(FeatureId(o.owner)).to_owned()
                    };
                    if bodies.len() > 1 {
                        ui.strong(format!("Body from {}", self.doc.model.name_of(body.origin)));
                    }
                    egui::Grid::new(("flat_summary", bi))
                        .num_columns(2)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("Flat size");
                            ui.monospace(format!(
                                "{} × {}",
                                units.format_length(report.flat_size.x),
                                units.format_length(report.flat_size.y)
                            ));
                            ui.end_row();
                            ui.label("Thickness");
                            ui.monospace(units.format_length(report.thickness));
                            ui.end_row();
                            ui.label("Bend model");
                            ui.label(match report.model {
                                peet_sheetmetal::BendModel::KFactor(k) => {
                                    format!("K-factor {}", peet_model::feature::format_number(k))
                                }
                                peet_sheetmetal::BendModel::Allowance(v) => {
                                    format!("Bend allowance {}", units.format_length(v))
                                }
                                peet_sheetmetal::BendModel::Deduction(v) => {
                                    format!("Bend deduction {}", units.format_length(v))
                                }
                            });
                            ui.end_row();
                            ui.label("Cutouts");
                            ui.label(report.cutouts.to_string());
                            ui.end_row();
                        });
                    if report.pieces > 1 {
                        ui.colored_label(
                            features_ui::WARNING,
                            format!("The blank is in {} separate pieces.", report.pieces),
                        );
                    }
                    ui.add_space(6.0);
                    if report.bends.is_empty() {
                        ui.weak("No bends: the part is a flat plate.");
                    } else {
                        egui::Grid::new(("bend_rows", bi))
                            .striped(true)
                            .num_columns(9)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                for h in [
                                    "#", "Feature", "Direction", "Angle", "Inner R", "K", "BA",
                                    "BD", "Length",
                                ] {
                                    ui.strong(h);
                                }
                                ui.end_row();
                                for (i, row) in report.bends.iter().enumerate() {
                                    ui.label((i + 1).to_string());
                                    ui.label(name(row.origin));
                                    ui.label(if row.up { "Up" } else { "Down" });
                                    ui.monospace(units.format_angle(row.angle));
                                    ui.monospace(units.format_length(row.radius));
                                    ui.monospace(format!("{:.3}", row.k_factor));
                                    ui.monospace(units.format_length(row.allowance));
                                    ui.monospace(units.format_length(row.deduction));
                                    ui.monospace(units.format_length(row.length));
                                    ui.end_row();
                                }
                            });
                        ui.add_space(2.0);
                        ui.weak("Bends are listed in the order they were made. Up means towards the side the flat pattern is seen from (the DXF's view).");
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button("Copy as Text")
                            .on_hover_text("Tab-separated, for pasting into a spreadsheet.")
                            .clicked()
                        {
                            ui.ctx().copy_text(report.to_text(name));
                        }
                        if ui.button("Export DXF…").clicked() {
                            self.selected_geom = vec![GeomRef::Face {
                                body: self
                                    .doc
                                    .evaluation()
                                    .bodies
                                    .iter()
                                    .position(|b| std::sync::Arc::ptr_eq(b, body))
                                    .unwrap_or(0),
                                face: peet_kernel::FaceId(0),
                            }];
                            command = Some(CommandId::ExportDxf);
                        }
                    });
                    if bi + 1 < bodies.len() {
                        ui.separator();
                    }
                }
            });
        self.windows.bend_table = open;
        command
    }

    fn parameters_window(&mut self, ctx: &egui::Context) {
        let mut open = self.windows.parameters;
        let mut params = self.doc.model.parameters.clone();
        let mut units_changed = false;
        egui::Window::new("Parameters")
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Document units");
                    egui::ComboBox::from_id_salt("units")
                        .selected_text(params.units.length.label())
                        .show_ui(ui, |ui| {
                            for u in peet_sketch::expr::LengthUnit::ALL {
                                if ui
                                    .selectable_label(params.units.length == u, u.label())
                                    .clicked()
                                    && params.units.length != u
                                {
                                    params.set_units(peet_sketch::expr::Units::new(u));
                                    units_changed = true;
                                }
                            }
                        });
                })
                .response
                .on_hover_text("Lengths are shown in this unit, and plain numbers you type are taken in it. Angles are in degrees.");
                ui.add_space(4.0);
                ui.weak("Named values for expressions anywhere a value is typed: 2 * height + 5, 3in, 30deg. Sketch dimension names (d1, d2, …) work in that sketch's expressions too.");
                ui.add_space(6.0);
                let mut remove = None;
                egui::Grid::new("parameters")
                    .num_columns(4)
                    .spacing([8.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("Name");
                        ui.strong("Expression");
                        ui.strong("Value");
                        ui.end_row();
                        for i in 0..params.entries.len() {
                            let (name, expression, display) = {
                                let e = &params.entries[i];
                                (e.name.clone(), e.expression.clone(), e.display(&params.units))
                            };
                            ui.monospace(&name);
                            let key = egui::Id::new(("param_expr", name.as_str()));
                            let mut text = ui.data(|m| m.get_temp::<String>(key)).unwrap_or_else(|| expression.clone());
                            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(160.0));
                            if r.changed() {
                                ui.data_mut(|m| m.insert_temp(key, text.clone()));
                            }
                            if r.lost_focus() {
                                ui.data_mut(|m| m.remove::<String>(key));
                                if text != expression {
                                    match params.set(&name, &text) {
                                        Ok(_) => self.param_error = None,
                                        Err(e) => self.param_error = Some(format!("{name}: {e}")),
                                    }
                                }
                            }
                            if display == "error" {
                                ui.colored_label(ERROR, "error");
                            } else {
                                ui.monospace(display);
                            }
                            if ui.small_button("✖").on_hover_text("Delete").clicked() {
                                remove = Some(name.clone());
                            }
                            ui.end_row();
                        }
                        let (new_name, new_expr) = &mut self.new_param;
                        ui.add(egui::TextEdit::singleline(new_name).desired_width(90.0).hint_text("name"));
                        ui.add(egui::TextEdit::singleline(new_expr).desired_width(160.0).hint_text("value or expression"));
                        if ui.button("Add").clicked() && !new_name.trim().is_empty() {
                            let name = new_name.trim().to_owned();
                            if params.entries.iter().any(|e| e.name == name) {
                                self.param_error = Some(format!("'{name}' already exists."));
                            } else {
                                match params.set(&name, new_expr.trim()) {
                                    Ok(_) => {
                                        new_name.clear();
                                        new_expr.clear();
                                        self.param_error = None;
                                    }
                                    Err(e) => self.param_error = Some(format!("{name}: {e}")),
                                }
                            }
                        }
                        ui.end_row();
                    });
                if let Some(name) = remove {
                    params.remove(&name);
                }
                if let Some(e) = &self.param_error {
                    ui.colored_label(ERROR, e);
                }
            });
        self.windows.parameters = open;
        if params != self.doc.model.parameters {
            let label = if units_changed {
                "Change Units"
            } else {
                "Edit Parameters"
            };
            self.change(label, |m| m.parameters = params);
            if let (Some(editor), Some(work)) = (&mut self.sketch, &mut self.sketch_work) {
                editor.refresh(work, &self.doc.model.parameters);
            }
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(then) = self.files.confirm else {
            return;
        };
        let mut choice = None;
        egui::Window::new("Unsaved Changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!(
                    "{} has changes that aren't saved.",
                    self.doc.title()
                ));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Don't Save").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(2);
                    }
                });
            });
        match choice {
            Some(0) => {
                self.files.confirm = None;
                self.save(false);
                if !self.doc.is_modified() {
                    self.after_discard(then);
                }
            }
            Some(1) => {
                self.files.confirm = None;
                self.after_discard(then);
            }
            Some(_) => self.files.confirm = None,
            None => {}
        }
    }

    fn recovery_window(&mut self, ctx: &egui::Context) {
        if self.files.recovered.is_none() {
            return;
        }
        let mut choice = None;
        egui::Window::new("Recover Unsaved Work")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label("PeetCAD closed before a part was saved. Recover it?");
                ui.weak("If you discard it, it is gone for good.");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Recover").clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Discard").clicked() {
                        choice = Some(false);
                    }
                });
            });
        match choice {
            Some(true) => {
                let bytes = self.files.recovered.take().unwrap_or_default();
                match files::document_from_bytes(&bytes, None) {
                    Ok((mut doc, _)) => {
                        // Recovered work is unsaved until the user saves it.
                        doc.finish_loading();
                        doc.mark_unsaved();
                        self.set_document(doc);
                        self.info("Recovered the unsaved part. Save it to keep it.");
                    }
                    Err(e) => {
                        self.error(format!("The unsaved work couldn't be recovered: {e}"));
                        self.files.discard_autosave();
                    }
                }
            }
            Some(false) => self.files.discard_autosave(),
            None => {}
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.windows.settings;
        let mut projection_changed = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let s = &mut self.settings;
                ui.heading("Appearance");
                egui::ComboBox::from_label("Theme")
                    .selected_text(s.theme.label())
                    .show_ui(ui, |ui| {
                        for t in ThemeChoice::ALL {
                            ui.selectable_value(&mut s.theme, t, t.label());
                        }
                    });
                ui.add_space(8.0);
                ui.heading("Mouse navigation");
                egui::ComboBox::from_label("Mouse preset")
                    .selected_text(s.mouse_preset.label())
                    .show_ui(ui, |ui| {
                        for p in MousePreset::ALL {
                            ui.selectable_value(&mut s.mouse_preset, p, p.label());
                        }
                    });
                ui.horizontal(|ui| {
                    ui.label("Rotation");
                    ui.radio_value(&mut s.orbit, OrbitChoice::Turntable, "Turntable")
                        .on_hover_text("Keeps the Z axis vertical. Best for most parts.");
                    ui.radio_value(&mut s.orbit, OrbitChoice::Trackball, "Trackball")
                        .on_hover_text("Rotates freely around the screen axes.");
                });
                ui.checkbox(&mut s.invert_zoom, "Invert mouse wheel zoom");
                ui.checkbox(&mut s.animate_views, "Animate view changes");
                ui.add_space(8.0);
                ui.heading("Viewport");
                projection_changed |= ui
                    .checkbox(&mut s.perspective, "Perspective projection")
                    .changed();
                ui.checkbox(&mut s.show_grid, "Show grid");
                ui.checkbox(&mut s.show_view_cube, "Show view cube");
                ui.add_space(8.0);
                ui.heading("Files");
                ui.checkbox(&mut s.save_caches, "Save display data with parts")
                    .on_hover_text("Parts open instantly, but the files are larger. Without it the part is rebuilt when opened.");
                ui.add_space(8.0);
                if ui.button("Reset to defaults").clicked() {
                    *s = Settings::default();
                    projection_changed = true;
                }
            });
        self.windows.settings = open;
        if projection_changed && let Some(vp) = &mut self.viewport {
            vp.set_projection(if self.settings.perspective {
                Projection::Perspective
            } else {
                Projection::Orthographic
            });
        }
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let preset = self.settings.mouse_preset;
        egui::Window::new("Keyboard & Mouse")
            .open(&mut self.windows.shortcuts)
            .collapsible(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.heading(format!("Mouse ({})", preset.label()));
                egui::Grid::new("mouse_help")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (input, action) in preset.describe() {
                            ui.label(input);
                            ui.label(action);
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                ui.heading("Keyboard");
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        egui::Grid::new("key_help")
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                for cmd in CommandId::ALL.into_iter().filter(|c| c.available()) {
                                    let info = cmd.info();
                                    if let Some(sc) = info.shortcut {
                                        ui.label(format!("{}: {}", info.category, info.label));
                                        ui.monospace(ctx.format_shortcut(&sc));
                                        ui.end_row();
                                    }
                                }
                            });
                    });
            });
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let (adapter, backend) = (&self.adapter_name, &self.backend_name);
        egui::Window::new("About PeetCAD")
            .open(&mut self.windows.about)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading(format!("{APP_NAME} {VERSION}"));
                ui.label("A lightweight parametric CAD application, with sheet metal first.");
                ui.add_space(6.0);
                egui::Grid::new("about").num_columns(2).show(ui, |ui| {
                    ui.weak("Platform");
                    ui.label(peet_platform::target_description());
                    ui.end_row();
                    ui.weak("GPU");
                    ui.label(format!("{adapter} ({backend})"));
                    ui.end_row();
                    ui.weak("License");
                    ui.label("MIT");
                    ui.end_row();
                });
            });
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|first| first.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

const REFERENCES: [CommandId; 4] = [
    CommandId::RefPlane,
    CommandId::RefAxis,
    CommandId::RefPoint,
    CommandId::RefCoordSystem,
];

const RELATIONS: [CommandId; 11] = [
    CommandId::RelCoincident,
    CommandId::RelHorizontal,
    CommandId::RelVertical,
    CommandId::RelParallel,
    CommandId::RelPerpendicular,
    CommandId::RelTangent,
    CommandId::RelEqual,
    CommandId::RelConcentric,
    CommandId::RelMidpoint,
    CommandId::RelSymmetric,
    CommandId::RelFix,
];

/// The length handle of the selected edge flange: an arrow at its far end that drags the
/// flange longer or shorter, as one undo step.
fn flange_handle(
    ui: &mut Ui,
    viewport: &Viewport,
    doc: &Document,
    selected: Option<ItemId>,
    drag: &mut Option<FlangeDrag>,
) -> Option<FlangeEdit> {
    let Some(ItemId::Feature(id)) = selected else {
        *drag = None;
        return None;
    };
    let Some(FeatureKind::EdgeFlange(def)) = doc.feature(id).map(|f| &f.kind) else {
        return None;
    };
    let length = def
        .length
        .evaluate(ScalarKind::Length, &doc.model.parameters)
        .ok();
    let (tip, dir) = peet_model::sheet::edge_flange_handle(&doc.evaluation().bodies, id)?;
    let scene = doc.visible_body_bounds();
    let wpp = viewport.world_per_point();
    let (Some(a), Some(b)) = (
        viewport.project(tip, &scene),
        viewport.project(tip + dir * wpp * 50.0, &scene),
    ) else {
        return None;
    };
    if a.distance(b) < 8.0 {
        return None; // looking straight along the flange: nothing to grab
    }
    let response = ui
        .interact(
            egui::Rect::from_two_pos(a, b).expand(8.0),
            ui.id().with(("flange_handle", id.0)),
            egui::Sense::drag(),
        )
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text("Drag to change the flange length");
    let hot = response.hovered() || response.dragged();
    let color = if hot {
        PICKING
    } else {
        egui::Color32::from_rgb(230, 120, 30)
    };
    let painter = ui.painter();
    let stroke = egui::Stroke::new(if hot { 3.0 } else { 2.0 }, color);
    painter.line_segment([a, b], stroke);
    let back = (a - b).normalized() * 10.0;
    let side = egui::vec2(-back.y, back.x) * 0.5;
    painter.line_segment([b, b + back + side], stroke);
    painter.line_segment([b, b + back - side], stroke);
    painter.circle_filled(a, 3.5, color);
    if hot && let Some(l) = length {
        painter.text(
            b + (b - a).normalized() * 6.0 + egui::vec2(4.0, 0.0),
            egui::Align2::LEFT_CENTER,
            doc.model.parameters.units.format_length(l),
            egui::FontId::proportional(13.0),
            color,
        );
    }

    let along = |p: egui::Pos2, origin: DVec3, dir: DVec3| -> Option<f64> {
        // The point of the flange's line closest to the pointer ray.
        let ray = viewport.ray_at(p);
        let w0 = origin - ray.origin;
        let b = dir.dot(ray.direction);
        let denom = 1.0 - b * b;
        (denom > 1e-6).then(|| (b * ray.direction.dot(w0) - dir.dot(w0)) / denom)
    };
    if response.drag_started()
        && let (Some(p), Some(l)) = (response.interact_pointer_pos(), length)
        && let Some(s) = along(p, tip, dir)
    {
        *drag = Some(FlangeDrag {
            id,
            origin: tip,
            dir,
            length: l,
            start: s,
        });
    }
    if response.dragged()
        && let Some(d) = *drag
        && d.id == id
        && let Some(p) = response.interact_pointer_pos()
        && let Some(s) = along(p, d.origin, d.dir)
    {
        // Half-millimetre steps.
        let new = (((d.length + s - d.start) * 2.0).round() / 2.0).max(0.5);
        if length.is_none_or(|l| (l - new).abs() > 1e-9) {
            return Some(FlangeEdit::Length { id, length: new });
        }
    }
    if response.drag_stopped() {
        *drag = None;
        return Some(FlangeEdit::Done);
    }
    None
}

/// An arrow on the selected extrusion showing which way and how far it goes.
fn extrude_arrow(ui: &Ui, viewport: &Viewport, doc: &Document, selected: Option<ItemId>) {
    let Some(ItemId::Feature(id)) = selected else {
        return;
    };
    let Some(e) = doc.feature(id).and_then(|f| f.extrude()) else {
        return;
    };
    let Some((plane, _)) = doc.sketch_placement(e.sketch) else {
        return;
    };
    let Some(sketch) = doc.model.sketch(e.sketch) else {
        return;
    };
    let p = &e.params;
    let sketch_box = crate::document::sketch_bounds(&plane, &sketch.sketch);
    let base = if sketch_box.is_empty() {
        plane.origin()
    } else {
        sketch_box.center()
    };
    let units = doc.model.parameters.units;
    let depth = p
        .depth
        .evaluate(ScalarKind::Length, &doc.model.parameters)
        .unwrap_or(p.depth.value);
    let wpp = viewport.world_per_point();
    let normal = plane.normal() * p.direction();
    // (start, end, label) of each arrow, in model space.
    let fixed = wpp * 60.0;
    let arrows: Vec<(DVec3, DVec3, String)> = match &p.end {
        peet_model::EndCondition::Blind => {
            vec![(base, base + normal * depth, units.format_length(depth))]
        }
        peet_model::EndCondition::Symmetric => vec![
            (
                base,
                base + normal * depth * 0.5,
                units.format_length(depth),
            ),
            (base, base - normal * depth * 0.5, String::new()),
        ],
        other => vec![(base, base + normal * fixed, other.label().to_owned())],
    };
    let scene = doc.visible_body_bounds();
    let color = if p.operation == peet_model::Operation::Cut {
        egui::Color32::from_rgb(230, 70, 60)
    } else {
        egui::Color32::from_rgb(255, 150, 30)
    };
    let painter = ui.painter();
    let stroke = egui::Stroke::new(2.5, color);
    for (from, to, label) in arrows {
        let (Some(a), Some(b)) = (viewport.project(from, &scene), viewport.project(to, &scene))
        else {
            continue;
        };
        painter.circle_filled(a, 3.5, color);
        let len = a.distance(b);
        if len < 4.0 {
            // Looking straight along the extrusion: a target mark instead.
            painter.circle_stroke(a, 9.0, stroke);
            continue;
        }
        let dir = (b - a) / len;
        let side = egui::vec2(-dir.y, dir.x);
        // Keep the head a readable size even when the arrow is short on screen.
        let head = 14.0_f32.min(len * 0.6).max(8.0);
        let tip = b;
        let neck = tip - dir * head;
        painter.line_segment([a, neck], stroke);
        painter.add(egui::Shape::convex_polygon(
            vec![tip, neck + side * head * 0.45, neck - side * head * 0.45],
            color,
            egui::Stroke::NONE,
        ));
        if !label.is_empty() {
            let at = tip + dir * 8.0;
            let align = if dir.x >= 0.0 {
                egui::Align2::LEFT_CENTER
            } else {
                egui::Align2::RIGHT_CENTER
            };
            painter.text(at, align, &label, egui::FontId::proportional(13.0), color);
        }
    }
}

/// A menu entry for a command, with its shortcut. Returns true if clicked.
fn menu_button(ui: &mut Ui, cmd: CommandId, state: CommandState) -> bool {
    let info = cmd.info();
    let mut button = match state.checked {
        Some(checked) => egui::Button::selectable(checked, info.label),
        None => egui::Button::new(info.label),
    };
    if let Some(sc) = info.shortcut {
        button = button.shortcut_text(ui.ctx().format_shortcut(&sc));
    }
    ui.add_enabled(state.enabled, button)
        .on_hover_text(info.description)
        .clicked()
}

/// A compact toolbar button for a command. Returns true if clicked.
impl eframe::App for PeetApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.perf.begin_frame();
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.poll_files(&ctx);
        self.poll_imports(&ctx);
        // A part opened with cached bodies was shown last frame; now build it for real.
        if self.doc.finish_loading() {
            ctx.request_repaint();
        }

        // Closing the window with unsaved changes asks first.
        if ctx.input(|i| i.viewport().close_requested())
            && self.doc.is_modified()
            && !self.quit_requested
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.files.confirm = Some(AfterDiscard::Quit);
        }
        if self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        let mut pending = self.shortcut_commands(&ctx);
        if (self.picking.is_some() || self.picking_sketch_plane)
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.picking = None;
            self.picking_sketch_plane = false;
        }
        self.hovered = None;

        let mut tab = self.ribbon_tab;
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui, &mut pending, &mut tab);
            ui.add_space(3.0);
        });
        self.ribbon_tab = tab;
        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));
        if self.settings.show_feature_tree {
            egui::Panel::left("feature_tree")
                .resizable(true)
                .default_size(230.0)
                .size_range(160.0..=480.0)
                .show(ui, |ui| self.feature_tree(ui));
        }
        if self.settings.show_properties {
            egui::Panel::right("properties")
                .resizable(true)
                .default_size(270.0)
                .size_range(180.0..=480.0)
                .show(ui, |ui| self.properties(ui, &mut pending));
        }

        let dark = ctx.theme() == egui::Theme::Dark;
        let mut pick_click = None;
        let mut flange_edit = None;
        let mut sketch_plane_click = None;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let (Some(viewport), Some(render_state)) = (&mut self.viewport, &self.render_state) else {
                ui.centered_and_justified(|ui| {
                    ui.label("The 3D viewport needs a GPU (WebGPU, WebGL2, DirectX 12 or Vulkan), and none was found.");
                });
                return;
            };
            let events = viewport.show(
                ui,
                &ViewportParams {
                    render_state,
                    settings: &self.settings,
                    document: &self.doc,
                    selected: self.selected,
                    hovered: self.hovered,
                    dark,
                    editing_sketch: self.sketch.as_ref().map(|e| e.item),
                    hovered_geom: if self.sketch.is_some() { None } else { self.hovered_geom },
                    selected_geom: &self.selected_geom,
                    show_std_planes: self.picking_sketch_plane,
                },
            );
            self.hovered_geom = viewport.hovered_geom;
            let handles = self.sketch.is_none()
                && self.picking.is_none()
                && !self.picking_sketch_plane
                && !self.doc.is_flat();
            flange_edit = flange_handle(
                ui,
                viewport,
                &self.doc,
                self.selected.filter(|_| handles),
                &mut self.flange_drag,
            );
            extrude_arrow(ui, viewport, &self.doc, self.selected.filter(|_| handles));
            if let (Some(editor), Some(work)) = (&mut self.sketch, &mut self.sketch_work) {
                if let Some(response) = &events.response {
                    editor.show(ui, response, &viewport.camera, work, &self.doc.model.parameters, dark);
                }
            } else if events.clicked_background {
                let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                if self.picking_sketch_plane {
                    sketch_plane_click = Some((events.clicked_geom, events.clicked_plane));
                } else if self.picking.is_some() {
                    pick_click = Some((events.clicked_geom, events.clicked_plane));
                } else if let Some(g) = events.clicked_geom {
                    self.selected = None;
                    if additive {
                        if let Some(i) = self.selected_geom.iter().position(|x| *x == g) {
                            self.selected_geom.remove(i);
                        } else {
                            self.selected_geom.push(g);
                        }
                    } else {
                        self.selected_geom = vec![g];
                    }
                } else if let Some(plane) = events.clicked_plane {
                    self.selected_geom.clear();
                    self.selected = Some(plane);
                } else {
                    self.selected = None;
                    self.selected_geom.clear();
                }
            }
            if !self.initial_fit_done {
                viewport.zoom_to_fit(&self.doc.visible_bounds(), false);
                self.initial_fit_done = true;
                ctx.request_repaint();
            }
            if self.settings.show_perf_overlay {
                let info = PerfInfo {
                    adapter: self.adapter_name.clone(),
                    backend: self.backend_name.clone(),
                    size_px: viewport.size_px,
                    samples: viewport.samples,
                    render_cpu_ms: viewport.render_cpu_ms,
                    triangles: viewport.stats.triangles,
                    lines: viewport.stats.lines,
                    draw_calls: viewport.stats.draw_calls,
                };
                let rect = ui.max_rect();
                self.perf.show(ui, rect, &info);
            }
        });

        match flange_edit {
            Some(FlangeEdit::Length { id, length }) => {
                let label = format!("Drag {} Length", self.doc.model.name_of(id));
                // One key for the whole drag: its changes are one undo step.
                self.change_model(&label, Some(0x666c_616e_6765 ^ u64::from(id.0)), |m| {
                    if let Some(f) = m.feature_mut(id)
                        && let FeatureKind::EdgeFlange(e) = &mut f.kind
                    {
                        e.length = Scalar::new(length);
                    }
                });
            }
            Some(FlangeEdit::Done) => self.doc.seal_history(),
            None => {}
        }
        if let Some((geom, plane)) = pick_click {
            let picked = self.picked(geom, plane);
            self.finish_pick(picked);
        }
        if let Some((geom, plane)) = sketch_plane_click {
            self.finish_sketch_plane_pick(geom, plane);
        }
        self.settings_window(&ctx);
        self.shortcuts_window(&ctx);
        self.about_window(&ctx);
        self.parameters_window(&ctx);
        self.sheet_checks_window(&ctx);
        self.gauge_window(&ctx);
        self.mass_properties_window(&ctx);
        self.notice_window(&ctx);
        if let Some(cmd) = self.bend_table_window(&ctx) {
            pending.push(cmd);
        }
        self.confirm_window(&ctx);
        self.recovery_window(&ctx);
        let states = CommandId::ALL.map(|c| (c, self.command_state(c)));
        let state_of = |c: CommandId| {
            states
                .iter()
                .find(|(id, _)| *id == c)
                .map(|(_, s)| *s)
                .unwrap_or_default()
        };
        if let Some(cmd) = self.palette.show(&ctx, state_of) {
            pending.push(cmd);
        }

        for cmd in pending {
            self.execute(&ctx, cmd);
        }
        self.update_title(&ctx);
        self.perf.end_frame();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
        // Called periodically and on shutdown: make sure unsaved work is on disk.
        self.files.autosave(&self.doc, true);
    }
}

// ---- Phase 5 commands ----

/// What a pattern or mirror command makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyKind {
    Linear,
    Circular,
    Mirror,
}

impl PeetApp {
    /// The selected edges that are on sheet metal bodies.
    fn selected_sheet_edges(&self) -> Vec<EdgeRef> {
        self.selected_geom
            .iter()
            .filter_map(|g| match *g {
                GeomRef::Edge { body, edge } => {
                    let b = &self.doc.bodies.get(body)?.source;
                    b.sheet.as_ref()?;
                    b.edge_ref(edge)
                }
                _ => None,
            })
            .collect()
    }

    /// Adds a hem on each selected sheet metal edge, or one waiting for a pick.
    fn start_hem(&mut self) {
        let edges = self.selected_sheet_edges();
        let pick = edges.is_empty();
        let mut ids = Vec::new();
        self.change(
            if edges.len() > 1 {
                "Add Hems"
            } else {
                "Add Hem"
            },
            |m| {
                if edges.is_empty() {
                    ids.push(m.add_hem(None));
                }
                for e in edges {
                    ids.push(m.add_hem(Some(e)));
                }
            },
        );
        let last = ids.last().copied();
        self.selected = last.map(ItemId::Feature);
        self.selected_geom.clear();
        if pick && let Some(id) = last {
            self.picking = Some((id, Slot::HemEdge));
            self.status_message = None;
        }
    }

    /// Adds a feature made from the open or selected sketch, which must be on a flat
    /// face of a sheet metal part.
    fn start_from_sheet_sketch(
        &mut self,
        label: &str,
        add: impl FnOnce(&mut peet_model::Model, FeatureId) -> FeatureId,
    ) {
        let Some(sketch) = self.extrude_source() else {
            return;
        };
        self.close_sketch();
        if !self.sketch_on_sheet(sketch) {
            self.error(format!(
                "{label}: draw the sketch on a flat face of the sheet metal part first (select the face, then New Sketch)."
            ));
            return;
        }
        let mut id = None;
        self.change(&format!("Add {label}"), |m| id = Some(add(m, sketch)));
        self.selected = id.map(ItemId::Feature);
        self.selected_geom.clear();
    }

    /// Adds a mitre flange from the open or selected sketch along the selected edges.
    fn start_miter_flange(&mut self) {
        let Some(sketch) = self.extrude_source() else {
            return;
        };
        let edges = self.selected_sheet_edges();
        self.close_sketch();
        let pick = edges.is_empty();
        let mut id = None;
        self.change("Add Miter Flange", |m| {
            id = Some(m.add_miter_flange(sketch, edges));
        });
        self.selected = id.map(ItemId::Feature);
        self.selected_geom.clear();
        if pick && let Some(id) = id {
            self.picking = Some((id, Slot::MiterEdge));
            self.info("Click the edges for the flange to run along, one after the other (Add in the properties for each).");
        }
    }

    /// Adds a corner treatment for the corners at the selected faces (or every corner).
    fn start_corner(&mut self) {
        let faces: Vec<peet_model::FaceRef> = self
            .selected_geom
            .iter()
            .filter_map(|g| match *g {
                GeomRef::Face { body, face } => {
                    let b = &self.doc.bodies.get(body)?.source;
                    b.sheet.as_ref()?;
                    Some(b.face_ref(face))
                }
                _ => None,
            })
            .collect();
        // Start from the part's relief size.
        let relief = self
            .doc
            .sheet_body(None)
            .and_then(|b| b.sheet.as_ref())
            .map(|s| s.layout.default_corner());
        let mut id = None;
        self.change("Add Corner", |m| {
            let c = m.add_corner(faces);
            if let (Some(spec), Some(f)) = (relief, m.feature_mut(c))
                && let FeatureKind::Corner(def) = &mut f.kind
            {
                def.relief = spec.relief;
                def.relief_size = peet_model::Scalar::new(spec.relief_size);
                def.gap = peet_model::Scalar::new(spec.gap);
            }
            id = Some(c);
        });
        self.selected = id.map(ItemId::Feature);
        self.selected_geom.clear();
    }

    /// The selected feature, if a pattern or a mirror can copy it.
    fn copy_source(&self) -> Option<FeatureId> {
        let id = self.selected_feature()?;
        self.doc
            .feature(id)
            .is_some_and(|f| f.kind.can_be_copied())
            .then_some(id)
    }

    /// Adds a pattern or a mirror of the selected feature.
    fn start_copy(&mut self, kind: CopyKind) {
        let Some(seed) = self.copy_source() else {
            self.error("Select the feature to copy in the tree first: an extrusion, a cut, a revolve, a hole, a sheet metal cut or a form.");
            return;
        };
        use peet_model::{AxisRef, LinearDirection, PatternDef, Scalar, StdAxis, StdPlane};
        // A direction along the face the seed is sketched on, if it is on one.
        let sketch_plane = self
            .doc
            .feature(seed)
            .and_then(|f| f.kind.sketch())
            .and_then(|s| self.doc.sketch_placement(s))
            .map(|(plane, _)| plane);
        let in_plane = |axis: StdAxis| {
            sketch_plane.is_none_or(|p| p.normal().dot(axis.axis().dir).abs() < 0.5)
        };
        let along = StdAxis::ALL
            .into_iter()
            .find(|a| in_plane(*a))
            .unwrap_or(StdAxis::X);
        let normal = sketch_plane.map_or(StdAxis::Z, |p| {
            StdAxis::ALL
                .into_iter()
                .max_by(|a, b| {
                    let d = |x: &StdAxis| p.normal().dot(x.axis().dir).abs();
                    d(a).total_cmp(&d(b))
                })
                .unwrap_or(StdAxis::Z)
        });
        let mut id = None;
        let label = match kind {
            CopyKind::Linear => "Add Linear Pattern",
            CopyKind::Circular => "Add Circular Pattern",
            CopyKind::Mirror => "Add Mirror",
        };
        self.change(label, |m| {
            id = Some(match kind {
                CopyKind::Linear => m.add_pattern(
                    vec![seed],
                    PatternDef::Linear {
                        first: LinearDirection {
                            direction: AxisRef::Standard(along),
                            spacing: Scalar::new(20.0),
                            count: 2,
                            flip: false,
                        },
                        second: None,
                    },
                ),
                CopyKind::Circular => m.add_pattern(
                    vec![seed],
                    PatternDef::Circular {
                        axis: AxisRef::Standard(normal),
                        count: 4,
                        angle: Scalar::new(360.0),
                        flip: false,
                    },
                ),
                CopyKind::Mirror => {
                    // A standard plane square to the sketch, to start with.
                    let plane = StdPlane::ALL
                        .into_iter()
                        .find(|p| {
                            sketch_plane
                                .is_none_or(|s| s.normal().dot(p.plane().normal()).abs() < 0.5)
                        })
                        .unwrap_or(StdPlane::Right);
                    m.add_mirror(vec![seed], PlaneRef::Standard(plane))
                }
            });
        });
        self.selected = id.map(ItemId::Feature);
        self.selected_geom.clear();
    }

    fn export_step(&mut self) {
        let bodies = self.doc.evaluation().bodies.clone();
        if bodies.is_empty() {
            self.error("There are no bodies to export.");
            return;
        }
        let title = self.doc.title();
        let stem = title.strip_suffix(".peet").unwrap_or(&title).to_owned();
        let text = match peet_ops::export_bytes(
            &self.doc,
            peet_ops::Format::Step,
            None,
            peet_io::step::StepSchema::Ap214,
        ) {
            Ok((bytes, _)) => bytes,
            Err(e) => return self.error(e),
        };
        match peet_platform::save_file(
            &format!("{stem}.step"),
            ("STEP file", &["step", "stp"]),
            &text,
        ) {
            Ok(peet_platform::SaveOutcome::Saved(to)) => self.info(format!(
                "Exported {} as STEP (AP214) to {to}",
                if bodies.len() == 1 {
                    "the body".to_owned()
                } else {
                    format!("{} bodies", bodies.len())
                }
            )),
            Ok(peet_platform::SaveOutcome::Cancelled) => {}
            Err(e) => self.error(e),
        }
    }

    /// Puts an opened DXF into the open sketch, or into a new one.
    fn finish_dxf_import(&mut self, file: &peet_platform::OpenedFile) {
        let options = peet_io::dxf_import::ImportOptions::flat_pattern();
        if let (Some(editor), Some(work)) = (&mut self.sketch, &mut self.sketch_work) {
            let result = editor.edit_externally(work, |s| {
                peet_io::dxf_import::import_into(s, &file.bytes, &options)
            });
            match result {
                Ok(report) => self.info(import_summary(&file.name, &report)),
                Err(e) => self.error(format!("Couldn't import {}: {e}", file.name)),
            }
            return;
        }
        let imported = match peet_io::dxf_import::import(&file.bytes, &options) {
            Ok(i) => i,
            Err(e) => {
                self.error(format!("Couldn't import {}: {e}", file.name));
                return;
            }
        };
        let (plane, placement) = self.selected_plane().unwrap_or((
            PlaneRef::Standard(peet_model::StdPlane::Top),
            peet_model::StdPlane::Top.plane(),
        ));
        let mut id = None;
        let sketch = imported.sketch;
        self.change("Import DXF", |m| {
            let s = m.add_sketch(plane, placement);
            if let Some(f) = m.feature_mut(s).and_then(|f| f.sketch_mut()) {
                f.sketch = sketch;
            }
            id = Some(s);
        });
        self.selected = id.map(ItemId::Feature);
        self.selected_geom.clear();
        self.info(import_summary(&file.name, &imported.report));
        let bounds = self.doc.visible_bounds();
        let animate = self.settings.animate_views;
        if let Some(vp) = &mut self.viewport {
            vp.zoom_to_fit(&bounds, animate);
        }
    }

    /// The manufacturing checks of every sheet metal body.
    fn sheet_checks_window(&mut self, ctx: &egui::Context) {
        let mut open = self.windows.sheet_checks;
        let units = self.doc.model.parameters.units;
        let mut rules = self.settings.check_rules;
        egui::Window::new("Check for Manufacture")
            .open(&mut open)
            .resizable(true)
            .default_width(560.0)
            .show(ctx, |ui| {
                let bodies: Vec<_> = self
                    .doc
                    .evaluation()
                    .bodies
                    .iter()
                    .filter(|b| b.sheet.is_some())
                    .cloned()
                    .collect();
                if bodies.is_empty() {
                    ui.weak("There is no sheet metal part. Start one with Base Flange.");
                }
                // Features that failed to build are the first thing to fix.
                let failed: Vec<(String, String)> = self
                    .doc
                    .model
                    .features()
                    .filter(|f| f.kind.is_sheet_metal())
                    .filter_map(|f| match self.doc.evaluation().status(f.id) {
                        Some(peet_model::Status::Failed(m)) => Some((f.name.clone(), m.clone())),
                        _ => None,
                    })
                    .collect();
                for (name, message) in &failed {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(features_ui::ERROR, "⚠");
                        ui.label(format!("{name} doesn't build: {message}"));
                    });
                }
                for (bi, body) in bodies.iter().enumerate() {
                    let Some(sheet) = &body.sheet else { continue };
                    if bodies.len() > 1 {
                        ui.strong(format!("Body from {}", self.doc.model.name_of(body.origin)));
                    }
                    let findings = peet_sheetmetal::check(sheet, &rules, |o| {
                        self.doc.model.name_of(FeatureId(o)).to_owned()
                    });
                    if findings.is_empty() && failed.is_empty() {
                        ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::from_rgb(60, 170, 80), "✔");
                            ui.label("Nothing found: the part passes every check.");
                        });
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(("findings", bi))
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for f in &findings {
                                ui.horizontal_wrapped(|ui| {
                                    let (mark, color) = match f.severity {
                                        peet_sheetmetal::Severity::Error => {
                                            ("⚠", features_ui::ERROR)
                                        }
                                        peet_sheetmetal::Severity::Warning => {
                                            ("⚠", features_ui::WARNING)
                                        }
                                        peet_sheetmetal::Severity::Info => {
                                            ("ℹ", ui.visuals().weak_text_color())
                                        }
                                    };
                                    ui.colored_label(color, mark);
                                    ui.strong(f.kind.label());
                                    ui.label(&f.message);
                                });
                            }
                        });
                    if bi + 1 < bodies.len() {
                        ui.separator();
                    }
                }
                ui.add_space(6.0);
                ui.collapsing("Rules", |ui| {
                    ui.weak("Each limit is a multiple of the thickness (t), plus a multiple of the bend radius (R), plus a fixed length. Set all three to zero to turn a check off.");
                    egui::Grid::new("check_rules")
                        .num_columns(5)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            for h in ["Check", "× t", "× R", &format!("+ {}", units.length.suffix()), ""] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            let row = |ui: &mut Ui, label: &str, tip: &str, r: &mut peet_sheetmetal::Rule| {
                                ui.label(label).on_hover_text(tip);
                                ui.add(egui::DragValue::new(&mut r.thickness).speed(0.05).range(0.0..=50.0));
                                ui.add(egui::DragValue::new(&mut r.radius).speed(0.05).range(0.0..=50.0));
                                ui.add(egui::DragValue::new(&mut r.constant).speed(0.05).range(0.0..=1000.0));
                                ui.weak(r.formula());
                                ui.end_row();
                            };
                            row(ui, "Shortest flange", "The flat length a flange needs next to a bend for the press brake to grip it.", &mut rules.min_flange);
                            row(ui, "Hole to bend", "The least distance from a hole or a cut to a bend region.", &mut rules.hole_to_bend);
                            row(ui, "Hole to edge", "The least distance from a hole to the edge of the blank.", &mut rules.hole_to_edge);
                            row(ui, "Hole to hole", "The least distance between two holes.", &mut rules.hole_to_hole);
                            row(ui, "Smallest hole", "The smallest hole that can be cut cleanly.", &mut rules.min_hole);
                            row(ui, "Collision depth", "How far two folded parts may run into each other before it is reported.", &mut rules.collision);
                        });
                    if ui.button("Defaults").clicked() {
                        rules = peet_sheetmetal::CheckRules::default();
                    }
                });
            });
        self.settings.check_rules = rules;
        self.windows.sheet_checks = open;
    }

    /// The base flange whose settings the gauge window applies to: the selected one, the
    /// one of the selected body, or the first.
    fn gauge_target(&self) -> Option<FeatureId> {
        let is_base = |id: FeatureId| {
            matches!(
                self.doc.feature(id).map(|f| &f.kind),
                Some(FeatureKind::BaseFlange(_))
            )
        };
        self.selected_feature()
            .filter(|id| is_base(*id))
            .or_else(|| {
                let body = self.selected_geom.first().map(|g| g.body())?;
                let origin = self.doc.bodies.get(body)?.source.origin;
                is_base(origin).then_some(origin)
            })
            .or_else(|| {
                self.doc
                    .model
                    .features()
                    .find(|f| matches!(f.kind, FeatureKind::BaseFlange(_)))
                    .map(|f| f.id)
            })
    }

    /// Material and gauge tables: apply an entry to the part, edit, import and export.
    fn gauge_window(&mut self, ctx: &egui::Context) {
        let mut open = self.windows.gauges;
        let units = self.doc.model.parameters.units;
        let mut library = self.settings.materials.clone();
        let target = self.gauge_target();
        let mut apply: Option<peet_sheetmetal::GaugeEntry> = None;
        let mut import = false;
        let mut export = false;
        let mut message = self.gauge_message.clone();
        let mut tab = self.gauge_table.min(library.tables.len().saturating_sub(1));
        egui::Window::new("Materials and Gauges")
            .open(&mut open)
            .resizable(true)
            .default_width(640.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Material");
                    egui::ComboBox::from_id_salt("gauge_material")
                        .selected_text(
                            library
                                .tables
                                .get(tab)
                                .map_or("None", |t| t.name.as_str())
                                .to_owned(),
                        )
                        .show_ui(ui, |ui| {
                            for (i, t) in library.tables.iter().enumerate() {
                                ui.selectable_value(&mut tab, i, &t.name);
                            }
                        });
                    if ui.button("New material").clicked() {
                        library.tables.push(peet_sheetmetal::GaugeTable {
                            name: format!("Material {}", library.tables.len() + 1),
                            material: format!("Material {}", library.tables.len() + 1),
                            entries: Vec::new(),
                        });
                        tab = library.tables.len() - 1;
                    }
                    if !library.tables.is_empty() && ui.button("Delete material").clicked() {
                        library.tables.remove(tab);
                        tab = tab.saturating_sub(1);
                    }
                });
                ui.add_space(4.0);
                let target_name = target.map(|t| self.doc.model.name_of(t).to_owned());
                if let Some(table) = library.tables.get_mut(tab) {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        if ui.text_edit_singleline(&mut table.name).changed() {
                            table.material = table.name.clone();
                        }
                    });
                    let mut remove = None;
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        egui::Grid::new("gauge_rows")
                            .striped(true)
                            .num_columns(7)
                            .spacing([10.0, 4.0])
                            .show(ui, |ui| {
                                for h in ["Gauge", "Thickness", "Bend radius", "K-factor", "Notes", "", ""] {
                                    ui.strong(h);
                                }
                                ui.end_row();
                                for (i, e) in table.entries.iter_mut().enumerate() {
                                    ui.add(egui::TextEdit::singleline(&mut e.gauge).desired_width(70.0));
                                    let scale = units.length.mm_per_unit();
                                    let mut thickness = e.thickness / scale;
                                    if ui.add(egui::DragValue::new(&mut thickness).speed(0.01).range(0.001..=1000.0).suffix(format!(" {}", units.length.suffix()))).changed() {
                                        e.thickness = thickness * scale;
                                    }
                                    let mut radius = e.radius / scale;
                                    if ui.add(egui::DragValue::new(&mut radius).speed(0.01).range(0.0..=1000.0).suffix(format!(" {}", units.length.suffix()))).changed() {
                                        e.radius = radius * scale;
                                    }
                                    match &mut e.model {
                                        peet_sheetmetal::BendModel::KFactor(k) => {
                                            ui.add(egui::DragValue::new(k).speed(0.005).range(0.0..=1.0));
                                        }
                                        peet_sheetmetal::BendModel::Allowance(v) => {
                                            ui.label(format!("BA {}", units.format_length(*v)));
                                        }
                                        peet_sheetmetal::BendModel::Deduction(v) => {
                                            ui.label(format!("BD {}", units.format_length(*v)));
                                        }
                                    }
                                    ui.add(egui::TextEdit::singleline(&mut e.notes).desired_width(120.0));
                                    let tip = match &target_name {
                                        Some(n) => format!("Set the thickness, bend radius and bend model of {n} from this row."),
                                        None => "There is no sheet metal part yet: start one with Base Flange.".to_owned(),
                                    };
                                    if ui.add_enabled(target.is_some(), egui::Button::new("Apply").small()).on_hover_text(tip).clicked() {
                                        apply = Some(e.clone());
                                    }
                                    if ui.small_button("✖").on_hover_text("Delete this row.").clicked() {
                                        remove = Some(i);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                    if let Some(i) = remove {
                        table.entries.remove(i);
                    }
                    if ui.button("Add gauge").clicked() {
                        let last = table.entries.last().cloned();
                        table.entries.push(last.unwrap_or(peet_sheetmetal::GaugeEntry {
                            gauge: "1.5 mm".to_owned(),
                            thickness: 1.5,
                            radius: 1.5,
                            model: peet_sheetmetal::BendModel::KFactor(0.44),
                            notes: String::new(),
                        }));
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Import CSV…").on_hover_text("Replace the tables with those in a CSV file (from a colleague or a spreadsheet).").clicked() {
                        import = true;
                    }
                    if ui.button("Export CSV…").on_hover_text("Save every table as one CSV file, to share or to edit in a spreadsheet.").clicked() {
                        export = true;
                    }
                    if ui.button("Restore built-in tables").clicked() {
                        library = peet_sheetmetal::MaterialLibrary::builtin();
                        tab = 0;
                        message = None;
                    }
                });
                if let Some(m) = &message {
                    ui.add_space(4.0);
                    ui.colored_label(features_ui::ERROR, m);
                }
                ui.add_space(4.0);
                ui.weak("The built-in values are starting points: check radius and K-factor against a test bend in your shop. Applying a row copies its values into the part; the part doesn't change when the table does.");
            });
        self.gauge_table = tab;
        self.gauge_message = message;
        self.settings.materials = library;
        self.windows.gauges = open;
        if let (Some(entry), Some(target)) = (apply, target) {
            let name = self.doc.model.name_of(target).to_owned();
            let label = entry.gauge.clone();
            self.change(&format!("Apply {label} to {name}"), |m| {
                if let Some(f) = m.feature_mut(target)
                    && let FeatureKind::BaseFlange(b) = &mut f.kind
                {
                    use peet_model::{BendModelDef, Scalar};
                    b.settings.thickness = Scalar::new(entry.thickness);
                    b.settings.radius = Scalar::new(entry.radius);
                    b.settings.model = match entry.model {
                        peet_sheetmetal::BendModel::KFactor(k) => {
                            BendModelDef::KFactor(Scalar::new(k))
                        }
                        peet_sheetmetal::BendModel::Allowance(v) => {
                            BendModelDef::Allowance(Scalar::new(v))
                        }
                        peet_sheetmetal::BendModel::Deduction(v) => {
                            BendModelDef::Deduction(Scalar::new(v))
                        }
                    };
                }
            });
            self.info(format!("Applied {label} to {name}."));
        }
        if export {
            let csv = self.settings.materials.to_csv();
            match peet_platform::save_file("gauges.csv", ("CSV table", &["csv"]), csv.as_bytes()) {
                Ok(peet_platform::SaveOutcome::Saved(to)) => {
                    self.info(format!("Saved the gauge tables to {to}"));
                }
                Ok(peet_platform::SaveOutcome::Cancelled) => {}
                Err(e) => self.error(e),
            }
        }
        if import {
            self.gauge_import = Some(peet_platform::open_file(("CSV table", &["csv"])));
        }
    }

    /// Takes the results of the import dialogs when they arrive.
    fn poll_imports(&mut self, ctx: &egui::Context) {
        if let Some(p) = &self.dxf_import
            && let Some(result) = p.take()
        {
            self.dxf_import = None;
            match result {
                Ok(Some(file)) => self.finish_dxf_import(&file),
                Ok(None) => {}
                Err(e) => self.error(e),
            }
        }
        if let Some(p) = &self.step_import
            && let Some(result) = p.take()
        {
            self.step_import = None;
            match result {
                Ok(Some(file)) => self.finish_step_import(&file),
                Ok(None) => {}
                Err(e) => self.error(e),
            }
        }
        if let Some(p) = &self.gauge_import
            && let Some(result) = p.take()
        {
            self.gauge_import = None;
            match result {
                Ok(Some(file)) => {
                    let text = String::from_utf8_lossy(&file.bytes);
                    match peet_sheetmetal::MaterialLibrary::from_csv(&text) {
                        Ok(lib) => {
                            let n: usize = lib.tables.iter().map(|t| t.entries.len()).sum();
                            self.settings.materials = lib;
                            self.gauge_table = 0;
                            self.gauge_message = None;
                            self.info(format!("Imported {n} gauges from {}", file.name));
                        }
                        Err(errors) => {
                            let mut text: Vec<String> =
                                errors.iter().take(6).map(ToString::to_string).collect();
                            if errors.len() > 6 {
                                text.push(format!("…and {} more.", errors.len() - 6));
                            }
                            self.gauge_message =
                                Some(format!("{} wasn't imported. {}", file.name, text.join(" ")));
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => self.error(e),
            }
        }
        if self.dxf_import.is_some() || self.gauge_import.is_some() || self.step_import.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

/// What a DXF import brought in, for the status bar.
fn import_summary(name: &str, report: &peet_io::dxf_import::ImportReport) -> String {
    let mut text = format!(
        "Imported {} curves from {name} ({})",
        report.entities_imported,
        report.unit_used.label()
    );
    let skipped: usize = report.skipped.iter().map(|(_, n)| n).sum();
    if skipped > 0 {
        let kinds: Vec<String> = report
            .skipped
            .iter()
            .map(|(k, n)| format!("{n} {k}"))
            .collect();
        text.push_str(&format!("; left out {}", kinds.join(", ")));
    }
    text
}
