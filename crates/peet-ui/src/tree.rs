//! The feature tree: the origin and standard planes, then the history in build order with
//! the rollback bar.
//!
//! Rows are dragged to reorder features and the rollback bar is dragged to roll the part
//! back; right-clicking a row offers the rest. The tree only reports what the user did
//! ([`TreeAction`]); the app applies it to the document, so every change is undoable.

use egui::{Color32, RichText, Sense, Stroke, Ui};
use peet_model::{Datum, FeatureId, FeatureKind, Output, Status};

use crate::document::{Document, ItemId};
use crate::features_ui::{ERROR, WARNING};

/// Something the user did in the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeAction {
    Select(Option<ItemId>),
    /// Double-click or "Edit": open a sketch, or show a feature's properties.
    Edit(ItemId),
    SetVisible(ItemId, bool),
    SetSuppressed(FeatureId, bool),
    Delete(FeatureId),
    /// Put the rollback bar below this many features (`None`: at the end).
    Rollback(Option<usize>),
    /// Move a feature so that it ends up at this index.
    Move(FeatureId, usize),
}

/// What the tree needs to know besides the document.
pub struct TreeView {
    pub selected: Option<ItemId>,
    /// The sketch open for editing.
    pub editing: Option<ItemId>,
}

/// What is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Feature(FeatureId),
    Rollback,
}

/// Draws the tree. Returns the actions, and the row under the cursor (to highlight in the
/// viewport).
pub fn tree_ui(ui: &mut Ui, doc: &Document, view: &TreeView) -> (Vec<TreeAction>, Option<ItemId>) {
    let mut actions = Vec::new();
    let mut hovered = None;
    let dark = ui.visuals().dark_mode;

    for datum in Datum::ALL {
        let item = ItemId::Datum(datum);
        ui.horizontal(|ui| {
            let mut visible = doc.model.datum_visible(datum);
            if ui
                .checkbox(&mut visible, "")
                .on_hover_text(if visible { "Hide" } else { "Show" })
                .changed()
            {
                actions.push(TreeAction::SetVisible(item, visible));
            }
            let selected = view.selected == Some(item);
            let r = ui.selectable_label(selected, datum.label());
            if r.clicked() {
                actions.push(TreeAction::Select((!selected).then_some(item)));
            }
            if r.hovered() {
                hovered = Some(item);
            }
        });
    }
    ui.separator();

    let built = doc.model.rollback_index();
    let count = doc.model.len();
    let checkbox_width = ui.spacing().interact_size.y + ui.spacing().item_spacing.x;
    for (index, feature) in doc.model.features().enumerate() {
        if index == built {
            rollback_bar(ui, index, count, &mut actions);
        }
        let item = ItemId::Feature(feature.id);
        let status = doc.status(feature.id);
        let row = ui.horizontal(|ui| {
            let has_visibility = !feature.kind.is_solid();
            if has_visibility {
                let mut visible = feature.visible;
                if ui
                    .checkbox(&mut visible, "")
                    .on_hover_text(if visible { "Hide" } else { "Show" })
                    .changed()
                {
                    actions.push(TreeAction::SetVisible(item, visible));
                }
            } else {
                // Solid features have no visibility of their own.
                ui.add_space(checkbox_width);
            }
            let selected = view.selected == Some(item);
            let text = row_text(
                doc,
                feature.id,
                &feature.name,
                &feature.kind,
                status,
                view,
                dark,
            );
            let button = egui::Button::selectable(selected, text).sense(Sense::click_and_drag());
            let mut r = ui.add(button);
            if let Some(m) = status.and_then(Status::message) {
                r = r.on_hover_text(m);
            } else {
                r = r.on_hover_text(feature.kind.type_name());
            }
            if r.drag_started() {
                r.dnd_set_drag_payload(Drag::Feature(feature.id));
            }
            if r.clicked() {
                actions.push(TreeAction::Select((!selected).then_some(item)));
            }
            if r.double_clicked() {
                actions.push(TreeAction::Edit(item));
            }
            r.context_menu(|ui| {
                if ui.button("Edit").clicked() {
                    actions.push(TreeAction::Edit(item));
                    ui.close();
                }
                if has_visibility {
                    let label = if feature.visible { "Hide" } else { "Show" };
                    if ui.button(label).clicked() {
                        actions.push(TreeAction::SetVisible(item, !feature.visible));
                        ui.close();
                    }
                }
                let label = if feature.suppressed {
                    "Unsuppress"
                } else {
                    "Suppress"
                };
                if ui.button(label).clicked() {
                    actions.push(TreeAction::SetSuppressed(feature.id, !feature.suppressed));
                    ui.close();
                }
                ui.separator();
                if ui
                    .button("Roll Back to Here")
                    .on_hover_text("Build only the features above this one.")
                    .clicked()
                {
                    actions.push(TreeAction::Rollback(Some(index)));
                    ui.close();
                }
                if built < count && ui.button("Roll to End").clicked() {
                    actions.push(TreeAction::Rollback(None));
                    ui.close();
                }
                ui.separator();
                if ui.button("Delete").clicked() {
                    actions.push(TreeAction::Delete(feature.id));
                    ui.close();
                }
            });
            r
        });
        let r = row.inner;
        if r.hovered() {
            hovered = Some(item);
        }
        // Dropping on a row inserts above or below it, by which half the pointer is in.
        let below = ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| p.y > r.rect.center().y);
        let at = if below { index + 1 } else { index };
        if let Some(payload) = r.dnd_hover_payload::<Drag>() {
            let y = if below { r.rect.bottom() } else { r.rect.top() };
            let color = match *payload {
                Drag::Rollback => Color32::from_rgb(230, 160, 40),
                Drag::Feature(_) => ui.visuals().selection.stroke.color,
            };
            ui.painter()
                .hline(row.response.rect.x_range(), y, Stroke::new(2.0, color));
        }
        if let Some(payload) = r.dnd_release_payload::<Drag>() {
            match *payload {
                Drag::Rollback => {
                    actions.push(TreeAction::Rollback((at < count).then_some(at)));
                }
                Drag::Feature(id) => {
                    if let Some(from) = doc.model.index_of(id) {
                        let to = if at > from { at - 1 } else { at };
                        if to != from {
                            actions.push(TreeAction::Move(id, to));
                        }
                    }
                }
            }
        }
    }
    if built >= count {
        rollback_bar(ui, count, count, &mut actions);
    }
    if count == 0 {
        ui.add_space(8.0);
        ui.weak("Select a plane and press S (New Sketch) to start sketching.");
    }
    (actions, hovered)
}

/// The rollback bar: a line that can be dragged up and down the tree.
fn rollback_bar(ui: &mut Ui, index: usize, count: usize, actions: &mut Vec<TreeAction>) {
    let width = ui.available_width();
    let (rect, r) = ui.allocate_exact_size(egui::vec2(width, 8.0), Sense::click_and_drag());
    let rolled_back = index < count;
    let color = if rolled_back {
        Color32::from_rgb(230, 160, 40)
    } else {
        ui.visuals().weak_text_color()
    };
    let stroke = if r.hovered() || r.dragged() { 3.0 } else { 2.0 };
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(stroke, color));
    if r.drag_started() {
        r.dnd_set_drag_payload(Drag::Rollback);
    }
    let r = r
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text(if rolled_back {
            "Rollback bar: the features below it are not built. Drag it, or double-click to roll to the end."
        } else {
            "Rollback bar: drag it up to see and edit the part as it was at an earlier step."
        });
    if r.double_clicked() && rolled_back {
        actions.push(TreeAction::Rollback(None));
    }
}

fn row_text(
    doc: &Document,
    id: FeatureId,
    name: &str,
    kind: &FeatureKind,
    status: Option<&Status>,
    view: &TreeView,
    dark: bool,
) -> RichText {
    let editing = view.editing == Some(ItemId::Feature(id));
    let marker = match (kind, doc.evaluation().output(id)) {
        (FeatureKind::Sketch(_), Output::Sketch { definition, .. }) => definition.marker(),
        _ => "",
    };
    let mut text = match status {
        Some(Status::Failed(_)) => RichText::new(format!("⚠ {marker}{name}")).color(ERROR),
        Some(Status::Warning(_)) => RichText::new(format!("⚠ {marker}{name}")).color(WARNING),
        Some(Status::Suppressed) => RichText::new(name).weak().strikethrough(),
        Some(Status::RolledBack) => RichText::new(name).color(if dark {
            Color32::from_gray(110)
        } else {
            Color32::from_gray(150)
        }),
        _ => RichText::new(format!("{marker}{name}")),
    };
    if editing {
        text = text.strong();
    }
    text
}
