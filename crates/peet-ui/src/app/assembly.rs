//! The application's side of assemblies: the component tree, a component's properties,
//! and the commands that start an assembly, insert a part and open a component's part.
//!
//! What these do to the assembly is done by operations (`peet_ops`), so a script does
//! the same; this file turns clicks into them and shows the results.

use egui::{RichText, Ui};
use peet_math::{DQuat, DVec3};
use peet_model::{CompId, MateEnd, MateGeom, MateId, MateKind, ScalarKind, Status};
use peet_ops::{
    CompSel, ComponentChange, Input, InsertSource, MateChange, MateEndSel, MateType, Op, Placing,
    Source,
};

use crate::bodies::GeomRef;
use crate::document::Document;

use super::PeetApp;
use super::scripting::{Coverage, coverage};
use crate::commands::CommandId;
use crate::features_ui::{ERROR, WARNING};

/// Undo keys of the drags in a component's properties (one step per drag).
const MOVE_KEY: u64 = 0x636f_6d70_6d6f_7665;

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
    let frame = doc.placed.get(body)?.frame;
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
            | CommandId::EditComponent
            | CommandId::MateCoincident
            | CommandId::MateConcentric
            | CommandId::MateParallel
            | CommandId::MateDistance
            | CommandId::MateAngle
            | CommandId::MateFasten
    )
}

impl PeetApp {
    /// The selected component, if the document is an assembly that still has it.
    pub(super) fn selected_component(&self) -> Option<CompId> {
        let id = self.selected_component?;
        self.doc.model.assembly()?.component(id).map(|c| c.id)
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
        let freedom = self.doc.evaluation().freedom;
        ui.weak(match freedom {
            0 => "Fully held: nothing can move.".to_owned(),
            1 => "1 way left to move.".to_owned(),
            n => format!("{n} ways left to move."),
        })
        .on_hover_text("How many ways the components can still move: six for each that is not fixed, less what the mates hold.");
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
        for action in actions {
            match action {
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
