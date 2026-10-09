//! The configurations list, above the feature tree: the part's configurations, with the
//! active one marked.
//!
//! Double-clicking a configuration makes it the active one; right-clicking offers the
//! rest. Like the tree, the list only reports what the user did ([`ConfigAction`]); the
//! app applies it as operations.

use egui::{RichText, Ui};

use peet_model::ConfigId;

use crate::document::Document;

/// Something the user did in the configurations list. Configurations are given by name,
/// as the operations take them.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigAction {
    Activate(String),
    /// A new configuration, a copy of the active one.
    Add,
    /// A new configuration, a copy of this one.
    Copy(String),
    Rename {
        from: String,
        to: String,
    },
    Delete(String),
}

/// What the list remembers between frames.
#[derive(Default)]
pub struct ConfigList {
    /// The configuration being renamed, the text so far, and whether its field has been
    /// given the keyboard yet.
    renaming: Option<(ConfigId, String, bool)>,
}

/// A name no configuration has yet: "Configuration2", "Configuration3", …
pub fn new_name(doc: &Document) -> String {
    let taken = |name: &str| doc.model.configuration_named(name).is_some();
    (doc.model.configurations().len() + 1..)
        .map(|n| format!("Configuration{n}"))
        .find(|name| !taken(name))
        .unwrap_or_default()
}

/// Draws the list. Returns what the user did.
pub fn configs_ui(ui: &mut Ui, doc: &Document, list: &mut ConfigList) -> Vec<ConfigAction> {
    let mut actions = Vec::new();
    let model = &doc.model;
    let active = model.active_configuration();
    let count = model.configurations().len();
    let title = if count > 1 {
        format!("Configurations: {}", active.name)
    } else {
        "Configurations".to_owned()
    };
    egui::CollapsingHeader::new(title)
        .id_salt("configurations")
        .default_open(false)
        .show(ui, |ui| {
            for c in model.configurations() {
                let is_active = c.id == active.id;
                if let Some((id, text, focused)) = &mut list.renaming
                    && *id == c.id
                {
                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(140.0));
                    if !*focused {
                        r.request_focus();
                        *focused = true;
                    }
                    if r.lost_focus() {
                        let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
                        let to = text.trim().to_owned();
                        if !cancelled && !to.is_empty() && to != c.name {
                            actions.push(ConfigAction::Rename {
                                from: c.name.clone(),
                                to,
                            });
                        }
                        list.renaming = None;
                    }
                    continue;
                }
                let text = if is_active {
                    RichText::new(format!("● {}", c.name)).strong()
                } else {
                    RichText::new(format!("○ {}", c.name))
                };
                let mut hint = if is_active {
                    "The active configuration: the one that is built, shown and exported."
                        .to_owned()
                } else {
                    "Double-click to make it the active configuration.".to_owned()
                };
                if !c.comment.is_empty() {
                    hint = format!("{}\n{hint}", c.comment);
                }
                let r = ui
                    .add(egui::Button::selectable(is_active, text))
                    .on_hover_text(hint);
                if r.double_clicked() && !is_active {
                    actions.push(ConfigAction::Activate(c.name.clone()));
                }
                r.context_menu(|ui| {
                    if ui
                        .add_enabled(!is_active, egui::Button::new("Make Active"))
                        .clicked()
                    {
                        actions.push(ConfigAction::Activate(c.name.clone()));
                        ui.close();
                    }
                    if ui.button("Rename").clicked() {
                        list.renaming = Some((c.id, c.name.clone(), false));
                        ui.close();
                    }
                    if ui
                        .button("Add a Copy")
                        .on_hover_text("A new configuration that starts the same as this one.")
                        .clicked()
                    {
                        actions.push(ConfigAction::Copy(c.name.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(count > 1, egui::Button::new("Delete"))
                        .on_disabled_hover_text("A part has at least one configuration.")
                        .clicked()
                    {
                        actions.push(ConfigAction::Delete(c.name.clone()));
                        ui.close();
                    }
                });
            }
            if ui
                .small_button("+ Add Configuration")
                .on_hover_text(
                    "A new configuration that starts as a copy of the active one. Features can then be suppressed, and parameters given other values, in it alone.",
                )
                .clicked()
            {
                actions.push(ConfigAction::Add);
            }
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_names_are_not_taken() {
        let mut doc = Document::default();
        assert_eq!(new_name(&doc), "Configuration2");
        doc.change("Add", |m| {
            m.add_configuration("Configuration2", None).unwrap();
            m.add_configuration("Configuration4", None).unwrap();
        });
        assert_eq!(new_name(&doc), "Configuration5");
        doc.change("Add", |m| {
            m.add_configuration("Configuration5", None).unwrap();
        });
        assert_eq!(new_name(&doc), "Configuration6");
    }
}
