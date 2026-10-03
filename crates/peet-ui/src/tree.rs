//! The feature tree: the origin and standard planes, then the history in build order with
//! the rollback bar.
//!
//! Rows are dragged to reorder features and the rollback bar is dragged to roll the part
//! back; right-clicking a row offers the rest. The tree only reports what the user did
//! ([`TreeAction`]); the app applies it to the document, so every change is undoable.

use egui::{Color32, RichText, Sense, Stroke, Ui};
use std::collections::{HashMap, HashSet};

use peet_model::{Datum, Feature, FeatureId, FeatureKind, Output, Status};

use crate::document::{Document, ItemId};
use crate::features_ui::{ERROR, WARNING};
use crate::icons::{self, Icon};

/// Side of the feature-type tile in front of each row.
const TILE: f32 = 18.0;

/// Something the user did in the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeAction {
    Select(Option<ItemId>),
    /// Double-click or "Edit": open a sketch, or show a feature's properties.
    Edit(ItemId),
    SetVisible(ItemId, bool),
    /// Suppress or unsuppress a feature: in the active configuration, or (`all`) in every
    /// configuration.
    SetSuppressed {
        feature: FeatureId,
        on: bool,
        all: bool,
    },
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
            nest_marker(ui, Nest::Leaf);
            let mut visible = doc.model.datum_visible(datum);
            if ui
                .checkbox(&mut visible, "")
                .on_hover_text(if visible { "Hide" } else { "Show" })
                .changed()
            {
                actions.push(TreeAction::SetVisible(item, visible));
            }
            let icon = match datum {
                Datum::Origin => Icon::CoordSystem,
                Datum::Plane(_) => Icon::Planes,
            };
            type_tile(ui, icon, dark, false);
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

    // Like other CAD tools, a sketch used by a (built) feature is listed under that
    // feature rather than on its own; the first feature using it gets it.
    let (absorbed, children) = nesting(doc, built);
    let mut cx = RowContext {
        doc,
        view,
        actions: Vec::new(),
        hovered: None,
        dark,
        built,
        count,
    };
    for (index, feature) in doc.model.features().enumerate() {
        if index == built {
            rollback_bar(ui, index, count, &mut cx.actions);
        }
        if absorbed.contains(&feature.id) {
            continue; // shown under the feature that uses it
        }
        let Some(kids) = children.get(&feature.id) else {
            feature_row(ui, &mut cx, feature, index, Nest::Leaf);
            continue;
        };
        let key = egui::Id::new(("tree_expanded", feature.id));
        // A nested sketch that is selected or open shows even if its parent is collapsed.
        let active = kids.iter().any(|(_, k)| {
            let item = Some(ItemId::Feature(k.id));
            view.selected == item || view.editing == item
        });
        let open = active || ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
        feature_row(ui, &mut cx, feature, index, Nest::Parent { open, key });
        if open {
            for &(kid_index, kid) in kids {
                feature_row(ui, &mut cx, kid, kid_index, Nest::Child);
            }
        }
    }
    if built >= count {
        rollback_bar(ui, count, count, &mut cx.actions);
    }
    if count == 0 {
        ui.add_space(8.0);
        ui.weak("Select a plane and press S (New Sketch) to start sketching.");
    }
    actions.append(&mut cx.actions);
    (actions, hovered.or(cx.hovered))
}

/// The tinted feature-type icon in front of a row.
fn type_tile(ui: &mut Ui, icon: Icon, dark: bool, faded: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(TILE, TILE), Sense::hover());
    let fg = ui.visuals().text_color();
    icons::tile(ui.painter(), rect, icon, dark, fg, faded);
}

/// Each feature's nested sketches, with their model indices.
type Children<'a> = HashMap<FeatureId, Vec<(usize, &'a Feature)>>;

/// Which sketches are listed under the features that use them: the absorbed sketches, and
/// each feature's nested sketches with their model indices. Only built features absorb
/// their sketch, so rolling back above a feature brings its sketch back to the top level.
fn nesting(doc: &Document, built: usize) -> (HashSet<FeatureId>, Children<'_>) {
    let features: Vec<&Feature> = doc.model.features().collect();
    let mut absorbed = HashSet::new();
    let mut children: HashMap<FeatureId, Vec<(usize, &Feature)>> = HashMap::new();
    for f in features.iter().take(built) {
        let Some(sketch) = profile_sketch(&f.kind) else {
            continue;
        };
        let Some(index) = doc.model.index_of(sketch) else {
            continue;
        };
        if matches!(features[index].kind, FeatureKind::Sketch(_)) && absorbed.insert(sketch) {
            children
                .entry(f.id)
                .or_default()
                .push((index, features[index]));
        }
    }
    (absorbed, children)
}

/// Where a row sits in the nesting of sketches under the features that use them.
#[derive(Clone, Copy, Debug)]
enum Nest {
    /// No children.
    Leaf,
    /// Has nested sketches; `key` stores whether it is expanded.
    Parent { open: bool, key: egui::Id },
    /// A sketch shown under the feature that uses it.
    Child,
}

/// Width of the expand arrow column, and the extra indent of nested rows.
const ARROW: f32 = 14.0;
const INDENT: f32 = 16.0;

/// The expand arrow, or the space it would take, at the start of a row.
fn nest_marker(ui: &mut Ui, nest: Nest) {
    match nest {
        // Allocated rather than spaced, so the item spacing matches the arrow's.
        Nest::Leaf => {
            ui.allocate_exact_size(egui::vec2(ARROW, TILE), Sense::hover());
        }
        Nest::Child => {
            ui.allocate_exact_size(egui::vec2(ARROW + INDENT, TILE), Sense::hover());
        }
        Nest::Parent { open, key } => {
            let (rect, r) = ui.allocate_exact_size(egui::vec2(ARROW, TILE), Sense::click());
            let color = if r.hovered() {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            let c = rect.center();
            let pts = if open {
                vec![
                    c + egui::vec2(-4.0, -2.0),
                    c + egui::vec2(4.0, -2.0),
                    c + egui::vec2(0.0, 3.0),
                ]
            } else {
                vec![
                    c + egui::vec2(-2.0, -4.0),
                    c + egui::vec2(3.0, 0.0),
                    c + egui::vec2(-2.0, 4.0),
                ]
            };
            ui.painter()
                .add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
            let r = r.on_hover_text(if open {
                "Collapse"
            } else {
                "Show the sketch it uses"
            });
            if r.clicked() {
                ui.data_mut(|d| d.insert_temp(key, !open));
            }
        }
    }
}

/// The sketch a feature is built from, if any.
fn profile_sketch(kind: &FeatureKind) -> Option<FeatureId> {
    kind.sketch()
}

/// Everything a feature row needs besides the feature.
struct RowContext<'a> {
    doc: &'a Document,
    view: &'a TreeView,
    actions: Vec<TreeAction>,
    hovered: Option<ItemId>,
    dark: bool,
    built: usize,
    count: usize,
}

fn feature_row(ui: &mut Ui, cx: &mut RowContext<'_>, feature: &Feature, index: usize, nest: Nest) {
    let (doc, view, dark) = (cx.doc, cx.view, cx.dark);
    let (built, count) = (cx.built, cx.count);
    let actions = &mut cx.actions;
    let hovered = &mut cx.hovered;
    let item = ItemId::Feature(feature.id);
    let status = doc.status(feature.id);
    let row = ui.horizontal(|ui| {
        nest_marker(ui, nest);
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
            // Solid features have no visibility of their own; keep the column aligned.
            ui.add_visible(false, egui::Checkbox::without_text(&mut false));
        }
        let faded = status.is_some_and(|s| s.is_suppressed() || *s == Status::RolledBack);
        type_tile(ui, Icon::for_feature(&feature.kind), dark, faded);
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
        } else if let Some(Status::SuppressedBy(parent)) = status {
            r = r.on_hover_text(format!(
                "Suppressed with {}, which it is built on.",
                doc.model.name_of(*parent)
            ));
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
            // With several configurations: in the active one, or in all of them.
            let several = doc.model.configurations().len() > 1;
            let mut suppress = ui.button(label);
            if several {
                suppress = suppress.on_hover_text(format!(
                    "In this configuration ({}).",
                    doc.model.active_configuration().name
                ));
            }
            if suppress.clicked() {
                actions.push(TreeAction::SetSuppressed {
                    feature: feature.id,
                    on: !feature.suppressed,
                    all: false,
                });
                ui.close();
            }
            if several {
                for on in [true, false] {
                    let word = if on { "Suppress" } else { "Unsuppress" };
                    if ui.button(format!("{word} in All Configurations")).clicked() {
                        actions.push(TreeAction::SetSuppressed {
                            feature: feature.id,
                            on,
                            all: true,
                        });
                        ui.close();
                    }
                }
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
        *hovered = Some(item);
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
        Some(Status::Suppressed | Status::SuppressedBy(_)) => {
            RichText::new(name).weak().strikethrough()
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use peet_math::{DVec2, Plane};
    use peet_model::{Operation, PlaneRef, StdPlane};

    fn sketch_and_extrude() -> (Document, FeatureId, FeatureId) {
        let mut doc = Document::default();
        let mut ids = (FeatureId(0), FeatureId(0));
        doc.change("Add", |m| {
            let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
            if let Some(f) = m.feature_mut(s).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(&mut f.sketch, DVec2::ZERO, DVec2::new(40.0, 20.0));
            }
            ids = (s, m.add_extrude(s, Operation::Add));
        });
        (doc, ids.0, ids.1)
    }

    #[test]
    fn a_used_sketch_nests_under_its_feature() {
        let (doc, sketch, extrude) = sketch_and_extrude();
        let (absorbed, children) = nesting(&doc, doc.model.rollback_index());
        assert!(absorbed.contains(&sketch));
        let kids: Vec<FeatureId> = children[&extrude].iter().map(|(_, f)| f.id).collect();
        assert_eq!(kids, [sketch]);
    }

    #[test]
    fn an_unused_sketch_stays_at_the_top_level() {
        let mut doc = Document::default();
        doc.change("Add", |m| {
            m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
        });
        let (absorbed, children) = nesting(&doc, doc.model.rollback_index());
        assert!(absorbed.is_empty() && children.is_empty());
    }

    #[test]
    fn rolling_back_above_the_feature_releases_its_sketch() {
        let (mut doc, sketch, _) = sketch_and_extrude();
        doc.change("Roll Back", |m| m.set_rollback(Some(1)));
        let (absorbed, _) = nesting(&doc, doc.model.rollback_index());
        assert!(!absorbed.contains(&sketch));
    }

    #[test]
    fn a_shared_sketch_nests_under_the_first_feature_only() {
        let (mut doc, sketch, first) = sketch_and_extrude();
        let mut second = None;
        doc.change("Add", |m| {
            second = Some(m.add_extrude(sketch, Operation::Add))
        });
        let (_, children) = nesting(&doc, doc.model.rollback_index());
        assert!(children.contains_key(&first));
        assert!(!children.contains_key(&second.unwrap()));
    }
}
