//! Performance overlay: frame timings, renderer statistics and startup time.
//!
//! egui only repaints when something changes, so an idle PeetCAD uses no CPU. That also
//! means "FPS" is meaningless while idle; the overlay therefore reports the CPU time spent
//! building each frame, and offers a continuous-repaint mode for benchmarking.

use std::collections::VecDeque;

use egui::{Color32, Rect, Stroke, Ui, pos2, vec2};
use peet_platform::Instant;

const HISTORY: usize = 240;

pub struct PerfMonitor {
    /// CPU time spent in our per-frame UI + render code, in ms.
    cpu_ms: VecDeque<f32>,
    /// Time between the starts of consecutive frames, in ms.
    interval_ms: VecDeque<f32>,
    frame_start: Option<Instant>,
    last_frame_start: Option<Instant>,
    /// Time from process start to the end of the first frame, once known.
    pub startup_ms: Option<f64>,
    process_start: Instant,
    /// Repaint every frame instead of only on input, to measure sustained frame rates.
    pub continuous: bool,
}

impl PerfMonitor {
    pub fn new(process_start: Instant) -> Self {
        Self {
            cpu_ms: VecDeque::with_capacity(HISTORY),
            interval_ms: VecDeque::with_capacity(HISTORY),
            frame_start: None,
            last_frame_start: None,
            startup_ms: None,
            process_start,
            continuous: false,
        }
    }

    pub fn begin_frame(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_frame_start {
            push(&mut self.interval_ms, (now - last).as_secs_f32() * 1000.0);
        }
        self.last_frame_start = Some(now);
        self.frame_start = Some(now);
    }

    pub fn end_frame(&mut self) {
        if let Some(start) = self.frame_start.take() {
            push(&mut self.cpu_ms, start.elapsed().as_secs_f32() * 1000.0);
        }
        if self.startup_ms.is_none() {
            let ms = peet_platform::elapsed_ms(self.process_start);
            log::info!("First frame ready {ms:.0} ms after start");
            self.startup_ms = Some(ms);
        }
    }

    /// Draws the overlay in the top-left corner of the viewport.
    pub fn show(&mut self, ui: &mut Ui, viewport: Rect, info: &PerfInfo) {
        let (avg, max) = stats(&self.cpu_ms);
        let (avg_interval, _) = stats(&self.interval_ms);
        egui::Area::new(egui::Id::new("perf_overlay"))
            .fixed_pos(viewport.left_top() + vec2(8.0, 8.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(ui.visuals().extreme_bg_color.gamma_multiply(0.92))
                    .show(ui, |ui| {
                        ui.set_width(250.0);
                        egui::Grid::new("perf_grid")
                            .num_columns(2)
                            .spacing([12.0, 2.0])
                            .show(ui, |ui| {
                                let mut row = |label: &str, value: String| {
                                    ui.weak(label);
                                    ui.monospace(value);
                                    ui.end_row();
                                };
                                row("GPU", info.adapter.clone());
                                row("Backend", info.backend.clone());
                                row(
                                    "Viewport",
                                    format!(
                                        "{}×{} px, MSAA {}x",
                                        info.size_px[0], info.size_px[1], info.samples
                                    ),
                                );
                                row("Frame CPU", format!("{avg:.2} ms avg, {max:.2} max"));
                                row("Render CPU", format!("{:.2} ms", info.render_cpu_ms));
                                if self.continuous && avg_interval > 0.0 {
                                    row("Frame rate", format!("{:.0} FPS", 1000.0 / avg_interval));
                                }
                                row("Triangles", info.triangles.to_string());
                                row("Lines", info.lines.to_string());
                                row("Draw calls", info.draw_calls.to_string());
                                if let Some(ms) = self.startup_ms {
                                    row("Startup", format!("{ms:.0} ms to first frame"));
                                }
                            });
                        sparkline(ui, &self.cpu_ms);
                        ui.checkbox(&mut self.continuous, "Continuous repaint (benchmark)");
                    });
            });
        if self.continuous {
            ui.ctx().request_repaint();
        }
    }
}

/// Renderer details shown in the overlay.
pub struct PerfInfo {
    pub adapter: String,
    pub backend: String,
    pub size_px: [u32; 2],
    pub samples: u32,
    pub render_cpu_ms: f64,
    pub triangles: usize,
    pub lines: usize,
    pub draw_calls: usize,
}

fn push(buf: &mut VecDeque<f32>, value: f32) {
    if buf.len() == HISTORY {
        buf.pop_front();
    }
    buf.push_back(value);
}

fn stats(buf: &VecDeque<f32>) -> (f32, f32) {
    if buf.is_empty() {
        return (0.0, 0.0);
    }
    let sum: f32 = buf.iter().sum();
    let max = buf.iter().copied().fold(0.0, f32::max);
    (sum / buf.len() as f32, max)
}

/// A tiny bar chart of recent frame CPU times, with a reference line at 8 ms (120 FPS budget).
fn sparkline(ui: &mut Ui, values: &VecDeque<f32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let budget_ms = 8.0;
    let scale_max = values.iter().copied().fold(budget_ms * 1.5, f32::max);
    let y_of = |ms: f32| rect.bottom() - (ms / scale_max).min(1.0) * rect.height();
    let bar_w = rect.width() / HISTORY as f32;
    for (i, &ms) in values.iter().enumerate() {
        let x = rect.left() + i as f32 * bar_w;
        let color = if ms > budget_ms {
            Color32::from_rgb(230, 120, 60)
        } else {
            Color32::from_rgb(90, 170, 110)
        };
        painter.rect_filled(
            Rect::from_min_max(pos2(x, y_of(ms)), pos2(x + bar_w.max(1.0), rect.bottom())),
            0.0,
            color,
        );
    }
    let y = y_of(budget_ms);
    painter.line_segment(
        [pos2(rect.left(), y), pos2(rect.right(), y)],
        Stroke::new(1.0, ui.visuals().weak_text_color()),
    );
}
