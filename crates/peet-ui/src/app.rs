//! The PeetCAD application shell: menus, toolbar, panels, windows and command dispatch.

use eframe::egui_wgpu::RenderState;
use egui::{Align, Layout, Ui};
use peet_kernel::Surface;
use peet_model::{
    AxisDef, CoordSystemDef, Datum, FeatureId, FeatureKind, PlaneDef, PlaneRef, PointDef, PointRef,
    Scalar, StdPlane,
};
use peet_platform::Instant;
use peet_render::{Projection, StandardView};

use crate::bodies::GeomRef;
use crate::commands::{CommandId, CommandState};
use crate::document::{Document, FileLocation, ItemId, Persistent, SketchItem};
use crate::features_ui::{self, ERROR, PICKING, Picked, Slot};
use crate::files::{self, AfterDiscard, FileState};
use crate::palette::CommandPalette;
use crate::perf::{PerfInfo, PerfMonitor};
use crate::settings::{MousePreset, OrbitChoice, STORAGE_KEY, Settings, ThemeChoice};
use crate::sketch_ui::{self, SketchEditor};
use crate::tree::{TreeAction, TreeView, tree_ui};
use crate::viewport::{Viewport, ViewportParams};

pub const APP_NAME: &str = "PeetCAD";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default)]
struct OpenWindows {
    settings: bool,
    shortcuts: bool,
    about: bool,
    plane_picker: bool,
    parameters: bool,
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
    /// Last message for the status bar, and whether it is an error.
    status_message: Option<(String, bool)>,
    files: FileState,
    /// The user chose to quit (after deciding about unsaved changes).
    quit_requested: bool,
    /// The window title last set, so it is only sent when it changes.
    title: String,
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

        let render_state = cc.wgpu_render_state.clone();
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
            status_message: None,
            files: FileState::default(),
            quit_requested: false,
            title: String::new(),
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
    /// through the rebuild.
    fn change(&mut self, label: &str, f: impl FnOnce(&mut peet_model::Model)) -> bool {
        let kept: Vec<Persistent> = self
            .selected_geom
            .iter()
            .filter_map(|g| self.doc.persist(*g))
            .collect();
        let changed = self.doc.change(label, f);
        if changed {
            self.restore_selection(&kept);
        }
        changed
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
        let kept: Vec<Persistent> = self
            .selected_geom
            .iter()
            .filter_map(|g| self.doc.persist(*g))
            .collect();
        let label = if redo {
            self.doc.redo()
        } else {
            self.doc.undo()
        };
        if let Some(label) = label {
            self.restore_selection(&kept);
            self.info(format!("{} {label}", if redo { "Redid" } else { "Undid" }));
        }
    }

    /// Replaces the document (new, opened, recovered).
    fn set_document(&mut self, doc: Document) {
        self.close_sketch_discarding();
        self.doc = doc;
        self.selected = None;
        self.hovered = None;
        self.selected_geom.clear();
        self.hovered_geom = None;
        self.picking = None;
        self.initial_fit_done = false;
    }

    // ---- Selection helpers ----

    /// The selected planar face, if exactly one face is selected.
    fn selected_face(&self) -> Option<(PlaneRef, peet_math::Plane)> {
        match self.selected_geom[..] {
            [GeomRef::Face { body, face }] => {
                let b = self.doc.bodies.get(body)?;
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
                let b = self.doc.bodies.get(body)?;
                let surface = b.solid.face(face).surface;
                Some(Picked::Face {
                    face: b.face_ref(face),
                    planar: matches!(surface, Surface::Plane(_)),
                    round: matches!(surface, Surface::Cylinder(_)),
                })
            }
            Some(GeomRef::Edge { body, edge }) => {
                Some(Picked::Edge(self.doc.bodies.get(body)?.edge_ref(edge)?))
            }
            Some(GeomRef::Vertex { body, vertex }) => Some(Picked::Vertex(
                self.doc.bodies.get(body)?.vertex_ref(vertex),
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
        match features_ui::apply_pick(&mut kind, slot, picked) {
            Ok(()) => {
                self.change(&format!("Edit {name}"), |m| {
                    if let Some(f) = m.feature_mut(id) {
                        f.kind = kind;
                    }
                });
                self.status_message = None;
            }
            Err(e) => {
                self.error(e);
                self.picking = Some((id, slot));
            }
        }
    }

    // ---- Sketches ----

    /// Opens a sketch for editing and turns the view to look straight at it.
    fn open_sketch(&mut self, id: FeatureId) {
        self.close_sketch();
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
                self.set_document(Document::default());
                self.files.discard_autosave();
            }
            AfterDiscard::Open => {
                self.files.opening = Some(peet_platform::open_file(files::FILTER));
            }
            AfterDiscard::Sample => {
                let (model, _) = peet_model::samples::bracket();
                self.set_document(Document::from_model(model, None));
                self.files.discard_autosave();
                self.info("Opened the sample bracket. Try changing Sketch1's width (d1), or drag the rollback bar.");
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
                    let location = FileLocation {
                        name: file.name.clone(),
                        path: file.path.clone(),
                    };
                    match files::document_from_bytes(&file.bytes, Some(location)) {
                        Ok((doc, warnings)) => {
                            self.set_document(doc);
                            self.files.discard_autosave();
                            match warnings.first() {
                                Some(w) => self.error(w.clone()),
                                None => self.info(format!("Opened {}", file.name)),
                            }
                        }
                        Err(e) => self.error(format!("Couldn't open {}: {e}", file.name)),
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
            CommandId::CutExtrude => self.start_extrude(peet_model::Operation::Cut),
            CommandId::ExportStl => self.export_stl(),
            CommandId::NewSketch => match self.selected_plane() {
                Some((plane, placement)) => self.new_sketch(plane, placement),
                None => self.windows.plane_picker = true,
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
            for f in &body.tess.faces {
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

    fn menu_bar(&self, ui: &mut Ui, pending: &mut Vec<CommandId>) {
        egui::MenuBar::new().ui(ui, |ui| {
            let mut item = |ui: &mut Ui, cmd: CommandId| {
                if cmd.available() && menu_button(ui, cmd, self.command_state(cmd)) {
                    pending.push(cmd);
                    ui.close();
                }
            };
            ui.menu_button("File", |ui| {
                item(ui, CommandId::NewDocument);
                item(ui, CommandId::OpenDocument);
                item(ui, CommandId::OpenSample);
                ui.separator();
                item(ui, CommandId::SaveDocument);
                item(ui, CommandId::SaveDocumentAs);
                ui.separator();
                item(ui, CommandId::ExportStl);
                ui.separator();
                item(ui, CommandId::Settings);
                if CommandId::Quit.available() {
                    ui.separator();
                    item(ui, CommandId::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                let undo = self.doc.undo_label().filter(|_| self.sketch.is_none());
                if let Some(label) = undo {
                    ui.weak(format!("Undo: {label}"));
                }
                if let Some(label) = self.doc.redo_label().filter(|_| self.sketch.is_none()) {
                    ui.weak(format!("Redo: {label}"));
                }
                item(ui, CommandId::Undo);
                item(ui, CommandId::Redo);
                ui.separator();
                item(ui, CommandId::DeleteSelection);
                item(ui, CommandId::ToggleSuppress);
                item(ui, CommandId::RollToEnd);
            });
            ui.menu_button("Sketch", |ui| {
                item(ui, CommandId::NewSketch);
                item(ui, CommandId::EditSketch);
                item(ui, CommandId::ExitSketch);
                ui.separator();
                for cmd in SKETCH_TOOLS {
                    item(ui, cmd);
                }
                ui.separator();
                for cmd in SKETCH_EDITS {
                    item(ui, cmd);
                }
                ui.menu_button("Relations", |ui| {
                    for cmd in RELATIONS {
                        item(ui, cmd);
                    }
                });
                ui.separator();
                item(ui, CommandId::ToggleConstruction);
                item(ui, CommandId::ToggleRelations);
            });
            ui.menu_button("Features", |ui| {
                item(ui, CommandId::Extrude);
                item(ui, CommandId::CutExtrude);
                ui.separator();
                ui.menu_button("Reference Geometry", |ui| {
                    for cmd in REFERENCES {
                        item(ui, cmd);
                    }
                });
            });
            ui.menu_button("Tools", |ui| {
                item(ui, CommandId::Parameters);
            });
            ui.menu_button("View", |ui| {
                ui.menu_button("Standard Views", |ui| {
                    for cmd in [
                        CommandId::ViewIsometric,
                        CommandId::ViewFront,
                        CommandId::ViewTop,
                        CommandId::ViewRight,
                        CommandId::ViewBack,
                        CommandId::ViewBottom,
                        CommandId::ViewLeft,
                    ] {
                        item(ui, cmd);
                    }
                });
                item(ui, CommandId::ZoomToFit);
                ui.separator();
                item(ui, CommandId::ToggleProjection);
                item(ui, CommandId::ToggleGrid);
                item(ui, CommandId::ToggleReferencePlanes);
                item(ui, CommandId::ToggleViewCube);
            });
            ui.menu_button("Window", |ui| {
                item(ui, CommandId::ToggleFeatureTree);
                item(ui, CommandId::ToggleProperties);
                item(ui, CommandId::TogglePerfOverlay);
            });
            ui.menu_button("Help", |ui| {
                item(ui, CommandId::CommandPalette);
                item(ui, CommandId::KeyboardShortcuts);
                ui.separator();
                item(ui, CommandId::About);
            });
        });
    }

    fn toolbar(&self, ui: &mut Ui, pending: &mut Vec<CommandId>) {
        if self.sketch.is_some() {
            self.sketch_toolbar(ui, pending);
            return;
        }
        ui.horizontal(|ui| {
            let tool = |ui: &mut Ui, pending: &mut Vec<CommandId>, cmd: CommandId, label: &str| {
                if tool_button(ui, cmd, label, self.command_state(cmd)) {
                    pending.push(cmd);
                }
            };
            tool(ui, pending, CommandId::SaveDocument, "Save");
            tool(ui, pending, CommandId::Undo, "Undo");
            tool(ui, pending, CommandId::Redo, "Redo");
            ui.separator();
            ui.weak("View");
            tool(ui, pending, CommandId::ViewIsometric, "Iso");
            tool(ui, pending, CommandId::ViewFront, "Front");
            tool(ui, pending, CommandId::ViewTop, "Top");
            tool(ui, pending, CommandId::ViewRight, "Right");
            tool(ui, pending, CommandId::ZoomToFit, "Fit");
            ui.separator();
            tool(ui, pending, CommandId::ToggleProjection, "Perspective");
            tool(ui, pending, CommandId::ToggleGrid, "Grid");
            tool(ui, pending, CommandId::ToggleReferencePlanes, "Planes");
            ui.separator();
            ui.weak("Sketch");
            tool(ui, pending, CommandId::NewSketch, "New Sketch");
            tool(ui, pending, CommandId::EditSketch, "Edit");
            tool(ui, pending, CommandId::Parameters, "Parameters");
            ui.separator();
            ui.weak("Features");
            tool(ui, pending, CommandId::Extrude, "Extrude");
            tool(ui, pending, CommandId::CutExtrude, "Cut");
            ui.menu_button("Reference", |ui| {
                for cmd in REFERENCES {
                    if menu_button(ui, cmd, self.command_state(cmd)) {
                        pending.push(cmd);
                        ui.close();
                    }
                }
            });
            ui.separator();
            tool(ui, pending, CommandId::ExportStl, "Export STL");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let palette = CommandId::CommandPalette;
                let label = format!(
                    "Commands  {}",
                    ui.ctx()
                        .format_shortcut(&palette.info().shortcut.expect("has shortcut"))
                );
                if ui
                    .button(label)
                    .on_hover_text(palette.info().description)
                    .clicked()
                {
                    pending.push(palette);
                }
            });
        });
    }

    fn sketch_toolbar(&self, ui: &mut Ui, pending: &mut Vec<CommandId>) {
        ui.horizontal_wrapped(|ui| {
            let tool = |ui: &mut Ui, pending: &mut Vec<CommandId>, cmd: CommandId, label: &str| {
                if tool_button(ui, cmd, label, self.command_state(cmd)) {
                    pending.push(cmd);
                }
            };
            tool(ui, pending, CommandId::ExitSketch, "✔ Exit Sketch");
            tool(ui, pending, CommandId::Extrude, "Extrude");
            tool(ui, pending, CommandId::CutExtrude, "Cut");
            ui.separator();
            for (cmd, label) in [
                (CommandId::SketchSelect, "Select"),
                (CommandId::SketchLine, "Line"),
                (CommandId::SketchRectangle, "Rectangle"),
                (CommandId::SketchCenterRectangle, "Ctr Rect"),
                (CommandId::SketchCircle, "Circle"),
                (CommandId::SketchArc, "Arc"),
                (CommandId::SketchSlot, "Slot"),
                (CommandId::SketchPolygon, "Polygon"),
                (CommandId::SketchPoint, "Point"),
            ] {
                tool(ui, pending, cmd, label);
            }
            ui.separator();
            tool(ui, pending, CommandId::SmartDimension, "Dimension");
            ui.separator();
            for (cmd, label) in [
                (CommandId::SketchTrim, "Trim"),
                (CommandId::SketchExtend, "Extend"),
                (CommandId::SketchFillet, "Fillet"),
                (CommandId::SketchOffset, "Offset"),
                (CommandId::SketchMirror, "Mirror"),
            ] {
                tool(ui, pending, cmd, label);
            }
            ui.separator();
            tool(ui, pending, CommandId::ToggleConstruction, "Construction");
            ui.menu_button("Relations", |ui| {
                for cmd in RELATIONS {
                    if menu_button(ui, cmd, self.command_state(cmd)) {
                        pending.push(cmd);
                        ui.close();
                    }
                }
            });
            ui.separator();
            tool(ui, pending, CommandId::Undo, "Undo");
            tool(ui, pending, CommandId::Redo, "Redo");
            ui.separator();
            tool(ui, pending, CommandId::ZoomToFit, "Fit");
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
            let display_error = self.doc.bodies.iter().find_map(|b| b.error.as_deref());
            if let Some((_, slot)) = self.picking {
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

    fn geometry_properties(&self, ui: &mut Ui) {
        let units = self.doc.model.parameters.units;
        for g in &self.selected_geom {
            let Some(body) = self.doc.bodies.get(g.body()) else {
                continue;
            };
            match *g {
                GeomRef::Face { face, .. } => {
                    let f = body.solid.face(face);
                    match f.surface {
                        Surface::Plane(_) => ui.label("Planar face"),
                        Surface::Cylinder(c) => ui.label(format!(
                            "Cylindrical face, R{}",
                            units.format_length(c.radius)
                        )),
                    };
                    ui.weak(capitalized(
                        &self.doc.model.describe_face(body.face_name(face)),
                    ));
                }
                GeomRef::Vertex { vertex, .. } => {
                    let p = body.solid.vertex(vertex).point;
                    ui.label(format!(
                        "Vertex at {}, {}, {}",
                        units.format_length_value(p.x),
                        units.format_length_value(p.y),
                        units.format_length_value(p.z)
                    ));
                }
                GeomRef::Edge { edge, .. } => {
                    let e = body.solid.edge(edge);
                    let len = body
                        .tess
                        .edges
                        .iter()
                        .find(|p| p.edge == edge)
                        .map(|p| {
                            p.points
                                .windows(2)
                                .map(|w| w[0].distance(w[1]))
                                .sum::<f64>()
                        })
                        .unwrap_or(0.0);
                    let kind = match e.curve {
                        peet_kernel::Curve3::Line(_) => "Line edge",
                        peet_kernel::Curve3::Circle(_) => "Circular edge",
                        peet_kernel::Curve3::Ellipse(_) => "Elliptical edge",
                    };
                    ui.label(format!("{kind}, length {}", units.format_length(len)));
                }
            }
        }
        ui.add_space(6.0);
        if self.selected_face().is_some() {
            ui.weak("Press S to sketch on this face, or add a reference plane from it.");
        }
    }

    // ---- Windows ----

    fn plane_picker_window(&mut self, ctx: &egui::Context) {
        let mut open = self.windows.plane_picker;
        let mut picked = None;
        egui::Window::new("New Sketch")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Choose the plane to sketch on:");
                ui.horizontal(|ui| {
                    for plane in StdPlane::ALL {
                        if ui.button(plane.label()).clicked() {
                            picked = Some(plane);
                        }
                    }
                });
                ui.weak("Tip: select a plane or a flat face first to skip this step.");
            });
        self.windows.plane_picker = open && picked.is_none();
        if let Some(plane) = picked {
            self.new_sketch(PlaneRef::Standard(plane), plane.plane());
        }
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

const SKETCH_TOOLS: [CommandId; 13] = [
    CommandId::SketchSelect,
    CommandId::SketchLine,
    CommandId::SketchRectangle,
    CommandId::SketchCenterRectangle,
    CommandId::SketchCircle,
    CommandId::SketchArc,
    CommandId::SketchSlot,
    CommandId::SketchPolygon,
    CommandId::SketchPoint,
    CommandId::SmartDimension,
    CommandId::SketchTrim,
    CommandId::SketchExtend,
    CommandId::SketchFillet,
];

const SKETCH_EDITS: [CommandId; 2] = [CommandId::SketchOffset, CommandId::SketchMirror];

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
fn tool_button(ui: &mut Ui, cmd: CommandId, label: &str, state: CommandState) -> bool {
    let info = cmd.info();
    let button = match state.checked {
        Some(checked) => egui::Button::selectable(checked, label),
        None => egui::Button::new(label),
    };
    let tip = match info.shortcut {
        Some(sc) => format!(
            "{}\n\nShortcut: {}",
            info.description,
            ui.ctx().format_shortcut(&sc)
        ),
        None => info.description.to_owned(),
    };
    ui.add_enabled(state.enabled, button)
        .on_hover_text(tip)
        .clicked()
}

impl eframe::App for PeetApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.perf.begin_frame();
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.poll_files(&ctx);
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
        if self.picking.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.picking = None;
        }
        self.hovered = None;

        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui, &mut pending));
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            self.toolbar(ui, &mut pending);
            ui.add_space(2.0);
        });
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
                },
            );
            self.hovered_geom = viewport.hovered_geom;
            if let (Some(editor), Some(work)) = (&mut self.sketch, &mut self.sketch_work) {
                if let Some(response) = &events.response {
                    editor.show(ui, response, &viewport.camera, work, &self.doc.model.parameters, dark);
                }
            } else if events.clicked_background {
                let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                if self.picking.is_some() {
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

        if let Some((geom, plane)) = pick_click {
            let picked = self.picked(geom, plane);
            self.finish_pick(picked);
        }
        self.settings_window(&ctx);
        self.shortcuts_window(&ctx);
        self.about_window(&ctx);
        self.plane_picker_window(&ctx);
        self.parameters_window(&ctx);
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
