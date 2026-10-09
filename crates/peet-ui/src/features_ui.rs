//! Properties panels for features: extrusions, sheet metal and reference geometry.
//!
//! Panels edit a copy of the feature's definition; the app compares it with the model
//! afterwards and applies the difference as an undoable change. References to other
//! geometry are set by clicking it in the viewport: a panel asks for a [`Slot`] to be
//! filled, and [`apply_pick`] puts what was clicked there.

use egui::{Color32, Ui};
use peet_model::{
    AxisDef, AxisRef, BaseFlangeFeature, BendModelDef, CoordSystemDef, CornerFeature,
    EdgeFlangeFeature, EdgeRef, EndCondition, FaceRef, FeatureId, FeatureKind, FormFeature,
    HemFeature, JogFeature, LinearDirection, MirrorFeature, MiterFlangeFeature, Operation,
    PatternDef, PatternFeature, PlaneDef, PlaneRef, PointDef, PointRef, RevolveAxisRef, Scalar,
    ScalarKind, SheetSettingsDef, SketchedBendFeature, StdAxis, VertexRef,
};
use peet_sheetmetal::corner::{CornerKind, CornerRelief};
use peet_sheetmetal::{
    BendLinePosition, FlangePosition, FormKind, HemKind, JogDimension, ReliefType,
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
    /// The edge an edge flange goes on.
    FlangeEdge,
    /// The edge a hem goes on.
    HemEdge,
    /// Another edge for a mitre flange to run along.
    MiterEdge,
    /// A flange's face at a corner, for a corner treatment.
    CornerFace,
    /// A pattern's direction or axis, as an edge.
    PatternAxis,
    /// A linear pattern's second direction, as an edge.
    PatternAxis2,
    /// The plane of a mirror.
    MirrorPlane,
    /// What a revolve turns about, as an edge or a reference axis.
    RevolveAxis,
    /// Another edge for a fillet or a chamfer.
    BlendEdge,
    /// Another face for a shell to open.
    ShellFace,
    /// Another face to draft.
    DraftFace,
    /// A draft's neutral plane.
    DraftNeutral,
    /// The sketch a sweep's path is drawn in.
    SweepPath,
    /// Another profile sketch for a loft.
    LoftProfile,
    /// The flat face a conversion to sheet metal keeps fixed.
    ConvertFace,
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
            Self::FlangeEdge => {
                "Click an edge along the top or bottom face of the sheet metal part (the flange bends towards that face)."
            }
            Self::HemEdge => {
                "Click an edge along the top or bottom face of the sheet metal part (the hem folds over that face)."
            }
            Self::MiterEdge => {
                "Click the next edge for the flange to run along: an edge of the same face, joined to the others."
            }
            Self::CornerFace => {
                "Click a face of a flange at the corner: its end face for that corner, or its flat face for both its corners."
            }
            Self::PatternAxis | Self::PatternAxis2 => {
                "Click a straight edge for the direction (or a round edge for an axis through its centre)."
            }
            Self::MirrorPlane => "Click a flat face or a plane to mirror across.",
            Self::RevolveAxis => {
                "Click a straight edge in the sketch's plane to revolve about (or a reference axis in the feature tree)."
            }
            Self::BlendEdge => {
                "Click the edges to round or chamfer, one after the other. Esc when done."
            }
            Self::ShellFace => {
                "Click the faces to remove, opening the hollow, one after the other. Esc when done."
            }
            Self::DraftFace => {
                "Click the faces to taper (flat, round along the pull, or freeform), one after the other (a picked face again to take it out). Esc when done."
            }
            Self::DraftNeutral => {
                "Click a flat face or a plane for the neutral plane: the faces keep their size where they cross it."
            }
            Self::SweepPath => {
                "Click the sketch of the path in the feature tree: lines and arcs joined end to end, or a spline, starting on the profile's plane."
            }
            Self::LoftProfile => {
                "Click the sketches of the next profiles in the feature tree, in the order the loft passes through them. Esc when done."
            }
            Self::ConvertFace => {
                "Click the flat face of the solid body that stays fixed: the part unfolds from it."
            }
        }
    }

    /// Whether picking carries on after a click, to pick several things in a row (until
    /// Esc or Done).
    pub fn repeats(self) -> bool {
        matches!(
            self,
            Self::BlendEdge | Self::ShellFace | Self::DraftFace | Self::LoftProfile
        )
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
    /// A reference axis.
    Axis(AxisRef),
    /// A sketch, clicked in the feature tree.
    Sketch(FeatureId),
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

/// Adds `item` to a list of picks, or takes it out if it is there already.
fn toggle<T: PartialEq>(list: &mut Vec<T>, item: T) {
    match list.iter().position(|x| *x == item) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(item),
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
        (FeatureKind::EdgeFlange(e), Slot::FlangeEdge) => match picked {
            Picked::Edge(r) => e.edge = Some(r),
            _ => return Err("Pick an edge of the sheet metal part, not a face.".into()),
        },
        (FeatureKind::Hem(h), Slot::HemEdge) => match picked {
            Picked::Edge(r) => h.edge = Some(r),
            _ => return Err("Pick an edge of the sheet metal part, not a face.".into()),
        },
        (FeatureKind::MiterFlange(m), Slot::MiterEdge) => match picked {
            Picked::Edge(r) => {
                if !m.edges.contains(&r) {
                    m.edges.push(r);
                }
            }
            _ => return Err("Pick an edge of the sheet metal part, not a face.".into()),
        },
        (FeatureKind::Corner(c), Slot::CornerFace) => match picked {
            Picked::Face { face, .. } => {
                if !c.faces.contains(&face) {
                    c.faces.push(face);
                }
            }
            _ => return Err("Pick a face of a flange at the corner.".into()),
        },
        (FeatureKind::Pattern(p), Slot::PatternAxis | Slot::PatternAxis2) => {
            let Picked::Edge(e) = picked else {
                return Err("Pick an edge for the direction.".into());
            };
            match (&mut p.def, slot) {
                (PatternDef::Linear { first, .. }, Slot::PatternAxis) => {
                    first.direction = AxisRef::Edge(e);
                }
                (
                    PatternDef::Linear {
                        second: Some(s), ..
                    },
                    Slot::PatternAxis2,
                ) => s.direction = AxisRef::Edge(e),
                (PatternDef::Circular { axis, .. }, Slot::PatternAxis) => {
                    *axis = AxisRef::Edge(e);
                }
                _ => return Err("That can't be used here.".into()),
            }
        }
        (FeatureKind::Mirror(m), Slot::MirrorPlane) => m.plane = picked.plane()?,
        (FeatureKind::Revolve(r), Slot::RevolveAxis) => match picked {
            Picked::Edge(e) => r.axis = RevolveAxisRef::Axis(AxisRef::Edge(e)),
            Picked::Axis(a) => r.axis = RevolveAxisRef::Axis(a),
            _ => {
                return Err(
                    "Pick a straight edge that lies in the sketch's plane, or a reference axis in the feature tree."
                        .into(),
                );
            }
        },
        (FeatureKind::Blend(b), Slot::BlendEdge) => match picked {
            Picked::Edge(e) => toggle(&mut b.edges, e),
            _ => return Err("Pick an edge of a body, not a face or a vertex.".into()),
        },
        (FeatureKind::Shell(s), Slot::ShellFace) => match picked {
            Picked::Face { face, .. } => toggle(&mut s.open, face),
            _ => return Err("Pick a face of the body to remove.".into()),
        },
        (FeatureKind::Draft(d), Slot::DraftFace) => match picked {
            // Flat faces, and round ones along the pull: the rebuild says which won't do.
            Picked::Face { face, .. } => toggle(&mut d.faces, face),
            _ => return Err("Pick a face of the body to taper.".into()),
        },
        (FeatureKind::Draft(d), Slot::DraftNeutral) => d.neutral = Some(picked.plane()?),
        (FeatureKind::Sweep(s), Slot::SweepPath) => match picked {
            Picked::Sketch(id) if id == s.profile => {
                return Err(
                    "That sketch is the profile. The path is drawn in another sketch.".into(),
                );
            }
            Picked::Sketch(id) => s.path = Some(id),
            _ => return Err("Pick the sketch of the path in the feature tree.".into()),
        },
        (FeatureKind::Loft(l), Slot::LoftProfile) => match picked {
            Picked::Sketch(id) if l.sections.contains(&id) => {
                return Err(
                    "That sketch is a profile of the loft already. Pick another sketch.".into(),
                );
            }
            Picked::Sketch(id) => l.sections.push(id),
            _ => return Err("Pick the sketch of a profile in the feature tree.".into()),
        },
        (FeatureKind::ConvertToSheet(c), Slot::ConvertFace) => match picked {
            Picked::Face {
                face, planar: true, ..
            } => c.face = Some(face),
            Picked::Face { .. } => {
                return Err(
                    "The fixed face must be flat: pick one of the part's flat walls.".into(),
                );
            }
            _ => return Err("Pick a flat face of the body to convert.".into()),
        },
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
    /// Stop picking (the Done button of a list of picks).
    pub stop_pick: bool,
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
            ScalarKind::Number => "",
        };
        ui.weak(unit);
        if value.expression.is_some()
            && let Ok(v) = value.evaluate(kind, params)
        {
            let shown = match kind {
                ScalarKind::Length => params.units.format_length(v),
                ScalarKind::Angle => params.units.format_angle(v),
                ScalarKind::Number => peet_model::feature::format_number(v),
            };
            ui.weak(format!("= {shown}"));
        }
    });
    if let Some(e) = error {
        ui.colored_label(ERROR, e);
    }
    changed
}

/// A label that long descriptions don't widen the whole panel with: shortened, with the
/// full text on hover.
pub(crate) fn short_label(ui: &mut Ui, text: String) {
    const MAX: usize = 28;
    if text.chars().count() > MAX {
        let short: String = text.chars().take(MAX - 1).collect();
        ui.label(format!("{}…", short.trim_end()))
            .on_hover_text(text);
    } else {
        ui.label(text);
    }
}

/// A reference shown as text with a button to pick it again.
pub(crate) fn reference_row(
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
            short_label(ui, text);
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

pub(crate) fn plane_text(doc: &Document, r: &PlaneRef) -> String {
    doc.plane_name(r)
}

pub(crate) fn axis_text(doc: &Document, r: &AxisRef) -> String {
    match r {
        AxisRef::Standard(a) => a.label().to_owned(),
        AxisRef::Feature(id) => doc.model.name_of(*id).to_owned(),
        AxisRef::Edge(_) => "An edge".to_owned(),
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
            FeatureKind::Sketch(_)
            | FeatureKind::Extrude(_)
            | FeatureKind::BaseFlange(_)
            | FeatureKind::EdgeFlange(_)
            | FeatureKind::SheetCut(_)
            | FeatureKind::Hem(_)
            | FeatureKind::SketchedBend(_)
            | FeatureKind::Jog(_)
            | FeatureKind::MiterFlange(_)
            | FeatureKind::Corner(_)
            | FeatureKind::Form(_)
            | FeatureKind::Pattern(_)
            | FeatureKind::Mirror(_)
            | FeatureKind::Revolve(_)
            | FeatureKind::Blend(_)
            | FeatureKind::Shell(_)
            | FeatureKind::Draft(_)
            | FeatureKind::Hole(_)
            | FeatureKind::Import(_)
            | FeatureKind::Sweep(_)
            | FeatureKind::Loft(_)
            | FeatureKind::ConvertToSheet(_) => {}
        });
    out
}

/// The bend model of a sheet metal body: which kind, and its value.
fn bend_model_rows(
    ui: &mut Ui,
    model: &mut BendModelDef,
    params: &Parameters,
    out: &mut PanelResult,
) {
    ui.label("Bend model");
    let label = match model {
        BendModelDef::KFactor(_) => "K-factor",
        BendModelDef::Allowance(_) => "Bend allowance",
        BendModelDef::Deduction(_) => "Bend deduction",
    };
    egui::ComboBox::from_id_salt("sm_model")
        .selected_text(label)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(label == "K-factor", "K-factor")
                .clicked()
                && !matches!(model, BendModelDef::KFactor(_))
            {
                *model = BendModelDef::KFactor(Scalar::new(0.44));
            }
            if ui
                .selectable_label(label == "Bend allowance", "Bend allowance")
                .on_hover_text("A fixed flat length for every bend.")
                .clicked()
                && !matches!(model, BendModelDef::Allowance(_))
            {
                *model = BendModelDef::Allowance(Scalar::new(2.0));
            }
            if ui
                .selectable_label(label == "Bend deduction", "Bend deduction")
                .on_hover_text("How much shorter than the outside lengths the flat is, per bend.")
                .clicked()
                && !matches!(model, BendModelDef::Deduction(_))
            {
                *model = BendModelDef::Deduction(Scalar::new(2.0));
            }
        });
    ui.end_row();
    match model {
        BendModelDef::KFactor(k) => {
            ui.label("K-factor").on_hover_text(
                "Where the neutral axis lies, as a fraction of the thickness from the inside of the bend (0.3 to 0.5 is typical).",
            );
            out.committed |= scalar_field(ui, "sm_k", k, ScalarKind::Number, params);
        }
        BendModelDef::Allowance(v) => {
            ui.label("Allowance");
            out.committed |= scalar_field(ui, "sm_ba", v, ScalarKind::Length, params);
        }
        BendModelDef::Deduction(v) => {
            ui.label("Deduction");
            out.committed |= scalar_field(ui, "sm_bd", v, ScalarKind::Length, params);
        }
    }
    ui.end_row();
}

/// A sheet metal body's settings (thickness, radius, bend model, reliefs).
fn sheet_settings_rows(
    ui: &mut Ui,
    s: &mut SheetSettingsDef,
    params: &Parameters,
    out: &mut PanelResult,
) {
    ui.label("Thickness");
    out.committed |= scalar_field(
        ui,
        "sm_thickness",
        &mut s.thickness,
        ScalarKind::Length,
        params,
    );
    ui.end_row();
    ui.label("Bend radius");
    out.committed |= scalar_field(ui, "sm_radius", &mut s.radius, ScalarKind::Length, params);
    ui.end_row();

    bend_model_rows(ui, &mut s.model, params, out);

    ui.label("Relief");
    egui::ComboBox::from_id_salt("sm_relief")
        .selected_text(s.relief.label())
        .show_ui(ui, |ui| {
            for r in ReliefType::ALL {
                ui.selectable_value(&mut s.relief, r, r.label());
            }
        })
        .response
        .on_hover_text("The cut made where a bend stops short of the end of an edge.");
    ui.end_row();
    if s.relief != ReliefType::Tear {
        ui.label("Relief ratio").on_hover_text(
            "Relief width, and how far it reaches past the bend, as a multiple of the thickness.",
        );
        out.committed |= scalar_field(
            ui,
            "sm_ratio",
            &mut s.relief_ratio,
            ScalarKind::Number,
            params,
        );
        ui.end_row();
    }
}

/// Convert to sheet metal: the fixed face, the bend model and the reliefs for flanges
/// added later. The thickness and the bend radii are the solid's own.
pub fn convert_panel(
    ui: &mut Ui,
    doc: &Document,
    c: &mut peet_model::ConvertToSheetFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("convert_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            let text = match &c.face {
                Some(f) => {
                    let name = doc.model.describe_face(&f.name);
                    let mut chars = name.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().chain(chars).collect(),
                        None => name,
                    }
                }
                None => "Largest flat face".to_owned(),
            };
            reference_row(ui, "Fixed face", text, Slot::ConvertFace, picking, &mut out);
            bend_model_rows(ui, &mut c.model, params, &mut out);
            ui.label("Relief");
            egui::ComboBox::from_id_salt("convert_relief")
                .selected_text(c.relief.label())
                .show_ui(ui, |ui| {
                    for r in ReliefType::ALL {
                        ui.selectable_value(&mut c.relief, r, r.label());
                    }
                })
                .response
                .on_hover_text(
                    "For flanges added to the converted body: the cut made where a bend stops short of the end of an edge.",
                );
            ui.end_row();
            if c.relief != ReliefType::Tear {
                ui.label("Relief ratio").on_hover_text(
                    "Relief width, and how far it reaches past the bend, as a multiple of the thickness.",
                );
                out.committed |= scalar_field(
                    ui,
                    "convert_ratio",
                    &mut c.relief_ratio,
                    ScalarKind::Number,
                    params,
                );
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.weak("The body must be a sheet of one thickness: flat walls joined by rounded bends, cut square through the sheet. The thickness and the bend radii are measured from it; the fixed face stays in place when the part is unfolded.");
    out
}

/// Base flange: the sheet settings, plus depth for open profiles.
pub fn base_flange_panel(ui: &mut Ui, doc: &Document, b: &mut BaseFlangeFeature) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    let open = doc.model.sketch(b.sketch).is_some_and(|s| {
        peet_sketch::region::find_regions(&s.sketch)
            .regions
            .is_empty()
    });
    egui::Grid::new("base_flange_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Sketch");
            ui.label(format!(
                "{} ({})",
                doc.model.name_of(b.sketch),
                if open { "open profile" } else { "plate" }
            ));
            ui.end_row();
            if open {
                ui.label("Depth");
                out.committed |=
                    scalar_field(ui, "bf_depth", &mut b.depth, ScalarKind::Length, params);
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut b.symmetric, "Mid-plane");
                    if !b.symmetric {
                        ui.checkbox(&mut b.flip_depth, "Reverse");
                    }
                });
                ui.end_row();
            }
            ui.label("Thickness side");
            ui.checkbox(&mut b.reverse, "Flip").on_hover_text(if open {
                "Put the thickness on the other side of the lines."
            } else {
                "Put the thickness on the other side of the sketch plane."
            });
            ui.end_row();
            sheet_settings_rows(ui, &mut b.settings, params, &mut out);
        });
    ui.add_space(6.0);
    ui.weak(if open {
        "The lines are one face of the sheet; each corner gets a bend with the bend radius."
    } else {
        "A flat plate from the sketch's regions. Add flanges on its edges with Edge Flange."
    });
    out
}

/// Edge flange: its edge, length, angle, position, offsets and radius.
pub fn edge_flange_panel(
    ui: &mut Ui,
    doc: &Document,
    e: &mut EdgeFlangeFeature,
    picking: Option<Slot>,
    default_radius: Option<f64>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("edge_flange_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            let text = match &e.edge {
                Some(r) => {
                    let names: Vec<String> = r
                        .faces
                        .iter()
                        .map(|f| doc.model.describe_face(f))
                        .collect();
                    names.join(" / ")
                }
                None => "None yet".to_owned(),
            };
            reference_row(ui, "Edge", text, Slot::FlangeEdge, picking, &mut out);
            ui.label("Length").on_hover_text(
                "Outside length: from the outer virtual sharp (where the outer faces would meet without the bend) to the end of the flange. Drag the arrow in the view to change it.",
            );
            out.committed |= scalar_field(ui, "ef_length", &mut e.length, ScalarKind::Length, params);
            ui.end_row();
            ui.label("Angle");
            out.committed |= scalar_field(ui, "ef_angle", &mut e.angle, ScalarKind::Angle, params);
            ui.end_row();
            ui.label("Position");
            egui::ComboBox::from_id_salt("ef_position")
                .selected_text(e.position.label())
                .show_ui(ui, |ui| {
                    for p in FlangePosition::ALL {
                        let tip = match p {
                            FlangePosition::MaterialInside => {
                                "The flange's outside is flush with the edge: the part keeps its outside size."
                            }
                            FlangePosition::MaterialOutside => {
                                "The flange's inside is flush with the edge."
                            }
                            FlangePosition::BendOutside => {
                                "The bend starts at the edge: the face keeps its full size."
                            }
                        };
                        ui.selectable_value(&mut e.position, p, p.label())
                            .on_hover_text(tip);
                    }
                });
            ui.end_row();
            ui.label("Offset start");
            out.committed |=
                scalar_field(ui, "ef_off0", &mut e.offset_start, ScalarKind::Length, params);
            ui.end_row();
            ui.label("Offset end");
            out.committed |=
                scalar_field(ui, "ef_off1", &mut e.offset_end, ScalarKind::Length, params);
            ui.end_row();
            ui.label("Direction");
            ui.checkbox(&mut e.flip, "Flip")
                .on_hover_text("Bend to the other side of the sheet.");
            ui.end_row();
            ui.label("Bend radius");
            ui.horizontal(|ui| {
                let mut custom = e.radius.is_some();
                if ui.checkbox(&mut custom, "Custom").changed() {
                    e.radius = custom.then(|| Scalar::new(default_radius.unwrap_or(1.0)));
                }
                if e.radius.is_none()
                    && let Some(r) = default_radius
                {
                    ui.weak(format!("body default, {}", params.units.format_length(r)));
                }
            });
            ui.end_row();
            if let Some(r) = &mut e.radius {
                ui.label("");
                out.committed |= scalar_field(ui, "ef_radius", r, ScalarKind::Length, params);
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.weak(
        "Set the offsets to stop the flange short of the corners; reliefs are cut where it does.",
    );
    out
}

// ---- Phase 5 panels ----

/// A row with a label and a value field.
#[allow(clippy::too_many_arguments)]
pub(crate) fn value_row(
    ui: &mut Ui,
    label: &str,
    tip: &str,
    id: &str,
    value: &mut Scalar,
    kind: ScalarKind,
    params: &Parameters,
    out: &mut PanelResult,
) {
    let l = ui.label(label);
    if !tip.is_empty() {
        l.on_hover_text(tip);
    }
    out.committed |= scalar_field(ui, id, value, kind, params);
    ui.end_row();
}

/// A custom bend radius, or the body's default.
fn radius_row(
    ui: &mut Ui,
    id: &str,
    radius: &mut Option<Scalar>,
    default_radius: Option<f64>,
    params: &Parameters,
    out: &mut PanelResult,
) {
    ui.label("Bend radius");
    ui.horizontal(|ui| {
        let mut custom = radius.is_some();
        if ui.checkbox(&mut custom, "Custom").changed() {
            *radius = custom.then(|| Scalar::new(default_radius.unwrap_or(1.0)));
        }
        if radius.is_none()
            && let Some(r) = default_radius
        {
            ui.weak(format!("body default, {}", params.units.format_length(r)));
        }
    });
    ui.end_row();
    if let Some(r) = radius {
        ui.label("");
        out.committed |= scalar_field(ui, id, r, ScalarKind::Length, params);
        ui.end_row();
    }
}

pub(crate) fn edge_text(doc: &Document, edge: &EdgeRef) -> String {
    let names: Vec<String> = edge
        .faces
        .iter()
        .map(|f| doc.model.describe_face(f))
        .collect();
    names.join(" / ")
}

/// Hem: its edge, kind and sizes.
pub fn hem_panel(
    ui: &mut Ui,
    doc: &Document,
    h: &mut HemFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("hem_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            let text = h
                .edge
                .as_ref()
                .map_or("None yet".to_owned(), |e| edge_text(doc, e));
            reference_row(ui, "Edge", text, Slot::HemEdge, picking, &mut out);
            ui.label("Type");
            egui::ComboBox::from_id_salt("hem_kind")
                .selected_text(h.kind.label())
                .show_ui(ui, |ui| {
                    for k in HemKind::ALL {
                        let tip = match k {
                            HemKind::Closed => "Folded flat onto the sheet.",
                            HemKind::Open => "Folded back with a gap.",
                            HemKind::Teardrop => {
                                "Folded past 180° so the end comes back towards the sheet."
                            }
                            HemKind::Rolled => "A curl with no flat end.",
                        };
                        ui.selectable_value(&mut h.kind, k, k.label())
                            .on_hover_text(tip);
                    }
                });
            ui.end_row();
            match h.kind {
                HemKind::Closed | HemKind::Open => {
                    value_row(
                        ui,
                        "Length",
                        "From the outside of the fold to the end of the hem.",
                        "hem_length",
                        &mut h.length,
                        ScalarKind::Length,
                        params,
                        &mut out,
                    );
                    if h.kind == HemKind::Open {
                        value_row(
                            ui,
                            "Gap",
                            "Between the sheet and the hem.",
                            "hem_gap",
                            &mut h.gap,
                            ScalarKind::Length,
                            params,
                            &mut out,
                        );
                    }
                }
                HemKind::Teardrop | HemKind::Rolled => {
                    value_row(
                        ui,
                        "Radius",
                        "Inner radius of the fold.",
                        "hem_radius",
                        &mut h.radius,
                        ScalarKind::Length,
                        params,
                        &mut out,
                    );
                    value_row(
                        ui,
                        "Angle",
                        "How far the edge turns: more than 180°.",
                        "hem_angle",
                        &mut h.angle,
                        ScalarKind::Angle,
                        params,
                        &mut out,
                    );
                    if h.kind == HemKind::Teardrop {
                        value_row(
                            ui,
                            "End length",
                            "The flat end after the fold.",
                            "hem_length",
                            &mut h.length,
                            ScalarKind::Length,
                            params,
                            &mut out,
                        );
                    }
                }
            }
            ui.label("Position");
            ui.checkbox(&mut h.inside, "Keep the outline")
                .on_hover_text("The outside of the fold is flush with the original edge. Off: the fold starts at the edge and the part grows.");
            ui.end_row();
            value_row(
                ui,
                "Offset start",
                "",
                "hem_off0",
                &mut h.offset_start,
                ScalarKind::Length,
                params,
                &mut out,
            );
            value_row(
                ui,
                "Offset end",
                "",
                "hem_off1",
                &mut h.offset_end,
                ScalarKind::Length,
                params,
                &mut out,
            );
            ui.label("Direction");
            ui.checkbox(&mut h.flip, "Flip")
                .on_hover_text("Fold to the other side of the sheet.");
            ui.end_row();
        });
    out
}

fn position_combo(ui: &mut Ui, id: &str, position: &mut BendLinePosition) {
    ui.label("Position")
        .on_hover_text("Where the bend sits relative to the sketched line.");
    egui::ComboBox::from_id_salt(id)
        .selected_text(position.label())
        .show_ui(ui, |ui| {
            for p in BendLinePosition::ALL {
                let tip = match p {
                    BendLinePosition::Centerline => "The line is the middle of the bend.",
                    BendLinePosition::MaterialInside => {
                        "The line is where the outer faces of the two sides meet."
                    }
                    BendLinePosition::MaterialOutside => {
                        "The line is where the inner faces of the two sides meet."
                    }
                    BendLinePosition::BendOutside => "The bend starts at the line.",
                };
                ui.selectable_value(position, p, p.label())
                    .on_hover_text(tip);
            }
        });
    ui.end_row();
}

/// Sketched bend: angle, radius, position and directions.
pub fn sketched_bend_panel(
    ui: &mut Ui,
    doc: &Document,
    b: &mut SketchedBendFeature,
    default_radius: Option<f64>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("sketched_bend_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Sketch");
            ui.label(doc.model.name_of(b.sketch));
            ui.end_row();
            value_row(
                ui,
                "Angle",
                "",
                "sb_angle",
                &mut b.angle,
                ScalarKind::Angle,
                params,
                &mut out,
            );
            position_combo(ui, "sb_position", &mut b.position);
            ui.label("Direction");
            ui.checkbox(&mut b.flip, "Flip")
                .on_hover_text("Bend away from the face the sketch is on.");
            ui.end_row();
            ui.label("Fixed side");
            ui.checkbox(&mut b.flip_fixed, "Other side")
                .on_hover_text("On the part's first face, the larger side stays where it is. Tick to keep the other side instead.");
            ui.end_row();
            radius_row(ui, "sb_radius", &mut b.radius, default_radius, params, &mut out);
        });
    ui.add_space(6.0);
    ui.weak("Each line of the sketch is a bend. Draw the lines right across the face.");
    out
}

/// Jog: offset, angle, position and directions.
pub fn jog_panel(
    ui: &mut Ui,
    doc: &Document,
    j: &mut JogFeature,
    default_radius: Option<f64>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("jog_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Sketch");
            ui.label(doc.model.name_of(j.sketch));
            ui.end_row();
            value_row(
                ui,
                "Offset",
                "How far the far side is stepped.",
                "jog_offset",
                &mut j.offset,
                ScalarKind::Length,
                params,
                &mut out,
            );
            ui.label("Measured");
            egui::ComboBox::from_id_salt("jog_dimension")
                .selected_text(j.dimension.label())
                .show_ui(ui, |ui| {
                    for d in JogDimension::ALL {
                        ui.selectable_value(&mut j.dimension, d, d.label());
                    }
                });
            ui.end_row();
            value_row(
                ui,
                "Angle",
                "The angle of both bends (90° for a square step).",
                "jog_angle",
                &mut j.angle,
                ScalarKind::Angle,
                params,
                &mut out,
            );
            position_combo(ui, "jog_position", &mut j.position);
            ui.label("Direction");
            ui.checkbox(&mut j.flip, "Flip")
                .on_hover_text("Step away from the face the sketch is on.");
            ui.end_row();
            ui.label("Fixed side");
            ui.checkbox(&mut j.flip_fixed, "Other side");
            ui.end_row();
            radius_row(
                ui,
                "jog_radius",
                &mut j.radius,
                default_radius,
                params,
                &mut out,
            );
        });
    ui.add_space(6.0);
    ui.weak("The flat pattern keeps its length, so the far side also moves in a little.");
    out
}

/// Mitre flange: its profile, edges, gap and offsets.
pub fn miter_flange_panel(
    ui: &mut Ui,
    doc: &Document,
    m: &mut MiterFlangeFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("miter_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Profile");
            ui.label(doc.model.name_of(m.sketch));
            ui.end_row();
            ui.label("Edges");
            ui.horizontal(|ui| {
                if picking == Some(Slot::MiterEdge) {
                    ui.colored_label(PICKING, "click an edge…");
                } else {
                    ui.label(format!("{} picked", m.edges.len()));
                }
                if ui
                    .small_button("Add")
                    .on_hover_text(Slot::MiterEdge.prompt())
                    .clicked()
                {
                    out.pick = Some(Slot::MiterEdge);
                }
                if !m.edges.is_empty() && ui.small_button("Clear").clicked() {
                    m.edges.clear();
                }
            });
            ui.end_row();
            value_row(
                ui,
                "Gap",
                "Left between the flanges where the edges meet.",
                "miter_gap",
                &mut m.gap,
                ScalarKind::Length,
                params,
                &mut out,
            );
            value_row(
                ui,
                "Offset start",
                "Sets the flange back from the start of the chain of edges.",
                "miter_off0",
                &mut m.offset_start,
                ScalarKind::Length,
                params,
                &mut out,
            );
            value_row(
                ui,
                "Offset end",
                "",
                "miter_off1",
                &mut m.offset_end,
                ScalarKind::Length,
                params,
                &mut out,
            );
        });
    ui.add_space(6.0);
    ui.weak("Draw the profile square to one of the edges, starting at that edge's top or bottom corner (sketch on the face at the end of the edge). Its lines are measured to the corners; a bend with the body's radius goes at each.");
    out
}

/// Corner treatment: faces, kind, gap and relief.
pub fn corner_panel(
    ui: &mut Ui,
    doc: &Document,
    c: &mut CornerFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("corner_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Corners");
            ui.horizontal(|ui| {
                if picking == Some(Slot::CornerFace) {
                    ui.colored_label(PICKING, "click a flange's face…");
                } else if c.faces.is_empty() {
                    ui.label("Every corner");
                } else {
                    ui.label(format!("At {} face(s)", c.faces.len()));
                }
                if ui
                    .small_button("Add")
                    .on_hover_text(Slot::CornerFace.prompt())
                    .clicked()
                {
                    out.pick = Some(Slot::CornerFace);
                }
                if !c.faces.is_empty() && ui.small_button("All").clicked() {
                    c.faces.clear();
                }
            });
            ui.end_row();
            ui.label("Type");
            egui::ComboBox::from_id_salt("corner_kind")
                .selected_text(c.kind.label())
                .show_ui(ui, |ui| {
                    for k in CornerKind::ALL {
                        let tip = match k {
                            CornerKind::Butt => {
                                "The later flange butts against the earlier one, which runs to the corner (a closed corner)."
                            }
                            CornerKind::Overlap => {
                                "The earlier flange butts against the later one (a closed corner, the other way round)."
                            }
                            CornerKind::Open => {
                                "Both flanges stop short of the corner."
                            }
                        };
                        ui.selectable_value(&mut c.kind, k, k.label())
                            .on_hover_text(tip);
                    }
                });
            ui.end_row();
            value_row(
                ui,
                "Gap",
                "Left between the flanges.",
                "corner_gap",
                &mut c.gap,
                ScalarKind::Length,
                params,
                &mut out,
            );
            ui.label("Relief");
            egui::ComboBox::from_id_salt("corner_relief")
                .selected_text(c.relief.label())
                .show_ui(ui, |ui| {
                    for r in CornerRelief::ALL {
                        let tip = match r {
                            CornerRelief::Rectangular => {
                                "A rectangle cut out where the two bends meet."
                            }
                            CornerRelief::Tear => {
                                "Only what the two bends would share is removed."
                            }
                        };
                        ui.selectable_value(&mut c.relief, r, r.label())
                            .on_hover_text(tip);
                    }
                });
            ui.end_row();
            if c.relief == CornerRelief::Rectangular {
                value_row(
                    ui,
                    "Relief size",
                    "How far the relief reaches past the bends.",
                    "corner_relief_size",
                    &mut c.relief_size,
                    ScalarKind::Length,
                    params,
                    &mut out,
                );
            }
        });
    ui.add_space(6.0);
    ui.weak("Flanges on neighbouring edges meet in a butt corner with a 0.1 mm gap unless a corner feature says otherwise.");
    out
}

/// Form: kind, height and direction.
pub fn form_panel(ui: &mut Ui, doc: &Document, f: &mut FormFeature) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("form_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Sketch");
            ui.label(doc.model.name_of(f.sketch));
            ui.end_row();
            ui.label("Type");
            egui::ComboBox::from_id_salt("form_kind")
                .selected_text(f.kind.label())
                .show_ui(ui, |ui| {
                    for k in FormKind::ALL {
                        let tip = match k {
                            FormKind::Dimple => "A round plateau at each circle.",
                            FormKind::Emboss => "A plateau for each closed polygon.",
                            FormKind::Louver => {
                                "A hood for each closed polygon, cut open along one side."
                            }
                        };
                        ui.selectable_value(&mut f.kind, k, k.label())
                            .on_hover_text(tip);
                    }
                });
            ui.end_row();
            value_row(
                ui,
                "Height",
                "How far the plateau stands out of the sheet's face.",
                "form_height",
                &mut f.height,
                ScalarKind::Length,
                params,
                &mut out,
            );
            ui.label("Direction");
            ui.checkbox(&mut f.flip, "Flip").on_hover_text(
                "Press into the face the sketch is on, so the form stands out of the other side.",
            );
            ui.end_row();
            if f.kind == FormKind::Louver {
                ui.label("Open side");
                ui.horizontal(|ui| {
                    ui.label(format!("Side {}", f.open_side + 1));
                    if ui
                        .small_button("Next")
                        .on_hover_text("Open the next side round the outline.")
                        .clicked()
                    {
                        f.open_side = (f.open_side + 1) % 60;
                    }
                    if f.open_side > 0 && ui.small_button("First").clicked() {
                        f.open_side = 0;
                    }
                });
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.weak("Forms have square walls one thickness thick. The outline is the outside of the wall. On the flat pattern they are marked on the FORMS layer; a louver's open side is cut.");
    out
}

/// A choice of axis: the standard axes, reference axes, or an edge to pick.
#[allow(clippy::too_many_arguments)]
fn axis_row(
    ui: &mut Ui,
    doc: &Document,
    label: &str,
    id: &str,
    axis: &mut AxisRef,
    slot: Slot,
    picking: Option<Slot>,
    out: &mut PanelResult,
) {
    ui.label(label);
    ui.horizontal(|ui| {
        if picking == Some(slot) {
            ui.colored_label(PICKING, "click an edge…");
            return;
        }
        egui::ComboBox::from_id_salt(id)
            .selected_text(axis_text(doc, axis))
            .show_ui(ui, |ui| {
                for a in StdAxis::ALL {
                    ui.selectable_value(axis, AxisRef::Standard(a), a.label());
                }
                for f in doc.model.features() {
                    if matches!(f.kind, FeatureKind::Axis(_) | FeatureKind::CoordSystem(_)) {
                        ui.selectable_value(axis, AxisRef::Feature(f.id), &f.name);
                    }
                }
                if ui
                    .selectable_label(matches!(axis, AxisRef::Edge(_)), "Pick an edge…")
                    .on_hover_text(slot.prompt())
                    .clicked()
                {
                    out.pick = Some(slot);
                }
            });
    });
    ui.end_row();
}

/// The features a pattern or a mirror copies, with a way to change them.
fn seeds_row(ui: &mut Ui, doc: &Document, own: FeatureId, seeds: &mut Vec<FeatureId>) {
    ui.label("Features");
    ui.vertical(|ui| {
        let names: Vec<&str> = seeds.iter().map(|s| doc.model.name_of(*s)).collect();
        ui.label(if names.is_empty() {
            "None".to_owned()
        } else {
            names.join(", ")
        });
        egui::ComboBox::from_id_salt("seed_choice")
            .selected_text("Change…")
            .show_ui(ui, |ui| {
                let before = doc.model.index_of(own).unwrap_or(usize::MAX);
                for (i, f) in doc.model.features().enumerate() {
                    if i >= before || !f.kind.can_be_copied() {
                        continue;
                    }
                    let mut on = seeds.contains(&f.id);
                    if ui.checkbox(&mut on, &f.name).changed() {
                        if on {
                            seeds.push(f.id);
                        } else {
                            seeds.retain(|s| *s != f.id);
                        }
                    }
                }
            });
    });
    ui.end_row();
}

/// Pattern: what it copies, and where the copies go.
pub fn pattern_panel(
    ui: &mut Ui,
    doc: &Document,
    own: FeatureId,
    p: &mut PatternFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    let params = &doc.model.parameters;
    egui::Grid::new("pattern_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            seeds_row(ui, doc, own, &mut p.seeds);
            match &mut p.def {
                PatternDef::Linear { first, second } => {
                    axis_row(
                        ui,
                        doc,
                        "Direction",
                        "pattern_dir1",
                        &mut first.direction,
                        Slot::PatternAxis,
                        picking,
                        &mut out,
                    );
                    value_row(
                        ui,
                        "Spacing",
                        "",
                        "pattern_spacing1",
                        &mut first.spacing,
                        ScalarKind::Length,
                        params,
                        &mut out,
                    );
                    value_row(ui, "Count", "A whole number from 2 to 10000, the original included, or an expression returning one.", "pattern_count1", &mut first.count, ScalarKind::Number, params, &mut out);
                    ui.label("");
                    ui.checkbox(&mut first.flip, "Reverse");
                    ui.end_row();
                    ui.label("Second direction");
                    let mut on = second.is_some();
                    if ui.checkbox(&mut on, "A grid").changed() {
                        *second = on.then(|| LinearDirection {
                            direction: match first.direction {
                                AxisRef::Standard(StdAxis::Y) => AxisRef::Standard(StdAxis::X),
                                _ => AxisRef::Standard(StdAxis::Y),
                            },
                            spacing: first.spacing.clone(),
                            count: Scalar::new(2.0),
                            flip: false,
                        });
                    }
                    ui.end_row();
                    if let Some(s) = second {
                        axis_row(
                            ui,
                            doc,
                            "Direction 2",
                            "pattern_dir2",
                            &mut s.direction,
                            Slot::PatternAxis2,
                            picking,
                            &mut out,
                        );
                        value_row(
                            ui,
                            "Spacing 2",
                            "",
                            "pattern_spacing2",
                            &mut s.spacing,
                            ScalarKind::Length,
                            params,
                            &mut out,
                        );
                        value_row(ui, "Count 2", "A whole number from 2 to 10000, the original included, or an expression returning one.", "pattern_count2", &mut s.count, ScalarKind::Number, params, &mut out);
                        ui.label("");
                        ui.checkbox(&mut s.flip, "Reverse");
                        ui.end_row();
                    }
                }
                PatternDef::Circular {
                    axis,
                    count,
                    angle,
                    flip,
                } => {
                    axis_row(
                        ui,
                        doc,
                        "Axis",
                        "pattern_axis",
                        axis,
                        Slot::PatternAxis,
                        picking,
                        &mut out,
                    );
                    value_row(ui, "Count", "A whole number from 2 to 10000, the original included, or an expression returning one.", "pattern_count_circular", count, ScalarKind::Number, params, &mut out);
                    value_row(
                        ui,
                        "Angle",
                        "The angle the copies are spread over. 360° spaces them evenly all the way round.",
                        "pattern_angle",
                        angle,
                        ScalarKind::Angle,
                        params,
                        &mut out,
                    );
                    ui.label("");
                    ui.checkbox(flip, "Reverse");
                    ui.end_row();
                }
            }
        });
    ui.add_space(6.0);
    ui.weak("Copies extrusions, cuts, revolves, holes, sheet metal cuts and forms. Sheet metal copies stay on the face their original is sketched on.");
    out
}

/// Mirror: what it copies, and the plane.
pub fn mirror_panel(
    ui: &mut Ui,
    doc: &Document,
    own: FeatureId,
    m: &mut MirrorFeature,
    picking: Option<Slot>,
) -> PanelResult {
    let mut out = PanelResult::default();
    egui::Grid::new("mirror_props")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            seeds_row(ui, doc, own, &mut m.seeds);
            reference_row(
                ui,
                "Plane",
                plane_text(doc, &m.plane),
                Slot::MirrorPlane,
                picking,
                &mut out,
            );
        });
    ui.add_space(6.0);
    ui.weak("Copies extrusions, cuts, revolves, holes, sheet metal cuts and forms. For sheet metal, the plane must be square to the face the original is sketched on.");
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
