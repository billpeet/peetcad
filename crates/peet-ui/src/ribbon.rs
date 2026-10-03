//! A ribbon-style toolbar: commands in captioned groups, with large icon-over-label buttons
//! for the main commands and small icon-beside-label buttons stacked three high for the
//! rest.

use egui::{Align2, Color32, FontId, Rect, Response, Sense, Ui, vec2};

use crate::commands::{CommandId, CommandState};
use crate::icons::{Category, Icon};

/// Height of the button area of a group (three small buttons, or one large one).
const BODY_HEIGHT: f32 = 64.0;
const SMALL_HEIGHT: f32 = 20.0;
const LARGE_ICON: f32 = 26.0;
const SMALL_ICON: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Large,
    Small,
    /// Just the icon (the quick-access buttons); the label shows in the tooltip.
    Icon,
}

/// A captioned group of buttons, followed by a divider.
pub fn group(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let body = ui
            .horizontal(|ui| {
                ui.set_height(BODY_HEIGHT);
                ui.spacing_mut().item_spacing.x = 2.0;
                add_contents(ui);
            })
            .response
            .rect;
        let (rect, _) = ui.allocate_exact_size(vec2(body.width(), 15.0), Sense::hover());
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            title,
            FontId::proportional(10.5),
            ui.visuals().weak_text_color(),
        );
    });
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, BODY_HEIGHT + 12.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(4.0),
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
}

/// Small buttons stacked in a column.
pub fn stack(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.add_space(1.0);
        add_contents(ui);
    });
}

/// A command button. Returns true if clicked.
pub fn command(ui: &mut Ui, cmd: CommandId, label: &str, state: CommandState, size: Size) -> bool {
    let icon = Icon::for_command(cmd);
    let selected = state.checked == Some(true);
    let r = paint_button(ui, icon, label, size, state.enabled, selected, false);
    r.on_hover_text(tooltip(ui, cmd, label)).clicked() && state.enabled
}

/// A button opening a menu.
pub fn dropdown(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    tip: &str,
    size: Size,
    add_contents: impl FnOnce(&mut Ui),
) {
    let r = paint_button(ui, Some(icon), label, size, true, false, true).on_hover_text(tip);
    egui::Popup::menu(&r).show(add_contents);
}

fn tooltip(ui: &Ui, cmd: CommandId, label: &str) -> String {
    let info = cmd.info();
    match info.shortcut {
        Some(sc) => format!(
            "{label}\n{}\n\nShortcut: {}",
            info.description,
            ui.ctx().format_shortcut(&sc)
        ),
        None => format!("{label}\n{}", info.description),
    }
}

fn paint_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    size: Size,
    enabled: bool,
    selected: bool,
    caret: bool,
) -> Response {
    let dark = ui.visuals().dark_mode;
    let font = match size {
        Size::Large => FontId::proportional(11.5),
        Size::Small | Size::Icon => FontId::proportional(12.5),
    };
    let text_color = ui.visuals().text_color();
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), text_color);
    let caret_w = if caret { 9.0 } else { 0.0 };
    let desired = match size {
        Size::Large => vec2((galley.size().x + caret_w + 12.0).max(50.0), BODY_HEIGHT),
        Size::Small => vec2(
            SMALL_ICON + 6.0 + galley.size().x + caret_w + 10.0,
            SMALL_HEIGHT,
        ),
        Size::Icon => vec2(SMALL_HEIGHT + 4.0, SMALL_HEIGHT + 2.0),
    };
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, r) = ui.allocate_exact_size(desired, sense);
    if !ui.is_rect_visible(rect) {
        return r;
    }

    let visuals = ui.visuals();
    let accent = icon.map_or(Category::Neutral, Icon::category).color(dark);
    let bg = if selected {
        Some(visuals.selection.bg_fill.gamma_multiply(0.55))
    } else if enabled && (r.is_pointer_button_down_on() || r.has_focus()) {
        Some(visuals.widgets.active.weak_bg_fill)
    } else if enabled && r.hovered() {
        Some(visuals.widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    let painter = ui.painter();
    if let Some(bg) = bg {
        painter.rect_filled(rect, 5.0, bg);
    }
    if selected {
        painter.rect_stroke(
            rect,
            5.0,
            visuals.selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    let (fg, accent, text) = if enabled {
        (visuals.text_color(), accent, text_color)
    } else {
        let weak = visuals.weak_text_color().gamma_multiply(0.6);
        (weak, accent.gamma_multiply(0.3), weak)
    };

    match size {
        Size::Large => {
            let icon_rect = Rect::from_center_size(
                rect.center_top() + vec2(0.0, 6.0 + LARGE_ICON / 2.0),
                vec2(LARGE_ICON, LARGE_ICON),
            );
            if let Some(icon) = icon {
                icon.paint(painter, icon_rect, fg, accent);
            }
            let text_pos = rect.center_top() + vec2(-caret_w / 2.0, 6.0 + LARGE_ICON + 8.0);
            painter.text(text_pos, Align2::CENTER_TOP, label, font, text);
            if caret {
                caret_mark(
                    painter,
                    text_pos + vec2(galley.size().x / 2.0 + 6.0, 7.0),
                    text,
                );
            }
        }
        Size::Icon => {
            if let Some(icon) = icon {
                let r = Rect::from_center_size(rect.center(), vec2(SMALL_ICON, SMALL_ICON));
                icon.paint(painter, r, fg, accent);
            }
        }
        Size::Small => {
            let icon_rect = Rect::from_min_size(
                rect.left_top() + vec2(4.0, (SMALL_HEIGHT - SMALL_ICON) / 2.0),
                vec2(SMALL_ICON, SMALL_ICON),
            );
            if let Some(icon) = icon {
                icon.paint(painter, icon_rect, fg, accent);
            }
            let text_pos = egui::pos2(icon_rect.right() + 6.0, rect.center().y);
            painter.text(text_pos, Align2::LEFT_CENTER, label, font, text);
            if caret {
                caret_mark(painter, text_pos + vec2(galley.size().x + 6.0, 0.0), text);
            }
        }
    }
    r
}

fn caret_mark(painter: &egui::Painter, c: egui::Pos2, color: Color32) {
    painter.add(egui::Shape::convex_polygon(
        vec![
            c + vec2(-3.5, -2.0),
            c + vec2(3.5, -2.0),
            c + vec2(0.0, 2.0),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

/// A ribbon tab.
pub struct Tab<'a, T> {
    pub id: T,
    pub label: &'a str,
    /// Contextual tabs (shown only in a mode, like sketch editing) are tinted.
    pub contextual: Option<Category>,
}

/// The row of tabs. Clicking one makes it current.
pub fn tab_bar<T: Copy + PartialEq>(ui: &mut Ui, tabs: &[Tab<'_, T>], current: &mut T) {
    let dark = ui.visuals().dark_mode;
    for tab in tabs {
        let selected = *current == tab.id;
        let font = FontId::proportional(13.0);
        let tint = tab.contextual.map(|c| c.color(dark));
        let color = match (selected, tint) {
            (_, Some(t)) => t,
            (true, None) => ui.visuals().strong_text_color(),
            (false, None) => ui.visuals().text_color(),
        };
        let galley = ui
            .painter()
            .layout_no_wrap(tab.label.to_owned(), font, color);
        let size = vec2(galley.size().x + 22.0, 26.0);
        let (rect, r) = ui.allocate_exact_size(size, Sense::click());
        let visuals = ui.visuals();
        let painter = ui.painter();
        if selected {
            painter.rect_filled(
                rect,
                egui::CornerRadius {
                    nw: 6,
                    ne: 6,
                    sw: 0,
                    se: 0,
                },
                visuals.faint_bg_color,
            );
        } else if r.hovered() {
            painter.rect_filled(rect.shrink(2.0), 5.0, visuals.widgets.hovered.weak_bg_fill);
        }
        if let Some(t) = tint {
            painter.rect_filled(rect.shrink2(vec2(4.0, 3.0)), 5.0, t.gamma_multiply(0.15));
        }
        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
        if selected {
            let line = tint.unwrap_or(visuals.selection.stroke.color);
            painter.hline(
                rect.x_range().shrink(6.0),
                rect.bottom() - 1.5,
                egui::Stroke::new(2.5, line),
            );
        }
        if r.clicked() {
            *current = tab.id;
        }
    }
}
