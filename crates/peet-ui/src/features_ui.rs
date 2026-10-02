//! Properties panels for features: extrusions and reference geometry.
//!
//! Panels edit a copy of the feature's definition; the app compares it with the model
//! afterwards and applies the difference as an undoable change. References to other
//! geometry are set by clicking it in the viewport: a panel asks for a [`Slot`] to be
//! filled, and [`apply_pick`] puts what was clicked there.

use egui::{Color32, Ui};
use peet_model::{
    AxisDef, AxisRef, CoordSystemDef, EdgeRef, EndCondition, FaceRef, FeatureKind, Operation,
    PlaneDef, PlaneRef, PointDef, PointRef, Scalar, ScalarKind, StdAxis, VertexRef,
};
use peet_sketch::expr::Parameters;

use crate::document::Document;

pub const ERROR: Color32 = Color32::from_rgb(229, 72, 77);
pub const WARNING: Color32 = Color32::from_rgb(230, 160, 40);
pub const PICKING: Color32 = Color32::from_rgb(255, 150, 30);

/// A reference of a feature that can be set by clicking in the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// An extrusion's "up to" face or plane.
    UpTo,
    /// The plane a sketch lies on.
    SketchPlane,
    /// A reference plane's first (or only) reference.
    PlaneFirst,
    /// A mid plane's second plane.
    PlaneSecond,
    /// What an axis is made from.
    AxisFirst,
    /// The second plane of an axis where two planes meet.
    AxisSecond,
    PointVertex,
    CsysOrigin,
    CsysOrientation,
}

impl Slot {
    /// What to click, for the status bar.
    pub fn prompt(self) -> &'static str {
        match self {
            Self::UpTo => "Click a flat face or a plane parallel to the sketch (Up to face).",
            Self::SketchPlane => "Click a flat face or a plane to put the sketch on.",
            Self::PlaneFirst | Self::PlaneSecond | Self::AxisSecond | Self::CsysOrientation => {
                "Click a flat face or a plane."
            }
            Self::AxisFirst => "Click a straight edge, a round edge, a round face or a plane.",
            Self::PointVertex | Self::CsysOrigin => "Click a vertex.",
        }
    }
}

/// What was clicked, as references.
#[derive(Clone, Debug)]
pub enum Picked {
    Face {
        face: FaceRef,
        planar: bool,
        round: bool,
    },
    Edge(EdgeRef),
    Vertex(VertexRef),
    /// A standard or reference plane.
    Plane(PlaneRef),
}

impl Picked {
    fn plane(self) -> Result<PlaneRef, String> {
        match self {
            Self::Plane(p) => Ok(p),
            Self::Face {
                face, planar: true, ..
            } => Ok(PlaneRef::Face(face)),
            Self::Face { .. } => Err("That face isn't flat: pick a flat face or a plane.".into()),
            _ => Err("Pick a flat face or a plane.".into()),
        }
    }
}

/// Puts a clicked reference into a feature definition.
pub fn apply_pick(kind: &mut FeatureKind, slot: Slot, picked: Picked) -> Result<(), String> {
    match (kind, slot) {
        (FeatureKind::Extrude(e), Slot::UpTo) => {
            e.params.end = EndCondition::UpTo(picked.plane()?);
        }
        (FeatureKind::Sketch(s), Slot::SketchPlane) => s.plane = picked.plane()?,
        (FeatureKind::Plane(def), Slot::PlaneFirst) => {
            let p = picked.plane()?;
            match def {
                PlaneDef::Offset { from, .. } | PlaneDef::Angled { from, .. } => *from = p,
                PlaneDef::Midplane { a, .. } => *a = p,
            }
        }
        (FeatureKind::Plane(PlaneDef::Midplane { b, .. }), Slot::PlaneSecond) => {
            *b = picked.plane()?;
        }
        (FeatureKind::Axis(def), Slot::AxisFirst) => {
            *def = match picked {
                Picked::Edge(e) => AxisDef::Edge(e),
                Picked::Face {
                    face, round: true, ..
                } => AxisDef::Cylinder(face),
                other => {
                    let second = match def {
                        AxisDef::TwoPlanes(_, b) => b.clone(),
                        _ => PlaneRef::Standard(peet_model::StdPlane::Front),
                    };
                    AxisDef::TwoPlanes(other.plane()?, second)
                }
            };
        }
        (FeatureKind::Axis(AxisDef::TwoPlanes(_, b)), Slot::AxisSecond) => *b = picked.plane()?,
        (FeatureKind::Point(def), Slot::PointVertex) => match picked {
            Picked::Vertex(v) => *def = PointDef::Vertex(v),
            _ => return Err("Pick a vertex.".into()),
        },
        (FeatureKind::CoordSystem(def), Slot::CsysOrigin) => match picked {
            Picked::Vertex(v) => def.origin = PointRef::Vertex(v),
            _ => return Err("Pick a vertex for the origin.".into()),
        },
        (FeatureKind::CoordSystem(def), Slot::CsysOrientation) => {
            def.orientation = picked.plane()?;
        }
        _ => return Err("That can't be used here.".into()),
    }
    Ok(())
}

/// What the user did in a panel.
#[derive(Default)]
pub struct PanelResult {
    /// Ask for a reference to be picked in the viewport.
    pub pick: Option<Slot>,
    /// A value field finished editing (ends merging of undo steps).
    pub committed: bool,
}

/// An editable value that accepts expressions. Applies the input when the field loses
/// focus or Enter is pressed. Returns whether the value changed.
pub fn scalar_field(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut Scalar,
    kind: ScalarKind,
    params: &Parameters,
) -> bool {
    let key = ui.make_persistent_id(("scalar", id));
    let current = value.input_text(kind, params);
    let mut text = ui
        .data(|m| m.get_temp::<String>(key))
        .unwrap_or_else(|| current.clone());
    let error_key = key.with("error");
    let error: Option<String> = ui.data(|m| m.get_temp(error_key));
    let mut changed = false;
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(110.0)
                .hint_text("value or expression"),
        );
        if r.changed() {
            ui.data_mut(|m| m.insert_temp(key, text.clone()));
        }
        if r.lost_focus() {
            ui.data_mut(|m| m.remove::<String>(key));
            if text.trim() != current && !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                match value.set_input(&text, kind, params) {
                    Ok(_) => {
                        changed = true;
                        ui.data_mut(|m| m.remove::<String>(error_key));
                    }
                    Err(e) => {
                        ui.data_mut(|m| m.insert_temp(error_key, e));
                    }
                }
            }
        }
        let unit = match kind {
            ScalarKind::Length => params.units.length.suffix(),
            ScalarKind::Angle => "°",
        };
        ui.weak(unit);
        if value.expression.is_some()
            && let Ok(v) = value.evaluate(kind, params)
        {
            let shown = match kind {
                ScalarKind::Length => params.units.format_length(v),
                ScalarKind::Angle => params.units.format_angle(v),
            };
            ui.weak(format!("= {shown}"));
        }
    });
    if let Some(e) = error {
        ui.colored_label(ERROR, e);
    }
    changed
}

/// A reference shown as text with a button to pick it again.
fn reference_row(
    ui: &mut Ui,
    label: &str,
    text: String,
    slot: Slot,
    picking: Option<Slot>,
    out: &mut PanelResult,
) {
    ui.label(label);
    ui.horizontal(|ui| {
        if picking == Some(slot) {
            ui.colored_label(PICKING, "click it…");
        } else {
            ui.label(text);
        }
        if ui
            .small_button("Pick")
            .on_hover_text(slot.prompt())
            .clicked()
        {
            out.pick = Some(slot);
        }
    });
    ui.end_row();
}

fn plane_text(doc: &Document, r: &PlaneRef) -> String {
    doc.plane_name(r)
}

fn axis_text(doc: &Document, r: &AxisRef) -> String {
    match r {
        AxisRef::Standard(a) => a.label().to_owned(),
        AxisRef::Feature(id) => doc.model.name_of(*id).to_owned(),
    }
}

fn point_text(doc: &Document, r: &PointRef) -> String {
    match r {
        PointRef::Origin => "Origin".to_owned(),
        PointRef::Feature(id) => doc.model.name_of(*id).to_owned(),
        PointRef::Vertex(_) => "A vertex".to_owned(),
    }
}

/// Extrude parameters.
pub fn extrude_panel(
    ui: &mut Ui,
    doc: &Document,
    e: &mut peet_model::ExtrudeFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    let f = &mut e.params;
    egui::Grid::new("extrude_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Sketch");
            ui.label(doc.model.name_of(e.sketch));
            ui.end_row();

            ui.label("Operation");
            egui::ComboBox::from_id_salt("extrude_op")
                .selected_text(f.operation.label())
                .show_ui(ui, |ui| {
                    for op in [Operation::NewBody, Operation::Add, Operation::Cut] {
                        ui.selectable_value(&mut f.operation, op, op.label());
                    }
                });
            ui.end_row();

            ui.label("End");
            let current = std::mem::discriminant(&f.end);
            egui::ComboBox::from_id_salt("extrude_end")
                .selected_text(f.end.label())
                .show_ui(ui, |ui| {
                    for end in [
                        EndCondition::Blind,
                        EndCondition::Symmetric,
                        EndCondition::ThroughAll,
                    ] {
                        let label = end.label();
                        if ui
                            .selectable_label(current == std::mem::discriminant(&end), label)
                            .clicked()
                        {
                            f.end = end;
                        }
                    }
                    if ui
                        .selectable_label(matches!(f.end, EndCondition::UpTo(_)), "Up to face…")
                        .on_hover_text(Slot::UpTo.prompt())
                        .clicked()
                    {
                        out.pick = Some(Slot::UpTo);
                    }
                });
            ui.end_row();

            if f.end.uses_depth() {
                ui.label("Depth");
                out.committed |= scalar_field(
                    ui,
                    "extrude_depth",
                    &mut f.depth,
                    ScalarKind::Length,
                    params,
                );
                ui.end_row();
            }
            if let EndCondition::UpTo(r) = &f.end {
                let text = plane_text(doc, r);
                reference_row(ui, "Up to", text, Slot::UpTo, picking, &mut out);
            } else if picking == Some(Slot::UpTo) {
                ui.label("Up to");
                ui.colored_label(PICKING, "click it…");
                ui.end_row();
            }
            if !matches!(f.end, EndCondition::Symmetric | EndCondition::UpTo(_)) {
                ui.label("Direction");
                ui.checkbox(&mut f.reverse, "Reverse");
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.weak("Regions: the sketch's outer regions with their holes (islands inside holes are extruded too).");
    out
}

/// The definition of a reference plane, axis, point or coordinate system.
pub fn reference_panel(
    ui: &mut Ui,
    doc: &Document,
    kind: &mut FeatureKind,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("reference_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| match kind {
            FeatureKind::Plane(def) => {
                ui.label("Type");
                let label = match def {
                    PlaneDef::Offset { .. } => "Offset",
                    PlaneDef::Angled { .. } => "At an angle",
                    PlaneDef::Midplane { .. } => "Mid plane",
                };
                let base = match def {
                    PlaneDef::Offset { from, .. } | PlaneDef::Angled { from, .. } => from.clone(),
                    PlaneDef::Midplane { a, .. } => a.clone(),
                };
                egui::ComboBox::from_id_salt("plane_type")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(label == "Offset", "Offset").clicked() {
                            *def = PlaneDef::Offset {
                                from: base.clone(),
                                distance: Scalar::new(10.0),
                                flip: false,
                            };
                        }
                        if ui
                            .selectable_label(label == "At an angle", "At an angle")
                            .clicked()
                        {
                            *def = PlaneDef::Angled {
                                from: base.clone(),
                                about: AxisRef::Standard(StdAxis::X),
                                angle: Scalar::new(45.0),
                            };
                        }
                        if ui
                            .selectable_label(label == "Mid plane", "Mid plane")
                            .clicked()
                        {
                            *def = PlaneDef::Midplane {
                                a: base.clone(),
                                b: base.clone(),
                            };
                        }
                    });
                ui.end_row();
                match def {
                    PlaneDef::Offset {
                        from,
                        distance,
                        flip,
                    } => {
                        reference_row(
                            ui,
                            "From",
                            plane_text(doc, from),
                            Slot::PlaneFirst,
                            picking,
                            &mut out,
                        );
                        ui.label("Distance");
                        out.committed |= scalar_field(
                            ui,
                            "plane_distance",
                            distance,
                            ScalarKind::Length,
                            params,
                        );
                        ui.end_row();
                        ui.label("Direction");
                        ui.checkbox(flip, "Flip");
                        ui.end_row();
                    }
                    PlaneDef::Angled { from, about, angle } => {
                        reference_row(
                            ui,
                            "From",
                            plane_text(doc, from),
                            Slot::PlaneFirst,
                            picking,
                            &mut out,
                        );
                        ui.label("About");
                        egui::ComboBox::from_id_salt("plane_axis")
                            .selected_text(axis_text(doc, about))
                            .show_ui(ui, |ui| {
                                for a in StdAxis::ALL {
                                    ui.selectable_value(about, AxisRef::Standard(a), a.label());
                                }
                                for f in doc.model.features() {
                                    if matches!(
                                        f.kind,
                                        FeatureKind::Axis(_) | FeatureKind::CoordSystem(_)
                                    ) {
                                        ui.selectable_value(about, AxisRef::Feature(f.id), &f.name);
                                    }
                                }
                            });
                        ui.end_row();
                        ui.label("Angle");
                        out.committed |=
                            scalar_field(ui, "plane_angle", angle, ScalarKind::Angle, params);
                        ui.end_row();
                    }
                    PlaneDef::Midplane { a, b } => {
                        reference_row(
                            ui,
                            "Between",
                            plane_text(doc, a),
                            Slot::PlaneFirst,
                            picking,
                            &mut out,
                        );
                        reference_row(
                            ui,
                            "and",
                            plane_text(doc, b),
                            Slot::PlaneSecond,
                            picking,
                            &mut out,
                        );
                    }
                }
            }
            FeatureKind::Axis(def) => {
                let text = match def {
                    AxisDef::Edge(_) => "Along an edge".to_owned(),
                    AxisDef::Cylinder(f) => format!("Axis of {}", doc.model.describe_face(&f.name)),
                    AxisDef::TwoPlanes(a, _) => plane_text(doc, a),
                };
                reference_row(ui, "From", text, Slot::AxisFirst, picking, &mut out);
                if let AxisDef::TwoPlanes(_, b) = def {
                    reference_row(
                        ui,
                        "Meeting",
                        plane_text(doc, b),
                        Slot::AxisSecond,
                        picking,
                        &mut out,
                    );
                }
            }
            FeatureKind::Point(def) => {
                ui.label("Type");
                let at_vertex = matches!(def, PointDef::Vertex(_));
                egui::ComboBox::from_id_salt("point_type")
                    .selected_text(if at_vertex {
                        "At a vertex"
                    } else {
                        "Coordinates"
                    })
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(!at_vertex, "Coordinates").clicked() && at_vertex {
                            *def = PointDef::Coordinates {
                                x: Scalar::new(0.0),
                                y: Scalar::new(0.0),
                                z: Scalar::new(0.0),
                            };
                        }
                        if ui.selectable_label(at_vertex, "At a vertex").clicked() && !at_vertex {
                            out.pick = Some(Slot::PointVertex);
                        }
                    });
                ui.end_row();
                match def {
                    PointDef::Vertex(_) => {
                        reference_row(
                            ui,
                            "Vertex",
                            "A vertex".to_owned(),
                            Slot::PointVertex,
                            picking,
                            &mut out,
                        );
                    }
                    PointDef::Coordinates { x, y, z } => {
                        for (label, s) in [("X", x), ("Y", y), ("Z", z)] {
                            ui.label(label);
                            out.committed |=
                                scalar_field(ui, ("point", label), s, ScalarKind::Length, params);
                            ui.end_row();
                        }
                    }
                }
            }
            FeatureKind::CoordSystem(CoordSystemDef {
                origin,
                orientation,
            }) => {
                ui.label("Origin");
                ui.horizontal(|ui| {
                    if picking == Some(Slot::CsysOrigin) {
                        ui.colored_label(PICKING, "click a vertex…");
                    } else {
                        ui.label(point_text(doc, origin));
                    }
                    egui::ComboBox::from_id_salt("csys_origin")
                        .selected_text("…")
                        .show_ui(ui, |ui| {
                            ui.selectable_value(origin, PointRef::Origin, "Origin");
                            for f in doc.model.features() {
                                if matches!(f.kind, FeatureKind::Point(_)) {
                                    ui.selectable_value(origin, PointRef::Feature(f.id), &f.name);
                                }
                            }
                            if ui.selectable_label(false, "Pick a vertex…").clicked() {
                                out.pick = Some(Slot::CsysOrigin);
                            }
                        });
                });
                ui.end_row();
                reference_row(
                    ui,
                    "Axes of",
                    plane_text(doc, orientation),
                    Slot::CsysOrientation,
                    picking,
                    &mut out,
                );
            }
            FeatureKind::Sketch(_) | FeatureKind::Extrude(_) => {}
        });
    out
}

/// A status line for a feature: its error or warning, if any.
pub fn status_line(ui: &mut Ui, status: Option<&peet_model::Status>) {
    match status {
        Some(peet_model::Status::Failed(m)) => {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ERROR, "⚠");
                ui.label(m);
            });
            ui.add_space(4.0);
        }
        Some(peet_model::Status::Warning(m)) => {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(WARNING, "⚠");
                ui.label(m);
            });
            ui.add_space(4.0);
        }
        Some(peet_model::Status::Suppressed) => {
            ui.weak("Suppressed: it is skipped when the part is built.");
            ui.add_space(4.0);
        }
        Some(peet_model::Status::RolledBack) => {
            ui.weak("Below the rollback bar: not built.");
            ui.add_space(4.0);
        }
        _ => {}
    }
}
