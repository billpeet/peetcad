//! Editing a part in the context of its assembly, as SolidWorks' Edit Part: the part's
//! component stays highlighted among the others, whose faces, edges and corners can be
//! picked to make the part follow them (`in_context`): a sketch converts what was picked,
//! a sketch is put on a picked face, an extrusion runs up to one, a reference plane is made
//! from one. The part's external references are listed, locked and broken from its
//! context menu and its properties (`external_references`, `lock_references`,
//! `break_references`).
//!
//! What these do is done by operations (`peet_ops`), so a script does the same.
//!
//! The part's sketches are made and edited in place, as in SolidWorks' Edit Part: the
//! sketch is the part's (a working copy, as for a part's own sketch), shown on its plane
//! where the component is in the assembly, with the other components around it. Clicking
//! another component's edge, face or corner where the sketch has nothing picks it, to
//! convert it or make a sketch entity follow it (`in_context`). A new sketch goes on one
//! of the part's planes or faces, or on another component's flat face, which its plane
//! then follows. Finishing the sketch writes it into the part, which the assembly
//! rebuilds. Meanwhile the other components are drawn see-through (see
//! `viewport::shown_transparency`).

use egui::{RichText, Ui};
use peet_math::{Frame, Plane};
use peet_model::in_context::{Holder, RefStatus, change_at, model_at};
use peet_model::{CompId, FeatureId, FeatureKind, MateEnd, MateGeom, PlaneDef, PlaneRef, StdPlane};
use peet_ops::{CompSel, FeatureSel, InContextOp, MadeIn, MateEndSel, Op};
use peet_sketch::EntityId;
use peet_sketch::expr::Parameters;

use super::{PeetApp, RibbonTab};
use crate::bodies::GeomRef;
use crate::document::{Document, ItemId, SketchItem};
use crate::features_ui::{ERROR, WARNING};
use crate::sketch_ui::SketchEditor;

/// The open sketch, when it is one of the part edited in context.
pub(super) struct SketchInContext {
    /// The component whose part it is.
    pub component: CompId,
    /// The sketch's name in its part.
    pub name: String,
    /// The part's parameters, which the sketch's dimensions can use.
    pub parameters: Parameters,
}

/// Where a new sketch of the part edited in context goes.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SketchOn {
    /// One of the part's own planes or flat faces, at `placement` (in the part's
    /// coordinates).
    Part(PlaneRef, Plane),
    /// Another component's flat face, which the sketch's plane follows (SolidWorks'
    /// in-context sketch plane). `placement` is where it is now, in the part's coordinates.
    Other(MateEnd, Plane),
}

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
            self.stop_in_context();
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
            "Editing {name} in context: pick a face, edge or corner of another component, then what of {name} follows it, in the properties. New Sketch, or Edit beside a sketch, sketches here."
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
                self.stop_in_context();
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
                ui.label(format!("Picked {}.", self.describe_end(end)));
            }
            Err(why) => {
                ui.weak(why);
            }
        }
        ui.add_space(4.0);
        // A new sketch of the part: on the picked flat face (the part's, or another
        // component's), or on one of the part's planes.
        let mut sketch_on = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("New sketch on");
            let on_face = self.picked_sketch_plane(own);
            let hint = match &on_face {
                Ok(SketchOn::Other(..)) => "A sketch of the part on the picked face of another component: its plane follows that face (an in-context sketch plane).".to_owned(),
                Ok(SketchOn::Part(..)) => "A sketch on the picked face of the part.".to_owned(),
                Err(why) => why.clone(),
            };
            if ui
                .add_enabled(on_face.is_ok(), egui::Button::new("the picked face"))
                .on_hover_text(hint)
                .clicked()
            {
                sketch_on = on_face.ok();
            }
            for p in StdPlane::ALL {
                if ui
                    .button(p.label())
                    .on_hover_text(format!("A sketch on the part's {} plane.", p.label()))
                    .clicked()
                {
                    sketch_on = Some(SketchOn::Part(PlaneRef::Standard(p), p.plane()));
                }
            }
        });
        if let Some(on) = sketch_on {
            self.new_sketch_in_context(own, on);
            return;
        }
        ui.add_space(4.0);
        let mut edit = None;
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
                        if f.sketch().is_some()
                            && ui
                                .button("Edit")
                                .on_hover_text("Edit the sketch here, in the assembly, with the other components around it.")
                                .clicked()
                        {
                            edit = Some(f.id);
                        }
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
        if let Some(id) = edit {
            self.open_sketch_in_context(own, id);
            return;
        }
        if let (Some((feature, how)), Ok(end)) = (chosen, picked) {
            self.follow_picked(own, feature, how, end);
        }
        ui.add_space(6.0);
        self.external_references_ui(ui, own);
    }

    /// What picked geometry is, in words: "an edge of Plate-1".
    fn describe_end(&self, end: &MateEnd) -> String {
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
        format!("{what} of {names}")
    }

    /// Stops editing in context, finishing the sketch that is open in it first.
    pub(super) fn stop_in_context(&mut self) {
        if self.sketch_in.is_some() {
            self.close_sketch();
        }
        self.in_context = None;
    }

    /// The name of the sketch open for editing (in its part, when it is edited in context).
    pub(super) fn open_sketch_name(&self) -> String {
        match (&self.sketch_in, &self.sketch) {
            (Some(inside), _) => inside.name.clone(),
            (None, Some(editor)) => self.doc.item_name(editor.item),
            (None, None) => String::new(),
        }
    }

    /// Where a component's part's coordinates are in the assembly: as it is shown
    /// (`shown`, moved by the exploded view), or as it is.
    fn component_frame(&self, id: CompId, shown: bool) -> Frame {
        self.doc
            .bodies_of(id)
            .next()
            .map(|i| {
                let p = &self.doc.placed[i];
                if shown { p.shown } else { p.frame }
            })
            .or_else(|| self.doc.evaluation().component_frame(&[id]))
            .unwrap_or(Frame::WORLD)
    }

    /// The flat face picked to put a new sketch of the part edited in context on: the
    /// part's own, or another component's.
    fn picked_sketch_plane(&self, own: CompId) -> Result<SketchOn, String> {
        let [geom] = self.selected_geom[..] else {
            return Err(
                "Pick a flat face (of the part, or of another component) to sketch on it."
                    .to_owned(),
            );
        };
        let GeomRef::Face { body, face } = geom else {
            return Err("Pick a flat face, not an edge or a corner.".to_owned());
        };
        let (Some(placed), Some(view)) = (self.doc.placed.get(body), self.doc.bodies.get(body))
        else {
            return Err("Pick a flat face.".to_owned());
        };
        let b = &view.source;
        let plane = peet_model::face_sketch_plane(&b.solid, face)
            .ok_or("That face is curved: pick a flat face.")?;
        if placed.component() == Some(own) {
            return Ok(SketchOn::Part(PlaneRef::Face(b.face_ref(face)), plane));
        }
        let end = self
            .mate_end(geom)
            .ok_or("Pick a flat face of a component.")?;
        // Where the face is, in the part's coordinates.
        let world = placed.frame.compose(&plane.frame);
        let local = self.component_frame(own, false).inverse().compose(&world);
        Ok(SketchOn::Other(end, Plane { frame: local }))
    }

    /// New Sketch while a part is edited in context: on the picked flat face.
    pub(super) fn new_sketch_on_picked(&mut self) {
        let Some(own) = self.in_context_component() else {
            return;
        };
        match self.picked_sketch_plane(own) {
            Ok(on) => self.new_sketch_in_context(own, on),
            Err(why) => self.error(format!(
                "{why} Or choose one of the part's planes in the properties."
            )),
        }
    }

    /// Adds a sketch to the part edited in context and opens it there.
    pub(super) fn new_sketch_in_context(&mut self, own: CompId, on: SketchOn) {
        self.close_sketch();
        let (plane, placement, follow) = match on {
            SketchOn::Part(plane, placement) => (plane, placement, None),
            // On the standard plane most nearly like the face for now, where the face is:
            // then the plane follows the face, keeping that place on it.
            SketchOn::Other(end, placement) => {
                let n = placement.normal();
                let nearest = StdPlane::ALL
                    .into_iter()
                    .max_by(|a, b| {
                        let a = a.plane().normal().dot(n).abs();
                        let b = b.plane().normal().dot(n).abs();
                        a.total_cmp(&b)
                    })
                    .unwrap_or(StdPlane::Front);
                (PlaneRef::Standard(nearest), placement, Some(end))
            }
        };
        let mut id = None;
        self.change("New Sketch", |m| {
            if let Some((changed, made)) = change_at(m, &[own], &mut |part| {
                part.add_sketch(plane.clone(), placement)
            }) {
                *m = changed;
                id = Some(made);
            }
        });
        let Some(id) = id else {
            return self.error("The part can't take a sketch here.");
        };
        if let Some(end) = follow {
            let reply = self.perform(Op::InContext(InContextOp::Make {
                component: vec![CompSel::Id(own)],
                target: MadeIn::Plane(FeatureSel::Id(id)),
                from: Box::new(MateEndSel::Ref(end)),
                entity: None,
                construction: true,
            }));
            if !reply.ok {
                return;
            }
        }
        self.open_sketch_in_context(own, id);
    }

    /// A sketch of a part, as a working copy on its plane where the component is shown in
    /// the assembly; with the part's parameters and the sketch's name, and how many of the
    /// edges of the face it is on could not be projected.
    fn sketch_work_in_context(
        &self,
        own: CompId,
        id: FeatureId,
    ) -> Option<(SketchItem, usize, Parameters, String)> {
        let part = model_at(&self.doc.model, &[own])?;
        let part = Document::from_model((*part).clone(), None);
        let (mut work, skipped) = SketchItem::from_feature(&part, id)?;
        let frame = self.component_frame(own, true);
        work.plane = Plane {
            frame: frame.compose(&work.plane.frame),
        };
        Some((
            work,
            skipped,
            part.model.parameters.clone(),
            part.model.name_of(id).to_owned(),
        ))
    }

    /// Opens a sketch of a component's part for editing in the assembly, editing the part
    /// in context if it wasn't.
    pub(super) fn open_sketch_in_context(&mut self, own: CompId, id: FeatureId) -> bool {
        self.close_sketch();
        let Some((mut work, skipped, parameters, name)) = self.sketch_work_in_context(own, id)
        else {
            self.error("That is not a sketch of the part.");
            return false;
        };
        let plane = work.plane;
        self.in_context = Some(own);
        self.sketch = Some(SketchEditor::new(ItemId::Feature(id), &mut work));
        self.sketch_work = Some(work);
        self.sketch_in = Some(SketchInContext {
            component: own,
            name: name.clone(),
            parameters,
        });
        self.ribbon_tab = RibbonTab::Sketch;
        self.selected_geom.clear();
        self.picking = None;
        if skipped > 0 {
            self.info(format!(
                "{skipped} face edges could not be projected. Sketch references support straight edges and circles parallel to the sketch plane."
            ));
        } else {
            let part = self
                .doc
                .model
                .assembly()
                .map(|a| a.name_of(own).to_owned())
                .unwrap_or_default();
            self.info(format!(
                "Editing {name} of {part} in context. Click another component's edge where the sketch has nothing to convert it, or to make the selected entity follow it."
            ));
        }
        if let Some(vp) = &mut self.viewport {
            let rotation =
                peet_render::camera::rotation_from_back_up(plane.normal(), plane.frame.y_axis());
            vp.set_rotation(rotation, self.settings.animate_views);
        }
        true
    }

    /// `edit_sketch` with a component: opens that component's part's sketch in context.
    pub(super) fn edit_sketch_in_context(
        &mut self,
        own: CompId,
        sketch: &FeatureSel,
    ) -> Result<String, String> {
        let assembly = self
            .doc
            .model
            .assembly()
            .ok_or("This is a part: 'component' is for an assembly.")?;
        if assembly
            .definition_of(own)
            .is_some_and(|d| d.model.is_assembly())
        {
            return Err(format!(
                "{} is a sub-assembly: give one of its parts' components.",
                assembly.name_of(own)
            ));
        }
        let part = model_at(&self.doc.model, &[own]).ok_or("The component's part is gone.")?;
        let part = Document::from_model((*part).clone(), None);
        let id = sketch.resolve(&part)?;
        if part.model.sketch(id).is_none() {
            return Err(format!("{} is not a sketch.", part.model.name_of(id)));
        }
        if !self.open_sketch_in_context(own, id) {
            return Err("The sketch could not be opened.".to_owned());
        }
        Ok(part.model.name_of(id).to_owned())
    }

    /// Writes the working copy of a sketch edited in context into its part, as one undo
    /// step: the assembly then rebuilds the part.
    pub(super) fn store_sketch_in_context(
        &mut self,
        inside: &SketchInContext,
        id: FeatureId,
        work: &SketchItem,
    ) -> bool {
        let projections: Vec<_> = work
            .projections
            .iter()
            .filter(|p| work.sketch.entity(p.entity).is_some())
            .cloned()
            .collect();
        let sketch = work.sketch.clone();
        let own = inside.component;
        self.change(&format!("Edit {}", inside.name), |m| {
            if let Some((changed, ())) = change_at(m, &[own], &mut |part| {
                if let Some(f) = part.feature_mut(id) {
                    if let Some(s) = f.sketch_mut() {
                        s.projections.clone_from(&projections);
                        s.sketch = sketch.clone();
                    }
                    f.visible = true;
                }
            }) {
                *m = changed;
            }
        })
    }

    /// Whether geometry under the cursor can be picked while a sketch is edited in
    /// context: another component's.
    pub(super) fn pickable_in_sketch(&self, geom: GeomRef) -> bool {
        let Some(own) = self.sketch_in.as_ref().map(|s| s.component) else {
            return false;
        };
        self.doc
            .placed
            .get(geom.body())
            .and_then(|p| p.component())
            .is_some_and(|c| c != own)
    }

    /// A click where the open in-context sketch has nothing: picks the other
    /// component's geometry under it, or nothing.
    pub(super) fn pick_in_sketch(&mut self, geom: Option<GeomRef>) {
        self.selected_geom = geom
            .filter(|g| self.pickable_in_sketch(*g))
            .into_iter()
            .collect();
    }

    /// Makes the sketch open in context follow what was picked on another component: a
    /// new entity converting it (`entity` left out), or the sketch's `entity`.
    fn follow_in_sketch(&mut self, end: MateEnd, entity: Option<EntityId>) {
        let (Some(inside), Some(editor), Some(work)) = (
            self.sketch_in.take(),
            &self.sketch,
            self.sketch_work.clone(),
        ) else {
            return;
        };
        let Some(id) = editor.item.feature() else {
            self.sketch_in = Some(inside);
            return;
        };
        let construction = editor.options.construction;
        let own = inside.component;
        // The operation works on the part: the sketch so far goes into it first.
        self.store_sketch_in_context(&inside, id, &work);
        self.sketch_in = Some(inside);
        let reply = self.perform(Op::InContext(InContextOp::Make {
            component: vec![CompSel::Id(own)],
            target: MadeIn::Sketch(FeatureSel::Id(id)),
            from: Box::new(MateEndSel::Ref(end)),
            entity: entity.map(|e| e.0),
            construction,
        }));
        // Back from the part, as the operation (and the rebuild) left it.
        if let Some((fresh, _, parameters, _)) = self.sketch_work_in_context(own, id)
            && let (Some(editor), Some(work)) = (&mut self.sketch, &mut self.sketch_work)
        {
            editor.edit_externally(work, |s| *s = fresh.sketch);
            work.projections = fresh.projections;
            work.plane = fresh.plane;
            work.plane_name = fresh.plane_name;
            if let Some(inside) = &mut self.sketch_in {
                inside.parameters = parameters;
            }
        }
        if reply.ok {
            self.selected_geom.clear();
            let what = if entity.is_some() {
                "The selected entity follows it"
            } else {
                "Converted it"
            };
            self.info(format!("{what}: it follows in this assembly."));
        }
    }

    /// The properties of a sketch open in context, above the sketch's own: what was
    /// picked on another component, and what the sketch can do with it.
    pub(super) fn in_context_sketch_properties(&mut self, ui: &mut Ui) {
        let Some(own) = self.sketch_in.as_ref().map(|s| s.component) else {
            return;
        };
        let part = self
            .doc
            .model
            .assembly()
            .map(|a| a.name_of(own).to_owned())
            .unwrap_or_default();
        ui.weak(format!("A sketch of {part}, edited in the assembly."));
        let picked = self.picked_in_context(own);
        let selected: Vec<EntityId> = self
            .sketch
            .as_ref()
            .map(|e| e.selected_entities())
            .unwrap_or_default();
        let mut follow = None;
        match &picked {
            Ok(end) => {
                ui.label(format!("Picked {}.", self.describe_end(end)));
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("Convert")
                        .on_hover_text("Add it to the sketch, seen on the sketch's plane, as an entity that follows it in this assembly (SolidWorks' Convert Entities in context). The Construction toggle makes it construction geometry.")
                        .clicked()
                    {
                        follow = Some((end.clone(), None));
                    }
                    let one = match selected[..] {
                        [e] => Some(e),
                        _ => None,
                    };
                    if ui
                        .add_enabled(one.is_some(), egui::Button::new("Selected follows it"))
                        .on_hover_text("The selected sketch entity becomes what was picked, seen on the sketch's plane, and follows it in this assembly. It must be of the same kind (a line for a straight edge, a circle or arc for a round one, a point for a corner).")
                        .clicked()
                    {
                        follow = one.map(|e| (end.clone(), Some(e)));
                    }
                });
            }
            Err(_) => {
                ui.weak("Click an edge, face or corner of another component where the sketch has nothing to convert it, or to make the selected entity follow it.");
            }
        }
        ui.separator();
        if let Some((end, entity)) = follow {
            self.follow_in_sketch(end, entity);
        }
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

    /// A plate 100 × 60 × 5 and a cover 40 × 20 × 2 on it, both fixed, as the skill's
    /// example has them.
    fn plate_and_cover(ctx: &egui::Context) -> (PeetApp, CompId, CompId) {
        let mut app = PeetApp::headless();
        for op in [
            json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}),
            json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}),
            json!({"op": "new", "keep": true}),
            json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 20]}]}),
            json!({"op": "extrude", "sketch": "Sketch1", "depth": 2}),
            json!({"op": "new", "assembly": true, "keep": true}),
            json!({"op": "insert", "part": 1, "name": "Plate", "fixed": true}),
            json!({"op": "insert", "part": 2, "name": "Cover", "at": [0, 0, 5], "fixed": true}),
        ] {
            ok(&mut app, ctx, op);
        }
        let assembly = app.doc.model.assembly().unwrap();
        let id = |name: &str| assembly.components().find(|c| c.name == name).unwrap().id;
        let (plate, cover) = (id("Plate"), id("Cover"));
        (app, plate, cover)
    }

    /// The straight edge of a component's body from `a` to `b` (either way), in its part's
    /// coordinates.
    fn straight_edge(app: &PeetApp, component: CompId, a: DVec3, b: DVec3) -> GeomRef {
        let body = app.doc.bodies_of(component).next().unwrap();
        let solid = &app.doc.bodies[body].solid;
        let edge = solid
            .edge_ids()
            .find(|e| {
                let e = solid.edge(*e);
                let (p, q) = (solid.vertex(e.start).point, solid.vertex(e.end).point);
                (p.abs_diff_eq(a, 1e-6) && q.abs_diff_eq(b, 1e-6))
                    || (p.abs_diff_eq(b, 1e-6) && q.abs_diff_eq(a, 1e-6))
            })
            .expect("an edge there");
        GeomRef::Edge { body, edge }
    }

    /// The cover's part as the assembly has it.
    fn cover_part(app: &PeetApp, cover: CompId) -> peet_model::Model {
        (*model_at(&app.doc.model, &[cover]).unwrap()).clone()
    }

    #[test]
    fn a_part_is_sketched_in_the_assembly() {
        let ctx = egui::Context::default();
        let (mut app, plate, cover) = plate_and_cover(&ctx);
        app.selected_component = Some(cover);
        app.execute(&ctx, CommandId::EditInContext);
        // Nothing picked: New Sketch says what to pick, and opens nothing.
        assert!(app.command_state(CommandId::NewSketch).enabled);
        app.execute(&ctx, CommandId::NewSketch);
        assert!(app.sketch.is_none());

        // The cover's sketch, opened in place: on its plane where the cover is.
        let sketch = cover_part(&app, cover)
            .features()
            .find(|f| f.name == "Sketch1")
            .unwrap()
            .id;
        assert!(app.open_sketch_in_context(cover, sketch));
        assert_eq!(app.open_sketch_name(), "Sketch1");
        assert_eq!(app.ribbon_tab, RibbonTab::Sketch);
        let work = app.sketch_work.as_ref().unwrap();
        assert!(
            (work.plane.origin().z - 5.0).abs() < 1e-9,
            "{:?}",
            work.plane
        );
        let entities = work.sketch.entities().count();
        draw(&mut app);

        // The plate's edges can be picked; the cover's own can't.
        let right = straight_edge(
            &app,
            plate,
            DVec3::new(100.0, 0.0, 5.0),
            DVec3::new(100.0, 60.0, 5.0),
        );
        assert!(app.pickable_in_sketch(right));
        let own = straight_edge(
            &app,
            cover,
            DVec3::new(40.0, 0.0, 2.0),
            DVec3::new(40.0, 20.0, 2.0),
        );
        assert!(!app.pickable_in_sketch(own));
        app.pick_in_sketch(Some(own));
        assert!(app.selected_geom.is_empty());
        let back = straight_edge(
            &app,
            plate,
            DVec3::new(0.0, 60.0, 5.0),
            DVec3::new(100.0, 60.0, 5.0),
        );
        app.pick_in_sketch(Some(back));
        assert_eq!(app.selected_geom, [back]);
        draw(&mut app);

        // Converted: a new entity of the sketch that follows the plate's back edge.
        let end = app.picked_in_context(cover).unwrap();
        app.follow_in_sketch(end, None);
        assert!(app.sketch.is_some(), "still sketching");
        assert!(app.selected_geom.is_empty(), "{:?}", app.status_message);
        let work = app.sketch_work.as_ref().unwrap();
        assert!(work.sketch.entities().count() > entities);
        assert_eq!(app.reference_rows(cover).len(), 1);

        // The rectangle's right side follows the plate's right edge: the cover is as
        // wide as the plate once the sketch is finished.
        app.pick_in_sketch(Some(right));
        let end = app.picked_in_context(cover).unwrap();
        app.sketch.as_mut().unwrap().selection = vec![crate::sketch_ui::Sel::Entity(EntityId(4))];
        draw(&mut app);
        app.follow_in_sketch(end, Some(EntityId(4)));
        assert_eq!(app.reference_rows(cover).len(), 2);
        app.execute(&ctx, CommandId::ExitSketch);
        assert!(app.sketch.is_none() && app.sketch_in.is_none());
        assert_eq!(app.in_context_component(), Some(cover), "still in context");
        let width = app.doc.component_bounds(cover).size().x;
        assert!((width - 100.0).abs() < 1e-6, "{width}");
        assert!(app.untranslated().is_empty(), "{:?}", app.untranslated());
        // The plate moves: the cover follows it.
        ok(
            &mut app,
            &ctx,
            json!({"op": "place", "component": "Plate", "at": [20, 0, 0]}),
        );
        let width = app.doc.component_bounds(cover).size().x;
        assert!((width - 120.0).abs() < 1e-6, "{width}");

        // A new sketch on the plate's top face: its plane follows the face.
        app.selected_geom = vec![flat_face(&app, 0, DVec3::Z)];
        app.execute(&ctx, CommandId::NewSketch);
        assert_eq!(app.open_sketch_name(), "Sketch2");
        let plane = app.sketch_work.as_ref().unwrap().plane;
        assert!((plane.origin().z - 5.0).abs() < 1e-9, "{plane:?}");
        assert!(plane.normal().abs_diff_eq(DVec3::Z, 1e-9));
        app.execute(&ctx, CommandId::ExitSketch);
        let part = cover_part(&app, cover);
        let sketch2 = part.features().find(|f| f.name == "Sketch2").unwrap();
        assert!(matches!(
            sketch2.sketch().map(|s| &s.plane),
            Some(PlaneRef::External(_))
        ));

        // On a plane of the part, and the cover's own face.
        app.new_sketch_in_context(
            cover,
            SketchOn::Part(PlaneRef::Standard(StdPlane::Right), StdPlane::Right.plane()),
        );
        assert_eq!(app.open_sketch_name(), "Sketch3");
        app.selected_geom = vec![flat_face(&app, 1, DVec3::Z)];
        app.execute(&ctx, CommandId::ExitSketch);
        assert!(matches!(
            app.picked_sketch_plane(cover),
            Ok(SketchOn::Part(PlaneRef::Face(_), _))
        ));
        app.execute(&ctx, CommandId::NewSketch);
        assert_eq!(app.open_sketch_name(), "Sketch4");
        let plane = app.sketch_work.as_ref().unwrap().plane;
        assert!((plane.origin().z - 7.0).abs() < 1e-9, "{plane:?}");

        // Stopping editing in context finishes the sketch.
        app.execute(&ctx, CommandId::EditInContext);
        assert!(app.sketch.is_none() && app.in_context_component().is_none());
        assert!(
            cover_part(&app, cover)
                .features()
                .any(|f| f.name == "Sketch4")
        );
        assert!(app.untranslated().is_empty(), "{:?}", app.untranslated());
    }

    #[test]
    fn a_script_opens_a_sketch_in_context() {
        let ctx = egui::Context::default();
        let (mut app, _, cover) = plate_and_cover(&ctx);
        let run = |app: &mut PeetApp, op: Value| {
            let parsed = parse(&app.doc, &op).unwrap_or_else(|e| panic!("{op}: {e}"));
            app.apply_op(&ctx, &parsed, Undo::Step)
        };
        let e = run(&mut app, json!({"op": "edit_sketch", "sketch": "Sketch1"}));
        assert!(!e.ok && e.json["error"].as_str().unwrap().contains("component"));
        let reply = run(
            &mut app,
            json!({"op": "edit_sketch", "sketch": "Sketch1", "component": "Cover"}),
        );
        assert!(reply.ok, "{}", reply.json);
        assert_eq!(reply.json["in_context"], "Cover");
        assert_eq!(app.in_context_component(), Some(cover));
        let busy = run(
            &mut app,
            json!({"op": "place", "component": "Plate", "at": [20, 0, 0]}),
        );
        assert!(!busy.ok);
        assert!(
            busy.json["error"]
                .as_str()
                .unwrap()
                .contains("Sketch1 is open")
        );
        assert!(run(&mut app, json!({"op": "exit_sketch"})).ok);
        assert!(app.sketch.is_none());
    }

    #[test]
    fn the_other_components_are_see_through_in_context() {
        use crate::settings::InContextLook;
        use crate::viewport::shown_transparency;
        let ctx = egui::Context::default();
        let (mut app, plate, cover) = plate_and_cover(&ctx);
        let body = |app: &PeetApp, c| app.doc.bodies_of(c).next().unwrap();
        let look = |app: &PeetApp, c, in_context| {
            shown_transparency(&app.doc.placed[body(app, c)], in_context, &app.settings)
        };
        assert_eq!(look(&app, plate, None), 0.0);
        app.selected_component = Some(cover);
        app.execute(&ctx, CommandId::EditInContext);
        let edited = app.in_context_component();
        assert_eq!(
            look(&app, plate, edited),
            app.settings.in_context_transparency
        );
        assert_eq!(look(&app, cover, edited), 0.0, "the edited part as it is");
        // The display state is not changed.
        assert_eq!(app.doc.placed[body(&app, plate)].transparency, 0.0);
        app.settings.in_context_look = InContextLook::Opaque;
        assert_eq!(look(&app, plate, edited), 0.0);
        // As the display state has them: a see-through one stays so.
        ok(
            &mut app,
            &ctx,
            json!({"op": "set_transparency", "component": "Plate", "transparency": 0.5}),
        );
        app.settings.in_context_look = InContextLook::Maintain;
        assert_eq!(look(&app, plate, edited), 0.5);
        app.settings.in_context_look = InContextLook::Transparent;
        assert_eq!(look(&app, plate, edited), 0.75);
        draw(&mut app);
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
