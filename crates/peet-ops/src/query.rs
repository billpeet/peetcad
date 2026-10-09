//! Queries: what a script can read back from the part. Lengths are in document units.

use peet_document::Document;
use peet_kernel::validate::measure;
use peet_kernel::{Curve3, Surface};
use peet_model::{DependencyGraph, FeatureId, FeatureKind, Output, SketchStatus, Status};
use peet_sketch::solver::Solver;
use peet_sketch::{Geometry, Sketch};
use serde_json::{Map, Value, json};

use crate::args::{direction_out, length_out, point2_out, point3_out, round};
use crate::fields::FeatureArgs;
use crate::select;
use crate::value::GeomSel;
use peet_document::GeomRef;

fn status_word(status: Option<&Status>) -> &'static str {
    match status {
        Some(Status::Ok) => "ok",
        Some(Status::Warning(_)) => "warning",
        Some(Status::Failed(_)) => "failed",
        Some(Status::Suppressed | Status::SuppressedBy(_)) => "suppressed",
        Some(Status::RolledBack) => "rolled_back",
        None => "not_built",
    }
}

/// A feature in one line: id, name, type and how its last rebuild went.
pub fn brief(doc: &Document, id: FeatureId) -> Value {
    let Some(f) = doc.feature(id) else {
        return json!({ "id": id.0, "status": "deleted" });
    };
    let status = doc.status(id);
    let mut out = Map::new();
    out.insert("id".to_owned(), json!(id.0));
    out.insert("name".to_owned(), json!(f.name));
    out.insert("type".to_owned(), json!(f.kind.type_name()));
    out.insert("status".to_owned(), json!(status_word(status)));
    if let Some(m) = status.and_then(Status::message) {
        out.insert("message".to_owned(), json!(m));
    }
    if let Some(Status::SuppressedBy(parent)) = status {
        out.insert(
            "suppressed_by".to_owned(),
            json!(doc.model.name_of(*parent)),
        );
    }
    if let Some(source) = &f.suppression_expression {
        out.insert("suppression_expression".to_owned(), json!(source));
    }
    if doc.model.suppression_differs(id) {
        out.insert("suppressed_in".to_owned(), json!(suppressed_in(doc, id)));
    }
    Value::Object(out)
}

/// The names of the configurations a feature is suppressed in.
fn suppressed_in(doc: &Document, id: FeatureId) -> Vec<&str> {
    doc.model
        .configurations()
        .iter()
        .filter(|c| doc.model.suppressed_in(id, c.id) == Some(true))
        .map(|c| c.name.as_str())
        .collect()
}

/// The configurations, which one is active, and what differs between them: for each
/// feature whose suppression differs, the configurations it is suppressed in; for each
/// parameter whose expression differs, its expression in every configuration; for each
/// feature value and sketch dimension that differs, its value in every configuration (in
/// document units, or its expression).
pub fn configurations(doc: &Document) -> Value {
    let model = &doc.model;
    let active = model.active_configuration().id;
    let list: Vec<Value> = model
        .configurations()
        .iter()
        .map(|c| {
            let mut m = Map::new();
            m.insert("name".to_owned(), json!(c.name));
            if c.id == active {
                m.insert("active".to_owned(), json!(true));
            }
            if !c.comment.is_empty() {
                m.insert("comment".to_owned(), json!(c.comment));
            }
            Value::Object(m)
        })
        .collect();
    let suppressed: Map<String, Value> = model
        .features_that_differ()
        .into_iter()
        .map(|id| (model.name_of(id).to_owned(), json!(suppressed_in(doc, id))))
        .collect();
    let parameters: Map<String, Value> = model
        .parameters_that_differ()
        .into_iter()
        .map(|name| (name.to_owned(), Value::Object(expressions(doc, name))))
        .collect();
    // Feature values and sketch dimensions, as they would be typed.
    let mut values: Map<String, Value> = Map::new();
    for (id, slot) in model.values_that_differ() {
        let Some((kind, now)) = model.value(id, &slot) else {
            continue;
        };
        let name = model
            .values(id)
            .into_iter()
            .find(|v| v.slot == slot)
            .map_or_else(String::new, |v| v.name);
        let each: Map<String, Value> = model
            .configurations()
            .iter()
            .map(|c| {
                let v = model
                    .value_in(id, &slot, c.id)
                    .unwrap_or_else(|| now.clone());
                (c.name.clone(), json!(v.input_text(kind, &model.parameters)))
            })
            .collect();
        let of = values
            .entry(model.name_of(id).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        of[name] = Value::Object(each);
    }
    json!({
        "active": model.active_configuration().name,
        "configurations": list,
        "suppressed_in": suppressed,
        "parameters": parameters,
        "values": values,
    })
}

/// A parameter's expression in every configuration.
fn expressions(doc: &Document, name: &str) -> Map<String, Value> {
    doc.model
        .configurations()
        .iter()
        .filter_map(|c| {
            let e = doc.model.parameter_in(name, c.id)?;
            Some((c.name.clone(), json!(e)))
        })
        .collect()
}

/// Every feature that failed to build, in tree order.
pub fn failures(doc: &Document) -> Vec<Value> {
    if doc.is_assembly() {
        return crate::assembly::problems(doc);
    }
    doc.model
        .features()
        .filter(|f| doc.status(f.id).is_some_and(Status::is_failed))
        .map(|f| brief(doc, f.id))
        .collect()
}

pub fn features(doc: &Document) -> Value {
    let graph = DependencyGraph::new(&doc.model);
    let list: Vec<Value> = doc
        .model
        .features()
        .map(|f| {
            let mut v = brief(doc, f.id);
            if let Value::Object(m) = &mut v {
                if f.effective_suppression(&doc.model.parameters) == Ok(true) {
                    m.insert("suppressed".to_owned(), json!(true));
                }
                if !f.visible {
                    m.insert("hidden".to_owned(), json!(true));
                }
                let uses: Vec<&str> = graph
                    .dependencies(f.id)
                    .iter()
                    .map(|d| doc.model.name_of(*d))
                    .collect();
                if !uses.is_empty() {
                    m.insert("uses".to_owned(), json!(uses));
                }
            }
            v
        })
        .collect();
    let mut out = Map::new();
    out.insert("features".to_owned(), json!(list));
    if doc.model.is_rolled_back() {
        out.insert(
            "rolled_back_to".to_owned(),
            json!(doc.model.rollback_index()),
        );
    }
    Value::Object(out)
}

fn plane_out(plane: &peet_math::Plane, doc: &Document) -> Value {
    let units = &doc.model.parameters.units;
    json!({
        "origin": point3_out(plane.origin(), units),
        "x": direction_out(plane.frame.x_axis()),
        "y": direction_out(plane.frame.y_axis()),
        "normal": direction_out(plane.normal()),
    })
}

/// A sketch's plane and how well defined it is.
pub fn sketch_summary(doc: &Document, id: FeatureId) -> Value {
    let mut out = Map::new();
    if let Some((plane, status)) = doc.sketch_placement(id) {
        out.insert("plane".to_owned(), plane_out(&plane, doc));
        out.insert(
            "definition".to_owned(),
            json!(match status {
                SketchStatus::Under => "under_defined",
                SketchStatus::Fully => "fully_defined",
                SketchStatus::Over => "over_defined",
            }),
        );
    }
    if let Some(s) = doc.model.sketch(id) {
        out.insert(
            "degrees_of_freedom".to_owned(),
            json!(Solver::new().analyze(&s.sketch).dof),
        );
    }
    Value::Object(out)
}

fn sketch_detail(doc: &Document, sketch: &Sketch) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let units = &doc.model.parameters.units;
    let at = |id| point2_out(sketch.point(id), units);
    // What can still move: an entity, or a point of it, that nothing holds yet.
    let analysis = Solver::new().analyze(sketch);
    let free = |id: peet_sketch::EntityId| {
        let moves = |e| analysis.entity_status(e) == peet_sketch::solver::DofStatus::Under;
        moves(id)
            || sketch
                .entity(id)
                .is_some_and(|e| e.geometry.points().into_iter().any(moves))
    };
    let mut entities = Vec::new();
    for (id, e) in sketch.entities() {
        let mut v = match e.geometry {
            Geometry::Point { .. } if e.owner.is_some() || id == Sketch::ORIGIN => continue,
            Geometry::Point { pos } => json!({ "type": "point", "at": point2_out(pos, units) }),
            Geometry::Line { start, end } => json!({
                "type": "line", "start": start.0, "end": end.0,
                "from": at(start), "to": at(end),
            }),
            Geometry::Circle { center, radius } => json!({
                "type": "circle", "center": center.0,
                "at": at(center), "radius": length_out(radius, units),
            }),
            Geometry::Arc { center, start, end } => json!({
                "type": "arc", "center": center.0, "start": start.0, "end": end.0,
                "at": at(center), "from": at(start), "to": at(end),
            }),
            Geometry::Spline { ref points, closed } => json!({
                "type": "spline", "closed": closed,
                "points": points.iter().map(|p| p.0).collect::<Vec<u32>>(),
                "through": points.iter().map(|p| at(*p)).collect::<Vec<Value>>(),
            }),
        };
        if let Value::Object(m) = &mut v {
            m.insert("id".to_owned(), json!(id.0));
            if free(id) {
                m.insert("free".to_owned(), json!(true));
            }
            if e.locked {
                m.insert("locked".to_owned(), json!(true));
            }
            if e.construction {
                m.insert("construction".to_owned(), json!(true));
            }
        }
        entities.push(v);
    }
    let mut relations = Vec::new();
    let mut dimensions = Vec::new();
    for (id, c) in sketch.constraints() {
        let of: Vec<u32> = c.kind.entities().iter().map(|e| e.0).collect();
        match &c.dimension {
            None => relations.push(json!({ "id": id.0, "type": c.kind.label(), "of": of })),
            Some(d) => {
                let value = if c.kind.is_angular() {
                    json!(round(d.value))
                } else {
                    length_out(d.value, units)
                };
                let mut m = Map::new();
                m.insert("name".to_owned(), json!(d.name));
                m.insert("type".to_owned(), json!(c.kind.label()));
                m.insert("of".to_owned(), json!(of));
                m.insert("value".to_owned(), value);
                if let Some(e) = &d.expression {
                    m.insert("expression".to_owned(), json!(e));
                }
                if !d.driving {
                    m.insert("driven".to_owned(), json!(true));
                }
                dimensions.push(Value::Object(m));
            }
        }
    }
    (entities, relations, dimensions)
}

/// Everything about one feature: its fields as `edit` takes them, and for a sketch, what
/// is drawn in it.
pub fn feature(doc: &Document, id: FeatureId) -> Value {
    let Some(f) = doc.feature(id) else {
        return json!({});
    };
    let mut out = match brief(doc, id) {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    out.insert(
        "suppressed".to_owned(),
        json!(f.effective_suppression(&doc.model.parameters).ok()),
    );
    out.insert("manual_suppressed".to_owned(), json!(f.suppressed));
    out.insert(
        "suppression_expression".to_owned(),
        json!(f.suppression_expression),
    );
    out.insert("visible".to_owned(), json!(f.visible));
    out.insert(
        "fields".to_owned(),
        Value::Object(FeatureArgs::get(&f.kind, doc)),
    );
    let units = &doc.model.parameters.units;
    match doc.evaluation().output(id) {
        Output::Plane(p) => {
            out.insert("plane".to_owned(), plane_out(&p, doc));
        }
        Output::Frame(frame) => {
            out.insert(
                "plane".to_owned(),
                plane_out(&peet_math::Plane { frame }, doc),
            );
        }
        Output::Axis(a) => {
            out.insert(
                "axis".to_owned(),
                json!({ "origin": point3_out(a.origin, units), "direction": direction_out(a.dir) }),
            );
        }
        Output::Point(p) => {
            out.insert("point".to_owned(), point3_out(p, units));
        }
        _ => {}
    }
    if let FeatureKind::Sketch(s) = &f.kind {
        if let Value::Object(summary) = sketch_summary(doc, id) {
            out.extend(summary);
        }
        let (entities, relations, dimensions) = sketch_detail(doc, &s.sketch);
        out.insert("entities".to_owned(), json!(entities));
        out.insert("relations".to_owned(), json!(relations));
        out.insert("dimensions".to_owned(), json!(dimensions));
        out.insert("projections".to_owned(), json!(s.projections));
    }
    Value::Object(out)
}

pub fn parameters(doc: &Document) -> Value {
    let params = &doc.model.parameters;
    let list: Vec<Value> = params
        .entries
        .iter()
        .map(|p| {
            let mut v = json!({
                "name": p.name,
                "expression": p.expression,
                "value": p.display(&params.units),
            });
            if doc.model.parameter_differs(&p.name) {
                v["configurations"] = Value::Object(expressions(doc, &p.name));
            }
            v
        })
        .collect();
    json!({ "units": params.units.length.suffix(), "parameters": list })
}

pub fn bodies(doc: &Document) -> Value {
    let units = &doc.model.parameters.units;
    let list: Vec<Value> = doc
        .evaluation()
        .bodies
        .iter()
        .enumerate()
        .map(|(i, b)| {
            // A freeform face's own bounds are a box round its control points, which is
            // a little too big: the body's triangles give the size it really has.
            let freeform = b
                .solid
                .faces
                .iter()
                .any(|f| matches!(f.surface, Surface::Nurbs(_)));
            let bounds = if freeform {
                peet_math::Aabb::from_points(
                    select::mesh(doc, i)
                        .faces
                        .iter()
                        .flat_map(|f| f.positions.iter().copied()),
                )
            } else {
                b.solid.bounds()
            };
            let mut m = Map::new();
            m.insert("body".to_owned(), json!(i));
            m.insert("made_by".to_owned(), json!(doc.model.name_of(b.origin)));
            m.insert("sheet_metal".to_owned(), json!(b.sheet.is_some()));
            m.insert("min".to_owned(), point3_out(bounds.min, units));
            m.insert("max".to_owned(), point3_out(bounds.max, units));
            m.insert("size".to_owned(), point3_out(bounds.size(), units));
            m.insert(
                "volume_mm3".to_owned(),
                json!(round(measure::volume(&b.solid))),
            );
            m.insert("faces".to_owned(), json!(b.solid.faces.len()));
            m.insert("edges".to_owned(), json!(b.solid.edges.len()));
            if let Some(sheet) = &b.sheet {
                let r = sheet.report();
                m.insert("flat_size".to_owned(), point2_out(r.flat_size, units));
                m.insert("thickness".to_owned(), length_out(r.thickness, units));
                m.insert("bends".to_owned(), json!(r.bends.len()));
            }
            Value::Object(m)
        })
        .collect();
    json!({ "bodies": list })
}

fn body_arg(doc: &Document, body: Option<usize>) -> Result<Vec<usize>, String> {
    let n = doc.evaluation().bodies.len();
    match body {
        Some(b) if b < n => Ok(vec![b]),
        Some(b) => Err(format!("There is no body {b}: the part has {n} bodies.")),
        None => Ok((0..n).collect()),
    }
}

pub fn faces(doc: &Document, body: Option<usize>) -> Result<Value, String> {
    let units = &doc.model.parameters.units;
    let mut list = Vec::new();
    for bi in body_arg(doc, body)? {
        let b = &doc.evaluation().bodies[bi];
        for f in b.solid.face_ids() {
            let center = b.face_center(f);
            let mut m = Map::new();
            m.insert("body".to_owned(), json!(bi));
            m.insert("index".to_owned(), json!(f.0));
            m.insert("names".to_owned(), json!(select::face_names(doc, bi, f)));
            m.insert("what".to_owned(), json!(select::describe_face(doc, bi, f)));
            // The same, as the `feature` and `side` of a selector.
            let made_by: Vec<Value> = b
                .face_name(f)
                .origins()
                .iter()
                .filter(|o| !matches!(o.role, peet_model::FaceRole::Instance(_)))
                .map(|o| {
                    json!({
                        "feature": doc.model.name_of(o.feature),
                        "side": select::side_word(o.role),
                    })
                })
                .collect();
            m.insert("made_by".to_owned(), json!(made_by));
            match &b.solid.face(f).surface {
                Surface::Plane(_) => {
                    m.insert("surface".to_owned(), json!("plane"));
                    m.insert(
                        "normal".to_owned(),
                        direction_out(b.solid.face_normal_at(f, center)),
                    );
                }
                Surface::Cylinder(c) => {
                    m.insert("surface".to_owned(), json!("cylinder"));
                    m.insert("radius".to_owned(), length_out(c.radius, units));
                    m.insert("axis".to_owned(), direction_out(c.axis()));
                }
                Surface::Cone(_) => {
                    m.insert("surface".to_owned(), json!("cone"));
                }
                Surface::Sphere(_) => {
                    m.insert("surface".to_owned(), json!("sphere"));
                }
                Surface::Torus(_) => {
                    m.insert("surface".to_owned(), json!("torus"));
                }
                Surface::Nurbs(_) => {
                    m.insert("surface".to_owned(), json!("freeform"));
                }
            }
            m.insert("center".to_owned(), point3_out(center, units));
            m.insert(
                "area_mm2".to_owned(),
                json!(round(measure::face_area(&b.solid, f))),
            );
            list.push(Value::Object(m));
        }
    }
    Ok(json!({ "faces": list }))
}

pub fn edges(doc: &Document, body: Option<usize>) -> Result<Value, String> {
    let units = &doc.model.parameters.units;
    let mut list = Vec::new();
    for bi in body_arg(doc, body)? {
        let b = &doc.evaluation().bodies[bi];
        for e in b.solid.edge_ids() {
            let edge = b.solid.edge(e);
            let mut m = Map::new();
            m.insert("body".to_owned(), json!(bi));
            m.insert("index".to_owned(), json!(e.0));
            m.insert("names".to_owned(), json!(select::edge_names(doc, bi, e)));
            m.insert(
                "curve".to_owned(),
                json!(match &edge.curve {
                    Curve3::Line(_) => "line",
                    Curve3::Circle(_) => "circle",
                    Curve3::Ellipse(_) => "ellipse",
                    Curve3::Nurbs(_) => "freeform",
                }),
            );
            m.insert(
                "from".to_owned(),
                point3_out(b.solid.vertex(edge.start).point, units),
            );
            m.insert(
                "to".to_owned(),
                point3_out(b.solid.vertex(edge.end).point, units),
            );
            m.insert(
                "middle".to_owned(),
                point3_out(edge.point_at_fraction(0.5), units),
            );
            if let Some(faces) = b.edge_faces(e) {
                m.insert("faces".to_owned(), json!(faces.map(|f| f.0)));
            }
            list.push(Value::Object(m));
        }
    }
    Ok(json!({ "edges": list }))
}

fn sheet(doc: &Document, body: Option<usize>) -> Result<&peet_model::Body, String> {
    if let Some(b) = body {
        body_arg(doc, Some(b))?;
    }
    doc.sheet_body(body)
        .map(|b| &**b)
        .ok_or_else(|| "There is no sheet metal body: start one with base_flange.".to_owned())
}

pub fn bend_table(doc: &Document, body: Option<usize>) -> Result<Value, String> {
    let units = &doc.model.parameters.units;
    let body = sheet(doc, body)?;
    let Some(sheet) = &body.sheet else {
        return Ok(json!({}));
    };
    let r = sheet.report();
    let bends: Vec<Value> = r
        .bends
        .iter()
        .map(|b| {
            json!({
                "feature": doc.model.name_of(FeatureId(b.origin.owner)),
                "direction": if b.up { "up" } else { "down" },
                "angle": round(b.angle),
                "radius": length_out(b.radius, units),
                "k_factor": round(b.k_factor),
                "allowance": length_out(b.allowance, units),
                "deduction": length_out(b.deduction, units),
                "length": length_out(b.length, units),
            })
        })
        .collect();
    Ok(json!({
        "made_by": doc.model.name_of(body.origin),
        "flat_size": point2_out(r.flat_size, units),
        "flat_area_mm2": round(r.flat_area),
        "thickness": length_out(r.thickness, units),
        "pieces": r.pieces,
        "cutouts": r.cutouts,
        "bends": bends,
    }))
}

pub fn checks(
    doc: &Document,
    body: Option<usize>,
    rules: &peet_sheetmetal::CheckRules,
) -> Result<Value, String> {
    let body = sheet(doc, body)?;
    let Some(sheet) = &body.sheet else {
        return Ok(json!({}));
    };
    let findings: Vec<Value> = peet_sheetmetal::checks::check(sheet, rules, |owner| {
        doc.model.name_of(FeatureId(owner)).to_owned()
    })
    .iter()
    .map(|f| {
        let mut m = Map::new();
        m.insert(
            "severity".to_owned(),
            json!(f.severity.label().to_lowercase()),
        );
        m.insert("check".to_owned(), json!(f.kind.label()));
        m.insert("message".to_owned(), json!(f.message));
        if let Some(v) = f.found {
            m.insert("found_mm".to_owned(), json!(round(v)));
        }
        if let Some(v) = f.required {
            m.insert("required_mm".to_owned(), json!(round(v)));
        }
        Value::Object(m)
    })
    .collect();
    Ok(json!({
        "passed": findings.is_empty(),
        "findings": findings,
        "rules": crate::library::rules_out(rules),
    }))
}

/// The state of the document in a few lines.
pub fn status(doc: &Document) -> Value {
    let mut m = Map::new();
    m.insert("name".to_owned(), json!(doc.title()));
    if let Some(path) = doc.file.as_ref().and_then(|f| f.path.as_ref()) {
        m.insert("file".to_owned(), json!(path.to_string_lossy()));
    }
    m.insert("modified".to_owned(), json!(doc.is_modified()));
    m.insert(
        "configuration".to_owned(),
        json!(doc.model.active_configuration().name),
    );
    m.insert(
        "units".to_owned(),
        json!(doc.model.parameters.units.length.suffix()),
    );
    if let Some(material) = &doc.model.material {
        m.insert(
            "material".to_owned(),
            crate::library::material_out(material),
        );
    }
    if let Some(color) = doc.model.color {
        m.insert("color".to_owned(), crate::library::color_out(color));
    }
    if let Some(assembly) = doc.model.assembly() {
        m.insert("kind".to_owned(), json!("assembly"));
        m.insert(
            "components".to_owned(),
            json!(assembly.components().count()),
        );
        m.insert("parts".to_owned(), json!(assembly.definitions().count()));
        m.insert("mates".to_owned(), json!(assembly.mates().count()));
        m.insert("freedom".to_owned(), json!(doc.evaluation().freedom));
    } else {
        m.insert("kind".to_owned(), json!("part"));
        m.insert("features".to_owned(), json!(doc.model.len()));
    }
    m.insert("bodies".to_owned(), json!(doc.bodies.len()));
    m.insert("failures".to_owned(), json!(failures(doc)));
    if let Some(l) = doc.undo_label() {
        m.insert("undo".to_owned(), json!(l));
    }
    if let Some(l) = doc.redo_label() {
        m.insert("redo".to_owned(), json!(l));
    }
    Value::Object(m)
}

fn mass_out(
    volume: f64,
    area: f64,
    centroid: peet_math::DVec3,
    moments: [f64; 3],
    bounds: &peet_math::Aabb,
    doc: &Document,
) -> Map<String, Value> {
    let units = &doc.model.parameters.units;
    let mut m = Map::new();
    m.insert("volume_mm3".to_owned(), json!(round(volume)));
    if let Some(material) = &doc.model.material {
        m.insert("mass_kg".to_owned(), json!(round(material.mass_kg(volume))));
    }
    m.insert("area_mm2".to_owned(), json!(round(area)));
    m.insert("center_of_gravity".to_owned(), point3_out(centroid, units));
    m.insert(
        "principal_moments_mm5".to_owned(),
        json!(moments.map(round)),
    );
    m.insert("min".to_owned(), point3_out(bounds.min, units));
    m.insert("max".to_owned(), point3_out(bounds.max, units));
    m
}

/// Mass properties of one body, or of each and of all together. The moments are for a
/// density of 1; the mass is given if the part has a material.
pub fn mass(doc: &Document, body: Option<usize>) -> Result<Value, String> {
    let mut parts = Vec::new();
    let mut list = Vec::new();
    for bi in body_arg(doc, body)? {
        let b = &doc.evaluation().bodies[bi];
        let p = peet_kernel::query::mass_properties(&b.solid)
            .map_err(|e| format!("Body {bi} can't be measured: {e}"))?;
        let mut m = mass_out(
            p.volume,
            p.area,
            p.centroid,
            p.principal_moments,
            &p.bounds,
            doc,
        );
        m.insert("body".to_owned(), json!(bi));
        m.insert("made_by".to_owned(), json!(doc.model.name_of(b.origin)));
        list.push(Value::Object(m));
        parts.push(p);
    }
    let mut out = Map::new();
    if let Some(material) = &doc.model.material {
        out.insert(
            "material".to_owned(),
            crate::library::material_out(material),
        );
    }
    out.insert("bodies".to_owned(), json!(list));
    if let Some(t) = peet_document::MassTotal::of(&parts) {
        out.insert(
            "total".to_owned(),
            Value::Object(mass_out(
                t.volume,
                t.area,
                t.centroid,
                t.principal_moments,
                &t.bounds,
                doc,
            )),
        );
    }
    Ok(Value::Object(out))
}

fn geom(doc: &Document, sel: &GeomSel) -> Result<GeomRef, String> {
    Ok(match sel {
        GeomSel::Face(f) => {
            let (body, face) = select::face(doc, f)?;
            GeomRef::Face { body, face }
        }
        GeomSel::Edge(e) => {
            let (body, edge) = select::edge(doc, e)?;
            GeomRef::Edge { body, edge }
        }
        GeomSel::Vertex(v) => {
            let (body, vertex) = select::vertex(doc, v)?;
            GeomRef::Vertex { body, vertex }
        }
    })
}

/// The exact measurements of a face, an edge or a vertex, or the distance and the angle
/// between two.
pub fn measure(doc: &Document, a: &GeomSel, b: Option<&GeomSel>) -> Result<Value, String> {
    use peet_kernel::query::Description;
    let units = &doc.model.parameters.units;
    let gone = || "That can't be measured in the part as it is shown.".to_owned();
    let a = geom(doc, a)?;
    let mut m = Map::new();
    let Some(b) = b else {
        match doc.describe(a).ok_or_else(gone)? {
            Description::Vertex { point } => {
                m.insert("vertex".to_owned(), point3_out(point, units));
            }
            Description::Edge {
                length,
                radius,
                center,
            } => {
                m.insert("length".to_owned(), length_out(length, units));
                if let Some(r) = radius {
                    m.insert("radius".to_owned(), length_out(r, units));
                }
                if let Some(c) = center {
                    m.insert("center".to_owned(), point3_out(c, units));
                }
            }
            Description::Face { area, radius } => {
                m.insert("area_mm2".to_owned(), json!(round(area)));
                if let Some(r) = radius {
                    m.insert("radius".to_owned(), length_out(r, units));
                }
            }
        }
        return Ok(Value::Object(m));
    };
    let between = doc.measure_between(a, geom(doc, b)?).ok_or_else(gone)?;
    if let Some((distance, from, to)) = between.distance {
        m.insert("distance".to_owned(), length_out(distance, units));
        m.insert("from".to_owned(), point3_out(from, units));
        m.insert("to".to_owned(), point3_out(to, units));
    }
    if let Some(angle) = between.angle {
        m.insert("angle".to_owned(), json!(round(angle.to_degrees())));
    }
    m.insert("measured".to_owned(), json!(between.note));
    Ok(Value::Object(m))
}
