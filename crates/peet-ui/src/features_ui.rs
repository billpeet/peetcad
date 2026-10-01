//! Properties panels for features (extrude and cut-extrude).

use egui::{Color32, Ui};
use peet_model::{EndCondition, Operation};

use crate::document::ExtrudeItem;

/// What the user did in a feature panel.
#[derive(Default)]
pub struct PanelResult {
    /// A parameter changed: rebuild.
    pub changed: bool,
    /// The user wants to pick the face for "Up to face".
    pub pick_up_to_face: bool,
}

/// Extrude parameters. `picking` is true while waiting for an "up to" face click.
pub fn extrude_panel(ui: &mut Ui, ex: &mut ExtrudeItem, picking: bool) -> PanelResult {
    let mut out = PanelResult::default();
    let f = &mut ex.feature;
    if let Some(e) = &ex.error {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(Color32::from_rgb(229, 72, 77), "⚠");
            ui.label(e);
        });
        ui.add_space(4.0);
    }
    egui::Grid::new("extrude_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Operation");
            egui::ComboBox::from_id_salt("extrude_op")
                .selected_text(f.operation.label())
                .show_ui(ui, |ui| {
                    for op in [Operation::NewBody, Operation::Add, Operation::Cut] {
                        out.changed |= ui
                            .selectable_value(&mut f.operation, op, op.label())
                            .changed();
                    }
                });
            ui.end_row();

            ui.label("End");
            let current = f.end;
            egui::ComboBox::from_id_salt("extrude_end")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    for end in [
                        EndCondition::Blind,
                        EndCondition::Symmetric,
                        EndCondition::ThroughAll,
                    ] {
                        if ui
                            .selectable_label(
                                std::mem::discriminant(&current) == std::mem::discriminant(&end),
                                end.label(),
                            )
                            .clicked()
                        {
                            f.end = end;
                            out.changed = true;
                        }
                    }
                    if ui
                        .selectable_label(matches!(current, EndCondition::UpTo(_)), "Up to face…")
                        .on_hover_text("Then click a planar face parallel to the sketch.")
                        .clicked()
                    {
                        out.pick_up_to_face = true;
                    }
                });
            ui.end_row();

            if matches!(f.end, EndCondition::Blind | EndCondition::Symmetric) {
                ui.label("Depth");
                out.changed |= ui
                    .add(
                        egui::DragValue::new(&mut f.depth)
                            .range(0.001..=1e6)
                            .speed(0.1)
                            .suffix(" mm"),
                    )
                    .changed();
                ui.end_row();
            }
            if !matches!(f.end, EndCondition::Symmetric | EndCondition::UpTo(_)) {
                ui.label("Direction");
                out.changed |= ui.checkbox(&mut f.reverse, "Reverse").changed();
                ui.end_row();
            }
        });
    if matches!(f.end, EndCondition::UpTo(_)) || picking {
        ui.add_space(4.0);
        if picking {
            ui.colored_label(
                Color32::from_rgb(255, 150, 30),
                "Click a planar face parallel to the sketch…",
            );
        } else if ui.button("Pick another face").clicked() {
            out.pick_up_to_face = true;
        }
    }
    ui.add_space(6.0);
    ui.weak("Regions: the sketch's outer regions with their holes (islands inside holes are extruded too).");
    out
}
