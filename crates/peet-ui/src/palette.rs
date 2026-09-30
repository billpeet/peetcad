//! The command palette (Ctrl+K): fuzzy search over every command.

use egui::{Align2, Key, Modifiers, Sense, TextEdit, vec2};

use crate::commands::{CommandId, CommandState, fuzzy_score};

#[derive(Default)]
pub struct CommandPalette {
    open: bool,
    query: String,
    selected: usize,
}

impl CommandPalette {
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open = true;
            self.query.clear();
            self.selected = 0;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Shows the palette if open. Returns the command the user chose, if any.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        state: impl Fn(CommandId) -> CommandState,
    ) -> Option<CommandId> {
        if !self.open {
            return None;
        }

        // Rank the commands for the current query.
        let mut matches: Vec<(i32, CommandId)> = CommandId::ALL
            .into_iter()
            .filter(|c| c.available())
            .filter_map(|c| {
                let info = c.info();
                let haystack = format!("{}: {}", info.category, info.label);
                let score = fuzzy_score(&self.query, &haystack)?;
                // Commands that can't run right now sink to the bottom.
                let penalty = if state(c).enabled { 0 } else { 10_000 };
                Some((score - penalty, c))
            })
            .collect();
        if !self.query.is_empty() {
            matches.sort_by_key(|(score, _)| -score);
        }
        self.selected = self.selected.min(matches.len().saturating_sub(1));

        // Keyboard navigation, taken before the text box sees the keys.
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.close();
            return None;
        }
        if down && !matches.is_empty() {
            self.selected = (self.selected + 1) % matches.len();
        }
        if up && !matches.is_empty() {
            self.selected = (self.selected + matches.len() - 1) % matches.len();
        }

        let mut chosen = None;
        if enter && let Some((_, cmd)) = matches.get(self.selected) {
            chosen = Some(*cmd);
        }

        let area = egui::Area::new(egui::Id::new("command_palette"))
            .anchor(Align2::CENTER_TOP, vec2(0.0, 72.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(460.0);
                    let edit = ui.add(
                        TextEdit::singleline(&mut self.query)
                            .hint_text("Type a command…")
                            .desired_width(f32::INFINITY),
                    );
                    edit.request_focus();
                    if edit.changed() {
                        self.selected = 0;
                    }
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            if matches.is_empty() {
                                ui.weak("No matching commands");
                            }
                            for (index, (_, cmd)) in matches.iter().enumerate() {
                                let info = cmd.info();
                                let enabled = state(*cmd).enabled;
                                let is_selected = index == self.selected;
                                // Reserve a slot behind the row for its highlight.
                                let background = ui.painter().add(egui::Shape::Noop);
                                let row = ui.horizontal(|ui| {
                                    ui.set_width(ui.available_width());
                                    ui.weak(info.category);
                                    ui.add_enabled(
                                        enabled,
                                        egui::Label::new(info.label).selectable(false),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if let Some(sc) = info.shortcut {
                                                ui.weak(ctx.format_shortcut(&sc));
                                            }
                                        },
                                    );
                                });
                                let response = ui.interact(
                                    row.response.rect,
                                    row.response.id.with("row"),
                                    Sense::click(),
                                );
                                if is_selected || response.hovered() {
                                    ui.painter().set(
                                        background,
                                        egui::Shape::rect_filled(
                                            row.response.rect.expand(2.0),
                                            3.0,
                                            ui.visuals().selection.bg_fill.gamma_multiply(0.45),
                                        ),
                                    );
                                }
                                if is_selected && (up || down) {
                                    response.scroll_to_me(None);
                                }
                                if response.clicked() && enabled {
                                    chosen = Some(*cmd);
                                }
                                response.on_hover_text(info.description);
                            }
                        });
                });
            });

        // Clicking anywhere outside closes the palette.
        let clicked_outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p))
        });
        if clicked_outside {
            self.close();
        }

        if let Some(cmd) = chosen
            && state(cmd).enabled
        {
            self.close();
            return Some(cmd);
        }
        None
    }
}
