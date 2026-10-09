//! Properties panels for the general solid modelling features: revolve, sweep, fillet and
//! chamfer, shell, draft, hole and imported bodies.
//!
//! They work like the panels in [`crate::features_ui`]: each edits a copy of the feature's
//! definition, and asks for references to be picked in the viewport through a [`Slot`].

use egui::Ui;
use peet_model::{
    AxisRef, BlendFeature, BlendKind, DraftFeature, FeatureId, FeatureKind, HoleEnd, HoleFeature,
    HoleFit, HoleKind, ImportFeature, LoftFeature, METRIC, Operation, RevolveAxisRef,
    RevolveFeature, ScalarKind, ShellFeature, SweepFeature,
};

use crate::document::Document;
use crate::features_ui::{
    PICKING, PanelResult, Slot, axis_text, edge_text, plane_text, reference_row, short_label,
    value_row,
};

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|first| first.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// A list of picked edges or faces, each with a button to take it out, and a button to
/// pick more in the view. Returns the index of the one to take out.
#[allow(clippy::too_many_arguments)]
fn pick_list(
    ui: &mut Ui,
    label: &str,
    items: Vec<String>,
    empty: &str,
    noun: &str,
    slot: Slot,
    picking: Option<Slot>,
    out: &mut PanelResult,
) -> Option<usize> {
    let mut remove = None;
    ui.label(label);
    ui.vertical(|ui| {
        if items.is_empty() {
            ui.weak(empty);
        }
        for (i, text) in items.into_iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .small_button("✖")
                    .on_hover_text("Take it out of the list.")
                    .clicked()
                {
                    remove = Some(i);
                }
                short_label(ui, text);
            });
        }
        ui.horizontal(|ui| {
            if picking == Some(slot) {
                ui.colored_label(PICKING, format!("click {noun}…"));
                if ui
                    .small_button("Done")
                    .on_hover_text("Stop picking (Esc).")
                    .clicked()
                {
                    out.stop_pick = true;
                }
            } else if ui
                .small_button(format!("Add {noun}"))
                .on_hover_text(slot.prompt())
                .clicked()
            {
                out.pick = Some(slot);
            }
        });
    });
    ui.end_row();
    remove
}

/// The straight lines of a sketch with a name for each, construction lines first:
/// "Construction line 1 (30 mm)", "Line 2 (40 mm)".
fn sketch_line_labels(doc: &Document, sketch: FeatureId) -> Vec<(peet_sketch::EntityId, String)> {
    let units = doc.model.parameters.units;
    let (mut construction_lines, mut lines) = (0, 0);
    doc.revolve_axis_lines(sketch)
        .into_iter()
        .map(|(id, construction, length)| {
            let (kind, n) = if construction {
                construction_lines += 1;
                ("Construction line", construction_lines)
            } else {
                lines += 1;
                ("Line", lines)
            };
            (id, format!("{kind} {n} ({})", units.format_length(length)))
        })
        .collect()
}

/// What a revolve turns about, in words.
fn revolve_axis_text(doc: &Document, r: &RevolveFeature) -> String {
    match &r.axis {
        RevolveAxisRef::SketchX => "Sketch X axis".to_owned(),
        RevolveAxisRef::SketchY => "Sketch Y axis".to_owned(),
        RevolveAxisRef::SketchLine(id) => sketch_line_labels(doc, r.sketch)
            .into_iter()
            .find(|(line, _)| line == id)
            .map_or_else(|| "A deleted line".to_owned(), |(_, label)| label),
        RevolveAxisRef::Axis(a) => axis_text(doc, a),
    }
}

/// Revolve: axis, angle, direction and operation.
pub fn revolve_panel(
    ui: &mut Ui,
    doc: &Document,
    r: &mut RevolveFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("revolve_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Sketch");
            ui.label(doc.model.name_of(r.sketch));
            ui.end_row();

            ui.label("Axis").on_hover_text(
                "The line the sketch turns about. It must lie in the sketch's plane, with the profile on one side of it.",
            );
            if picking == Some(Slot::RevolveAxis) {
                ui.colored_label(PICKING, "click an edge or an axis…");
            } else {
                egui::ComboBox::from_id_salt("revolve_axis")
                    .selected_text(revolve_axis_text(doc, r))
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.axis, RevolveAxisRef::SketchX, "Sketch X axis")
                            .on_hover_text("The sketch's horizontal axis, through its origin.");
                        ui.selectable_value(&mut r.axis, RevolveAxisRef::SketchY, "Sketch Y axis")
                            .on_hover_text("The sketch's vertical axis, through its origin.");
                        for (id, label) in sketch_line_labels(doc, r.sketch) {
                            ui.selectable_value(&mut r.axis, RevolveAxisRef::SketchLine(id), label);
                        }
                        for f in doc.model.features() {
                            if matches!(f.kind, FeatureKind::Axis(_)) {
                                ui.selectable_value(
                                    &mut r.axis,
                                    RevolveAxisRef::Axis(AxisRef::Feature(f.id)),
                                    &f.name,
                                );
                            }
                        }
                        if ui
                            .selectable_label(
                                matches!(r.axis, RevolveAxisRef::Axis(AxisRef::Edge(_))),
                                "Pick an axis or edge…",
                            )
                            .on_hover_text(Slot::RevolveAxis.prompt())
                            .clicked()
                        {
                            out.pick = Some(Slot::RevolveAxis);
                        }
                    });
            }
            ui.end_row();

            value_row(
                ui,
                "Angle",
                "How far the sketch turns: 360° is all the way round.",
                "revolve_angle",
                &mut r.angle,
                ScalarKind::Angle,
                params,
                &mut out,
            );
            ui.label("Direction");
            ui.horizontal(|ui| {
                ui.checkbox(&mut r.symmetric, "Mid-plane")
                    .on_hover_text("Turn half the angle to each side of the sketch.");
                if !r.symmetric {
                    ui.checkbox(&mut r.reverse, "Reverse")
                        .on_hover_text("Turn the other way about the axis.");
                }
            });
            ui.end_row();

            ui.label("Operation");
            egui::ComboBox::from_id_salt("revolve_op")
                .selected_text(r.operation.label())
                .show_ui(ui, |ui| {
                    for op in [Operation::NewBody, Operation::Add, Operation::Cut] {
                        ui.selectable_value(&mut r.operation, op, op.label());
                    }
                });
            ui.end_row();
        });
    ui.add_space(6.0);
    ui.weak("Draw a construction line in the sketch for the axis (a centreline). The sketch's regions must not cross the axis.");
    out
}

/// The words under a sweep's panel.
pub const SWEEP_HINT: &str = "The path is a sketch of lines and arcs joined end to end, starting on the profile's plane and square to it. Corners between straight pieces are mitred; an arc must meet its neighbours tangent. A spline can be the path too: one end on the profile's plane, with no corners.";

/// Sweep: the path's sketch and the operation.
pub fn sweep_panel(
    ui: &mut Ui,
    doc: &Document,
    own: FeatureId,
    s: &mut SweepFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    egui::Grid::new("sweep_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Profile");
            ui.label(doc.model.name_of(s.profile));
            ui.end_row();

            ui.label("Path");
            if picking == Some(Slot::SweepPath) {
                ui.colored_label(PICKING, "click a sketch in the tree…");
            } else {
                let choices = doc.sweep_path_choices(own);
                egui::ComboBox::from_id_salt("sweep_path")
                    .selected_text(s.path.map_or("None yet", |p| doc.model.name_of(p)))
                    .show_ui(ui, |ui| {
                        if choices.is_empty() {
                            ui.weak("There is no other sketch before this feature.");
                        }
                        for id in choices {
                            ui.selectable_value(&mut s.path, Some(id), doc.model.name_of(id));
                        }
                        if ui
                            .selectable_label(false, "Pick a sketch…")
                            .on_hover_text(Slot::SweepPath.prompt())
                            .clicked()
                        {
                            out.pick = Some(Slot::SweepPath);
                        }
                    });
            }
            ui.end_row();

            ui.label("Operation");
            egui::ComboBox::from_id_salt("sweep_op")
                .selected_text(s.operation.label())
                .show_ui(ui, |ui| {
                    for op in [Operation::NewBody, Operation::Add, Operation::Cut] {
                        ui.selectable_value(&mut s.operation, op, op.label());
                    }
                });
            ui.end_row();
        });
    ui.add_space(6.0);
    if s.path.is_none() {
        ui.weak("Choose the path: draw it in a second sketch, before this feature in the tree.");
        ui.add_space(4.0);
    }
    ui.weak(SWEEP_HINT);
    out
}

/// The words under a loft's panel.
pub const LOFT_HINT: &str = "Profiles are joined edge to edge, so each needs the same number of edges; a circle adapts. Two profiles are joined straight, more smoothly.";

/// Loft: the profile sketches in order, and the operation.
pub fn loft_panel(
    ui: &mut Ui,
    doc: &Document,
    own: FeatureId,
    l: &mut LoftFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    // One change per frame: take a profile out, or swap it with the one after it.
    let (mut remove, mut swap) = (None, None);
    egui::Grid::new("loft_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Profiles")
                .on_hover_text("The sketches the loft passes through, in this order.");
            ui.vertical(|ui| {
                if l.sections.is_empty() {
                    ui.weak("None yet");
                }
                let last = l.sections.len().saturating_sub(1);
                // ⏶ and ⏷ rather than ▲ and ▼, which egui's proportional fonts lack.
                for (i, section) in l.sections.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(i > 0, egui::Button::new("⏶").small())
                            .on_hover_text("Earlier in the loft.")
                            .clicked()
                        {
                            swap = Some(i - 1);
                        }
                        if ui
                            .add_enabled(i < last, egui::Button::new("⏷").small())
                            .on_hover_text("Later in the loft.")
                            .clicked()
                        {
                            swap = Some(i);
                        }
                        if ui
                            .small_button("✖")
                            .on_hover_text("Take it out of the loft.")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        short_label(ui, doc.model.name_of(*section).to_owned());
                    });
                }
                ui.horizontal(|ui| {
                    if picking == Some(Slot::LoftProfile) {
                        ui.colored_label(PICKING, "click sketches in the tree…");
                        if ui
                            .small_button("Done")
                            .on_hover_text("Stop picking (Esc).")
                            .clicked()
                        {
                            out.stop_pick = true;
                        }
                    } else if ui
                        .small_button("Add profiles")
                        .on_hover_text(Slot::LoftProfile.prompt())
                        .clicked()
                    {
                        out.pick = Some(Slot::LoftProfile);
                    }
                });
            });
            ui.end_row();

            ui.label("Operation");
            egui::ComboBox::from_id_salt("loft_op")
                .selected_text(l.operation.label())
                .show_ui(ui, |ui| {
                    for op in [Operation::NewBody, Operation::Add, Operation::Cut] {
                        ui.selectable_value(&mut l.operation, op, op.label());
                    }
                });
            ui.end_row();
        });
    if let Some(i) = remove {
        l.sections.remove(i);
    } else if let Some(i) = swap {
        l.sections.swap(i, i + 1);
    }
    ui.add_space(6.0);
    if l.sections.len() < 2 {
        ui.weak(if doc.loft_profile_choices(own).is_empty() {
            "A loft needs at least two profiles: draw the next one in another sketch, on a different plane, before this feature in the tree."
        } else {
            "A loft needs at least two profiles: use Add profiles and click the next sketch in the feature tree."
        });
        ui.add_space(4.0);
    }
    ui.weak(LOFT_HINT);
    out
}

/// Fillet or chamfer: size, propagation and the edges.
pub fn blend_panel(
    ui: &mut Ui,
    doc: &Document,
    b: &mut BlendFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("blend_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            let (label, tip) = match b.kind {
                BlendKind::Fillet => ("Radius", "The radius of the rounding."),
                BlendKind::Chamfer => (
                    "Distance",
                    "How far the chamfer is set back from the edge, on both faces.",
                ),
            };
            value_row(
                ui,
                label,
                tip,
                "blend_size",
                &mut b.size,
                ScalarKind::Length,
                params,
                &mut out,
            );
            ui.label("");
            ui.checkbox(&mut b.chain, "Tangent propagation").on_hover_text(
                "Carry on along the edges that continue each picked edge smoothly, so one click takes a whole run of edges.",
            );
            ui.end_row();
            let items = b.edges.iter().map(|e| edge_text(doc, e)).collect();
            if let Some(i) = pick_list(
                ui,
                "Edges",
                items,
                "None yet",
                "edges",
                Slot::BlendEdge,
                picking,
                &mut out,
            ) {
                b.edges.remove(i);
            }
        });
    ui.add_space(6.0);
    ui.weak(match b.kind {
        BlendKind::Fillet => {
            "Rounds the edges with one radius. The radius must fit on the faces beside each edge."
        }
        BlendKind::Chamfer => {
            "Cuts the edges back by the same distance on both faces. The distance must fit on the faces beside each edge."
        }
    });
    out
}

/// Shell: wall thickness and the faces to open.
pub fn shell_panel(
    ui: &mut Ui,
    doc: &Document,
    s: &mut ShellFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("shell_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            value_row(
                ui,
                "Wall thickness",
                "The thickness left everywhere, measured inwards from the outside.",
                "shell_thickness",
                &mut s.thickness,
                ScalarKind::Length,
                params,
                &mut out,
            );
            let items = s
                .open
                .iter()
                .map(|f| capitalized(&doc.model.describe_face(&f.name)))
                .collect();
            if let Some(i) = pick_list(
                ui,
                "Open faces",
                items,
                "None: a closed hollow",
                "faces",
                Slot::ShellFace,
                picking,
                &mut out,
            ) {
                s.open.remove(i);
            }
        });
    ui.add_space(6.0);
    ui.weak("The open faces are removed, so the hollow can be seen and reached. With no open faces the body becomes a closed hollow: a solid with an empty space inside.");
    out
}

/// Draft: angle, neutral plane and the faces.
pub fn draft_panel(
    ui: &mut Ui,
    doc: &Document,
    d: &mut DraftFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("draft_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            value_row(
                ui,
                "Angle",
                "How far each face leans from the direction of pull.",
                "draft_angle",
                &mut d.angle,
                ScalarKind::Angle,
                params,
                &mut out,
            );
            ui.label("Direction");
            ui.checkbox(&mut d.flip, "Flip").on_hover_text(
                "Taper the other way: wider instead of narrower along the neutral plane's normal.",
            );
            ui.end_row();
            let text = d
                .neutral
                .as_ref()
                .map_or("None yet".to_owned(), |p| plane_text(doc, p));
            reference_row(
                ui,
                "Neutral plane",
                text,
                Slot::DraftNeutral,
                picking,
                &mut out,
            );
            let items = d
                .faces
                .iter()
                .map(|f| capitalized(&doc.model.describe_face(&f.name)))
                .collect();
            if let Some(i) = pick_list(
                ui,
                "Faces",
                items,
                "None yet",
                "faces",
                Slot::DraftFace,
                picking,
                &mut out,
            ) {
                d.faces.remove(i);
            }
        });
    ui.add_space(6.0);
    ui.weak("Flat faces can be drafted, round faces whose axis is along the direction of pull (they become cones), and freeform faces such as the walls of an extruded spline or the sides of a loft. The neutral plane's normal is the direction of pull; each face turns about the curve where it crosses the neutral plane, so the part keeps its size there.");
    out
}

/// After a hole's panel was shown: a size typed by hand is no longer the standard one,
/// and a diameter typed by hand is no longer the thread's tap drill.
fn note_custom_sizes(h: &mut HoleFeature, before: &HoleFeature) {
    if h.standard != before.standard {
        return; // set from a standard size just now
    }
    let sizes = |h: &HoleFeature| {
        [
            h.diameter.clone(),
            h.counterbore_diameter.clone(),
            h.counterbore_depth.clone(),
            h.countersink_diameter.clone(),
            h.countersink_angle.clone(),
        ]
    };
    if sizes(h) != sizes(before) {
        h.standard = None;
    }
    if h.diameter != before.diameter {
        h.thread = None;
    }
}

/// Hole wizard: standard size and fit, type, sizes and depth.
pub fn hole_panel(ui: &mut Ui, doc: &Document, h: &mut HoleFeature) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    let before = h.clone();
    ui.strong(h.summary());
    ui.add_space(4.0);
    egui::Grid::new("hole_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Sketch");
            let count = doc.hole_count(h.sketch);
            ui.label(format!(
                "{} ({count} hole{})",
                doc.model.name_of(h.sketch),
                if count == 1 { "" } else { "s" }
            ));
            ui.end_row();

            ui.label("Size").on_hover_text(
                "A standard metric screw size sets the diameters below. Type a diameter yourself for any other size.",
            );
            let current = h.standard.clone();
            let fit = current.as_ref().map_or(HoleFit::Normal, |(_, f)| *f);
            egui::ComboBox::from_id_salt("hole_size")
                .selected_text(current.as_ref().map_or("Custom", |(name, _)| name.as_str()))
                .show_ui(ui, |ui| {
                    for size in &METRIC {
                        let on = current.as_ref().is_some_and(|(name, _)| name == size.name);
                        if ui.selectable_label(on, size.name).clicked() && !on {
                            h.set_standard(size, fit);
                        }
                    }
                });
            ui.end_row();

            ui.label("Fit");
            let size = current
                .as_ref()
                .and_then(|(name, _)| METRIC.iter().find(|s| s.name == name));
            ui.add_enabled_ui(size.is_some(), |ui| {
                egui::ComboBox::from_id_salt("hole_fit")
                    .selected_text(if size.is_some() { fit.label() } else { "Custom" })
                    .show_ui(ui, |ui| {
                        for f in HoleFit::ALL {
                            let tip = match f {
                                HoleFit::Close => {
                                    "A clearance hole that locates the screw closely."
                                }
                                HoleFit::Normal => "The usual clearance hole for a screw.",
                                HoleFit::Loose => "A clearance hole with room for misalignment.",
                                HoleFit::Tapped => {
                                    "Drilled to the tap drill size, to be threaded for the screw."
                                }
                            };
                            if ui
                                .selectable_label(f == fit, f.label())
                                .on_hover_text(tip)
                                .clicked()
                                && f != fit
                                && let Some(size) = size
                            {
                                h.set_standard(size, f);
                            }
                        }
                    });
            })
            .response
            .on_disabled_hover_text("Choose a standard size first.");
            ui.end_row();

            ui.label("Type");
            egui::ComboBox::from_id_salt("hole_kind")
                .selected_text(h.kind.label())
                .show_ui(ui, |ui| {
                    for k in HoleKind::ALL {
                        let tip = match k {
                            HoleKind::Simple => "A plain bore.",
                            HoleKind::Counterbore => {
                                "A wider, flat-bottomed recess for a socket head screw."
                            }
                            HoleKind::Countersink => "A conical recess for a countersunk screw.",
                        };
                        ui.selectable_value(&mut h.kind, k, k.label())
                            .on_hover_text(tip);
                    }
                });
            ui.end_row();

            value_row(
                ui,
                "Diameter",
                "",
                "hole_diameter",
                &mut h.diameter,
                ScalarKind::Length,
                params,
                &mut out,
            );

            ui.label("End");
            egui::ComboBox::from_id_salt("hole_end")
                .selected_text(match h.end {
                    HoleEnd::Blind => "Blind",
                    HoleEnd::ThroughAll => "Through all",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut h.end, HoleEnd::Blind, "Blind")
                        .on_hover_text("To a given depth.");
                    ui.selectable_value(&mut h.end, HoleEnd::ThroughAll, "Through all")
                        .on_hover_text("Through every body in its way.");
                });
            ui.end_row();
            if h.end == HoleEnd::Blind {
                value_row(
                    ui,
                    "Depth",
                    "The depth of the full diameter, from the face.",
                    "hole_depth",
                    &mut h.depth,
                    ScalarKind::Length,
                    params,
                    &mut out,
                );
                value_row(
                    ui,
                    "Drill point",
                    "The angle of the drill's point: 118° for a twist drill, 180° for a flat bottom.",
                    "hole_tip",
                    &mut h.tip_angle,
                    ScalarKind::Angle,
                    params,
                    &mut out,
                );
            }
            match h.kind {
                HoleKind::Simple => {}
                HoleKind::Counterbore => {
                    value_row(
                        ui,
                        "Counterbore Ø",
                        "",
                        "hole_cbore_d",
                        &mut h.counterbore_diameter,
                        ScalarKind::Length,
                        params,
                        &mut out,
                    );
                    value_row(
                        ui,
                        "Counterbore depth",
                        "",
                        "hole_cbore_depth",
                        &mut h.counterbore_depth,
                        ScalarKind::Length,
                        params,
                        &mut out,
                    );
                }
                HoleKind::Countersink => {
                    value_row(
                        ui,
                        "Countersink Ø",
                        "The countersink's diameter at the face.",
                        "hole_csink_d",
                        &mut h.countersink_diameter,
                        ScalarKind::Length,
                        params,
                        &mut out,
                    );
                    value_row(
                        ui,
                        "Countersink angle",
                        "The full angle of the cone: 90° for metric countersunk screws.",
                        "hole_csink_angle",
                        &mut h.countersink_angle,
                        ScalarKind::Angle,
                        params,
                        &mut out,
                    );
                }
            }
            ui.label("Direction");
            ui.checkbox(&mut h.reverse, "Reverse")
                .on_hover_text("Drill the other way from the sketch.");
            ui.end_row();
            if let Some(thread) = &h.thread {
                ui.label("Thread");
                ui.label(thread);
                ui.end_row();
            }
        });
    note_custom_sizes(h, &before);
    ui.add_space(6.0);
    if h.thread.is_some() {
        ui.weak("The thread is cosmetic: the hole is modelled at the tap drill diameter, and the designation goes with it for drawings and notes.");
        ui.add_space(4.0);
    }
    ui.weak("A hole is drilled at every free-standing point of the sketch (at the circles' centres if it has no points), into the face the sketch is on.");
    out
}

/// Imported bodies: where they came from and what they are called.
pub fn import_panel(ui: &mut Ui, i: &ImportFeature) -> PanelResult {
    egui::Grid::new("import_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("File");
            short_label(ui, i.source.clone());
            ui.end_row();
            ui.label("Solids");
            ui.label(i.solids.len().to_string());
            ui.end_row();
            for (n, s) in i.solids.iter().enumerate() {
                ui.label(if n == 0 { "Names" } else { "" });
                short_label(
                    ui,
                    if s.name.trim().is_empty() {
                        format!("Body {}", n + 1)
                    } else {
                        s.name.clone()
                    },
                );
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.weak("Imported bodies have no history: they are kept as they came. Later features (cuts, holes, fillets) work on them. For a newer version of the file, import it again and delete this feature.");
    PanelResult::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features_ui::{Picked, apply_pick};
    use egui_kittest::Harness;
    use peet_math::{DVec2, Plane};
    use peet_model::{EdgeRef, FaceRef, PlaneRef, Scalar, StdAxis, StdPlane};

    /// A block, with a reference to one of its edges and to its top face.
    fn block() -> (Document, EdgeRef, FaceRef) {
        let mut doc = Document::default();
        doc.change("Add", |m| {
            let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
            if let Some(f) = m.feature_mut(s).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(&mut f.sketch, DVec2::ZERO, DVec2::new(40.0, 20.0));
            }
            m.add_extrude(s, Operation::Add);
        });
        let body = doc.bodies[0].source.clone();
        let edge = body
            .solid
            .edge_ids()
            .find_map(|e| body.edge_ref(e))
            .expect("the block has edges");
        let top = body
            .solid
            .face_ids()
            .find(|f| body.face_center(*f).z > 9.0)
            .expect("the block has a top");
        (doc, edge, body.face_ref(top))
    }

    #[test]
    fn every_slot_says_what_to_click() {
        for slot in [
            Slot::RevolveAxis,
            Slot::BlendEdge,
            Slot::ShellFace,
            Slot::DraftFace,
            Slot::DraftNeutral,
            Slot::SweepPath,
            Slot::LoftProfile,
        ] {
            assert!(slot.prompt().starts_with("Click"), "{slot:?}");
        }
        assert!(Slot::BlendEdge.repeats() && Slot::ShellFace.repeats());
        assert!(Slot::DraftFace.repeats() && Slot::LoftProfile.repeats());
        assert!(!Slot::DraftNeutral.repeats() && !Slot::FlangeEdge.repeats());
    }

    #[test]
    fn picks_fill_the_solid_features() {
        let (_, edge, top) = block();
        let face = |planar: bool| Picked::Face {
            face: top.clone(),
            planar,
            round: !planar,
        };

        // A fillet's edges: a click adds, a second click takes out.
        let mut kind = FeatureKind::Blend(Box::new(BlendFeature::new(BlendKind::Fillet, vec![])));
        apply_pick(&mut kind, Slot::BlendEdge, Picked::Edge(edge.clone())).unwrap();
        assert!(matches!(&kind, FeatureKind::Blend(b) if b.edges == vec![edge.clone()]));
        assert!(apply_pick(&mut kind, Slot::BlendEdge, face(true)).is_err());
        apply_pick(&mut kind, Slot::BlendEdge, Picked::Edge(edge.clone())).unwrap();
        assert!(matches!(&kind, FeatureKind::Blend(b) if b.edges.is_empty()));

        let mut kind = FeatureKind::Shell(Box::new(ShellFeature::new(vec![])));
        apply_pick(&mut kind, Slot::ShellFace, face(false)).unwrap();
        assert!(matches!(&kind, FeatureKind::Shell(s) if s.open.len() == 1));
        assert!(apply_pick(&mut kind, Slot::ShellFace, Picked::Edge(edge.clone())).is_err());

        // Draft takes faces (not edges), and a plane or a flat face as its neutral plane.
        let mut kind = FeatureKind::Draft(Box::new(DraftFeature::new(vec![], None)));
        assert!(apply_pick(&mut kind, Slot::DraftFace, Picked::Edge(edge.clone())).is_err());
        apply_pick(&mut kind, Slot::DraftFace, face(true)).unwrap();
        let top_plane = PlaneRef::Standard(StdPlane::Top);
        apply_pick(
            &mut kind,
            Slot::DraftNeutral,
            Picked::Plane(top_plane.clone()),
        )
        .unwrap();
        assert!(apply_pick(&mut kind, Slot::DraftNeutral, face(false)).is_err());
        assert!(matches!(
            &kind,
            FeatureKind::Draft(d) if d.faces.len() == 1 && d.neutral == Some(top_plane.clone())
        ));

        // A revolve's axis: an edge or a reference axis, not a face.
        let sketch = peet_sketch::Sketch::new();
        let mut kind = FeatureKind::Revolve(Box::new(RevolveFeature::new(
            FeatureId(1),
            &sketch,
            Operation::Add,
        )));
        apply_pick(&mut kind, Slot::RevolveAxis, Picked::Edge(edge.clone())).unwrap();
        assert!(matches!(
            &kind,
            FeatureKind::Revolve(r) if r.axis == RevolveAxisRef::Axis(AxisRef::Edge(edge.clone()))
        ));
        let z = AxisRef::Standard(StdAxis::Z);
        apply_pick(&mut kind, Slot::RevolveAxis, Picked::Axis(z.clone())).unwrap();
        assert!(
            matches!(&kind, FeatureKind::Revolve(r) if r.axis == RevolveAxisRef::Axis(z.clone()))
        );
        assert!(apply_pick(&mut kind, Slot::RevolveAxis, face(true)).is_err());
        // A slot of another feature is refused.
        assert!(apply_pick(&mut kind, Slot::BlendEdge, Picked::Edge(edge.clone())).is_err());

        // A sweep's path: another sketch, not its own profile, and not an edge.
        let (profile, path) = (FeatureId(1), FeatureId(2));
        let mut kind =
            FeatureKind::Sweep(Box::new(SweepFeature::new(profile, None, Operation::Add)));
        assert!(apply_pick(&mut kind, Slot::SweepPath, Picked::Sketch(profile)).is_err());
        assert!(apply_pick(&mut kind, Slot::SweepPath, Picked::Edge(edge)).is_err());
        apply_pick(&mut kind, Slot::SweepPath, Picked::Sketch(path)).unwrap();
        assert!(matches!(&kind, FeatureKind::Sweep(s) if s.path == Some(path)));

        // A loft's profiles: sketches, in the order they are clicked, each once.
        let mut kind = FeatureKind::Loft(Box::new(LoftFeature::new(vec![profile], Operation::Add)));
        assert!(apply_pick(&mut kind, Slot::LoftProfile, Picked::Sketch(profile)).is_err());
        assert!(apply_pick(&mut kind, Slot::LoftProfile, face(true)).is_err());
        apply_pick(&mut kind, Slot::LoftProfile, Picked::Sketch(path)).unwrap();
        apply_pick(&mut kind, Slot::LoftProfile, Picked::Sketch(FeatureId(3))).unwrap();
        assert!(matches!(
            &kind,
            FeatureKind::Loft(l) if l.sections == vec![profile, path, FeatureId(3)]
        ));
    }

    #[test]
    fn a_size_typed_by_hand_is_no_longer_standard() {
        let mut h = HoleFeature::new(FeatureId(1));
        let m8 = METRIC.iter().find(|s| s.name == "M8").unwrap();
        h.set_standard(m8, HoleFit::Tapped);
        assert_eq!(h.thread.as_deref(), Some("M8x1.25"));

        // Changing the depth or the type keeps the standard and the thread.
        let before = h.clone();
        h.end = HoleEnd::Blind;
        h.depth = Scalar::new(12.0);
        h.kind = HoleKind::Counterbore;
        note_custom_sizes(&mut h, &before);
        assert!(h.standard.is_some() && h.thread.is_some());

        // A counterbore typed by hand is a custom size, but still that thread.
        let before = h.clone();
        h.counterbore_diameter = Scalar::new(16.0);
        note_custom_sizes(&mut h, &before);
        assert!(h.standard.is_none() && h.thread.is_some());

        // A diameter typed by hand is not the tap drill any more.
        let before = h.clone();
        h.diameter = Scalar::new(7.0);
        note_custom_sizes(&mut h, &before);
        assert!(h.thread.is_none());

        // Choosing a size again is not a hand edit.
        let before = h.clone();
        h.set_standard(m8, HoleFit::Normal);
        note_custom_sizes(&mut h, &before);
        assert_eq!(h.standard, Some(("M8".to_owned(), HoleFit::Normal)));
    }

    #[test]
    fn revolve_axis_choices_list_construction_lines_first() {
        let mut doc = Document::default();
        let mut sketch = FeatureId(0);
        let mut axis = None;
        doc.change("Sketch", |m| {
            sketch = m.add_sketch(PlaneRef::Standard(StdPlane::Front), StdPlane::Front.plane());
            if let Some(f) = m.feature_mut(sketch).and_then(|f| f.sketch_mut()) {
                f.sketch
                    .add_line(DVec2::new(10.0, 0.0), DVec2::new(10.0, 40.0));
                let line = f.sketch.add_line(DVec2::ZERO, DVec2::new(0.0, 30.0));
                f.sketch.set_construction(line, true);
                axis = Some(line);
            }
        });
        let labels = sketch_line_labels(&doc, sketch);
        assert_eq!(
            labels[0],
            (axis.unwrap(), "Construction line 1 (30 mm)".to_owned())
        );
        assert_eq!(labels[1].1, "Line 1 (40 mm)");
        let mut r = RevolveFeature::new(
            sketch,
            &doc.model.sketch(sketch).unwrap().sketch,
            Operation::Add,
        );
        assert_eq!(revolve_axis_text(&doc, &r), "Construction line 1 (30 mm)");
        r.axis = RevolveAxisRef::SketchLine(peet_sketch::EntityId(9999));
        assert_eq!(revolve_axis_text(&doc, &r), "A deleted line");
        r.axis = RevolveAxisRef::SketchX;
        assert_eq!(revolve_axis_text(&doc, &r), "Sketch X axis");
    }

    /// Every panel lays out without a GPU, for a feature in each of its states.
    #[test]
    fn panels_draw() {
        let (mut doc, edge, top) = block();
        let mut sketch = FeatureId(0);
        doc.change("Sketch", |m| {
            sketch = m.add_sketch(PlaneRef::Standard(StdPlane::Front), StdPlane::Front.plane());
        });
        let geometry = doc.model.sketch(sketch).unwrap().sketch.clone();
        let mut kinds = vec![
            FeatureKind::Revolve(Box::new(RevolveFeature::new(
                sketch,
                &geometry,
                Operation::Cut,
            ))),
            FeatureKind::Blend(Box::new(BlendFeature::new(
                BlendKind::Fillet,
                vec![edge.clone()],
            ))),
            FeatureKind::Blend(Box::new(BlendFeature::new(BlendKind::Chamfer, vec![]))),
            FeatureKind::Shell(Box::new(ShellFeature::new(vec![top.clone()]))),
            FeatureKind::Sweep(Box::new(SweepFeature::new(sketch, None, Operation::Add))),
            FeatureKind::Loft(Box::new(LoftFeature::new(vec![sketch], Operation::Add))),
            FeatureKind::Loft(Box::new(LoftFeature::new(
                vec![sketch, FeatureId(1), FeatureId(9999)],
                Operation::Cut,
            ))),
            FeatureKind::Draft(Box::new(DraftFeature::new(vec![top], None))),
            FeatureKind::Import(Box::new(ImportFeature {
                source: "bracket.step".to_owned(),
                solids: Vec::new(),
            })),
        ];
        for (kind, fit) in [
            (HoleKind::Simple, HoleFit::Normal),
            (HoleKind::Counterbore, HoleFit::Tapped),
            (HoleKind::Countersink, HoleFit::Close),
        ] {
            let mut h = HoleFeature::new(sketch);
            h.kind = kind;
            h.end = HoleEnd::Blind;
            h.set_standard(&METRIC[3], fit);
            kinds.push(FeatureKind::Hole(Box::new(h)));
        }
        for kind in kinds {
            for picking in [
                None,
                Some(Slot::BlendEdge),
                Some(Slot::RevolveAxis),
                Some(Slot::SweepPath),
                Some(Slot::LoftProfile),
            ] {
                let before = kind.clone();
                let state = (kind.clone(), false);
                let doc = &doc;
                let mut harness = Harness::builder()
                    .with_size(egui::vec2(320.0, 700.0))
                    .build_ui_state(
                        |ui, (kind, stop): &mut (FeatureKind, bool)| {
                            let out = match kind {
                                FeatureKind::Revolve(r) => revolve_panel(ui, doc, r, picking),
                                FeatureKind::Blend(b) => blend_panel(ui, doc, b, picking),
                                FeatureKind::Shell(s) => shell_panel(ui, doc, s, picking),
                                FeatureKind::Draft(d) => draft_panel(ui, doc, d, picking),
                                FeatureKind::Hole(h) => hole_panel(ui, doc, h),
                                FeatureKind::Import(i) => import_panel(ui, i),
                                FeatureKind::Sweep(s) => {
                                    sweep_panel(ui, doc, FeatureId(u32::MAX), s, picking)
                                }
                                FeatureKind::Loft(l) => {
                                    loft_panel(ui, doc, FeatureId(u32::MAX), l, picking)
                                }
                                _ => unreachable!(),
                            };
                            *stop |= out.stop_pick || out.pick.is_some();
                        },
                        state,
                    );
                harness.step();
                harness.step();
                let (after, clicked) = harness.state();
                assert_eq!(*after, before, "drawing a panel changes nothing");
                assert!(!clicked, "nothing was clicked");
            }
        }
    }
}
