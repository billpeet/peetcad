//! The application's side of general solid modelling: the commands that start a revolve, a
//! sweep, a fillet, a shell, a draft or a hole from the selection, STEP import, the measurements
//! shown for selected faces and edges, and the mass properties window.
//!
//! What these do to the part is done by the document (`peet_document`), so a script does
//! the same; this file only turns the selection into arguments and shows the results.

use egui::Ui;
use peet_document::MassTotal;
use peet_kernel::query::{Description, MassProperties};
use peet_math::DVec3;
use peet_model::{BlendKind, FeatureId, Operation};
use peet_sketch::expr::{LengthUnit, Units};

use peet_ops::{Blend, Draft, FeatureArgs, Hole, Loft, Op, Revolve, Shell, Source, Sweep};

use super::PeetApp;
use crate::bodies::GeomRef;
use crate::document::ItemId;
use crate::features_ui::{self, Slot};

/// Something the user has to read and dismiss: what an import left out, or why it failed.
pub(super) struct Notice {
    title: String,
    heading: String,
    lines: Vec<String>,
    error: bool,
}

/// Densities to choose from in the mass properties window, kg/m³.
const DENSITIES: [(&str, f64); 8] = [
    ("Steel", crate::settings::STEEL_DENSITY),
    ("Stainless steel", 8000.0),
    ("Aluminium", 2700.0),
    ("Brass", 8500.0),
    ("Copper", 8960.0),
    ("Titanium", 4500.0),
    ("ABS", 1050.0),
    ("PLA", 1240.0),
];

const LB_PER_KG: f64 = 2.204_622_621_8;

/// A number to six significant digits, without trailing zeros.
fn significant(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return if v == 0.0 {
            "0".to_owned()
        } else {
            "?".to_owned()
        };
    }
    let magnitude = v.abs().log10().floor() as i32;
    let decimals = (5 - magnitude).clamp(0, 12) as usize;
    let text = format!("{v:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

fn imperial(units: Units) -> bool {
    matches!(units.length, LengthUnit::Inch | LengthUnit::Ft)
}

/// "800 mm²".
fn area_text(units: Units, mm2: f64) -> String {
    let scale = units.length.mm_per_unit();
    format!(
        "{} {}²",
        significant(mm2 / (scale * scale)),
        units.length.suffix()
    )
}

/// "8000 mm³".
fn volume_text(units: Units, mm3: f64) -> String {
    let scale = units.length.mm_per_unit();
    format!(
        "{} {}³",
        significant(mm3 / (scale * scale * scale)),
        units.length.suffix()
    )
}

/// The mass of a volume: grams or kilograms in metric documents, pounds otherwise.
fn mass_text(units: Units, mm3: f64, density: f64) -> String {
    let kg = mm3 * 1e-9 * density;
    if imperial(units) {
        format!("{} lb", significant(kg * LB_PER_KG))
    } else if kg.abs() < 1.0 {
        format!("{} g", significant(kg * 1000.0))
    } else {
        format!("{} kg", significant(kg))
    }
}

/// Moments of inertia (mm⁵ at density 1) as mass × length², in the document's units.
fn inertia_text(units: Units, moments: [f64; 3], density: f64) -> String {
    let scale = units.length.mm_per_unit();
    let mass = if imperial(units) { LB_PER_KG } else { 1.0 };
    let values: Vec<String> = moments
        .iter()
        .map(|m| significant(m * 1e-9 * density * mass / (scale * scale)))
        .collect();
    format!(
        "{} {}·{}²",
        values.join(", "),
        if imperial(units) { "lb" } else { "kg" },
        units.length.suffix()
    )
}

/// "20, 10, 5 mm".
fn point_text(units: Units, p: DVec3) -> String {
    format!(
        "{}, {}, {} {}",
        units.format_length_value(p.x),
        units.format_length_value(p.y),
        units.format_length_value(p.z),
        units.length.suffix()
    )
}

/// The rows of one body (or of the total) in the mass properties window.
#[allow(clippy::too_many_arguments)]
fn mass_rows(
    ui: &mut Ui,
    id: usize,
    units: Units,
    density: f64,
    volume: f64,
    area: f64,
    centroid: DVec3,
    moments: [f64; 3],
    size: DVec3,
) {
    egui::Grid::new(("mass_rows", id))
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Volume");
            ui.monospace(volume_text(units, volume));
            ui.end_row();
            ui.label("Surface area");
            ui.monospace(area_text(units, area));
            ui.end_row();
            ui.label("Mass");
            ui.monospace(mass_text(units, volume, density));
            ui.end_row();
            ui.label("Centre of gravity")
                .on_hover_text("X, Y, Z from the part's origin.");
            ui.monospace(point_text(units, centroid));
            ui.end_row();
            ui.label("Principal moments")
                .on_hover_text("The moments of inertia about the three principal axes through the centre of gravity, smallest first.");
            ui.monospace(inertia_text(units, moments, density));
            ui.end_row();
            ui.label("Bounding box")
                .on_hover_text("The size along X, Y and Z.");
            ui.monospace(format!(
                "{} × {} × {} {}",
                units.format_length_value(size.x),
                units.format_length_value(size.y),
                units.format_length_value(size.z),
                units.length.suffix()
            ));
            ui.end_row();
        });
}

impl PeetApp {
    /// Adds one feature by an operation. Returns it, or shows why it couldn't be added.
    pub(super) fn add_by_operation(&mut self, feature: FeatureArgs) -> Option<FeatureId> {
        let reply = self.perform(Op::add(feature));
        reply.created.first().copied()
    }

    /// Shows a new feature's properties.
    fn select_new(&mut self, id: Option<FeatureId>) {
        if let Some(id) = id {
            self.selected = Some(ItemId::Feature(id));
            self.selected_geom.clear();
        }
    }

    /// Revolves the open or selected sketch about its centreline.
    pub(super) fn start_revolve(&mut self, operation: Operation) {
        let Some(sketch) = self.extrude_source() else {
            return;
        };
        self.close_sketch();
        // The operation picks the axis (the sketch's centreline) and, for the first body
        // of a part, makes it a new body.
        let revolve = Revolve {
            sketch: Some(sketch.into()),
            ..Default::default()
        };
        let id = self.add_by_operation(if operation == Operation::Cut {
            FeatureArgs::CutRevolve(revolve)
        } else {
            FeatureArgs::Revolve(revolve)
        });
        self.select_new(id);
        if id.is_some() {
            self.show_in_3d(sketch, 1.0);
        }
    }

    /// Sweeps the open or selected sketch. If a sketch is open and another one is
    /// selected in the tree, that one is the path; otherwise the path is chosen next.
    pub(super) fn start_sweep(&mut self, operation: Operation) {
        let Some(profile) = self.extrude_source() else {
            return;
        };
        let path = self.selected_sketch().filter(|s| *s != profile);
        self.close_sketch();
        let sweep = Sweep {
            profile: Some(profile.into()),
            // No path yet: it is chosen afterwards.
            path: Some(path.map(Into::into)),
            ..Default::default()
        };
        let id = self.add_by_operation(if operation == Operation::Cut {
            FeatureArgs::CutSweep(sweep)
        } else {
            FeatureArgs::Sweep(sweep)
        });
        self.select_new(id);
        let Some(id) = id else {
            return;
        };
        if path.is_some() {
            self.show_in_3d(profile, 1.0);
        } else if self.doc.sweep_path_choices(id).is_empty() {
            self.error("A sweep needs a path: draw it in a second sketch (lines and arcs, or a spline, starting on the profile's plane), then choose it as the Path in the properties.");
        } else {
            self.picking = Some((id, Slot::SweepPath));
            self.status_message = None;
        }
    }

    /// Lofts from the open or selected sketch, the first profile. If a sketch is open and
    /// another one is selected in the tree, that one is the second; further profiles are
    /// then clicked in the tree, one after the other.
    pub(super) fn start_loft(&mut self, operation: Operation) {
        let Some(first) = self.extrude_source() else {
            return;
        };
        let mut sections = vec![first];
        sections.extend(self.selected_sketch().filter(|s| *s != first));
        self.close_sketch();
        // The operation hides the profiles and, for the first body of a part, makes it a
        // new body.
        let loft = Loft {
            profiles: Some(sections.iter().map(|s| (*s).into()).collect()),
            ..Default::default()
        };
        let id = self.add_by_operation(if operation == Operation::Cut {
            FeatureArgs::CutLoft(loft)
        } else {
            FeatureArgs::Loft(loft)
        });
        self.select_new(id);
        let Some(id) = id else {
            return;
        };
        if sections.len() > 1 {
            self.show_in_3d(first, 1.0);
        }
        if !self.doc.loft_profile_choices(id).is_empty() {
            self.picking = Some((id, Slot::LoftProfile));
            self.status_message = None;
        } else if sections.len() < 2 {
            self.error("A loft needs at least two profiles: draw the next one in another sketch, on a different plane, then use Add profiles in the properties.");
        }
    }

    /// Adds a fillet or a chamfer on the selected edges, or one waiting for edges.
    pub(super) fn start_blend(&mut self, kind: BlendKind) {
        let edges = self.doc.edge_refs(&self.selected_geom);
        let pick = edges.is_empty();
        let blend = Blend {
            edges: Some(edges.into_iter().map(Into::into).collect()),
            ..Default::default()
        };
        let id = self.add_by_operation(match kind {
            BlendKind::Fillet => FeatureArgs::Fillet(blend),
            BlendKind::Chamfer => FeatureArgs::Chamfer(blend),
        });
        self.select_new(id);
        if pick && let Some(id) = id {
            self.picking = Some((id, Slot::BlendEdge));
            self.status_message = None;
        }
    }

    /// Shells the bodies, opening the selected faces.
    pub(super) fn start_shell(&mut self) {
        let open = self.doc.face_refs(&self.selected_geom, false);
        let closed = open.is_empty();
        let id = self.add_by_operation(FeatureArgs::Shell(Shell {
            open: Some(open.into_iter().map(Into::into).collect()),
            ..Default::default()
        }));
        self.select_new(id);
        if closed && id.is_some() {
            self.info("The body is now a closed hollow. To open it, use Add faces in the properties and click the faces to remove.");
        }
    }

    /// Drafts the selected faces; the neutral plane is picked next.
    pub(super) fn start_draft(&mut self) {
        let faces = self.doc.face_refs(&self.selected_geom, false);
        let first = if faces.is_empty() {
            Slot::DraftFace
        } else {
            Slot::DraftNeutral
        };
        let id = self.add_by_operation(FeatureArgs::Draft(Draft {
            faces: Some(faces.into_iter().map(Into::into).collect()),
            // The neutral plane is picked next.
            neutral: Some(None),
            ..Default::default()
        }));
        self.select_new(id);
        if let Some(id) = id {
            self.picking = Some((id, first));
            self.status_message = None;
        }
    }

    /// Drills holes at the points of the open or selected sketch.
    pub(super) fn start_hole(&mut self) {
        let Some(sketch) = self.extrude_source() else {
            return;
        };
        self.close_sketch();
        let holes = self.doc.hole_count(sketch);
        let id = self.add_by_operation(FeatureArgs::Hole(Hole {
            sketch: Some(sketch.into()),
            ..Default::default()
        }));
        self.select_new(id);
        if id.is_some() && holes == 0 {
            self.error("The sketch has no points to drill at. Edit the sketch and place a point (or a circle) where each hole goes.");
        }
    }

    /// Adds the solids of an opened STEP file as bodies.
    pub(super) fn finish_step_import(&mut self, file: &peet_platform::OpenedFile) {
        self.close_sketch();
        let reply = self.perform_quietly(Op::ImportStep {
            file: Source::loaded(file.name.clone(), file.path.clone(), file.bytes.clone()),
        });
        let imported = match reply.created.first() {
            Some(id) if reply.ok => Ok(*id),
            _ => Err(reply.json["error"]
                .as_str()
                .unwrap_or("Nothing was imported.")
                .to_owned()),
        };
        match imported {
            Ok(feature) => {
                self.select_new(Some(feature));
                let bounds = self.doc.visible_bounds();
                let animate = self.settings.animate_views;
                if let Some(vp) = &mut self.viewport {
                    vp.zoom_to_fit(&bounds, animate);
                }
                let bodies = reply.json["bodies"].as_u64().unwrap_or(0);
                let what = if bodies == 1 {
                    "1 body".to_owned()
                } else {
                    format!("{bodies} bodies")
                };
                self.info(format!(
                    "Imported {what} from {} as {}",
                    file.name,
                    self.doc.model.name_of(feature)
                ));
                let warnings: Vec<String> = reply.json["warnings"]
                    .as_array()
                    .map(|w| {
                        w.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                if !warnings.is_empty() {
                    self.notice = Some(Notice {
                        title: "Import STEP".to_owned(),
                        heading: format!("{} was imported ({what}), with these notes:", file.name),
                        lines: warnings,
                        error: false,
                    });
                }
            }
            Err(e) => {
                self.error(format!("Couldn't import {}", file.name));
                self.notice = Some(Notice {
                    title: "Import STEP".to_owned(),
                    heading: format!("{} couldn't be imported.", file.name),
                    lines: vec![e],
                    error: true,
                });
            }
        }
    }

    /// The message waiting to be read, if any.
    pub(super) fn notice_window(&mut self, ctx: &egui::Context) {
        let Some(notice) = &self.notice else {
            return;
        };
        let mut close = false;
        egui::Window::new(&notice.title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        if notice.error {
                            features_ui::ERROR
                        } else {
                            features_ui::WARNING
                        },
                        "⚠",
                    );
                    ui.label(&notice.heading);
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for line in &notice.lines {
                            ui.label(line);
                        }
                    });
                ui.add_space(8.0);
                if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    close = true;
                }
            });
        if close {
            self.notice = None;
        }
    }

    /// The exact measurements of one selected face, edge or vertex, as rows under its
    /// description.
    pub(super) fn measurement_rows(&self, ui: &mut Ui, index: usize, g: GeomRef) {
        let units = self.doc.model.parameters.units;
        let Some(description) = self.doc.describe(g) else {
            return;
        };
        egui::Grid::new(("measure", index))
            .num_columns(2)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                let round = |ui: &mut Ui, radius: Option<f64>| {
                    if let Some(r) = radius {
                        ui.label("Radius");
                        ui.monospace(units.format_length(r));
                        ui.end_row();
                        ui.label("Diameter");
                        ui.monospace(units.format_length(2.0 * r));
                        ui.end_row();
                    }
                };
                match description {
                    Description::Vertex { point } => {
                        ui.label("Position");
                        ui.monospace(point_text(units, point));
                        ui.end_row();
                    }
                    Description::Edge {
                        length,
                        radius,
                        center,
                    } => {
                        ui.label("Length");
                        ui.monospace(units.format_length(length));
                        ui.end_row();
                        round(ui, radius);
                        if let Some(c) = center {
                            ui.label("Centre");
                            ui.monospace(point_text(units, c));
                            ui.end_row();
                        }
                    }
                    Description::Face { area, radius } => {
                        ui.label("Area");
                        ui.monospace(area_text(units, area));
                        ui.end_row();
                        round(ui, radius);
                    }
                }
            });
    }

    /// The distance and angle between the two selected things, when exactly two are.
    pub(super) fn between_rows(&self, ui: &mut Ui) {
        let [a, b] = self.selected_geom[..] else {
            return;
        };
        let Some(between) = self.doc.measure_between(a, b) else {
            return;
        };
        let units = self.doc.model.parameters.units;
        ui.separator();
        ui.strong("Between the two");
        egui::Grid::new("measure_between")
            .num_columns(2)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                if let Some((distance, _, _)) = between.distance {
                    ui.label("Distance");
                    ui.monospace(units.format_length(distance));
                    ui.end_row();
                }
                if let Some(angle) = between.angle {
                    ui.label("Angle");
                    ui.monospace(units.format_angle(angle.to_degrees()));
                    ui.end_row();
                }
            });
        match (between.distance, between.angle) {
            (Some(_), _) => {
                ui.weak(format!("Measured {}.", between.note));
            }
            (None, Some(_)) => {
                ui.weak("They meet at an angle, so there is no single distance between them.");
            }
            (None, None) => {
                ui.weak("There is no single distance or angle between these two.");
            }
        }
    }

    /// Volume, mass, centre of gravity and inertia of every body, and of all together.
    pub(super) fn mass_properties_window(&mut self, ctx: &egui::Context) {
        if !self.windows.mass_properties {
            return;
        }
        let mut open = true;
        let units = self.doc.model.parameters.units;
        let mut density = self.settings.density;
        egui::Window::new("Mass Properties")
            .open(&mut open)
            .resizable(true)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Density");
                    ui.add(
                        egui::DragValue::new(&mut density)
                            .range(1.0..=30000.0)
                            .speed(10.0)
                            .suffix(" kg/m³"),
                    )
                    .on_hover_text("What a cubic metre of the material weighs. It is remembered for next time.");
                    egui::ComboBox::from_id_salt("density_material")
                        .selected_text(
                            DENSITIES
                                .iter()
                                .find(|(_, d)| (d - density).abs() < 0.5)
                                .map_or("Material…", |(name, _)| name),
                        )
                        .show_ui(ui, |ui| {
                            for (name, d) in DENSITIES {
                                if ui
                                    .selectable_label(
                                        (d - density).abs() < 0.5,
                                        format!("{name} ({d} kg/m³)"),
                                    )
                                    .clicked()
                                {
                                    density = d;
                                }
                            }
                        });
                });
                ui.separator();
                if self.doc.bodies.is_empty() {
                    ui.weak("There are no bodies yet. Extrude or revolve a sketch to make one.");
                    return;
                }
                let several = self.doc.bodies.len() > 1;
                // Measuring a curved body takes a moment: one new body per frame, so the
                // window stays responsive while a part with many bodies is worked out.
                let mut measured_now = false;
                let mut waiting = false;
                let mut parts: Vec<MassProperties> = Vec::new();
                egui::ScrollArea::vertical()
                    .max_height(460.0)
                    .show(ui, |ui| {
                        for (i, body) in self.doc.bodies.iter().enumerate() {
                            if several {
                                ui.strong(format!(
                                    "Body {} (from {})",
                                    i + 1,
                                    self.doc.model.name_of(body.origin)
                                ));
                            }
                            if !body.is_measured() {
                                if measured_now {
                                    waiting = true;
                                    ui.weak("Working it out…");
                                    ui.add_space(6.0);
                                    continue;
                                }
                                measured_now = true;
                            }
                            match body.mass_properties() {
                                Ok(m) => {
                                    mass_rows(
                                        ui,
                                        i,
                                        units,
                                        density,
                                        m.volume,
                                        m.area,
                                        m.centroid,
                                        m.principal_moments,
                                        m.bounds.size(),
                                    );
                                    parts.push(*m);
                                }
                                Err(e) => {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.colored_label(features_ui::ERROR, "⚠");
                                        ui.label(format!("This body can't be measured: {e}"));
                                    });
                                }
                            }
                            ui.add_space(6.0);
                        }
                        if several
                            && !waiting
                            && parts.len() == self.doc.bodies.len()
                            && let Some(total) = MassTotal::of(&parts)
                        {
                            ui.separator();
                            ui.strong("All bodies together");
                            mass_rows(
                                ui,
                                usize::MAX,
                                units,
                                density,
                                total.volume,
                                total.area,
                                total.centroid,
                                total.principal_moments,
                                total.bounds.size(),
                            );
                        }
                    });
                if waiting {
                    ui.ctx().request_repaint();
                }
                if self.doc.is_flat() {
                    ui.add_space(4.0);
                    ui.weak("Sheet metal parts are measured folded, also while the flat pattern is shown.");
                }
            });
        self.settings.density = density;
        self.windows.mass_properties = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandId;
    use peet_math::{DVec2, Plane};
    use peet_model::{FeatureKind, PlaneRef, RevolveAxisRef, Status, StdPlane};

    /// The app with a 40 × 20 × 10 block in it.
    fn app_with_block() -> PeetApp {
        let mut app = PeetApp::headless();
        app.doc.change("Add", |m| {
            let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
            if let Some(f) = m.feature_mut(s).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(&mut f.sketch, DVec2::ZERO, DVec2::new(40.0, 20.0));
            }
            m.add_extrude(s, Operation::Add);
        });
        app
    }

    fn top_face(app: &PeetApp) -> GeomRef {
        let body = &app.doc.bodies[0];
        let face = body
            .solid
            .face_ids()
            .find(|f| body.face_center(*f).z > 9.0)
            .unwrap();
        GeomRef::Face { body: 0, face }
    }

    fn edges_of_top(app: &PeetApp) -> Vec<GeomRef> {
        let body = &app.doc.bodies[0];
        body.solid
            .edge_ids()
            .filter(|e| (body.solid.edge(*e).point_at_fraction(0.5).z - 10.0).abs() < 1e-9)
            .map(|edge| GeomRef::Edge { body: 0, edge })
            .collect()
    }

    fn selected_kind(app: &PeetApp) -> FeatureKind {
        let id = app.selected_feature().expect("the new feature is selected");
        app.doc.feature(id).unwrap().kind.clone()
    }

    fn run(app: &mut PeetApp, cmd: CommandId) {
        assert!(app.command_state(cmd).enabled, "{cmd:?} is disabled");
        app.execute(&egui::Context::default(), cmd);
    }

    #[test]
    fn commands_need_something_to_work_on() {
        let app = PeetApp::headless();
        for cmd in [
            CommandId::Revolve,
            CommandId::CutRevolve,
            CommandId::Sweep,
            CommandId::CutSweep,
            CommandId::Loft,
            CommandId::CutLoft,
            CommandId::Fillet,
            CommandId::Chamfer,
            CommandId::Shell,
            CommandId::Draft,
            CommandId::Hole,
        ] {
            assert!(!app.command_state(cmd).enabled, "{cmd:?} in an empty part");
        }
        for cmd in [
            CommandId::ImportStep,
            CommandId::MassProperties,
            CommandId::OpenSampleHousing,
        ] {
            assert!(app.command_state(cmd).enabled, "{cmd:?}");
        }
        let app = app_with_block();
        for cmd in [
            CommandId::Fillet,
            CommandId::Chamfer,
            CommandId::Shell,
            CommandId::Draft,
        ] {
            assert!(app.command_state(cmd).enabled, "{cmd:?} with a body");
        }
        // The sketch commands need a sketch selected (or open).
        assert!(!app.command_state(CommandId::Revolve).enabled);
        assert!(!app.command_state(CommandId::Hole).enabled);
    }

    #[test]
    fn revolve_command_revolves_the_selected_sketch() {
        let mut app = PeetApp::headless();
        let mut sketch = FeatureId(0);
        app.doc.change("Sketch", |m| {
            sketch = m.add_sketch(PlaneRef::Standard(StdPlane::Front), StdPlane::Front.plane());
            if let Some(f) = m.feature_mut(sketch).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(
                    &mut f.sketch,
                    DVec2::new(10.0, 0.0),
                    DVec2::new(20.0, 30.0),
                );
                let axis = f.sketch.add_line(DVec2::ZERO, DVec2::new(0.0, 30.0));
                f.sketch.set_construction(axis, true);
            }
        });
        app.selected = Some(ItemId::Feature(sketch));
        run(&mut app, CommandId::Revolve);
        let FeatureKind::Revolve(r) = selected_kind(&app) else {
            panic!("not a revolve");
        };
        assert_eq!(r.sketch, sketch);
        assert!(matches!(r.axis, RevolveAxisRef::SketchLine(_)));
        assert_eq!(r.operation, Operation::NewBody);
        let id = app.selected_feature().unwrap();
        assert_eq!(app.doc.status(id), Some(&Status::Ok));
        assert_eq!(app.doc.bodies.len(), 1);

        run(&mut app, CommandId::Undo);
        assert!(app.doc.feature(id).is_none());
        assert!(app.doc.bodies.is_empty());
        assert_eq!(app.selected, None, "a feature that is gone is not selected");

        // The cut uses the same sketch, as a cut.
        app.selected = Some(ItemId::Feature(sketch));
        run(&mut app, CommandId::CutRevolve);
        assert!(matches!(
            selected_kind(&app),
            FeatureKind::Revolve(r) if r.operation == Operation::Cut
        ));
    }

    #[test]
    fn sweep_command_takes_its_path_from_the_tree() {
        let mut app = PeetApp::headless();
        let (mut profile, mut path) = (FeatureId(0), FeatureId(0));
        app.doc.change("Sketches", |m| {
            profile = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
            if let Some(f) = m.feature_mut(profile).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(
                    &mut f.sketch,
                    DVec2::new(-5.0, -5.0),
                    DVec2::new(5.0, 5.0),
                );
            }
            let front = StdPlane::Front.plane();
            path = m.add_sketch(PlaneRef::Standard(StdPlane::Front), front);
            if let Some(f) = m.feature_mut(path).and_then(|f| f.sketch_mut()) {
                let a = front.to_plane_coords(DVec3::ZERO);
                let b = front.to_plane_coords(DVec3::new(0.0, 0.0, 30.0));
                f.sketch.add_line(a, b);
            }
        });
        app.selected = Some(ItemId::Feature(profile));
        run(&mut app, CommandId::Sweep);
        let id = app.selected_feature().unwrap();
        assert!(matches!(
            selected_kind(&app),
            FeatureKind::Sweep(s) if s.profile == profile && s.path.is_none()
        ));
        assert_eq!(app.picking, Some((id, Slot::SweepPath)));

        // Clicking the profile itself is refused; clicking the other sketch picks it.
        app.apply_tree(vec![crate::tree::TreeAction::Select(Some(
            ItemId::Feature(profile),
        ))]);
        assert_eq!(app.picking, Some((id, Slot::SweepPath)));
        assert!(matches!(&app.status_message, Some((_, true))));
        app.apply_tree(vec![crate::tree::TreeAction::Select(Some(
            ItemId::Feature(path),
        ))]);
        assert_eq!(app.picking, None);
        assert_eq!(app.selected, Some(ItemId::Feature(id)));
        assert!(matches!(selected_kind(&app), FeatureKind::Sweep(s) if s.path == Some(path)));
        assert_eq!(app.doc.status(id), Some(&Status::Ok));
        assert_eq!(app.doc.bodies.len(), 1);

        run(&mut app, CommandId::Undo);
        run(&mut app, CommandId::Undo);
        assert!(app.doc.feature(id).is_none());
        assert!(app.doc.bodies.is_empty());
    }

    #[test]
    fn loft_command_takes_its_profiles_from_the_tree() {
        let mut app = PeetApp::headless();
        let (mut low, mut high) = (FeatureId(0), FeatureId(0));
        let square = |m: &mut peet_model::Model, id: FeatureId, half: f64| {
            if let Some(f) = m.feature_mut(id).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(
                    &mut f.sketch,
                    DVec2::new(-half, -half),
                    DVec2::new(half, half),
                );
            }
        };
        app.doc.change("Sketches", |m| {
            // Two 20 × 20 squares on parallel planes, 30 apart.
            low = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
            square(m, low, 10.0);
            let plane = m.add(FeatureKind::Plane(peet_model::PlaneDef::Offset {
                from: PlaneRef::Standard(StdPlane::Top),
                distance: peet_model::Scalar::new(30.0),
                flip: false,
            }));
            high = m.add_sketch(PlaneRef::Feature(plane), Plane::TOP);
            square(m, high, 10.0);
        });
        let select = |app: &mut PeetApp, id: FeatureId| {
            app.apply_tree(vec![crate::tree::TreeAction::Select(Some(
                ItemId::Feature(id),
            ))]);
        };
        app.selected = Some(ItemId::Feature(low));
        run(&mut app, CommandId::Loft);
        let id = app.selected_feature().unwrap();
        assert!(matches!(
            selected_kind(&app),
            FeatureKind::Loft(l) if l.sections == vec![low] && l.operation == Operation::NewBody
        ));
        assert_eq!(app.picking, Some((id, Slot::LoftProfile)));
        assert!(app.doc.status(id).unwrap().message().is_some());

        // A profile again is refused; the other sketch is added, and picking carries on.
        select(&mut app, low);
        assert!(matches!(&app.status_message, Some((_, true))));
        assert_eq!(app.picking, Some((id, Slot::LoftProfile)));
        select(&mut app, high);
        assert_eq!(app.picking, Some((id, Slot::LoftProfile)), "still picking");
        assert_eq!(app.selected, Some(ItemId::Feature(id)));
        assert!(
            matches!(selected_kind(&app), FeatureKind::Loft(l) if l.sections == vec![low, high])
        );
        assert_eq!(app.doc.status(id), Some(&Status::Ok));
        assert_eq!(app.doc.bodies.len(), 1);
        let volume = app.doc.bodies[0].mass_properties().unwrap().volume;
        assert!((volume - 12000.0).abs() < 1e-3, "{volume}");
        assert!(!app.doc.feature(high).unwrap().visible, "used, so hidden");

        // A sketch drawn after the loft can't be one of its profiles.
        let mut later = FeatureId(0);
        app.doc.change("Later", |m| {
            later = m.add_sketch(PlaneRef::Standard(StdPlane::Front), StdPlane::Front.plane());
        });
        select(&mut app, later);
        assert!(matches!(&app.status_message, Some((m, true)) if m.contains("after")));
        assert!(matches!(selected_kind(&app), FeatureKind::Loft(l) if l.sections.len() == 2));
        run(&mut app, CommandId::Undo);

        // One step for the profile, one for the loft.
        run(&mut app, CommandId::Undo);
        assert!(matches!(selected_kind(&app), FeatureKind::Loft(l) if l.sections == vec![low]));
        assert!(app.doc.feature(high).unwrap().visible);
        run(&mut app, CommandId::Undo);
        assert!(app.doc.feature(id).is_none());
        assert!(app.doc.bodies.is_empty());

        // With nothing else to loft to, the command says what is missing.
        app.doc.change("Fewer", |m| {
            m.remove(high);
        });
        app.picking = None;
        app.selected = Some(ItemId::Feature(low));
        run(&mut app, CommandId::CutLoft);
        assert!(matches!(
            selected_kind(&app),
            FeatureKind::Loft(l) if l.operation == Operation::Cut
        ));
        assert_eq!(app.picking, None);
        assert!(matches!(&app.status_message, Some((_, true))));
    }

    /// The loft's panel, as the app shows it: the list, its buttons and the failure message.
    #[test]
    fn loft_properties_draw() {
        let mut app = PeetApp::headless();
        let mut low = FeatureId(0);
        app.doc.change("Sketch", |m| {
            low = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
            if let Some(f) = m.feature_mut(low).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(&mut f.sketch, DVec2::ZERO, DVec2::new(20.0, 20.0));
            }
            m.add_sketch(PlaneRef::Standard(StdPlane::Front), StdPlane::Front.plane());
        });
        app.selected = Some(ItemId::Feature(low));
        run(&mut app, CommandId::Loft);
        let id = app.selected_feature().unwrap();
        assert_eq!(app.picking, Some((id, Slot::LoftProfile)));
        let steps = app.doc.revision;
        let mut frame = 0;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(320.0, 700.0))
            .build_ui(|ui| {
                // Picking, then not.
                if frame == 2 {
                    app.picking = None;
                }
                frame += 1;
                let mut pending = Vec::new();
                app.properties(ui, &mut pending);
                assert!(pending.is_empty(), "nothing was clicked");
            });
        for _ in 0..4 {
            harness.step();
        }
        for text in ["Profiles", "Add profiles", "Operation"] {
            use egui_kittest::kittest::Queryable as _;
            assert!(harness.query_by_label(text).is_some(), "no {text}");
        }
        drop(harness);
        assert_eq!(app.doc.revision, steps, "drawing changed nothing");
    }

    #[test]
    fn fillet_command_takes_the_selected_edges() {
        let mut app = app_with_block();
        let edges = edges_of_top(&app);
        assert_eq!(edges.len(), 4);
        app.selected_geom = vec![edges[0], top_face(&app), edges[1]];
        run(&mut app, CommandId::Fillet);
        let FeatureKind::Blend(b) = selected_kind(&app) else {
            panic!("not a fillet");
        };
        assert_eq!((b.kind, b.edges.len()), (BlendKind::Fillet, 2));
        assert!(app.selected_geom.is_empty());
        assert_eq!(app.picking, None, "the edges were given");
        let id = app.selected_feature().unwrap();
        assert_eq!(app.doc.status(id), Some(&Status::Ok));

        run(&mut app, CommandId::Undo);
        assert!(app.doc.feature(id).is_none());
        assert_eq!(app.doc.model.len(), 2);

        // With nothing selected the chamfer waits for edges, and keeps taking them.
        app.selected_geom.clear();
        run(&mut app, CommandId::Chamfer);
        let id = app.selected_feature().unwrap();
        assert_eq!(app.picking, Some((id, Slot::BlendEdge)));
        // The body changes with every pick, so each edge is looked up afresh: the top
        // edge along the front, then the one along the back.
        let top_edge_at = |app: &PeetApp, y: f64| {
            let body = &app.doc.bodies[0];
            let edge = body
                .solid
                .edge_ids()
                .find(|e| {
                    let m = body.solid.edge(*e).point_at_fraction(0.5);
                    (m.z - 10.0).abs() < 1e-9 && (m.y - y).abs() < 1e-9
                })
                .expect("the edge is there");
            GeomRef::Edge { body: 0, edge }
        };
        for y in [0.0, 20.0] {
            let picked = app.picked(Some(top_edge_at(&app, y)), None);
            app.finish_pick(picked);
            assert_eq!(app.picking, Some((id, Slot::BlendEdge)), "still picking");
            assert_eq!(app.doc.status(id), Some(&Status::Ok));
        }
        let FeatureKind::Blend(b) = selected_kind(&app) else {
            panic!("not a chamfer");
        };
        assert_eq!((b.kind, b.edges.len()), (BlendKind::Chamfer, 2));

        // An edge of the chamfer itself can't be one of its edges.
        let own = {
            let body = &app.doc.bodies[0];
            let edge = body
                .solid
                .edge_ids()
                .find(|e| {
                    body.edge_ref(*e)
                        .is_some_and(|r| r.features().any(|f| f == id))
                })
                .expect("the chamfer made edges");
            GeomRef::Edge { body: 0, edge }
        };
        let picked = app.picked(Some(own), None);
        app.finish_pick(picked);
        assert!(matches!(&app.status_message, Some((_, true))));
        assert!(matches!(selected_kind(&app), FeatureKind::Blend(b) if b.edges.len() == 2));
    }

    #[test]
    fn shell_and_draft_commands_take_the_selected_faces() {
        let mut app = app_with_block();
        app.selected_geom = vec![top_face(&app)];
        run(&mut app, CommandId::Shell);
        let FeatureKind::Shell(s) = selected_kind(&app) else {
            panic!("not a shell");
        };
        assert_eq!(s.open.len(), 1);
        let id = app.selected_feature().unwrap();
        assert_eq!(app.doc.status(id), Some(&Status::Ok));
        run(&mut app, CommandId::Undo);

        // A draft of a side face asks for its neutral plane next; a plane picks it.
        let side = {
            let body = &app.doc.bodies[0];
            let face = body
                .solid
                .face_ids()
                .find(|f| body.face_center(*f).y.abs() < 1e-9)
                .unwrap();
            GeomRef::Face { body: 0, face }
        };
        app.selected_geom = vec![side];
        run(&mut app, CommandId::Draft);
        let id = app.selected_feature().unwrap();
        assert_eq!(app.picking, Some((id, Slot::DraftNeutral)));
        let picked = app.picked(
            None,
            Some(ItemId::Datum(peet_model::Datum::Plane(StdPlane::Top))),
        );
        app.finish_pick(picked);
        assert_eq!(app.picking, None);
        let FeatureKind::Draft(d) = selected_kind(&app) else {
            panic!("not a draft");
        };
        assert_eq!(d.faces.len(), 1);
        assert_eq!(d.neutral, Some(PlaneRef::Standard(StdPlane::Top)));
        assert_eq!(app.doc.status(id), Some(&Status::Ok));
    }

    #[test]
    fn hole_command_drills_at_the_selected_sketch() {
        let mut app = app_with_block();
        let GeomRef::Face { face, .. } = top_face(&app) else {
            unreachable!()
        };
        let body = app.doc.bodies[0].source.clone();
        let plane = peet_model::face_sketch_plane(&body.solid, face).unwrap();
        let mut sketch = FeatureId(0);
        app.doc.change("Sketch", |m| {
            sketch = m.add_sketch(PlaneRef::Face(body.face_ref(face)), plane);
            if let Some(f) = m.feature_mut(sketch).and_then(|f| f.sketch_mut()) {
                for x in [10.0, 30.0] {
                    f.sketch
                        .add_point(plane.to_plane_coords(DVec3::new(x, 10.0, 10.0)));
                }
            }
        });
        app.selected = Some(ItemId::Feature(sketch));
        run(&mut app, CommandId::Hole);
        let FeatureKind::Hole(h) = selected_kind(&app) else {
            panic!("not a hole");
        };
        assert_eq!(h.sketch, sketch);
        let id = app.selected_feature().unwrap();
        assert_eq!(app.doc.status(id), Some(&Status::Ok));
        let drilled = app.doc.bodies[0].mass_properties().unwrap().volume;
        assert!(drilled < 8000.0 - 600.0, "two M6 holes are gone: {drilled}");

        run(&mut app, CommandId::Undo);
        assert!(app.doc.feature(id).is_none());
        assert!((app.doc.bodies[0].mass_properties().unwrap().volume - 8000.0).abs() < 1e-6);
    }

    #[test]
    fn step_import_reports_what_happened() {
        let block = app_with_block();
        let options = peet_io::step::StepOptions {
            schema: peet_io::step::StepSchema::Ap214,
            product_name: "block".to_owned(),
            author: String::new(),
            organization: String::new(),
            timestamp: "2026-01-01T00:00:00".to_owned(),
        };
        let solid = &block.doc.evaluation().bodies[0].solid;
        let text = peet_io::step::write(&[("Block", solid)], &options);

        let mut app = PeetApp::headless();
        app.finish_step_import(&peet_platform::OpenedFile {
            name: "block.step".to_owned(),
            path: None,
            bytes: text.into_bytes(),
        });
        let FeatureKind::Import(i) = selected_kind(&app) else {
            panic!("not an import");
        };
        assert_eq!((i.source.as_str(), i.solids.len()), ("block.step", 1));
        assert_eq!(app.doc.bodies.len(), 1);
        assert!(app.notice.is_none(), "nothing to warn about");
        assert!(matches!(&app.status_message, Some((m, false)) if m.contains("block.step")));
        run(&mut app, CommandId::Undo);
        assert!(app.doc.bodies.is_empty());

        // A file that can't be read says why, in a dialog, and adds nothing.
        app.finish_step_import(&peet_platform::OpenedFile {
            name: "notes.step".to_owned(),
            path: None,
            bytes: b"not a STEP file".to_vec(),
        });
        let notice = app.notice.as_ref().expect("the failure is shown");
        assert!(notice.error && notice.heading.contains("notes.step"));
        assert!(!notice.lines[0].is_empty());
        assert!(app.doc.model.is_empty());
        assert!(matches!(&app.status_message, Some((_, true))));
    }

    #[test]
    fn the_housing_sample_opens() {
        let mut app = PeetApp::headless();
        run(&mut app, CommandId::OpenSampleHousing);
        assert_eq!(app.doc.bodies.len(), 1);
        assert_eq!(app.doc.evaluation().failures().count(), 0);
        assert!(!app.doc.is_modified());
    }

    /// The ribbon, every feature's properties, the measurements and the windows lay out
    /// without a GPU (the viewport itself is not shown).
    #[test]
    fn panels_and_windows_draw() {
        use super::super::RibbonTab;
        let mut app = PeetApp::headless();
        run(&mut app, CommandId::OpenSampleHousing);
        run(&mut app, CommandId::MassProperties);
        app.notice = Some(Notice {
            title: "Import STEP".to_owned(),
            heading: "part.step was imported (1 body), with these notes:".to_owned(),
            lines: vec!["The file names no unit: millimetres were assumed.".to_owned()],
            error: false,
        });
        let features: Vec<FeatureId> = app.doc.model.features().map(|f| f.id).collect();
        let faces: Vec<GeomRef> = app.doc.bodies[0]
            .solid
            .face_ids()
            .map(|face| GeomRef::Face { body: 0, face })
            .collect();
        let edges: Vec<GeomRef> = app.doc.bodies[0]
            .solid
            .edge_ids()
            .map(|edge| GeomRef::Edge { body: 0, edge })
            .collect();
        let mut frame = 0;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1400.0, 900.0))
            .build_ui(|ui| {
                // A different selection every frame: each feature, then pairs to measure.
                if let Some(id) = features.get(frame) {
                    app.selected = Some(ItemId::Feature(*id));
                    app.selected_geom.clear();
                } else {
                    let n = frame - features.len();
                    app.selected = None;
                    app.selected_geom = vec![faces[n % faces.len()], edges[n % edges.len()]];
                }
                frame += 1;
                let mut pending = Vec::new();
                for mut tab in [
                    RibbonTab::File,
                    RibbonTab::Model,
                    RibbonTab::SheetMetal,
                    RibbonTab::View,
                ] {
                    ui.push_id(tab as usize, |ui| app.toolbar(ui, &mut pending, &mut tab));
                }
                egui::ScrollArea::vertical().show(ui, |ui| app.properties(ui, &mut pending));
                let ctx = ui.ctx().clone();
                app.mass_properties_window(&ctx);
                app.notice_window(&ctx);
                assert!(pending.is_empty(), "nothing was clicked");
            });
        for _ in 0..features.len() + 6 {
            harness.step();
        }
        drop(harness);
        assert!(frame > features.len());
        assert!(
            app.doc.bodies[0].is_measured(),
            "the window measured the body"
        );
        assert!(app.windows.mass_properties && app.notice.is_some());
        assert_eq!(app.doc.undo_label(), None, "drawing changed nothing");
    }

    #[test]
    fn quantities_are_shown_in_document_units() {
        let mm = Units::new(LengthUnit::Mm);
        let inch = Units::new(LengthUnit::Inch);
        assert_eq!(significant(8000.0), "8000");
        assert_eq!(significant(1234.5678), "1234.57");
        assert_eq!(significant(0.000_123_456_7), "0.000123457");
        assert_eq!(significant(-2.5), "-2.5");
        assert_eq!(significant(0.0), "0");
        assert_eq!(area_text(mm, 800.0), "800 mm²");
        assert_eq!(volume_text(mm, 8000.0), "8000 mm³");
        assert_eq!(volume_text(inch, 25.4_f64.powi(3)), "1 in³");
        // 8000 mm³ of steel is 62.8 g.
        assert_eq!(mass_text(mm, 8000.0, 7850.0), "62.8 g");
        assert_eq!(mass_text(mm, 1e9, 7850.0), "7850 kg");
        assert_eq!(mass_text(inch, 1e9, 1000.0), "2204.62 lb");
        assert_eq!(
            inertia_text(mm, [1e9, 2e9, 3e9], 1000.0),
            "1000, 2000, 3000 kg·mm²"
        );
        assert_eq!(point_text(mm, DVec3::new(20.0, 10.0, 5.0)), "20, 10, 5 mm");
    }
}
