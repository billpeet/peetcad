//! The application's side of assemblies: the component tree, a component's properties,
//! and the commands that start an assembly, insert a part and open a component's part.
//!
//! What these do to the assembly is done by operations (`peet_ops`), so a script does
//! the same; this file turns clicks into them and shows the results.

use egui::{RichText, Ui};
use peet_math::{DQuat, DVec3};
use peet_model::{CompId, ExplodeId, MateEnd, MateGeom, MateId, MateKind, ScalarKind, Status};
use peet_ops::{
    CompSel, ComponentChange, ExplodeChange, Input, InsertSource, MateChange, MateEndSel, MateType,
    Op, Placing, Point3, Source,
};

use crate::bodies::GeomRef;
use crate::document::Document;

use super::PeetApp;
use super::scripting::{Coverage, coverage};
use crate::commands::CommandId;
use crate::features_ui::{ERROR, WARNING};

/// Undo keys of the drags in a component's properties (one step per drag).
const MOVE_KEY: u64 = 0x636f_6d70_6d6f_7665;
/// The same for a component's colour and for an explode step's distance.
const COLOR_KEY: u64 = 0x636f_6d70_636f_6c72;
const EXPLODE_KEY: u64 = 0x6578_706c_6f64_6562;
/// How long the assembly takes to come apart and go together again, in seconds.
const EXPLODE_TIME: f64 = 0.4;

/// Undo key of a drag of a component in the view (one step per drag).
const DRAG_KEY: u64 = 0x636f_6d70_6472_6167;

/// A drag of a component in the view, in progress.
#[derive(Clone, Copy, Debug)]
pub(super) struct ComponentDrag {
    id: CompId,
    /// The point that was grabbed, in the component's coordinates.
    point: DVec3,
    /// The plane the pointer moves the point in: through where it was grabbed, facing
    /// the viewer.
    through: DVec3,
    normal: DVec3,
}

/// What dragging a component in the view asks for this frame.
pub(super) enum DragEvent {
    /// Pull the point of the component towards this place.
    Pull { id: CompId, point: DVec3, to: DVec3 },
    /// The drag ended: what it changed is one undo step.
    Done,
    /// The component under the pointer can't be dragged, and why.
    Refused(String),
}

/// Where a ray first meets a shown body, in the document's coordinates.
fn ray_hit(doc: &Document, body: usize, ray: &peet_math::Ray) -> Option<DVec3> {
    let frame = doc.placed.get(body)?.shown;
    let (origin, dir) = (
        frame.to_local(ray.origin),
        frame.vector_to_local(ray.direction),
    );
    let mut nearest: Option<f64> = None;
    for face in &doc.bodies.get(body)?.tess().faces {
        for t in &face.triangles {
            let [a, b, c] = t.map(|i| face.positions[i as usize]);
            // Möller–Trumbore.
            let (e1, e2) = (b - a, c - a);
            let p = dir.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-12 {
                continue;
            }
            let s = origin - a;
            let u = s.dot(p) / det;
            let q = s.cross(e1);
            let v = dir.dot(q) / det;
            let along = e2.dot(q) / det;
            if u >= 0.0 && v >= 0.0 && u + v <= 1.0 && along > 0.0 {
                nearest = Some(nearest.map_or(along, |n| n.min(along)));
            }
        }
    }
    nearest.map(|along| frame.to_world(origin + dir * along))
}

/// Follows the pointer dragging a component in the view: starts a drag when the left
/// button goes down on a component that can move, and while it lasts asks for the
/// grabbed point to be pulled to under the pointer.
pub(super) fn drag_in_view(
    ui: &Ui,
    viewport: &crate::viewport::Viewport,
    doc: &Document,
    response: &egui::Response,
    hovered: Option<GeomRef>,
    drag: &mut Option<ComponentDrag>,
) -> Option<DragEvent> {
    use egui::PointerButton::Primary;
    let Some(assembly) = doc.model.assembly() else {
        *drag = None;
        return None;
    };
    // With Alt the left button turns the view, in every mouse preset.
    let plain = ui.input(|i| !i.modifiers.alt);
    if response.drag_started_by(Primary) && plain {
        let geom = hovered?;
        let c = assembly.component(doc.placed.get(geom.body())?.component()?)?;
        if doc.explode() > 0.0 {
            return Some(DragEvent::Refused(
                "The assembly is shown exploded, and its components are not where they are drawn. Turn Explode off to move them."
                    .to_owned(),
            ));
        }
        if c.fixed {
            return Some(DragEvent::Refused(format!(
                "{} is fixed, so it stays where it is. Untick Fixed in its properties to move it.",
                c.name
            )));
        }
        let pressed = ui.input(|i| i.pointer.press_origin())?;
        let ray = viewport.ray_at(pressed);
        // Where the pointer is on the component; failing that, its middle.
        let grabbed = ray_hit(doc, geom.body(), &ray).unwrap_or_else(|| {
            let b = doc.component_bounds(c.id);
            (b.min + b.max) * 0.5
        });
        *drag = Some(ComponentDrag {
            id: c.id,
            point: c.placement.to_local(grabbed),
            through: grabbed,
            normal: ray.direction,
        });
    }
    let active = (*drag)?;
    if response.drag_stopped() || assembly.component(active.id).is_none() {
        *drag = None;
        return Some(DragEvent::Done);
    }
    if !response.dragged_by(Primary) {
        return None;
    }
    let ray = viewport.ray_at(response.interact_pointer_pos()?);
    let facing = ray.direction.dot(active.normal);
    if facing.abs() < 1e-9 {
        return None;
    }
    let along = (active.through - ray.origin).dot(active.normal) / facing;
    Some(DragEvent::Pull {
        id: active.id,
        point: active.point,
        to: ray.origin + ray.direction * along,
    })
}

/// What a click in the component tree asks for.
enum TreeAction {
    Select(CompId),
    Open(CompId),
    Change(CompId, ComponentChange),
    SelectMate(MateId),
    ChangeMate(MateId, MateChange),
    Isolate(CompId),
    ShowAll,
    SelectExplode(ExplodeId),
    ChangeExplode(ExplodeId, ExplodeChange),
}

/// What to say about how a component can still move.
fn freedom_text(fixed: bool, freedom: Option<usize>) -> Option<String> {
    Some(match (fixed, freedom?) {
        (true, _) => "Fixed: it stays where it is.".to_owned(),
        (false, 0) => "Fully held by its mates.".to_owned(),
        (false, 6) => "Free: nothing holds it yet.".to_owned(),
        (false, 1) => "It can still move 1 way.".to_owned(),
        (false, n) => format!("It can still move {n} ways."),
    })
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
                    | "import_step"
                    | "show_all"
                    | "isolate"
                    | "explode_step"
                    | "explode"
                    | "update_links"
                    | "interference"
                    | "bom"
                    | "mass"
                    | "open_component"
                    | "delete"
                    | "suppress"
                    | "mate"
            ) || cmd == CommandId::ExportStep
        }
    }
}

/// Whether a command works only in an assembly.
pub(super) fn needs_assembly(cmd: CommandId) -> bool {
    matches!(
        cmd,
        CommandId::InsertComponent
            | CommandId::InsertLinkedComponent
            | CommandId::UpdateLinks
            | CommandId::InterferenceCheck
            | CommandId::BillOfMaterials
            | CommandId::EditComponent
            | CommandId::MateCoincident
            | CommandId::MateConcentric
            | CommandId::MateParallel
            | CommandId::MateDistance
            | CommandId::MateAngle
            | CommandId::MateFasten
            | CommandId::ShowAllComponents
            | CommandId::IsolateComponent
            | CommandId::AddExplodeStep
            | CommandId::ExplodeView
    )
}

impl PeetApp {
    /// The selected component, if the document is an assembly that still has it.
    pub(super) fn selected_component(&self) -> Option<CompId> {
        let id = self.selected_component?;
        self.doc.model.assembly()?.component(id).map(|c| c.id)
    }

    /// The selected step of the exploded view, if the document is an assembly that
    /// still has it and nothing else was selected since.
    pub(super) fn selected_explode(&self) -> Option<ExplodeId> {
        let id = self.selected_explode?;
        if self.selected_component.is_some() || self.selected_mate.is_some() {
            return None;
        }
        self.doc.model.assembly()?.explode_step(id).map(|s| s.id)
    }

    /// Adds a step to the exploded view that moves the selected component, and shows
    /// the assembly exploded so that it is seen.
    pub(super) fn add_explode_step(&mut self) {
        let Some(id) = self.selected_component() else {
            return self.error("Select the component to move first.");
        };
        // A start, to be changed in the step's properties: up, by about half the size
        // of the assembly.
        let size = self.doc.visible_body_bounds().size().max_element();
        let by = if size.is_finite() {
            (size * 0.5).clamp(10.0, 1000.0).round()
        } else {
            50.0
        };
        let reply = self.perform(Op::ExplodeStep {
            components: vec![CompSel::Id(id)],
            by: Point3::Mm(DVec3::new(0.0, 0.0, by)),
            name: None,
        });
        if !reply.ok {
            return;
        }
        let step = &reply.json["explode_step"];
        self.selected_explode = step["id"].as_u64().map(|i| ExplodeId(i as u32));
        self.selected_component = None;
        self.selected_mate = None;
        self.selected_geom.clear();
        self.explode_target = Some(1.0);
        self.info(format!(
            "Added {}. Set how far and which way in its properties; the component itself stays where its mates have it.",
            step["name"].as_str().unwrap_or("the step")
        ));
    }

    /// Shows the assembly exploded, or as it is put together again.
    pub(super) fn toggle_explode(&mut self) {
        let exploded = self.explode_target.unwrap_or(self.doc.explode()) > 0.5;
        self.explode_target = Some(if exploded { 0.0 } else { 1.0 });
    }

    /// Moves the exploded view a frame's worth towards where it is going.
    pub(super) fn animate_explode(&mut self, ctx: &egui::Context) {
        let Some(target) = self.explode_target else {
            return;
        };
        let now = self.doc.explode();
        let step = if self.settings.animate_views {
            f64::from(ctx.input(|i| i.stable_dt)).clamp(0.001, 0.1) / EXPLODE_TIME
        } else {
            1.0
        };
        let next = if (target - now).abs() <= step {
            target
        } else {
            now + step * (target - now).signum()
        };
        self.doc.set_explode(next);
        if next == target {
            self.explode_target = None;
        } else {
            ctx.request_repaint();
        }
    }

    pub(super) fn change_explode(&mut self, id: ExplodeId, change: ExplodeChange) {
        let deleted = change == ExplodeChange::Delete;
        let reply = self.perform(Op::EditExplodeStep {
            step: id.into(),
            change,
        });
        if reply.ok && deleted && self.selected_explode == Some(id) {
            self.selected_explode = None;
        }
    }

    /// The properties of a step of the exploded view: how far it moves its components,
    /// and which they are.
    pub(super) fn explode_properties(&mut self, ui: &mut Ui, id: ExplodeId) {
        let Some(assembly) = self.doc.model.assembly() else {
            return;
        };
        let Some(step) = assembly.explode_step(id).cloned() else {
            return;
        };
        let units = self.doc.model.parameters.units;
        let inside: Vec<(CompId, String)> = step
            .components
            .iter()
            .map(|c| (*c, assembly.name_of(*c).to_owned()))
            .collect();
        let outside: Vec<(CompId, String)> = assembly
            .components()
            .filter(|c| !step.components.contains(&c.id))
            .map(|c| (c.id, c.name.clone()))
            .collect();
        let mut by = step.offset.to_array().map(|v| units.from_mm(v));
        let (mut moved, mut move_done) = (false, false);
        let mut remove = None;
        let mut add = None;
        let mut delete = false;
        ui.strong(&step.name);
        ui.weak("A step of the exploded view. The components stay where their mates have them: they are only shown moved while Explode is on.");
        egui::Grid::new("explode_props")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for (axis, value) in ["X", "Y", "Z"].iter().zip(&mut by) {
                    ui.label(*axis)
                        .on_hover_text("How far the components move, along the assembly's axes.");
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
                ui.label("Moves");
                ui.vertical(|ui| {
                    for (c, name) in &inside {
                        ui.horizontal(|ui| {
                            ui.label(name);
                            if inside.len() > 1
                                && ui
                                    .small_button("×")
                                    .on_hover_text("Take the component out of this step.")
                                    .clicked()
                            {
                                remove = Some(*c);
                            }
                        });
                    }
                    if !outside.is_empty() {
                        egui::ComboBox::from_id_salt("explode_add")
                            .selected_text("Add a component…")
                            .show_ui(ui, |ui| {
                                for (c, name) in &outside {
                                    if ui.selectable_label(false, name).clicked() {
                                        add = Some(*c);
                                    }
                                }
                            });
                    }
                });
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let exploded = self.explode_target.unwrap_or(self.doc.explode()) > 0.5;
            if ui
                .button(if exploded { "Collapse" } else { "Explode" })
                .clicked()
            {
                self.toggle_explode();
            }
            delete = ui.button("Delete Step").clicked();
        });

        if moved {
            let offset = DVec3::from_array(by.map(|v| units.to_mm(v)));
            self.change_model(
                &format!("Edit {}", step.name),
                Some(EXPLODE_KEY ^ u64::from(id.0)),
                |m| {
                    if let Some(s) = m.assembly_mut().and_then(|a| a.explode_step_mut(id)) {
                        s.offset = offset;
                    }
                },
            );
        }
        if move_done {
            self.doc.seal_history();
        }
        if remove.is_some() || add.is_some() {
            let components: Vec<CompSel> = step
                .components
                .iter()
                .copied()
                .filter(|c| Some(*c) != remove)
                .chain(add)
                .map(CompSel::Id)
                .collect();
            self.change_explode(
                id,
                ExplodeChange::Edit {
                    by: None,
                    components: Some(components),
                },
            );
        }
        if delete {
            self.change_explode(id, ExplodeChange::Delete);
        }
    }

    /// The selected mate, if the document is an assembly that still has it.
    pub(super) fn selected_mate(&self) -> Option<MateId> {
        let id = self.selected_mate?;
        self.doc.model.assembly()?.mate(id).map(|m| m.id)
    }

    /// What is selected as the end of a mate: a face, an edge or a corner, with the
    /// component it is on.
    fn mate_end(&self, geom: GeomRef) -> Option<MateEnd> {
        let placed = self.doc.placed.get(geom.body())?;
        if placed.path.is_empty() {
            return None;
        }
        let body = &self.doc.bodies.get(geom.body())?.source;
        Some(MateEnd {
            path: placed.path.clone(),
            geom: Some(match geom {
                GeomRef::Face { face, .. } => MateGeom::Face(body.face_ref(face)),
                GeomRef::Edge { edge, .. } => MateGeom::Edge(body.edge_ref(edge)?),
                GeomRef::Vertex { vertex, .. } => MateGeom::Vertex(body.vertex_ref(vertex)),
            }),
        })
    }

    /// The two ends of a mate to add: what is selected, if it is one thing on each of
    /// two components.
    pub(super) fn mate_ends(&self) -> Option<(MateEnd, MateEnd)> {
        let [a, b] = self.selected_geom[..] else {
            return None;
        };
        let (a, b) = (self.mate_end(a)?, self.mate_end(b)?);
        (a.component() != b.component()).then_some((a, b))
    }

    /// Adds the mate a command asks for between the two selected ends.
    pub(super) fn add_mate(&mut self, cmd: CommandId) {
        let Some((a, b)) = self.mate_ends() else {
            return self.error(
                "Select a face, an edge or a corner on each of two components first (click one, Shift-click the other).",
            );
        };
        let kind = match cmd {
            CommandId::MateCoincident => MateType::Coincident,
            CommandId::MateConcentric => MateType::Concentric,
            CommandId::MateParallel => MateType::Parallel,
            // Starting values, changed in the mate's properties.
            CommandId::MateDistance => MateType::Distance(Input::Base(10.0)),
            CommandId::MateAngle => MateType::Angle(Input::Base(90.0)),
            CommandId::MateFasten => MateType::Fasten(None),
            _ => return,
        };
        let reply = self.perform(Op::Mate {
            kind,
            a: MateEndSel::Ref(a),
            b: MateEndSel::Ref(b),
            flip: None,
            name: None,
        });
        if !reply.ok {
            return;
        }
        let mate = &reply.json["mate"];
        self.selected_mate = mate["id"].as_u64().map(|i| MateId(i as u32));
        self.selected_component = None;
        self.selected_geom.clear();
        let name = mate["name"].as_str().unwrap_or("the mate").to_owned();
        match mate["message"].as_str() {
            Some(message) => self.error(format!("{name}: {message}")),
            None => self.info(format!(
                "Added {name}. If a component went the wrong way round, tick Flip in the mate's properties."
            )),
        }
    }

    pub(super) fn change_mate(&mut self, id: MateId, change: MateChange) {
        let deleted = change == MateChange::Delete;
        let reply = self.perform(Op::EditMate {
            mate: id.into(),
            change,
        });
        if reply.ok && deleted && self.selected_mate == Some(id) {
            self.selected_mate = None;
        }
    }

    /// Carries out what dragging a component in the view asked for this frame.
    pub(super) fn apply_drag(&mut self, event: DragEvent) {
        match event {
            DragEvent::Refused(why) => self.info(why),
            DragEvent::Done => self.doc.seal_history(),
            DragEvent::Pull { id, point, to } => {
                self.selected_component = Some(id);
                self.selected_mate = None;
                // One key for the whole drag: its steps are one undo step.
                let reply = self.perform_merging(
                    Op::Drag {
                        component: CompSel::Id(id),
                        point: Some(peet_ops::Point3::Mm(point)),
                        to: peet_ops::Point3::Mm(to),
                    },
                    DRAG_KEY ^ u64::from(id.0),
                );
                if let Some(e) = reply.json["error"].as_str().filter(|_| !reply.ok) {
                    self.error(e.to_owned());
                }
            }
        }
    }

    /// Asks for a part file to insert as a component.
    ///
    /// `linked`: the component follows the file, instead of the assembly keeping a copy.
    pub(super) fn start_insert_component(&mut self, linked: bool) {
        self.inserting_linked = linked;
        self.inserting = Some(peet_platform::open_file(crate::files::FILTER));
    }

    /// Reads linked parts again when their files are changed by another program.
    pub(super) fn watch_links(&mut self, ctx: &egui::Context) {
        self.watch_links_at(ctx, peet_platform::Instant::now());
    }

    /// [`PeetApp::watch_links`], at a given time (files are read once they have been
    /// left alone for a moment).
    pub(super) fn watch_links_at(&mut self, ctx: &egui::Context, now: peet_platform::Instant) {
        if peet_platform::is_web() || !self.settings.watch_links {
            self.link_watch = crate::link_watch::LinkWatch::default();
            return;
        }
        // What is watched is worked out again when an assembly changes (every rebuild
        // has a revision of its own) or is opened or closed.
        let signature =
            self.doc
                .documents()
                .filter(|(_, d)| d.is_assembly())
                .fold(0u64, |sum, (id, d)| {
                    sum.wrapping_mul(31)
                        .wrapping_add(d.revision)
                        .wrapping_add(u64::from(id.0) << 32)
                });
        let session = &self.doc;
        self.link_watch.follow(
            signature,
            || {
                session
                    .documents()
                    .filter(|(_, d)| d.is_assembly())
                    .flat_map(|(_, d)| peet_ops::linked_files(d))
                    .collect()
            },
            ctx,
        );
        // Not in the middle of a drag: the assembly is being changed as it is.
        if self.component_drag.is_none() {
            let changed = self.link_watch.take_changed(now);
            if !changed.is_empty() {
                self.read_changed_links(ctx, &changed);
            }
        }
        if let Some(wait) = self.link_watch.wake_in() {
            ctx.request_repaint_after(wait);
        }
    }

    /// Reads again the linked parts whose files are among `changed`, in every open
    /// assembly that links to them.
    fn read_changed_links(&mut self, ctx: &egui::Context, changed: &[std::path::PathBuf]) {
        let linking: Vec<peet_document::DocId> = self
            .doc
            .documents()
            .filter(|(_, d)| {
                d.is_assembly()
                    && peet_ops::linked_files(d)
                        .iter()
                        .any(|f| changed.contains(f))
            })
            .map(|(id, _)| id)
            .collect();
        for id in linking {
            let reply = self.apply_op_to(
                ctx,
                Some(&peet_ops::DocSel::Id(id)),
                &Op::UpdateLinks,
                peet_ops::Undo::Step,
            );
            let updated: Vec<&str> = reply.json["updated"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .collect();
            if let Some(warning) = reply.json["warnings"][0].as_str() {
                self.error(warning.to_owned());
            } else if let Some(e) = reply.json["error"].as_str() {
                self.error(e.to_owned());
            } else if reply.changed {
                self.info(format!(
                    "{} changed on disk and was read again.",
                    updated.join(", ")
                ));
            }
        }
    }

    /// The components that run into each other, with how much and where. Worked out
    /// when the window opens and when the assembly has changed (not during a drag).
    pub(super) fn interference_window(&mut self, ctx: &egui::Context) {
        if !self.windows.interference {
            return;
        }
        if !self.doc.is_assembly() {
            self.windows.interference = false;
            return;
        }
        let revision = self.doc.revision;
        let stale = self
            .interference
            .as_ref()
            .is_none_or(|(r, _)| *r != revision);
        if stale && self.component_drag.is_none() {
            self.interference = Some((revision, self.doc.interferences(None)));
        }
        let units = self.doc.model.parameters.units;
        let mut open = true;
        let mut select = None;
        egui::Window::new("Interference Check")
            .open(&mut open)
            .resizable(true)
            .default_width(420.0)
            .show(ctx, |ui| {
                let Some((_, found)) = &self.interference else {
                    ui.weak("Working it out…");
                    return;
                };
                if found.found.is_empty() && found.unchecked.is_empty() {
                    ui.label(match found.compared {
                        0 => "Nothing interferes: no two components are in each other's space.".to_owned(),
                        1 => "Nothing interferes: the one pair of components close enough to compare only touches, or is apart.".to_owned(),
                        n => format!("Nothing interferes: the {n} pairs of components close enough to compare only touch, or are apart."),
                    });
                    return;
                }
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for hit in &found.found {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(ERROR, "⚠");
                            if ui
                                .link(format!("{} and {}", hit.a, hit.b))
                                .on_hover_text("Select the first of them.")
                                .clicked()
                            {
                                select = Some(hit.top[0]);
                            }
                            ui.label(format!(
                                "share {} around {}",
                                super::solids::volume_text(units, hit.volume),
                                super::solids::point_text(units, hit.bounds.center())
                            ));
                        });
                    }
                    for (a, b, why) in &found.unchecked {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(WARNING, "?");
                            ui.label(format!("{a} and {b} couldn't be compared: {why}"));
                        });
                    }
                });
                ui.add_space(4.0);
                ui.weak("Components that only touch (faces against each other, a pin in a hole of its size) do not interfere.");
            });
        if let Some(id) = select {
            self.selected_component = Some(id);
            self.selected_mate = None;
            self.selected_geom.clear();
        }
        self.windows.interference = open;
    }

    /// The parts of the assembly and how many of each.
    pub(super) fn bill_of_materials_window(&mut self, ctx: &egui::Context) {
        if !self.windows.bill_of_materials {
            return;
        }
        if !self.doc.is_assembly() {
            self.windows.bill_of_materials = false;
            return;
        }
        let units = self.doc.model.parameters.units;
        let rows = self.doc.bill_of_materials(!self.bom_top_level);
        let mut open = true;
        let mut top_level = self.bom_top_level;
        let mut export = false;
        let kg = |m: f64| format!("{} kg", super::solids::significant(m));
        egui::Window::new("Bill of Materials")
            .open(&mut open)
            .resizable(true)
            .default_width(620.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut top_level, "Sub-assemblies as one line")
                        .on_hover_text("Without it, the parts in sub-assemblies are counted with the rest.");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        export = ui
                            .add_enabled(!rows.is_empty(), egui::Button::new("Save as CSV…"))
                            .on_hover_text("Every part, through sub-assemblies, as a table a spreadsheet opens.")
                            .clicked();
                    });
                });
                ui.separator();
                if rows.is_empty() {
                    ui.weak("The assembly has no components yet.");
                    return;
                }
                egui::ScrollArea::both().max_height(420.0).show(ui, |ui| {
                    egui::Grid::new("bom")
                        .num_columns(7)
                        .spacing([14.0, 5.0])
                        .striped(true)
                        .show(ui, |ui| {
                            for head in ["Item", "Part", "Qty", "Material", "Mass each", "Mass total", "Sheet"] {
                                ui.strong(head);
                            }
                            ui.end_row();
                            for row in &rows {
                                ui.label(row.item.to_string());
                                ui.label(&row.part)
                                    .on_hover_text(row.components.join(", "));
                                ui.label(row.quantity.to_string());
                                ui.label(row.material.as_deref().unwrap_or("—"));
                                ui.label(row.mass.map_or_else(|| "—".to_owned(), kg));
                                ui.label(
                                    row.mass
                                        .map_or_else(|| "—".to_owned(), |m| kg(m * row.quantity as f64)),
                                );
                                ui.label(row.sheet.as_ref().map_or_else(String::new, |s| {
                                    format!(
                                        "{} thick, flat {} × {} {}",
                                        units.format_length_value(s.thickness),
                                        units.format_length_value(s.flat_size.x),
                                        units.format_length_value(s.flat_size.y),
                                        units.length.suffix()
                                    )
                                }));
                                ui.end_row();
                            }
                        });
                });
                let total: Option<f64> =
                    rows.iter().map(|r| r.mass.map(|m| m * r.quantity as f64)).sum();
                ui.add_space(4.0);
                match total {
                    Some(total) => {
                        ui.label(format!("Together: {}.", kg(total)));
                    }
                    None => {
                        ui.weak("A part with no material has no mass: open it (Edit Part) and choose one in Mass Properties.");
                    }
                }
            });
        self.bom_top_level = top_level;
        self.windows.bill_of_materials = open;
        if export {
            self.export_bill_of_materials();
        }
    }

    /// Saves the bill of materials as CSV.
    fn export_bill_of_materials(&mut self) {
        let bytes = match peet_ops::export_bytes(
            &self.doc,
            peet_ops::Format::Csv,
            None,
            peet_io::step::StepSchema::Ap214,
        ) {
            Ok((bytes, _)) => bytes,
            Err(e) => return self.error(e),
        };
        let title = self.doc.title();
        let stem = title.strip_suffix(".peet").unwrap_or(&title).to_owned();
        match peet_platform::save_file(&format!("{stem} BOM.csv"), ("CSV table", &["csv"]), &bytes)
        {
            Ok(peet_platform::SaveOutcome::Saved(to)) => {
                self.info(format!("Saved the bill of materials to {to}"));
            }
            Ok(peet_platform::SaveOutcome::Cancelled) => {}
            Err(e) => self.error(e),
        }
    }

    /// What the assembly weighs, component by component.
    pub(super) fn assembly_mass_window(&mut self, ctx: &egui::Context) {
        let units = self.doc.model.parameters.units;
        let mass = self.doc.assembly_mass();
        let mut open = true;
        let kg = |m: Option<f64>| {
            m.map_or_else(
                || "—".to_owned(),
                |m| format!("{} kg", super::solids::significant(m)),
            )
        };
        egui::Window::new("Mass Properties")
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                let Some(mass) = &mass else {
                    ui.weak("There is nothing to weigh: no component shows a body.");
                    return;
                };
                egui::Grid::new("assembly_mass")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("Mass");
                        ui.monospace(kg(mass.mass));
                        ui.end_row();
                        ui.label("Volume");
                        ui.monospace(super::solids::volume_text(units, mass.volume));
                        ui.end_row();
                        ui.label("Centre of gravity").on_hover_text(if mass.mass.is_some() {
                            "Of the mass, in the assembly's coordinates."
                        } else {
                            "Of the volume (as if everything were one material), in the assembly's coordinates."
                        });
                        ui.monospace(super::solids::point_text(units, mass.centroid));
                        ui.end_row();
                        let size = mass.bounds.size();
                        ui.label("Bounding box");
                        ui.monospace(format!(
                            "{} × {} × {} {}",
                            units.format_length_value(size.x),
                            units.format_length_value(size.y),
                            units.format_length_value(size.z),
                            units.length.suffix()
                        ));
                        ui.end_row();
                    });
                if !mass.without_material.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(WARNING, "⚠");
                        ui.label(format!(
                            "The mass is not known: {} {} no material. Open the part (Edit Part) and choose one in Mass Properties.",
                            mass.without_material.join(", "),
                            if mass.without_material.len() == 1 { "has" } else { "have" }
                        ));
                    });
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    egui::Grid::new("component_mass")
                        .num_columns(3)
                        .spacing([14.0, 4.0])
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("Component");
                            ui.strong("Mass");
                            ui.strong("Centre of gravity");
                            ui.end_row();
                            for c in &mass.components {
                                ui.label(&c.name);
                                ui.monospace(kg(c.mass));
                                ui.monospace(super::solids::point_text(units, c.centroid));
                                ui.end_row();
                            }
                        });
                });
            });
        self.windows.mass_properties = open;
    }

    /// Reads the assembly's linked parts from their files again.
    pub(super) fn update_links(&mut self) {
        let reply = self.perform(Op::UpdateLinks);
        if !reply.ok {
            return;
        }
        let updated: Vec<&str> = reply.json["updated"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect();
        match reply.json["warnings"][0].as_str() {
            Some(warning) => self.error(warning.to_owned()),
            None if updated.is_empty() => {
                self.info("The linked parts are as their files have them.");
            }
            None => self.info(format!("Read again: {}.", updated.join(", "))),
        }
    }

    /// Makes a linked part the assembly's own copy.
    fn unlink_part(&mut self, part: peet_model::DefId) {
        if self.perform(Op::Unlink { part: part.into() }).ok {
            self.info("The part is the assembly's own now: it no longer follows the file.");
        }
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
            link: self.inserting_linked,
            absolute: false,
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
        let freedom = self.doc.evaluation().freedom;
        ui.weak(match freedom {
            0 => "Fully held: nothing can move.".to_owned(),
            1 => "1 way left to move.".to_owned(),
            n => format!("{n} ways left to move."),
        })
        .on_hover_text("How many ways the components can still move: six for each that is not fixed, less what the mates hold.");
        let mut actions = Vec::new();
        let any_hidden = assembly.components().any(|c| !c.visible);
        let selected_explode = self.selected_explode();
        for c in assembly.components() {
            let status = self.doc.evaluation().component_status(c.id);
            // As in other CAD systems: (f) fixed, (-) can still move.
            let freedom = self.doc.evaluation().component_freedom(c.id);
            let mut text = RichText::new(format!(
                "{}{}{}",
                if c.fixed {
                    "(f) "
                } else if freedom.is_some_and(|f| f > 0) {
                    "(-) "
                } else {
                    ""
                },
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
            } else if let Some(text) = freedom_text(c.fixed, freedom) {
                r = r.on_hover_text(text);
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
                pick(ui, "Isolate", TreeAction::Isolate(c.id));
                if any_hidden {
                    pick(ui, "Show All", TreeAction::ShowAll);
                }
                ui.separator();
                pick(ui, "Delete", change(ComponentChange::Delete));
            });
        }
        if assembly.mates().len() > 0 {
            ui.add_space(6.0);
            ui.strong("Mates");
        }
        for m in assembly.mates() {
            let status = self.doc.evaluation().mate_status(m.id);
            let names = |end: &MateEnd| {
                end.component()
                    .map_or("?", |c| assembly.name_of(c))
                    .to_owned()
            };
            let mut text = RichText::new(format!("{} ({}, {})", m.name, names(&m.a), names(&m.b)));
            text = match status {
                Some(Status::Failed(_)) => text.color(ERROR),
                Some(Status::Warning(_)) => text.color(WARNING),
                Some(Status::Suppressed) => text.weak().strikethrough(),
                _ => text,
            };
            let mut r = ui.selectable_label(self.selected_mate == Some(m.id), text);
            if let Some(message) = status.and_then(Status::message) {
                r = r.on_hover_text(message);
            }
            if r.clicked() {
                actions.push(TreeAction::SelectMate(m.id));
            }
            r.context_menu(|ui| {
                let mut pick = |ui: &mut Ui, label: &str, change: MateChange| {
                    if ui.button(label).clicked() {
                        actions.push(TreeAction::ChangeMate(m.id, change));
                        ui.close();
                    }
                };
                pick(
                    ui,
                    "Flip",
                    MateChange::Edit {
                        value: None,
                        flip: Some(!m.flip),
                    },
                );
                pick(
                    ui,
                    if m.suppressed {
                        "Unsuppress"
                    } else {
                        "Suppress"
                    },
                    MateChange::Suppress(!m.suppressed),
                );
                ui.separator();
                pick(ui, "Delete", MateChange::Delete);
            });
        }
        if assembly.explode_steps().len() > 0 {
            ui.add_space(6.0);
            ui.strong("Exploded view");
        }
        for s in assembly.explode_steps() {
            let names: Vec<&str> = s.components.iter().map(|c| assembly.name_of(*c)).collect();
            let r = ui.selectable_label(
                selected_explode == Some(s.id),
                format!("{} ({})", s.name, names.join(", ")),
            );
            if r.clicked() {
                actions.push(TreeAction::SelectExplode(s.id));
            }
            r.context_menu(|ui| {
                if ui.button("Delete").clicked() {
                    actions.push(TreeAction::ChangeExplode(s.id, ExplodeChange::Delete));
                    ui.close();
                }
            });
        }
        for action in actions {
            match action {
                TreeAction::Isolate(id) => {
                    self.perform(Op::Isolate {
                        components: vec![CompSel::Id(id)],
                    });
                }
                TreeAction::ShowAll => {
                    self.perform(Op::ShowAll);
                }
                TreeAction::SelectExplode(id) => {
                    self.selected_explode = Some(id);
                    self.selected_component = None;
                    self.selected_mate = None;
                    self.selected = None;
                    self.selected_geom.clear();
                }
                TreeAction::ChangeExplode(id, change) => self.change_explode(id, change),
                TreeAction::SelectMate(id) => {
                    self.selected_mate = Some(id);
                    self.selected_component = None;
                    self.selected = None;
                    self.selected_geom.clear();
                }
                TreeAction::ChangeMate(id, change) => self.change_mate(id, change),
                TreeAction::Select(id) => {
                    self.selected_component = Some(id);
                    self.selected_mate = None;
                    self.selected = None;
                    self.selected_geom.clear();
                }
                TreeAction::Open(id) => self.open_component(id),
                TreeAction::Change(id, change) => self.change_component(id, change),
            }
        }
    }

    /// The properties of a mate: what it joins, its value, which way round, and whether
    /// it holds.
    pub(super) fn mate_properties(&mut self, ui: &mut Ui, id: MateId) {
        let Some(assembly) = self.doc.model.assembly() else {
            return;
        };
        let Some(mate) = assembly.mate(id).cloned() else {
            return;
        };
        let units = self.doc.model.parameters.units;
        let status = self.doc.evaluation().mate_status(id).cloned();
        let end = |end: &MateEnd| {
            format!(
                "{} of {}",
                match &end.geom {
                    Some(MateGeom::Face(_)) => "A face",
                    Some(MateGeom::Edge(_)) => "An edge",
                    Some(MateGeom::Vertex(_)) => "A corner",
                    None => "The whole",
                },
                end.component().map_or("?", |c| assembly.name_of(c))
            )
        };
        let (first, second) = (end(&mate.a), end(&mate.b));
        // The value in what the user sees: document units, or degrees.
        let params = &self.doc.model.parameters;
        let mut value = match &mate.kind {
            MateKind::Distance(s) => s
                .evaluate(ScalarKind::Length, params)
                .ok()
                .map(|mm| (units.from_mm(mm), format!(" {}", units.length.suffix()))),
            MateKind::Angle(s) => s
                .evaluate(ScalarKind::Angle, params)
                .ok()
                .map(|deg| (deg, "°".to_owned())),
            _ => None,
        };
        let was = value.as_ref().map(|v| v.0);
        let mut flip = mate.flip;
        ui.strong(&mate.name);
        egui::Grid::new("mate_props")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Type");
                ui.label(mate.kind.word());
                ui.end_row();
                ui.label("Between");
                ui.vertical(|ui| {
                    ui.label(&first);
                    ui.label(&second);
                });
                ui.end_row();
                if let Some((v, suffix)) = &mut value {
                    ui.label(if matches!(mate.kind, MateKind::Angle(_)) {
                        "Angle"
                    } else {
                        "Distance"
                    });
                    ui.add(
                        egui::DragValue::new(v)
                            .speed(0.5)
                            .max_decimals(4)
                            .suffix(suffix.clone()),
                    );
                    ui.end_row();
                }
                ui.label("Flip").on_hover_text(
                    "Two flat faces are put against each other. Flipped, they look the same way: two side faces flush.",
                );
                ui.checkbox(&mut flip, "");
                ui.end_row();
            });
        if let Some(message) = status.as_ref().and_then(Status::message) {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ERROR, "⚠");
                ui.label(message);
            });
        }
        let new = value.map(|v| v.0).filter(|v| Some(*v) != was);
        if new.is_some() || flip != mate.flip {
            self.change_mate(
                id,
                MateChange::Edit {
                    value: new.map(Input::Number),
                    flip: (flip != mate.flip).then_some(flip),
                },
            );
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
        // A linked part: the file it follows.
        let link = assembly
            .definition(c.definition)
            .and_then(|d| d.link.as_ref())
            .map(|l| l.path.clone());
        let mut unlink = false;
        let units = self.doc.model.parameters.units;
        let status = self.doc.evaluation().component_status(id).cloned();
        let freedom = freedom_text(c.fixed, self.doc.evaluation().component_freedom(id));

        // The name is typed into a copy and set when the field is left.
        let mut name = match &self.component_name {
            Some((editing, text)) if *editing == id => text.clone(),
            _ => c.name.clone(),
        };
        let mut origin = c.placement.origin.to_array().map(|v| units.from_mm(v));
        let mut fixed = c.fixed;
        let mut visible = c.visible;
        // A colour of its own, or its part's.
        let part_color = assembly
            .definition(c.definition)
            .and_then(|d| d.model.color)
            .unwrap_or([150, 160, 175]);
        let mut own_color = c.color.is_some();
        let mut rgb = c.color.unwrap_or(part_color);
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
                if let Some(path) = &link {
                    ui.label("Linked to").on_hover_text(
                        "The part is read from this file whenever the assembly is opened.",
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.label(path);
                        unlink = ui
                            .small_button("Make Own Copy")
                            .on_hover_text("Keep the part in the assembly as it is now, and stop following the file.")
                            .clicked();
                    });
                    ui.end_row();
                }
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
                ui.label("Shown").on_hover_text(
                    "A hidden component is not drawn, but is still part of the assembly: counted, weighed and exported.",
                );
                ui.checkbox(&mut visible, "");
                ui.end_row();
                ui.label("Colour").on_hover_text(
                    "A colour for this component alone, in place of its part's: to tell instances apart.",
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut own_color, "Own");
                    if own_color {
                        ui.color_edit_button_srgb(&mut rgb);
                    }
                });
                ui.end_row();
                if let Some(text) = &freedom {
                    ui.label("Freedom").on_hover_text(
                        "How many ways the component can still move, by itself or along with what it is mated to.",
                    );
                    ui.label(text);
                    ui.end_row();
                }
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
        if visible != c.visible {
            self.change_component(id, ComponentChange::Show(visible));
        }
        let color = own_color.then_some(rgb);
        if color != c.color {
            // The steps of one drag in the colour picker are one undo step.
            self.change_model(
                &format!("Colour {}", c.name),
                Some(COLOR_KEY ^ u64::from(id.0)),
                |m| {
                    if let Some(c) = m.assembly_mut().and_then(|a| a.component_mut(id)) {
                        c.color = color;
                    }
                },
            );
        }
        if open {
            self.open_component(id);
        }
        if unlink {
            self.unlink_part(c.definition);
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

    #[test]
    fn components_are_mated_from_the_selection() {
        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        app.execute(&ctx, CommandId::NewAssembly);
        ok(&mut app, &ctx, json!({"op": "insert", "sample": "bracket"}));
        ok(
            &mut app,
            &ctx,
            json!({"op": "insert", "component": "Bracket-1", "at": [10, 20, 90]}),
        );
        assert_eq!(app.doc.evaluation().freedom, 6);
        // Nothing selected, one face, two faces of one component: no mate yet.
        assert!(!app.command_state(CommandId::MateCoincident).enabled);
        let below = flat_face(&app, 0, DVec3::NEG_Z);
        app.selected_geom = vec![below];
        assert!(!app.command_state(CommandId::MateCoincident).enabled);
        app.selected_geom = vec![below, flat_face(&app, 0, DVec3::NEG_Y)];
        assert!(!app.command_state(CommandId::MateDistance).enabled);

        // The undersides of the two brackets, a distance apart.
        app.selected_geom = vec![below, flat_face(&app, 1, DVec3::NEG_Z)];
        assert!(app.command_state(CommandId::MateDistance).enabled);
        app.execute(&ctx, CommandId::MateDistance);
        let id = app.selected_mate().expect("the new mate is selected");
        assert!(app.selected_geom.is_empty());
        let status = |app: &PeetApp| app.doc.evaluation().mate_status(id).cloned();
        assert_eq!(status(&app), Some(Status::Ok));
        assert_eq!(app.doc.evaluation().freedom, 3);
        assert_eq!(app.doc.evaluation().component_freedom(CompId(2)), Some(3));
        assert_eq!(
            freedom_text(false, Some(3)).as_deref(),
            Some("It can still move 3 ways.")
        );
        assert_eq!(
            freedom_text(true, Some(0)).as_deref(),
            Some("Fixed: it stays where it is.")
        );
        assert_eq!(freedom_text(false, None), None);
        // Against each other, 10 apart: the second one is turned over, below the first.
        let second = app
            .doc
            .model
            .assembly()
            .unwrap()
            .component(CompId(2))
            .unwrap();
        assert!(
            second
                .placement
                .vector_to_world(DVec3::Z)
                .abs_diff_eq(DVec3::NEG_Z, 1e-9)
        );
        draw(&mut app, &ctx);

        // Its properties: flipped, they look the same way; the distance is changed.
        app.change_mate(
            id,
            MateChange::Edit {
                value: Some(Input::Number(25.0)),
                flip: Some(true),
            },
        );
        let second = app
            .doc
            .model
            .assembly()
            .unwrap()
            .component(CompId(2))
            .unwrap();
        assert!(
            second
                .placement
                .vector_to_world(DVec3::Z)
                .abs_diff_eq(DVec3::Z, 1e-9)
        );
        // 25 along the first face's normal, which points down.
        assert!((second.placement.origin.z + 25.0).abs() < 1e-8);
        draw(&mut app, &ctx);

        // Suppress and delete act on the selected mate.
        app.execute(&ctx, CommandId::ToggleSuppress);
        assert_eq!(status(&app), Some(Status::Suppressed));
        assert_eq!(
            app.command_state(CommandId::ToggleSuppress).checked,
            Some(true)
        );
        app.execute(&ctx, CommandId::ToggleSuppress);
        assert_eq!(status(&app), Some(Status::Ok));

        // Fastened as they are, nothing can move; a mate that can not be solved says why.
        app.selected_geom = vec![below, flat_face(&app, 1, DVec3::NEG_Z)];
        app.execute(&ctx, CommandId::MateFasten);
        assert_eq!(app.doc.evaluation().freedom, 0);
        app.selected_geom = vec![below, flat_face(&app, 1, DVec3::NEG_Z)];
        app.execute(&ctx, CommandId::MateConcentric);
        let wrong = app.selected_mate().unwrap();
        assert!(app.doc.evaluation().mate_status(wrong).unwrap().is_failed());
        assert!(app.status_message.as_ref().is_some_and(|(_, error)| *error));
        draw(&mut app, &ctx);
        app.execute(&ctx, CommandId::DeleteSelection);
        assert!(app.selected_mate().is_none());
        assert_eq!(app.doc.model.assembly().unwrap().mates().count(), 2);
        assert!(app.untranslated().is_empty(), "{:?}", app.untranslated());
    }

    #[test]
    fn a_component_is_dragged_in_the_view() {
        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        app.execute(&ctx, CommandId::NewAssembly);
        ok(&mut app, &ctx, json!({"op": "insert", "sample": "bracket"}));
        ok(
            &mut app,
            &ctx,
            json!({"op": "insert", "component": "Bracket-1", "at": [0, 150, 0]}),
        );
        // The pointer's ray finds the point it is on, on the instance where it is.
        let down = |x: f64, y: f64| peet_math::Ray {
            origin: DVec3::new(x, y, 500.0),
            direction: DVec3::NEG_Z,
        };
        let on_first = ray_hit(&app.doc, 0, &down(60.0, 40.0)).expect("over the first");
        assert!(
            on_first
                .truncate()
                .abs_diff_eq(peet_math::DVec2::new(60.0, 40.0), 1e-9)
        );
        assert!(on_first.z > 0.0);
        assert!(ray_hit(&app.doc, 0, &down(60.0, 190.0)).is_none());
        let grabbed = ray_hit(&app.doc, 1, &down(60.0, 190.0)).expect("over the second");
        assert!((grabbed.z - on_first.z).abs() < 1e-9);

        // Pulled along in steps, as the pointer moves: the grabbed point follows it.
        let second = CompId(2);
        let point = app
            .doc
            .model
            .assembly()
            .unwrap()
            .component(second)
            .unwrap()
            .placement
            .to_local(grabbed);
        let before = app.doc.model.clone();
        for step in 1..=6 {
            let to = grabbed + DVec3::new(12.0, -5.0, 0.0) * f64::from(step);
            app.apply_drag(DragEvent::Pull {
                id: second,
                point,
                to,
            });
        }
        app.apply_drag(DragEvent::Done);
        assert_eq!(app.selected_component(), Some(second));
        let c = app.doc.model.assembly().unwrap().component(second).unwrap();
        let to = grabbed + DVec3::new(72.0, -30.0, 0.0);
        assert!(c.placement.to_world(point).abs_diff_eq(to, 1e-9));
        assert_eq!(c.placement.rotation, peet_math::DQuat::IDENTITY);
        // The whole drag is one undo step, made by operations.
        assert_eq!(app.doc.undo_label(), Some("Drag Bracket-2"));
        app.execute(&ctx, CommandId::Undo);
        assert_eq!(app.doc.model, before);
        assert!(app.journal().iter().any(|op| op.word() == "drag"));
        assert!(app.untranslated().is_empty(), "{:?}", app.untranslated());

        // The first component is fixed: it says so, and stays.
        app.apply_drag(DragEvent::Pull {
            id: CompId(1),
            point: DVec3::ZERO,
            to: DVec3::new(50.0, 0.0, 0.0),
        });
        assert!(
            app.status_message
                .as_ref()
                .is_some_and(|(m, error)| *error && m.contains("is fixed"))
        );
        assert_eq!(app.doc.model, before);
    }

    #[test]
    fn linked_parts_are_inserted_updated_and_made_own() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!("peet-ui-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bracket.peet");
        let part = peet_document::Document::from_model(peet_model::samples::bracket().0, None);
        std::fs::write(&file, part.save_bytes(false).unwrap()).unwrap();

        let mut app = PeetApp::headless();
        // In a part, and in an assembly with nothing linked, there is nothing to update.
        assert!(!app.command_state(CommandId::InsertLinkedComponent).enabled);
        app.execute(&ctx, CommandId::NewAssembly);
        assert!(app.command_state(CommandId::InsertLinkedComponent).enabled);
        assert!(!app.command_state(CommandId::UpdateLinks).enabled);

        // What Insert Linked Part does once the file is chosen.
        let reply = app.perform(Op::Insert {
            from: InsertSource::File(Source::path(file.clone())),
            name: None,
            placing: None,
            fixed: None,
            link: true,
            absolute: false,
        });
        assert!(reply.ok, "{}", reply.json);
        app.selected_component = Some(CompId(1));
        assert!(app.command_state(CommandId::UpdateLinks).enabled);
        draw(&mut app, &ctx);
        app.execute(&ctx, CommandId::UpdateLinks);
        assert!(
            app.status_message
                .as_ref()
                .is_some_and(|(m, e)| !e && m.contains("as their files"))
        );

        // The part changed in its file: Update Linked Parts reads it.
        let mut changed = peet_model::samples::bracket().0;
        changed.color = Some([10, 20, 30]);
        let changed = peet_document::Document::from_model(changed, None);
        std::fs::write(&file, changed.save_bytes(false).unwrap()).unwrap();
        app.execute(&ctx, CommandId::UpdateLinks);
        let part = |app: &PeetApp| {
            let a = app.doc.model.assembly().unwrap();
            a.definitions().next().unwrap().clone()
        };
        assert_eq!(part(&app).model.color, Some([10, 20, 30]));
        assert_eq!(app.doc.placed[0].color, Some([10, 20, 30]));
        assert_eq!(app.doc.undo_label(), Some("Update Links"));

        // Make Own Copy: it keeps the part, and no longer follows the file.
        let id = part(&app).id;
        app.unlink_part(id);
        assert!(part(&app).link.is_none());
        assert!(!app.command_state(CommandId::UpdateLinks).enabled);
        draw(&mut app, &ctx);
        assert!(app.untranslated().is_empty(), "{:?}", app.untranslated());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_linked_part_changed_on_disk_is_read_again_by_itself() {
        use crate::link_watch::SETTLE;
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!("peet-ui-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bracket.peet");
        let write = |color: Option<[u8; 3]>| {
            let mut model = peet_model::samples::bracket().0;
            model.color = color;
            let part = peet_document::Document::from_model(model, None);
            std::fs::write(&file, part.save_bytes(false).unwrap()).unwrap();
        };
        write(None);
        let mut app = PeetApp::headless();
        app.execute(&ctx, CommandId::NewAssembly);
        let reply = app.perform(Op::Insert {
            from: InsertSource::File(Source::path(file.clone())),
            name: None,
            placing: None,
            fixed: None,
            link: true,
            absolute: false,
        });
        assert!(reply.ok, "{}", reply.json);
        let asm = app.doc.current_id();
        let color = |app: &PeetApp| app.doc.get(asm).unwrap().placed[0].color;

        // The system says when the file is written, and it is read once it has been left
        // alone (give it a few seconds). The assembly is in the background meanwhile: a
        // part is being worked on.
        app.watch_links(&ctx);
        assert!(app.link_watch.is_notified(), "the system's watcher runs");
        assert_eq!(app.doc.undo_label(), Some("Insert Bracket-1"));
        let steps = app.journal().len();
        ok(&mut app, &ctx, json!({"op": "new", "keep": true}));
        app.watch_links(&ctx);
        write(Some([1, 2, 3]));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while color(&app) != Some([1, 2, 3]) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            app.watch_links(&ctx);
        }
        assert_eq!(color(&app), Some([1, 2, 3]), "told by the system");
        assert!(!app.doc.is_assembly());
        assert_eq!(app.doc.get(asm).unwrap().undo_label(), Some("Update Links"));
        assert!(
            app.status_message
                .as_ref()
                .is_some_and(|(m, e)| !e && m.contains("changed on disk"))
        );
        // Once: nothing more happens to the file, and nothing more is done.
        let read = app.journal().len();
        assert!(read > steps);
        std::thread::sleep(SETTLE * 2);
        app.watch_links(&ctx);
        assert_eq!(app.journal().len(), read);

        // A file that was missing and is put back is seen arriving.
        std::fs::remove_file(&file).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !app.status_message.as_ref().is_some_and(|(_, e)| *e)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
            app.watch_links(&ctx);
        }
        assert!(
            app.status_message
                .as_ref()
                .is_some_and(|(m, e)| *e && m.contains("can't be found")),
            "{:?}",
            app.status_message
        );
        assert_eq!(color(&app), Some([1, 2, 3]), "as it was last read");
        write(Some([9, 9, 9]));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while color(&app) != Some([9, 9, 9]) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            app.watch_links(&ctx);
        }
        assert_eq!(color(&app), Some([9, 9, 9]), "put back");

        // Turned off in the settings, nothing is watched.
        app.settings.watch_links = false;
        app.watch_links(&ctx);
        assert!(!app.link_watch.is_notified());
        write(None);
        std::thread::sleep(std::time::Duration::from_millis(100));
        app.watch_links_at(&ctx, peet_platform::Instant::now() + SETTLE * 10);
        assert_eq!(color(&app), Some([9, 9, 9]));

        // Where the system can't watch, the files are looked at instead.
        app.settings.watch_links = true;
        app.link_watch.look_instead();
        let start = peet_platform::Instant::now();
        app.watch_links_at(&ctx, start);
        write(Some([7, 7, 7]));
        let second = std::time::Duration::from_secs(1);
        app.watch_links_at(&ctx, start + second);
        app.watch_links_at(&ctx, start + second + SETTLE);
        assert_eq!(color(&app), Some([7, 7, 7]));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn components_are_isolated_coloured_and_exploded() {
        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        for cmd in [
            CommandId::ShowAllComponents,
            CommandId::IsolateComponent,
            CommandId::AddExplodeStep,
            CommandId::ExplodeView,
        ] {
            assert!(!app.command_state(cmd).enabled, "{cmd:?} in a part");
        }
        app.execute(&ctx, CommandId::NewAssembly);
        ok(&mut app, &ctx, json!({"op": "insert", "sample": "bracket"}));
        ok(
            &mut app,
            &ctx,
            json!({"op": "insert", "component": "Bracket-1", "at": [0, 0, 60]}),
        );
        let (first, second) = (CompId(1), CompId(2));
        // Nothing selected, nothing hidden, no steps: all four are off.
        for cmd in [
            CommandId::ShowAllComponents,
            CommandId::IsolateComponent,
            CommandId::AddExplodeStep,
            CommandId::ExplodeView,
        ] {
            assert!(!app.command_state(cmd).enabled, "{cmd:?}");
        }

        // Isolate the selected component, and bring the other back.
        app.selected_component = Some(second);
        app.execute(&ctx, CommandId::IsolateComponent);
        let visible = |app: &PeetApp, id| {
            let assembly = app.doc.model.assembly().unwrap();
            assembly.component(id).unwrap().visible
        };
        assert!(!visible(&app, first) && visible(&app, second));
        assert_eq!(app.doc.undo_label(), Some("Isolate Bracket-2"));
        assert!(app.command_state(CommandId::ShowAllComponents).enabled);
        draw(&mut app, &ctx);
        app.execute(&ctx, CommandId::ShowAllComponents);
        assert!(visible(&app, first));
        assert!(!app.command_state(CommandId::ShowAllComponents).enabled);

        // A colour of its own, as its properties set it: an operation.
        let changed = app.change_model("Colour Bracket-2", Some(1), |m| {
            m.assembly_mut()
                .unwrap()
                .component_mut(second)
                .unwrap()
                .color = Some([9, 8, 7]);
        });
        assert!(changed);
        assert_eq!(app.doc.placed[1].color, Some([9, 8, 7]));
        assert_ne!(app.doc.placed[0].color, Some([9, 8, 7]));

        // An explode step for the selected component: added, selected, and shown.
        app.execute(&ctx, CommandId::AddExplodeStep);
        let step = app.selected_explode().expect("the new step is selected");
        assert!(app.selected_component().is_none());
        let offset = app.doc.model.assembly().unwrap().explode_offset(second);
        assert!(offset.z >= 10.0 && offset.x == 0.0, "{offset}");
        assert_eq!(app.doc.placed[1].frame.origin.z, 60.0);
        // It comes apart over a few frames, and nothing but the view changes.
        let model = app.doc.model.clone();
        assert_eq!(
            app.command_state(CommandId::ExplodeView).checked,
            Some(true)
        );
        let mut frames = 0;
        while app.explode_target.is_some() {
            app.animate_explode(&ctx);
            frames += 1;
            assert!(frames < 1000);
        }
        assert!(frames > 1, "animated");
        assert_eq!(app.doc.explode(), 1.0);
        assert_eq!(app.doc.placed[1].shown.origin.z, 60.0 + offset.z);
        assert_eq!(app.doc.placed[0].shown, app.doc.placed[0].frame);
        assert_eq!(app.doc.model, model);
        draw(&mut app, &ctx);

        // Its distance, as its properties change it; and without animation, at once.
        app.change_model("Edit Explode1", Some(2), |m| {
            let a = m.assembly_mut().unwrap();
            a.explode_step_mut(step).unwrap().offset = DVec3::new(25.0, 0.0, 0.0);
        });
        assert_eq!(app.doc.placed[1].shown.origin, DVec3::new(25.0, 0.0, 60.0));
        app.settings.animate_views = false;
        app.execute(&ctx, CommandId::ExplodeView);
        assert_eq!(
            app.command_state(CommandId::ExplodeView).checked,
            Some(false)
        );
        app.animate_explode(&ctx);
        assert_eq!(app.doc.explode(), 0.0);
        assert_eq!(app.doc.placed[1].shown, app.doc.placed[1].frame);

        // A script's `explode` shows it too, and the command sees it.
        ok(&mut app, &ctx, json!({"op": "explode"}));
        assert_eq!(
            app.command_state(CommandId::ExplodeView).checked,
            Some(true)
        );

        // Deleted from its properties: gone, and the command is off again.
        app.change_explode(step, ExplodeChange::Delete);
        assert!(app.selected_explode().is_none());
        assert!(!app.command_state(CommandId::ExplodeView).enabled);
        assert_eq!(app.doc.placed[1].shown, app.doc.placed[1].frame);
        draw(&mut app, &ctx);
    }

    #[test]
    fn an_assembly_is_checked_listed_and_weighed_in_its_windows() {
        let ctx = egui::Context::default();
        let mut app = PeetApp::headless();
        // A part has no bill of materials and nothing to interfere with.
        assert!(!app.command_state(CommandId::InterferenceCheck).enabled);
        assert!(!app.command_state(CommandId::BillOfMaterials).enabled);
        app.execute(&ctx, CommandId::NewAssembly);
        assert!(
            !app.command_state(CommandId::BillOfMaterials).enabled,
            "nothing in it yet"
        );
        ok(&mut app, &ctx, json!({"op": "insert", "sample": "bracket"}));
        // A second bracket half inside the first, and a sheet metal part clear of both.
        ok(
            &mut app,
            &ctx,
            json!({"op": "insert", "component": "Bracket-1", "at": [30, 0, 0]}),
        );
        ok(
            &mut app,
            &ctx,
            json!({"op": "insert", "sample": "enclosure", "at": [0, 400, 0]}),
        );
        for cmd in [
            CommandId::InterferenceCheck,
            CommandId::BillOfMaterials,
            CommandId::MassProperties,
        ] {
            assert!(app.command_state(cmd).enabled, "{cmd:?}");
            app.execute(&ctx, cmd);
        }
        let windows = |app: &mut PeetApp| {
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(1400.0, 900.0))
                .build_ui(|ui| {
                    let ctx = ui.ctx().clone();
                    app.interference_window(&ctx);
                    app.bill_of_materials_window(&ctx);
                    app.mass_properties_window(&ctx);
                });
            harness.run_steps(2);
        };
        windows(&mut app);
        let (_, found) = app
            .interference
            .as_ref()
            .expect("checked when the window opened");
        assert_eq!(found.found.len(), 1);
        assert_eq!(
            (found.found[0].a.as_str(), found.found[0].b.as_str()),
            ("Bracket-1", "Bracket-2")
        );
        assert!(found.found[0].volume > 1000.0);

        // Moved clear: checked again at the next frame, and nothing interferes.
        ok(
            &mut app,
            &ctx,
            json!({"op": "place", "component": "Bracket-2", "at": [0, -200, 0]}),
        );
        app.bom_top_level = true;
        windows(&mut app);
        assert!(app.interference.as_ref().unwrap().1.found.is_empty());
        let rows = app.doc.bill_of_materials(true);
        assert_eq!(rows.iter().map(|r| r.quantity).collect::<Vec<_>>(), [2, 1]);
        assert!(
            app.doc.assembly_mass().unwrap().mass.is_none(),
            "no materials yet"
        );

        // In a part the assembly windows close; Mass Properties is the part's again.
        app.selected_component = Some(CompId(1));
        app.execute(&ctx, CommandId::EditComponent);
        windows(&mut app);
        assert!(!app.windows.interference && !app.windows.bill_of_materials);
        assert!(app.windows.mass_properties);
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
            CommandId::ImportDxf,
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
