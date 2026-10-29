//! The display states list, under an assembly's configurations list: the display states
//! the active configuration can show (SolidWorks' display states), with the one shown
//! marked.
//!
//! Double-clicking one shows it; right-clicking offers the rest. Like the configurations
//! list, it only reports what the user did ([`DisplayAction`]); the app applies it as
//! operations.

use egui::{RichText, Ui};

use peet_model::DisplayStateId;

use crate::document::Document;

/// Something the user did in the display states list.
#[derive(Clone, Debug, PartialEq)]
pub enum DisplayAction {
    Activate(DisplayStateId),
    /// A new display state, as the components look now.
    Add,
    Rename {
        id: DisplayStateId,
        to: String,
    },
    Delete(DisplayStateId),
    /// Link display states to configurations, or not.
    Link(bool),
}

/// What the list remembers between frames.
#[derive(Default)]
pub struct DisplayList {
    /// The display state being renamed, the text so far, and whether its field has been
    /// given the keyboard yet.
    renaming: Option<(DisplayStateId, String, bool)>,
}

/// A name no display state the active configuration can show has yet: "Display State-2", …
pub fn new_name(doc: &Document) -> String {
    let states = doc.model.display_states();
    (states.len() + 1..)
        .map(|n| format!("Display State-{n}"))
        .find(|name| !states.iter().any(|s| s.name == *name))
        .unwrap_or_default()
}

/// Draws the list (for an assembly). Returns what the user did.
pub fn display_ui(ui: &mut Ui, doc: &Document, list: &mut DisplayList) -> Vec<DisplayAction> {
    let mut actions = Vec::new();
    let model = &doc.model;
    if !model.is_assembly() {
        return actions;
    }
    let states = model.display_states();
    let shown = model.active_display_state().map(|s| s.id);
    let title = match model.active_display_state() {
        Some(s) => format!("Display States: {}", s.name),
        None => "Display States".to_owned(),
    };
    egui::CollapsingHeader::new(title)
        .id_salt("display_states")
        .default_open(false)
        .show(ui, |ui| {
            for s in &states {
                let is_shown = Some(s.id) == shown;
                if let Some((id, text, focused)) = &mut list.renaming
                    && *id == s.id
                {
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(140.0));
                    if !*focused {
                        r.request_focus();
                        *focused = true;
                    }
                    if r.lost_focus() {
                        let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        let to = text.trim().to_owned();
                        if !cancelled && !to.is_empty() && to != s.name {
                            actions.push(DisplayAction::Rename { id: s.id, to });
                        }
                        list.renaming = None;
                    }
                    continue;
                }
                let text = if is_shown {
                    RichText::new(format!("● {}", s.name)).strong()
                } else {
                    RichText::new(format!("○ {}", s.name))
                };
                let hint = if is_shown {
                    "The display state shown: whether each component is shown, its colour and its transparency are set in it."
                } else {
                    "Double-click to show it."
                };
                let r = ui
                    .add(egui::Button::selectable(is_shown, text))
                    .on_hover_text(hint);
                if r.double_clicked() && !is_shown {
                    actions.push(DisplayAction::Activate(s.id));
                }
                r.context_menu(|ui| {
                    if ui
                        .add_enabled(!is_shown, egui::Button::new("Show"))
                        .clicked()
                    {
                        actions.push(DisplayAction::Activate(s.id));
                        ui.close();
                    }
                    if ui.button("Rename").clicked() {
                        list.renaming = Some((s.id, s.name.clone(), false));
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(states.len() > 1, egui::Button::new("Delete"))
                        .on_disabled_hover_text("One display state is always shown.")
                        .clicked()
                    {
                        actions.push(DisplayAction::Delete(s.id));
                        ui.close();
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui
                    .small_button("+ Add Display State")
                    .on_hover_text("A new display state, as the components look now: hide, colour or make see-through components in it alone.")
                    .clicked()
                {
                    actions.push(DisplayAction::Add);
                }
                let mut linked = model.display_states_linked();
                if ui
                    .checkbox(&mut linked, "Linked")
                    .on_hover_text("Link display states to configurations, as SolidWorks does: each configuration has its own, and shows the one it last showed. Off: every configuration can show every one.")
                    .changed()
                {
                    actions.push(DisplayAction::Link(linked));
                }
            });
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_names_are_not_taken() {
        let mut doc = Document::default();
        doc.change("Assembly", |m| *m = peet_model::Model::new_assembly());
        assert_eq!(new_name(&doc), "Display State-1");
        doc.change("Add", |m| {
            m.add_display_state("Display State-1", None).unwrap();
            m.add_display_state("Display State-3", None).unwrap();
        });
        assert_eq!(new_name(&doc), "Display State-4");
    }
}
