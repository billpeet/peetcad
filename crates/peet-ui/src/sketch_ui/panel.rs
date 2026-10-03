//! The properties panel in sketch mode, and the inline dimension value editor.

use egui::{Color32, RichText, Ui, vec2};
use peet_sketch::expr::Parameters;
use peet_sketch::solver::ConstraintStatus;
use peet_sketch::{ConstraintId, ConstraintKind, EntityKind, Sketch};

use super::{
    Sel, SketchEditor, SketchView, Tool, describe_constraint, describe_entity, dof_color,
    format_value, relations_for,
};
use crate::commands::CommandId;
use crate::document::{SketchItem, SketchStatus};

const RELATION_COMMANDS: [CommandId; 11] = [
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

impl SketchEditor {
    /// Fills the properties panel. Returns a command the user picked (relation buttons).
    pub fn properties_ui(
        &mut self,
        ui: &mut Ui,
        item: &mut SketchItem,
        params: &Parameters,
    ) -> Option<CommandId> {
        super::set_display_units(params);
        let mut picked = None;
        let dark = ui.visuals().dark_mode;
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Sketch status.
            let status = self.status();
            let color = match status {
                SketchStatus::Under => dof_color(peet_sketch::solver::DofStatus::Under, dark),
                SketchStatus::Fully => ui.visuals().text_color(),
                SketchStatus::Over => dof_color(peet_sketch::solver::DofStatus::Over, dark),
            };
            ui.label(RichText::new(self.status_text()).color(color).strong());
            ui.weak(format!(
                "On {} · solved in {:.2} ms",
                item.plane_name, self.solve_ms
            ));
            self.diagnoses_ui(ui, item);
            ui.separator();

            // Tool and its options.
            ui.strong(self.tool.label());
            ui.add(egui::Label::new(RichText::new(self.hint()).weak()).wrap());
            ui.add_space(4.0);
            self.tool_options_ui(ui);
            ui.separator();

            // Selection.
            if let Some(cmd) = self.selection_ui(ui, item, params) {
                picked = Some(cmd);
            }
        });
        picked
    }

    fn diagnoses_ui(&mut self, ui: &mut Ui, item: &mut SketchItem) {
        if self.analysis.diagnoses.is_empty() {
            return;
        }
        ui.add_space(4.0);
        let mut select = None;
        let mut delete = None;
        for d in &self.analysis.diagnoses {
            let what = describe_constraint(&item.sketch, d.constraint);
            let others: Vec<String> = d
                .involved
                .iter()
                .map(|c| describe_constraint(&item.sketch, *c))
                .collect();
            let text = match d.status {
                ConstraintStatus::Conflicting => {
                    format!("{what} conflicts with {}", join_or_none(&others))
                }
                _ => format!("{what} is already implied by {}", join_or_none(&others)),
            };
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(229, 72, 77), "⚠");
                ui.label(text);
                if ui.small_button("Select").clicked() {
                    select = Some(d);
                }
                if ui
                    .small_button("Delete")
                    .on_hover_text("Delete this relation or dimension")
                    .clicked()
                {
                    delete = Some(d.constraint);
                }
            });
        }
        if let Some(d) = select {
            let mut sel = vec![Sel::Constraint(d.constraint)];
            sel.extend(d.involved.iter().map(|c| Sel::Constraint(*c)));
            self.selection = sel;
        }
        if let Some(c) = delete {
            let before = item.sketch.clone();
            item.sketch.remove_constraint(c);
            self.push_undo(before);
            self.resolve(item);
        }
    }

    fn tool_options_ui(&mut self, ui: &mut Ui) {
        let o = &mut self.options;
        if self.tool.is_drawing() {
            ui.checkbox(&mut o.construction, "Construction geometry")
                .on_hover_text(
                    "New geometry helps constrain the sketch but isn't part of the profile (X).",
                );
            ui.checkbox(&mut o.inference, "Automatic relations")
                .on_hover_text("Snap to points, curves, horizontal and vertical while drawing. Hold Ctrl to skip once.");
        }
        match self.tool {
            Tool::Polygon => {
                ui.horizontal(|ui| {
                    ui.label("Sides");
                    ui.add(egui::DragValue::new(&mut o.polygon_sides).range(3..=64));
                });
            }
            Tool::Fillet => {
                ui.horizontal(|ui| {
                    ui.label("Radius");
                    ui.add(
                        egui::DragValue::new(&mut o.fillet_radius)
                            .range(0.001..=f64::MAX)
                            .speed(0.1)
                            .suffix(" mm"),
                    );
                });
            }
            _ => {}
        }
        ui.checkbox(&mut o.show_relations, "Show relations");
    }

    fn selection_ui(
        &mut self,
        ui: &mut Ui,
        item: &mut SketchItem,
        params: &Parameters,
    ) -> Option<CommandId> {
        let mut picked = None;
        if self.selection.is_empty() {
            ui.weak(
                "Nothing selected. Click geometry to see its relations, or a dimension to edit it.",
            );
            return None;
        }
        let entities = self.selected_entities();
        ui.strong(format!("Selection ({})", self.selection.len()));
        for s in self.selection.clone().iter().take(12) {
            let text = match *s {
                Sel::Entity(e) => describe_entity(&item.sketch, e),
                Sel::Constraint(c) => describe_constraint(&item.sketch, c),
            };
            ui.label(text);
        }
        if self.selection.len() > 12 {
            ui.weak(format!("… and {} more", self.selection.len() - 12));
        }

        // A single dimension: edit it here too.
        if let [Sel::Constraint(c)] = self.selection[..]
            && item
                .sketch
                .constraint(c)
                .is_some_and(|c| c.dimension.is_some())
        {
            ui.add_space(6.0);
            self.dimension_properties(ui, item, params, c);
        }

        // Geometry details.
        if let [e] = entities[..] {
            ui.add_space(4.0);
            geometry_details(ui, &item.sketch, e);
        }

        // Relations that fit the selection.
        let applicable: Vec<CommandId> = RELATION_COMMANDS
            .into_iter()
            .filter(|c| !relations_for(*c, &item.sketch, &entities).is_empty())
            .collect();
        if !applicable.is_empty() {
            ui.add_space(6.0);
            ui.strong("Add relation");
            ui.horizontal_wrapped(|ui| {
                for cmd in applicable {
                    let info = cmd.info();
                    let tip = match info.shortcut {
                        Some(sc) => {
                            format!("{} ({})", info.description, ui.ctx().format_shortcut(&sc))
                        }
                        None => info.description.to_owned(),
                    };
                    if ui.button(info.label).on_hover_text(tip).clicked() {
                        picked = Some(cmd);
                    }
                }
            });
        }

        // Construction toggle and operations on curves.
        let curves: Vec<_> = entities
            .iter()
            .copied()
            .filter(|e| item.sketch.kind(*e).is_some_and(EntityKind::is_curve))
            .collect();
        if !curves.is_empty() {
            ui.add_space(6.0);
            let all = curves
                .iter()
                .all(|e| item.sketch.entity(*e).is_some_and(|x| x.construction));
            let mut construction = all;
            if ui
                .checkbox(&mut construction, "Construction geometry")
                .changed()
            {
                picked = Some(CommandId::ToggleConstruction);
            }
            ui.horizontal(|ui| {
                if ui
                    .button("Offset")
                    .on_hover_text("Offset the selected curve or connected chain (O). Negative distances go to the other side.")
                    .clicked()
                {
                    picked = Some(CommandId::SketchOffset);
                }
                ui.add(
                    egui::DragValue::new(&mut self.options.offset_distance)
                        .speed(0.1)
                        .suffix(" mm"),
                );
            });
            if entities.len() >= 2
                && entities
                    .last()
                    .is_some_and(|a| item.sketch.kind(*a) == Some(EntityKind::Line))
                && ui
                    .button("Mirror")
                    .on_hover_text("Mirror the selection about the last selected line (M).")
                    .clicked()
            {
                picked = Some(CommandId::SketchMirror);
            }
        }

        // Existing relations on the selected geometry.
        let mut existing: Vec<ConstraintId> = Vec::new();
        for e in &entities {
            for c in item.sketch.constraints_on(*e) {
                if !existing.contains(&c) {
                    existing.push(c);
                }
            }
            // Relations on a curve's own points belong to it too.
            if let Some(entity) = item.sketch.entity(*e) {
                for p in entity.geometry.points() {
                    for c in item.sketch.constraints_on(p) {
                        if !existing.contains(&c) {
                            existing.push(c);
                        }
                    }
                }
            }
        }
        if !existing.is_empty() {
            ui.add_space(6.0);
            ui.strong("Existing relations");
            let mut delete = None;
            for c in existing {
                ui.horizontal(|ui| {
                    if ui.small_button("✖").on_hover_text("Delete").clicked() {
                        delete = Some(c);
                    }
                    let status = self.analysis.constraint_status(c);
                    let text = describe_constraint(&item.sketch, c);
                    let label = if status == ConstraintStatus::Ok {
                        ui.label(text)
                    } else {
                        ui.colored_label(Color32::from_rgb(229, 72, 77), text)
                    };
                    if label.hovered() {
                        self.hover = Some(Sel::Constraint(c));
                    }
                });
            }
            if let Some(c) = delete {
                let before = item.sketch.clone();
                item.sketch.remove_constraint(c);
                self.push_undo(before);
                self.resolve(item);
            }
        }
        picked
    }

    fn dimension_properties(
        &mut self,
        ui: &mut Ui,
        item: &mut SketchItem,
        params: &Parameters,
        id: ConstraintId,
    ) {
        let Some(c) = item.sketch.constraint(id).cloned() else {
            return;
        };
        let Some(d) = c.dimension.clone() else {
            return;
        };
        egui::Grid::new("dimension_props")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                let mut name = d.name.clone();
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(120.0));
                if r.changed() {
                    let valid = is_identifier(&name)
                        && item.sketch.dimension_by_name(&name).is_none_or(|x| x == id)
                        && params.get(&name).is_none();
                    if valid && let Some(dim) = item.sketch.constraint_mut(id).and_then(|c| c.dimension.as_mut()) {
                        dim.name = name;
                    }
                }
                ui.end_row();

                ui.label("Value");
                let key = egui::Id::new(("dim_value", id.0));
                let mut text = ui
                    .data(|m| m.get_temp::<String>(key))
                    .unwrap_or_else(|| d.expression.clone().unwrap_or_else(|| super::dim_value_text(&c.kind, d.value)));
                let r = ui.add_enabled(
                    d.driving,
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(120.0)
                        .hint_text("number or expression"),
                );
                if r.changed() {
                    ui.data_mut(|m| m.insert_temp(key, text.clone()));
                }
                if r.lost_focus() {
                    ui.data_mut(|m| m.remove::<String>(key));
                    let current = d.expression.clone().unwrap_or_else(|| super::dim_value_text(&c.kind, d.value));
                    if text.trim() != current && !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.checked_edit(item, "Dimension", |s| {
                            super::set_dimension(s, params, id, &text)
                        });
                    }
                }
                ui.end_row();

                if d.expression.is_some() {
                    ui.label("Result");
                    let unit = if c.kind.is_angular() { "°".to_owned() } else { format!(" {}", super::length_suffix()) };
                    ui.monospace(format!("{}{unit}", super::dim_value_text(&c.kind, d.value)));
                    ui.end_row();
                }

                ui.label("Driving");
                let mut driving = d.driving;
                if ui
                    .checkbox(&mut driving, "")
                    .on_hover_text("Driving dimensions constrain the geometry. Driven (reference) dimensions only measure it.")
                    .changed()
                {
                    self.checked_edit(item, "Driving dimension", |s| {
                        if let Some(dim) = s.constraint_mut(id).and_then(|c| c.dimension.as_mut()) {
                            dim.driving = driving;
                            if driving && let Some(v) = s.measure(&c.kind) {
                                // Start from the measured value so nothing jumps.
                                if let Some(dim) = s.constraint_mut(id).and_then(|c| c.dimension.as_mut()) {
                                    dim.value = v;
                                }
                            }
                        }
                        Ok(())
                    });
                }
                ui.end_row();

                // Point-to-point distances can switch between aligned, horizontal and vertical.
                if let ConstraintKind::Distance(a, b)
                | ConstraintKind::HorizontalDistance(a, b)
                | ConstraintKind::VerticalDistance(a, b) = c.kind
                    && item.sketch.kind(a) == Some(EntityKind::Point)
                    && item.sketch.kind(b) == Some(EntityKind::Point)
                {
                    ui.label("Measure");
                    ui.horizontal(|ui| {
                        for (label, kind) in [
                            ("Aligned", ConstraintKind::Distance(a, b)),
                            ("Horizontal", ConstraintKind::HorizontalDistance(a, b)),
                            ("Vertical", ConstraintKind::VerticalDistance(a, b)),
                        ] {
                            if ui.selectable_label(c.kind == kind, label).clicked() && c.kind != kind {
                                self.checked_edit(item, label, |s| {
                                    let v = s.measure(&kind).unwrap_or(0.0);
                                    let con = s.constraint_mut(id).ok_or("missing")?;
                                    con.kind = kind;
                                    if let Some(dim) = con.dimension.as_mut() {
                                        dim.value = v;
                                        dim.expression = None;
                                    }
                                    Ok(())
                                });
                            }
                        }
                    });
                    ui.end_row();
                }
            });
    }

    /// The inline editor that pops up on a dimension's label.
    pub(super) fn dimension_editor(
        &mut self,
        ui: &mut Ui,
        view: &SketchView,
        item: &mut SketchItem,
        params: &Parameters,
    ) {
        let Some(edit) = &mut self.edit else {
            return;
        };
        let id = edit.dimension;
        let Some(c) = item.sketch.constraint(id) else {
            self.edit = None;
            return;
        };
        let Some(anchor) = super::draw::dimension_anchor(&item.sketch, &c.kind) else {
            self.edit = None;
            return;
        };
        let offset = c
            .dimension
            .as_ref()
            .map_or(peet_math::DVec2::ZERO, |d| d.label_offset);
        let pos = view.to_screen(anchor + offset) - vec2(60.0, 14.0);
        let mut apply = false;
        let mut cancel = false;
        egui::Area::new(egui::Id::new("sketch_dim_edit"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let out = egui::TextEdit::singleline(&mut edit.text)
                        .desired_width(120.0)
                        .font(egui::TextStyle::Monospace)
                        .show(ui);
                    let r = out.response.clone();
                    if edit.focus {
                        // Select the whole value so typing replaces it straight away.
                        r.request_focus();
                        let mut state = out.state;
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::select_all(
                                &out.galley,
                            )));
                        state.store(ui.ctx(), r.id);
                        edit.focus = false;
                    }
                    if r.lost_focus() {
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            cancel = true;
                        } else {
                            apply = true;
                        }
                    }
                    ui.weak("Enter a value or expression (e.g. width / 2)");
                });
            });
        if cancel {
            if let Some(before) = self.edit.take().and_then(|e| e.before) {
                self.push_undo(before);
            }
            return;
        }
        if apply {
            let Some(edit) = self.edit.as_mut() else {
                return;
            };
            let text = edit.text.clone();
            let before = edit.before.take();
            let ok = self
                .checked_edit(item, "Dimension", |s| {
                    super::set_dimension(s, params, id, &text)
                })
                .is_some();
            if ok {
                self.edit = None;
                if let Some(before) = before {
                    // One undo step for "add dimension and set its value".
                    self.undo.pop();
                    self.push_undo(before);
                }
            } else if let Some(edit) = self.edit.as_mut() {
                // Keep the editor open so the user can fix the value.
                edit.before = before;
                edit.focus = true;
            }
        }
    }
}

fn geometry_details(ui: &mut Ui, sketch: &Sketch, e: peet_sketch::EntityId) {
    let Some(kind) = sketch.kind(e) else {
        return;
    };
    egui::Grid::new("geometry_details")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            let row = |ui: &mut Ui, k: &str, v: String| {
                ui.weak(k);
                ui.monospace(v);
                ui.end_row();
            };
            match kind {
                EntityKind::Point => {
                    let p = sketch.point(e);
                    row(ui, "X", super::length_with_unit(p.x));
                    row(ui, "Y", super::length_with_unit(p.y));
                }
                EntityKind::Line => {
                    if let Some(c) = sketch.curve(e) {
                        row(ui, "Length", super::length_with_unit(c.length()));
                        if let Some(d) = c.line_direction() {
                            row(
                                ui,
                                "Angle",
                                format!("{}°", format_value(d.to_angle().to_degrees())),
                            );
                        }
                    }
                }
                EntityKind::Circle | EntityKind::Arc => {
                    if let Some(c) = sketch.curve(e) {
                        let r = c.radius().unwrap_or(0.0);
                        let center = c.center().unwrap_or_default();
                        row(ui, "Radius", super::length_with_unit(r));
                        row(
                            ui,
                            "Centre",
                            format!(
                                "{}, {}",
                                super::length_text(center.x),
                                super::length_text(center.y)
                            ),
                        );
                        if kind == EntityKind::Arc {
                            row(ui, "Length", super::length_with_unit(c.length()));
                        }
                    }
                }
            }
        });
}

fn join_or_none(names: &[String]) -> String {
    if names.is_empty() {
        "other relations".to_owned()
    } else {
        names.join(", ")
    }
}

/// Whether `s` is a valid parameter or dimension name.
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
