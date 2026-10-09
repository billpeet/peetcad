//! The configurations table: everything that differs between the part's configurations,
//! one row each and one column per configuration, edited in place.
//!
//! Rows are the features suppressed in some configurations, the parameters with other
//! values, and the feature values and sketch dimensions with other values. The values of
//! one chosen feature are listed as well, whether they differ or not: typing another
//! value in one configuration's column is how a value is made to differ.
//!
//! The table only reports what the user did, as the operations that do it; the app
//! applies them.

use egui::{RichText, Ui};
use serde_json::{Value, json};

use peet_model::{ConfigId, FeatureId, FeatureKind, Slot, SlotValue};
use peet_ops::{Configs, Input, Op};

use crate::document::Document;
use crate::features_ui::ERROR;

/// What the table remembers between frames.
#[derive(Default)]
pub struct ConfigTable {
    /// The feature whose values are all listed (`None`: the one selected in the tree).
    pub feature: Option<FeatureId>,
    /// Why the last change could not be made.
    pub error: Option<String>,
}

/// A row of the table.
enum Row {
    Suppressed(FeatureId),
    Parameter(String),
    Value(FeatureId, SlotValue),
}

/// The operation that sets a feature's value (a numeric field, or a sketch's dimension)
/// to what was typed, in the configurations of `scope`.
pub fn set_value_op(
    doc: &Document,
    feature: FeatureId,
    value: &SlotValue,
    text: &str,
    scope: &Configs,
) -> Result<Op, String> {
    match &value.slot {
        Slot::Dimension(_) => Ok(Op::SetDimension {
            sketch: feature.into(),
            name: value.name.clone(),
            value: Input::Expr(text.to_owned()),
            configurations: Some(scope.clone()),
        }),
        Slot::Field(name) => {
            let configurations = match scope {
                Configs::This => json!("this"),
                Configs::All => json!("all"),
                Configs::Named(names) => json!(names),
            };
            // A bend model's value is a field of `bend`.
            let (field, to): (&str, Value) = match name.as_str() {
                "k_factor" | "allowance" | "deduction" => ("bend", json!({ name: text })),
                _ => (name, json!(text)),
            };
            peet_ops::parse(
                doc,
                &json!({
                    "op": "edit",
                    "feature": feature.0,
                    field: to,
                    "configurations": configurations,
                }),
            )
        }
    }
}

/// A text cell that keeps what is being typed until the field is left. Returns the new
/// text when it is committed and differs from `current`.
fn text_cell(ui: &mut Ui, key: egui::Id, current: &str) -> Option<String> {
    let mut text = ui
        .data(|m| m.get_temp::<String>(key))
        .unwrap_or_else(|| current.to_owned());
    let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(90.0));
    if r.changed() {
        ui.data_mut(|m| m.insert_temp(key, text.clone()));
    }
    if r.lost_focus() {
        ui.data_mut(|m| m.remove::<String>(key));
        let text = text.trim();
        if !text.is_empty() && text != current {
            return Some(text.to_owned());
        }
    }
    None
}

/// Draws the table. Returns the operations for what the user changed.
pub fn config_table_ui(
    ui: &mut Ui,
    doc: &Document,
    table: &mut ConfigTable,
    selected: Option<FeatureId>,
) -> Vec<Op> {
    let mut ops: Vec<Op> = Vec::new();
    let model = &doc.model;
    let configs: Vec<(ConfigId, String)> = model
        .configurations()
        .iter()
        .map(|c| (c.id, c.name.clone()))
        .collect();
    let active = model.active_configuration().id;
    if configs.len() < 2 {
        ui.weak("This part has one configuration. Add another in the list above the feature tree; what differs between them is then listed here.");
        return ops;
    }

    // The feature whose values are listed in full.
    let shown = table
        .feature
        .or(selected)
        .filter(|id| model.feature(*id).is_some());
    ui.horizontal(|ui| {
        ui.label("Show every value of");
        egui::ComboBox::from_id_salt("config_table_feature")
            .selected_text(shown.map_or("(no feature)", |id| model.name_of(id)))
            .show_ui(ui, |ui| {
                if ui.selectable_label(shown.is_none(), "(no feature)").clicked() {
                    table.feature = None;
                }
                for f in model.features() {
                    if ui.selectable_label(shown == Some(f.id), &f.name).clicked() {
                        table.feature = Some(f.id);
                    }
                }
            });
    })
    .response
    .on_hover_text("A value typed in one configuration's column changes that configuration only: that is how a value is made to differ.");
    ui.add_space(4.0);

    let mut rows: Vec<Row> = Vec::new();
    let mut flagged = model.features_that_differ();
    if let Some(id) = shown
        && !flagged.contains(&id)
    {
        flagged.push(id);
    }
    rows.extend(flagged.into_iter().map(Row::Suppressed));
    rows.extend(
        model
            .parameters_that_differ()
            .into_iter()
            .map(|n| Row::Parameter(n.to_owned())),
    );
    let mut listed: Vec<(FeatureId, Slot)> = model.values_that_differ();
    if let Some(id) = shown {
        for v in model.values(id) {
            if !listed.contains(&(id, v.slot.clone())) {
                listed.push((id, v.slot));
            }
        }
    }
    for (id, slot) in listed {
        if let Some(v) = model.values(id).into_iter().find(|v| v.slot == slot) {
            rows.push(Row::Value(id, v));
        }
    }
    if rows.is_empty() {
        ui.weak("Nothing differs between the configurations yet. Choose a feature above to list its values, or suppress a feature in one configuration from the tree.");
        return ops;
    }

    egui::ScrollArea::both().show(ui, |ui| {
        egui::Grid::new("config_table")
            .num_columns(configs.len() + 2)
            .spacing([8.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.strong("");
                for (id, name) in &configs {
                    if *id == active {
                        ui.strong(format!("● {name}"));
                    } else {
                        ui.label(name);
                    }
                }
                ui.label("");
                ui.end_row();
                for row in &rows {
                    let named = |c: &str| Configs::Named(vec![c.to_owned()]);
                    // What makes the row the same in every configuration again.
                    let mut same: Option<Op> = None;
                    let differs = match row {
                        Row::Suppressed(id) => {
                            ui.label(format!("{} suppressed", model.name_of(*id)));
                            for (config, name) in &configs {
                                let mut on = model.suppressed_in(*id, *config).unwrap_or(false);
                                if ui.checkbox(&mut on, "").changed() {
                                    ops.push(Op::Suppress {
                                        feature: (*id).into(),
                                        on,
                                        configurations: named(name),
                                    });
                                }
                            }
                            same = Some(Op::Suppress {
                                feature: (*id).into(),
                                on: model.feature(*id).is_some_and(|f| f.suppressed),
                                configurations: Configs::All,
                            });
                            model.suppression_differs(*id)
                        }
                        Row::Parameter(param) => {
                            ui.label(RichText::new(param).monospace());
                            for (config, name) in &configs {
                                let now = model.parameter_in(param, *config).unwrap_or_default();
                                let key = egui::Id::new(("config_param", param, config.0));
                                if let Some(text) = text_cell(ui, key, now) {
                                    ops.push(Op::SetParameter {
                                        name: param.clone(),
                                        value: Input::Expr(text),
                                        configurations: named(name),
                                    });
                                }
                            }
                            if let Some(now) = model.parameter_in(param, active) {
                                same = Some(Op::SetParameter {
                                    name: param.clone(),
                                    value: Input::Expr(now.to_owned()),
                                    configurations: Configs::All,
                                });
                            }
                            true
                        }
                        Row::Value(id, value) => {
                            let what = match model.feature(*id).map(|f| &f.kind) {
                                Some(FeatureKind::Sketch(_)) => "dimension",
                                _ => "value",
                            };
                            ui.label(format!("{} of {}", value.name, model.name_of(*id)))
                                .on_hover_text(format!(
                                    "The {what} {} of {}.",
                                    value.name,
                                    model.name_of(*id)
                                ));
                            let text_in = |config: ConfigId| {
                                model
                                    .value_in(*id, &value.slot, config)
                                    .unwrap_or_else(|| value.value.clone())
                                    .input_text(value.kind, &model.parameters)
                            };
                            for (config, name) in &configs {
                                let now = text_in(*config);
                                let key =
                                    egui::Id::new(("config_value", id.0, &value.slot, config.0));
                                if let Some(text) = text_cell(ui, key, &now) {
                                    match set_value_op(doc, *id, value, &text, &named(name)) {
                                        Ok(op) => ops.push(op),
                                        Err(e) => table.error = Some(e),
                                    }
                                }
                            }
                            same =
                                set_value_op(doc, *id, value, &text_in(active), &Configs::All).ok();
                            model.value_differs(*id, &value.slot)
                        }
                    };
                    if ui
                        .add_enabled(differs && same.is_some(), egui::Button::new("=").small())
                        .on_hover_text("Use the active configuration's value in all of them.")
                        .clicked()
                    {
                        ops.extend(same);
                    }
                    ui.end_row();
                }
            });
    });
    if let Some(e) = &table.error {
        ui.add_space(4.0);
        ui.colored_label(ERROR, e);
    }
    ops
}
