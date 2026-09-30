//! The PeetCAD application shell: menus, toolbar, panels, windows and command dispatch.

use eframe::egui_wgpu::RenderState;
use egui::{Align, Layout, RichText, Ui};
use peet_platform::Instant;
use peet_render::{Projection, StandardView};

use crate::commands::{CommandId, CommandState};
use crate::document::{DEMO_BODY, Document, ItemId, ItemKind};
use crate::palette::CommandPalette;
use crate::perf::{PerfInfo, PerfMonitor};
use crate::settings::{MousePreset, OrbitChoice, STORAGE_KEY, Settings, ThemeChoice};
use crate::viewport::{Viewport, ViewportParams};

pub const APP_NAME: &str = "PeetCAD";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default)]
struct OpenWindows {
    settings: bool,
    shortcuts: bool,
    about: bool,
}

pub struct PeetApp {
    settings: Settings,
    applied_theme: Option<ThemeChoice>,
    document: Document,
    selected: Option<ItemId>,
    /// Item under the cursor in the feature tree this frame (highlighted in the viewport).
    hovered: Option<ItemId>,
    render_state: Option<RenderState>,
    viewport: Option<Viewport>,
    initial_fit_done: bool,
    perf: PerfMonitor,
    palette: CommandPalette,
    windows: OpenWindows,
    adapter_name: String,
    backend_name: String,
}

impl PeetApp {
    /// Creates the app. `process_start` is used to report startup time.
    pub fn new(cc: &eframe::CreationContext<'_>, process_start: Instant) -> Self {
        log::info!(
            "Window and GPU ready {:.0} ms after start",
            peet_platform::elapsed_ms(process_start)
        );
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();

        let render_state = cc.wgpu_render_state.clone();
        let (adapter_name, backend_name) = render_state.as_ref().map_or_else(
            || ("none".to_owned(), "none".to_owned()),
            |rs| {
                let info = rs.adapter.get_info();
                // Browsers hide the GPU model from WebGPU pages for privacy.
                let name = if info.name.is_empty() {
                    "Browser GPU".to_owned()
                } else {
                    info.name
                };
                (name, format!("{:?}", info.backend))
            },
        );
        log::info!("GPU: {adapter_name} ({backend_name})");

        let pipelines_start = Instant::now();
        let mut viewport = render_state.as_ref().map(Viewport::new);
        log::info!(
            "Viewport renderer created in {:.0} ms",
            peet_platform::elapsed_ms(pipelines_start)
        );
        if let Some(vp) = &mut viewport {
            vp.camera.projection = if settings.perspective {
                Projection::Perspective
            } else {
                Projection::Orthographic
            };
        }

        Self {
            settings,
            applied_theme: None,
            document: Document::default(),
            selected: None,
            hovered: None,
            render_state,
            viewport,
            initial_fit_done: false,
            perf: PerfMonitor::new(process_start),
            palette: CommandPalette::default(),
            windows: OpenWindows::default(),
            adapter_name,
            backend_name,
        }
    }

    fn command_state(&self, cmd: CommandId) -> CommandState {
        let has_viewport = self.viewport.is_some();
        let on = |checked: bool| CommandState {
            enabled: true,
            checked: Some(checked),
        };
        let enabled = |enabled: bool| CommandState {
            enabled,
            checked: None,
        };
        match cmd {
            // Undo arrives with the document model (Phase 3).
            CommandId::Undo | CommandId::Redo => enabled(false),
            CommandId::ViewIsometric
            | CommandId::ViewFront
            | CommandId::ViewBack
            | CommandId::ViewLeft
            | CommandId::ViewRight
            | CommandId::ViewTop
            | CommandId::ViewBottom
            | CommandId::ZoomToFit => enabled(has_viewport),
            CommandId::ToggleProjection => on(self.settings.perspective),
            CommandId::ToggleGrid => on(self.settings.show_grid),
            CommandId::ToggleReferencePlanes => on(self.document.planes_visible()),
            CommandId::ToggleViewCube => on(self.settings.show_view_cube),
            CommandId::ToggleFeatureTree => on(self.settings.show_feature_tree),
            CommandId::ToggleProperties => on(self.settings.show_properties),
            CommandId::TogglePerfOverlay => on(self.settings.show_perf_overlay),
            CommandId::ToggleDemoPart => {
                on(self.document.item(DEMO_BODY).is_some_and(|i| i.visible))
            }
            CommandId::CommandPalette
            | CommandId::Settings
            | CommandId::KeyboardShortcuts
            | CommandId::About
            | CommandId::Quit => enabled(true),
        }
    }

    fn execute(&mut self, ctx: &egui::Context, cmd: CommandId) {
        if !self.command_state(cmd).enabled {
            return;
        }
        log::debug!("Command: {cmd:?}");
        let animate = self.settings.animate_views;
        let view = |vp: &mut Option<Viewport>, v: StandardView| {
            if let Some(vp) = vp {
                vp.set_view(v, animate);
            }
        };
        match cmd {
            CommandId::Undo | CommandId::Redo => {}
            CommandId::CommandPalette => self.palette.toggle(),
            CommandId::ViewIsometric => view(&mut self.viewport, StandardView::Isometric),
            CommandId::ViewFront => view(&mut self.viewport, StandardView::Front),
            CommandId::ViewBack => view(&mut self.viewport, StandardView::Back),
            CommandId::ViewLeft => view(&mut self.viewport, StandardView::Left),
            CommandId::ViewRight => view(&mut self.viewport, StandardView::Right),
            CommandId::ViewTop => view(&mut self.viewport, StandardView::Top),
            CommandId::ViewBottom => view(&mut self.viewport, StandardView::Bottom),
            CommandId::ZoomToFit => {
                let bounds = self.document.visible_body_bounds();
                if let Some(vp) = &mut self.viewport {
                    vp.zoom_to_fit(&bounds, animate);
                }
            }
            CommandId::ToggleProjection => {
                self.settings.perspective = !self.settings.perspective;
                if let Some(vp) = &mut self.viewport {
                    vp.set_projection(if self.settings.perspective {
                        Projection::Perspective
                    } else {
                        Projection::Orthographic
                    });
                }
            }
            CommandId::ToggleGrid => self.settings.show_grid ^= true,
            CommandId::ToggleReferencePlanes => {
                let visible = self.document.planes_visible();
                self.document.set_planes_visible(!visible);
            }
            CommandId::ToggleViewCube => self.settings.show_view_cube ^= true,
            CommandId::ToggleFeatureTree => self.settings.show_feature_tree ^= true,
            CommandId::ToggleProperties => self.settings.show_properties ^= true,
            CommandId::TogglePerfOverlay => self.settings.show_perf_overlay ^= true,
            CommandId::ToggleDemoPart => {
                if let Some(item) = self.document.item_mut(DEMO_BODY) {
                    item.visible = !item.visible;
                }
            }
            CommandId::Settings => self.windows.settings = true,
            CommandId::KeyboardShortcuts => self.windows.shortcuts = true,
            CommandId::About => self.windows.about = true,
            CommandId::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
        ctx.request_repaint();
    }

    /// Commands triggered by keyboard shortcuts this frame.
    fn shortcut_commands(&self, ctx: &egui::Context) -> Vec<CommandId> {
        // While typing in a text field, only the palette shortcut stays active.
        let typing = ctx.egui_wants_keyboard_input();
        CommandId::ALL
            .into_iter()
            .filter(|c| c.available())
            .filter(|c| !typing || *c == CommandId::CommandPalette)
            .filter_map(|cmd| {
                let shortcut = cmd.info().shortcut?;
                if !self.command_state(cmd).enabled {
                    return None;
                }
                ctx.input_mut(|i| i.consume_shortcut(&shortcut))
                    .then_some(cmd)
            })
            .collect()
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied_theme != Some(self.settings.theme) {
            // egui already follows the system theme by default. Skipping the initial call
            // avoids an "unhandled viewport command" warning from the web backend.
            let initial_system =
                self.applied_theme.is_none() && self.settings.theme == ThemeChoice::System;
            if !initial_system {
                ctx.set_theme(self.settings.theme.preference());
            }
            self.applied_theme = Some(self.settings.theme);
        }
    }

    fn menu_bar(&self, ui: &mut Ui, pending: &mut Vec<CommandId>) {
        egui::MenuBar::new().ui(ui, |ui| {
            let mut item = |ui: &mut Ui, cmd: CommandId| {
                if cmd.available() && menu_button(ui, cmd, self.command_state(cmd)) {
                    pending.push(cmd);
                    ui.close();
                }
            };
            ui.menu_button("File", |ui| {
                item(ui, CommandId::Settings);
                if CommandId::Quit.available() {
                    ui.separator();
                    item(ui, CommandId::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                item(ui, CommandId::Undo);
                item(ui, CommandId::Redo);
            });
            ui.menu_button("View", |ui| {
                ui.menu_button("Standard Views", |ui| {
                    for cmd in [
                        CommandId::ViewIsometric,
                        CommandId::ViewFront,
                        CommandId::ViewTop,
                        CommandId::ViewRight,
                        CommandId::ViewBack,
                        CommandId::ViewBottom,
                        CommandId::ViewLeft,
                    ] {
                        item(ui, cmd);
                    }
                });
                item(ui, CommandId::ZoomToFit);
                ui.separator();
                item(ui, CommandId::ToggleProjection);
                item(ui, CommandId::ToggleGrid);
                item(ui, CommandId::ToggleReferencePlanes);
                item(ui, CommandId::ToggleViewCube);
                item(ui, CommandId::ToggleDemoPart);
            });
            ui.menu_button("Window", |ui| {
                item(ui, CommandId::ToggleFeatureTree);
                item(ui, CommandId::ToggleProperties);
                item(ui, CommandId::TogglePerfOverlay);
            });
            ui.menu_button("Help", |ui| {
                item(ui, CommandId::CommandPalette);
                item(ui, CommandId::KeyboardShortcuts);
                ui.separator();
                item(ui, CommandId::About);
            });
        });
    }

    fn toolbar(&self, ui: &mut Ui, pending: &mut Vec<CommandId>) {
        ui.horizontal(|ui| {
            let mut tool = |ui: &mut Ui, cmd: CommandId, label: &str| {
                if tool_button(ui, cmd, label, self.command_state(cmd)) {
                    pending.push(cmd);
                }
            };
            ui.weak("View");
            tool(ui, CommandId::ViewIsometric, "Iso");
            tool(ui, CommandId::ViewFront, "Front");
            tool(ui, CommandId::ViewTop, "Top");
            tool(ui, CommandId::ViewRight, "Right");
            tool(ui, CommandId::ZoomToFit, "Fit");
            ui.separator();
            tool(ui, CommandId::ToggleProjection, "Perspective");
            tool(ui, CommandId::ToggleGrid, "Grid");
            tool(ui, CommandId::ToggleReferencePlanes, "Planes");
            ui.separator();
            ui.weak("Sketch · Features · Sheet Metal: coming soon");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let palette = CommandId::CommandPalette;
                let label = format!(
                    "Commands  {}",
                    ui.ctx()
                        .format_shortcut(&palette.info().shortcut.expect("has shortcut"))
                );
                if ui
                    .button(label)
                    .on_hover_text(palette.info().description)
                    .clicked()
                {
                    pending.push(palette);
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.weak(self.settings.mouse_preset.status_hint());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label("mm");
                ui.separator();
                ui.label(if self.settings.perspective {
                    "Perspective"
                } else {
                    "Orthographic"
                });
                if let Some(p) = self.viewport.as_ref().and_then(|v| v.cursor_on_ground) {
                    ui.separator();
                    ui.monospace(format!("X {:>9.2}  Y {:>9.2}", p.x, p.y));
                }
            });
        });
    }

    fn feature_tree(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.strong("Feature Tree");
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new(&self.document.name).strong())
                .default_open(true)
                .show(ui, |ui| {
                    let mut last_was_reference = true;
                    for item in &mut self.document.items {
                        let is_reference = !matches!(item.kind, ItemKind::Body { .. });
                        if last_was_reference && !is_reference {
                            ui.separator();
                        }
                        last_was_reference = is_reference;
                        let row = ui.horizontal(|ui| {
                            ui.checkbox(&mut item.visible, "")
                                .on_hover_text(if item.visible { "Hide" } else { "Show" });
                            let selected = self.selected == Some(item.id);
                            let label = ui.selectable_label(selected, &item.name);
                            if label.clicked() {
                                self.selected = if selected { None } else { Some(item.id) };
                            }
                            label
                        });
                        if row.inner.hovered() {
                            self.hovered = Some(item.id);
                        }
                    }
                });
            ui.add_space(8.0);
            ui.weak("Sketches and features will appear here.");
        });
    }

    fn properties(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.strong("Properties");
        ui.separator();
        let Some(item) = self.selected.and_then(|id| self.document.item_mut(id)) else {
            ui.weak("Select an item in the feature tree to see its properties.");
            return;
        };
        egui::Grid::new("properties")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut item.name);
                ui.end_row();
                ui.label("Type");
                ui.label(item.kind.type_name());
                ui.end_row();
                ui.label("Visible");
                ui.checkbox(&mut item.visible, "");
                ui.end_row();
                match &item.kind {
                    ItemKind::Origin => {
                        ui.label("Position");
                        ui.monospace("0, 0, 0");
                        ui.end_row();
                    }
                    ItemKind::ReferencePlane(which) => {
                        let n = which.plane().normal();
                        ui.label("Normal");
                        ui.monospace(format!("{:.0}, {:.0}, {:.0}", n.x, n.y, n.z));
                        ui.end_row();
                    }
                    ItemKind::Body { mesh } => {
                        let size = mesh.bounds().size();
                        ui.label("Size");
                        ui.monospace(format!("{:.1} × {:.1} × {:.1} mm", size.x, size.y, size.z));
                        ui.end_row();
                        ui.label("Triangles");
                        ui.monospace(mesh.triangle_count().to_string());
                        ui.end_row();
                    }
                }
            });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.windows.settings;
        let mut projection_changed = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let s = &mut self.settings;
                ui.heading("Appearance");
                egui::ComboBox::from_label("Theme")
                    .selected_text(s.theme.label())
                    .show_ui(ui, |ui| {
                        for t in ThemeChoice::ALL {
                            ui.selectable_value(&mut s.theme, t, t.label());
                        }
                    });
                ui.add_space(8.0);
                ui.heading("Mouse navigation");
                egui::ComboBox::from_label("Mouse preset")
                    .selected_text(s.mouse_preset.label())
                    .show_ui(ui, |ui| {
                        for p in MousePreset::ALL {
                            ui.selectable_value(&mut s.mouse_preset, p, p.label());
                        }
                    });
                ui.horizontal(|ui| {
                    ui.label("Rotation");
                    ui.radio_value(&mut s.orbit, OrbitChoice::Turntable, "Turntable")
                        .on_hover_text("Keeps the Z axis vertical. Best for most parts.");
                    ui.radio_value(&mut s.orbit, OrbitChoice::Trackball, "Trackball")
                        .on_hover_text("Rotates freely around the screen axes.");
                });
                ui.checkbox(&mut s.invert_zoom, "Invert mouse wheel zoom");
                ui.checkbox(&mut s.animate_views, "Animate view changes");
                ui.add_space(8.0);
                ui.heading("Viewport");
                projection_changed |= ui
                    .checkbox(&mut s.perspective, "Perspective projection")
                    .changed();
                ui.checkbox(&mut s.show_grid, "Show grid");
                ui.checkbox(&mut s.show_view_cube, "Show view cube");
                ui.add_space(8.0);
                if ui.button("Reset to defaults").clicked() {
                    *s = Settings::default();
                    projection_changed = true;
                }
            });
        self.windows.settings = open;
        if projection_changed && let Some(vp) = &mut self.viewport {
            vp.set_projection(if self.settings.perspective {
                Projection::Perspective
            } else {
                Projection::Orthographic
            });
        }
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let preset = self.settings.mouse_preset;
        egui::Window::new("Keyboard & Mouse")
            .open(&mut self.windows.shortcuts)
            .collapsible(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.heading(format!("Mouse ({})", preset.label()));
                egui::Grid::new("mouse_help")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (input, action) in preset.describe() {
                            ui.label(input);
                            ui.label(action);
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                ui.heading("Keyboard");
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        egui::Grid::new("key_help")
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                for cmd in CommandId::ALL.into_iter().filter(|c| c.available()) {
                                    let info = cmd.info();
                                    if let Some(sc) = info.shortcut {
                                        ui.label(format!("{}: {}", info.category, info.label));
                                        ui.monospace(ctx.format_shortcut(&sc));
                                        ui.end_row();
                                    }
                                }
                            });
                    });
            });
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let (adapter, backend) = (&self.adapter_name, &self.backend_name);
        egui::Window::new("About PeetCAD")
            .open(&mut self.windows.about)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading(format!("{APP_NAME} {VERSION}"));
                ui.label("A lightweight parametric CAD application, with sheet metal first.");
                ui.add_space(6.0);
                egui::Grid::new("about").num_columns(2).show(ui, |ui| {
                    ui.weak("Platform");
                    ui.label(peet_platform::target_description());
                    ui.end_row();
                    ui.weak("GPU");
                    ui.label(format!("{adapter} ({backend})"));
                    ui.end_row();
                    ui.weak("License");
                    ui.label("MIT");
                    ui.end_row();
                });
            });
    }
}

/// A menu entry for a command, with its shortcut. Returns true if clicked.
fn menu_button(ui: &mut Ui, cmd: CommandId, state: CommandState) -> bool {
    let info = cmd.info();
    let mut button = match state.checked {
        Some(checked) => egui::Button::selectable(checked, info.label),
        None => egui::Button::new(info.label),
    };
    if let Some(sc) = info.shortcut {
        button = button.shortcut_text(ui.ctx().format_shortcut(&sc));
    }
    ui.add_enabled(state.enabled, button)
        .on_hover_text(info.description)
        .clicked()
}

/// A compact toolbar button for a command. Returns true if clicked.
fn tool_button(ui: &mut Ui, cmd: CommandId, label: &str, state: CommandState) -> bool {
    let info = cmd.info();
    let button = match state.checked {
        Some(checked) => egui::Button::selectable(checked, label),
        None => egui::Button::new(label),
    };
    let tip = match info.shortcut {
        Some(sc) => format!(
            "{}\n\nShortcut: {}",
            info.description,
            ui.ctx().format_shortcut(&sc)
        ),
        None => info.description.to_owned(),
    };
    ui.add_enabled(state.enabled, button)
        .on_hover_text(tip)
        .clicked()
}

impl eframe::App for PeetApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.perf.begin_frame();
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);

        let mut pending = self.shortcut_commands(&ctx);
        self.hovered = None;

        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui, &mut pending));
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            self.toolbar(ui, &mut pending);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));
        if self.settings.show_feature_tree {
            egui::Panel::left("feature_tree")
                .resizable(true)
                .default_size(220.0)
                .size_range(160.0..=480.0)
                .show(ui, |ui| self.feature_tree(ui));
        }
        if self.settings.show_properties {
            egui::Panel::right("properties")
                .resizable(true)
                .default_size(250.0)
                .size_range(180.0..=480.0)
                .show(ui, |ui| self.properties(ui));
        }

        let dark = ctx.theme() == egui::Theme::Dark;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let (Some(viewport), Some(render_state)) = (&mut self.viewport, &self.render_state) else {
                ui.centered_and_justified(|ui| {
                    ui.label("The 3D viewport needs a GPU (WebGPU, WebGL2, DirectX 12 or Vulkan), and none was found.");
                });
                return;
            };
            let events = viewport.show(
                ui,
                &ViewportParams {
                    render_state,
                    settings: &self.settings,
                    document: &self.document,
                    selected: self.selected,
                    hovered: self.hovered,
                    dark,
                },
            );
            if events.clicked_background {
                self.selected = None;
            }
            if !self.initial_fit_done {
                viewport.zoom_to_fit(&self.document.visible_body_bounds(), false);
                self.initial_fit_done = true;
                ctx.request_repaint();
            }
            if self.settings.show_perf_overlay {
                let info = PerfInfo {
                    adapter: self.adapter_name.clone(),
                    backend: self.backend_name.clone(),
                    size_px: viewport.size_px,
                    samples: viewport.samples,
                    render_cpu_ms: viewport.render_cpu_ms,
                    triangles: viewport.stats.triangles,
                    lines: viewport.stats.lines,
                    draw_calls: viewport.stats.draw_calls,
                };
                let rect = ui.max_rect();
                self.perf.show(ui, rect, &info);
            }
        });

        self.settings_window(&ctx);
        self.shortcuts_window(&ctx);
        self.about_window(&ctx);
        let states = CommandId::ALL.map(|c| (c, self.command_state(c)));
        let state_of = |c: CommandId| {
            states
                .iter()
                .find(|(id, _)| *id == c)
                .map(|(_, s)| *s)
                .unwrap_or_default()
        };
        if let Some(cmd) = self.palette.show(&ctx, state_of) {
            pending.push(cmd);
        }

        for cmd in pending {
            self.execute(&ctx, cmd);
        }
        self.perf.end_frame();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
    }
}
