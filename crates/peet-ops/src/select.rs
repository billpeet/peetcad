//! Resolving selectors: finding the plane, face, edge or vertex an operation means in the
//! part as it is now, and turning it into the persistent reference a click would make
//! (see `peet_model::naming`), so it follows the geometry through later edits.
//!
//! | Wanted | Selector (JSON) |
//! |---|---|
//! | plane | `"top"`, `"front"`, `"right"`, a reference plane's name, or a face selector |
//! | axis | `"x"`, `"y"`, `"z"`, a reference axis's name, or an edge selector |
//! | point | `"origin"`, a reference point's name, or a vertex selector |
//! | face | `{"at": [x,y,z]}` a point on it; `{"normal": [x,y,z]}` its outward normal; `{"feature": "Extrude1", "side": "end"}` what made it; `{"body": 0, "index": 5}` from a `faces` query. Fields combine: all must match |
//! | edge | `{"between": [[x,y,z],[x,y,z]]}` its ends; `{"at": [x,y,z]}` a point on it; `{"faces": [face, face]}` where two faces meet; `{"body": 0, "index": 7}` |
//! | vertex | `{"at": [x,y,z]}`; `{"body": 0, "index": 2}` |
//!
//! Coordinates are model coordinates in document units. A selector must match exactly one
//! thing: none or several is an error that lists the candidates.

use std::borrow::Cow;
use std::sync::Arc;

use peet_document::Document;
use peet_kernel::tessellate::{SolidMesh, tessellate};
use peet_kernel::{EdgeId, FaceId, Surface, VertexId};
use peet_math::{DVec3, Plane};
use peet_model::naming::{find_edge, find_face, find_vertex};
use peet_model::{
    AxisRef, Body, EdgeRef, FaceRef, FaceRole, Output, PlaneRef, PointRef, VertexRef,
};
use serde_json::Value;

use crate::args::{mm3, point3_out};
use crate::value::{
    AxisSel, EdgeQuery, EdgeSel, FaceQuery, FaceSel, PlaneSel, PointSel, Side, VertexQuery,
    VertexSel,
};

/// How close a point must be to count as on a vertex or exactly on a surface, in mm.
const ON: f64 = 1e-6;

fn bodies(doc: &Document) -> &[Arc<Body>] {
    &doc.evaluation().bodies
}

/// The triangles and edge polylines of a model body (the folded one, whatever is shown).
pub(crate) fn mesh(doc: &Document, body: usize) -> Cow<'_, SolidMesh> {
    match doc.bodies.get(body) {
        Some(view) if !view.flat => Cow::Borrowed(view.tess()),
        _ => {
            let solid = &bodies(doc)[body].solid;
            Cow::Owned(tessellate(solid, peet_document::tolerance(solid)).unwrap_or_default())
        }
    }
}

fn point_segment(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// Distance from a point to a triangle.
fn point_triangle(p: DVec3, [a, b, c]: [DVec3; 3]) -> f64 {
    let n = (b - a).cross(c - a);
    if n.length_squared() > 0.0 {
        let n = n.normalize();
        let q = p - n * (p - a).dot(n);
        let inside = [(a, b), (b, c), (c, a)]
            .iter()
            .all(|(s, e)| (*e - *s).cross(q - *s).dot(n) >= 0.0);
        if inside {
            return p.distance(q);
        }
    }
    point_segment(p, a, b)
        .min(point_segment(p, b, c))
        .min(point_segment(p, c, a))
}

fn check_body(doc: &Document, body: Option<usize>) -> Result<(), String> {
    match body {
        Some(i) if i >= bodies(doc).len() => Err(format!(
            "There is no body {i}: the part has {} bodies.",
            bodies(doc).len()
        )),
        _ => Ok(()),
    }
}

fn side_matches(role: FaceRole, side: Side) -> bool {
    match side {
        Side::Start => role == FaceRole::NearCap,
        Side::End => role == FaceRole::FarCap,
        Side::Side => matches!(role, FaceRole::Side(_)),
        Side::Top => matches!(role, FaceRole::SheetTop(_)),
        Side::Bottom => matches!(role, FaceRole::SheetBottom(_)),
        Side::Bend => matches!(role, FaceRole::BendTop(_) | FaceRole::BendBottom(_)),
        Side::Wall => matches!(role, FaceRole::Wall(..)),
    }
}

/// What a face is, for messages and queries: "the end face of Extrude1".
pub(crate) fn describe_face(doc: &Document, body: usize, face: FaceId) -> String {
    doc.model.describe_face(bodies(doc)[body].face_name(face))
}

fn exactly_one<T: Copy>(
    what: &str,
    selector: &Value,
    found: Vec<(usize, T)>,
    describe: impl Fn(usize, &T) -> String,
) -> Result<(usize, T), String> {
    match found.len() {
        1 => Ok(found[0]),
        0 => Err(format!(
            "No {what} matches {selector}. The 'faces' and 'edges' operations list what a body has."
        )),
        n => {
            let mut lines: Vec<String> =
                found.iter().take(8).map(|(b, t)| describe(*b, t)).collect();
            if n > 8 {
                lines.push(format!("and {} more", n - 8));
            }
            Err(format!(
                "{n} {what}s match {selector}: add a field to pick one. They are: {}.",
                lines.join("; ")
            ))
        }
    }
}

fn direction(d: [f64; 3]) -> Result<DVec3, String> {
    DVec3::from_array(d)
        .try_normalize()
        .ok_or_else(|| "a direction can't be [0, 0, 0]".to_owned())
}

fn find_face_query(doc: &Document, q: &FaceQuery) -> Result<(usize, FaceId), String> {
    let units = &doc.model.parameters.units;
    check_body(doc, q.body)?;
    let at = q.at.map(|p| mm3(p, units));
    let normal = q.normal.map(direction).transpose()?;
    let feature = q.feature.as_ref().map(|f| f.resolve(doc)).transpose()?;
    if at.is_none()
        && normal.is_none()
        && feature.is_none()
        && q.side.is_none()
        && q.index.is_none()
    {
        return Err(
            "A face selector needs more than a body: add at, normal, feature, side or index."
                .to_owned(),
        );
    }

    let mut found = Vec::new();
    for (bi, body) in bodies(doc).iter().enumerate() {
        if q.body.is_some_and(|b| b != bi) {
            continue;
        }
        let mut tess = None;
        for f in body.solid.face_ids() {
            if q.index.is_some_and(|i| i != f.0) {
                continue;
            }
            if feature.is_some() || q.side.is_some() {
                let named = body.face_name(f).origins().iter().any(|o| {
                    feature.is_none_or(|id| o.feature == id)
                        && q.side.is_none_or(|s| side_matches(o.role, s))
                });
                if !named {
                    continue;
                }
            }
            if let Some(n) = normal {
                // A curved face has no one normal: it matches only at a given point.
                if at.is_none() && !matches!(body.solid.face(f).surface, Surface::Plane(_)) {
                    continue;
                }
                let p = at.unwrap_or_else(|| body.face_center(f));
                if body.solid.face_normal_at(f, p).dot(n) < 0.999 {
                    continue;
                }
            }
            if let Some(p) = at {
                if body.solid.face(f).surface.signed_distance(p).abs() > ON {
                    continue;
                }
                let tess = tess.get_or_insert_with(|| mesh(doc, bi));
                let reach = peet_document::tolerance(&body.solid) + ON;
                let on_face = tess.faces.iter().filter(|m| m.face == f).any(|m| {
                    m.triangles
                        .iter()
                        .any(|t| point_triangle(p, t.map(|i| m.positions[i as usize])) <= reach)
                });
                if !on_face {
                    continue;
                }
            }
            found.push((bi, f));
        }
    }
    exactly_one("face", &q.json(), found, |b, f| {
        format!(
            "body {b} face {} ({}, centre {})",
            f.0,
            describe_face(doc, b, *f),
            point3_out(bodies(doc)[b].face_center(*f), units)
        )
    })
}

/// The face a selector means, in the part as it is now: `(body, face)`.
pub(crate) fn face(doc: &Document, sel: &FaceSel) -> Result<(usize, FaceId), String> {
    match sel {
        FaceSel::Find(q) => find_face_query(doc, q),
        FaceSel::Ref(r) => find_face(bodies(doc), r)
            .map(|f| (f.body, f.id))
            .ok_or_else(|| format!("{} no longer exists.", doc.model.describe_face(&r.name))),
    }
}

pub(crate) fn face_ref(doc: &Document, sel: &FaceSel) -> Result<FaceRef, String> {
    match sel {
        FaceSel::Ref(r) => Ok(r.clone()),
        FaceSel::Find(_) => {
            let (b, f) = face(doc, sel)?;
            Ok(bodies(doc)[b].face_ref(f))
        }
    }
}

fn find_edge_query(doc: &Document, q: &EdgeQuery) -> Result<(usize, EdgeId), String> {
    let units = &doc.model.parameters.units;
    check_body(doc, q.body)?;
    let between = q.between.map(|[p, r]| (mm3(p, units), mm3(r, units)));
    let at = q.at.map(|p| mm3(p, units));
    let faces = match &q.faces {
        None => None,
        Some(f) => Some((face(doc, &f[0])?, face(doc, &f[1])?)),
    };
    if between.is_none() && at.is_none() && faces.is_none() && q.index.is_none() {
        return Err(
            "An edge selector needs more than a body: add between, at, faces or index.".to_owned(),
        );
    }

    let mut found = Vec::new();
    for (bi, body) in bodies(doc).iter().enumerate() {
        if q.body.is_some_and(|b| b != bi) {
            continue;
        }
        let mut tess = None;
        for e in body.solid.edge_ids() {
            if q.index.is_some_and(|i| i != e.0) {
                continue;
            }
            let edge = body.solid.edge(e);
            let (s, t) = (
                body.solid.vertex(edge.start).point,
                body.solid.vertex(edge.end).point,
            );
            if let Some((p, r)) = between {
                let same = |a: DVec3, b: DVec3| a.distance(b) < ON;
                if !((same(s, p) && same(t, r)) || (same(s, r) && same(t, p))) {
                    continue;
                }
            }
            if let Some(((ba, fa), (bb, fb))) = faces {
                let meets = ba == bi
                    && bb == bi
                    && body
                        .edge_faces(e)
                        .is_some_and(|[x, y]| (x == fa && y == fb) || (x == fb && y == fa));
                if !meets {
                    continue;
                }
            }
            if let Some(p) = at {
                let tess = tess.get_or_insert_with(|| mesh(doc, bi));
                let reach = peet_document::tolerance(&body.solid) + ON;
                let on_edge = tess.edges.iter().filter(|m| m.edge == e).any(|m| {
                    m.points
                        .windows(2)
                        .any(|w| point_segment(p, w[0], w[1]) <= reach)
                });
                if !on_edge {
                    continue;
                }
            }
            found.push((bi, e));
        }
    }
    exactly_one("edge", &q.json(), found, |b, e| {
        let edge = bodies(doc)[b].solid.edge(*e);
        format!(
            "body {b} edge {} (from {} to {})",
            e.0,
            point3_out(bodies(doc)[b].solid.vertex(edge.start).point, units),
            point3_out(bodies(doc)[b].solid.vertex(edge.end).point, units)
        )
    })
}

/// The edge a selector means, in the part as it is now: `(body, edge)`.
pub(crate) fn edge(doc: &Document, sel: &EdgeSel) -> Result<(usize, EdgeId), String> {
    match sel {
        EdgeSel::Find(q) => find_edge_query(doc, q),
        EdgeSel::Ref(r) => find_edge(bodies(doc), r)
            .map(|f| (f.body, f.id))
            .ok_or_else(|| "That edge no longer exists.".to_owned()),
    }
}

pub(crate) fn edge_ref(doc: &Document, sel: &EdgeSel) -> Result<EdgeRef, String> {
    match sel {
        EdgeSel::Ref(r) => Ok(r.clone()),
        EdgeSel::Find(_) => {
            let (b, e) = edge(doc, sel)?;
            bodies(doc)[b].edge_ref(e).ok_or_else(|| {
                "That edge can't be referred to: the body is not closed there.".to_owned()
            })
        }
    }
}

fn find_vertex_query(doc: &Document, q: &VertexQuery) -> Result<(usize, VertexId), String> {
    let units = &doc.model.parameters.units;
    check_body(doc, q.body)?;
    let at = q.at.map(|p| mm3(p, units));
    if at.is_none() && q.index.is_none() {
        return Err("A vertex selector needs more than a body: add at or index.".to_owned());
    }
    let mut found = Vec::new();
    for (bi, body) in bodies(doc).iter().enumerate() {
        if q.body.is_some_and(|b| b != bi) {
            continue;
        }
        for id in body.solid.vertex_ids() {
            if q.index.is_some_and(|i| i != id.0) {
                continue;
            }
            if at.is_some_and(|p| body.solid.vertex(id).point.distance(p) > ON) {
                continue;
            }
            found.push((bi, id));
        }
    }
    exactly_one("vertex", &q.json(), found, |b, id| {
        format!(
            "body {b} vertex {} at {}",
            id.0,
            point3_out(bodies(doc)[b].solid.vertex(*id).point, units)
        )
    })
}

pub(crate) fn vertex_ref(doc: &Document, sel: &VertexSel) -> Result<VertexRef, String> {
    match sel {
        VertexSel::Ref(r) => {
            find_vertex(bodies(doc), r)
                .ok_or_else(|| "That vertex no longer exists.".to_owned())?;
            Ok(r.clone())
        }
        VertexSel::Find(q) => {
            let (b, id) = find_vertex_query(doc, q)?;
            Ok(bodies(doc)[b].vertex_ref(id))
        }
    }
}

/// Something flat: a standard plane, a reference plane or a planar face. Also gives where
/// it is now (a sketch on it starts there).
pub(crate) fn plane(doc: &Document, sel: &PlaneSel) -> Result<(PlaneRef, Plane), String> {
    match sel {
        PlaneSel::Standard(p) => Ok((PlaneRef::Standard(*p), p.plane())),
        PlaneSel::Face(f) => {
            let (b, id) = face(doc, f)?;
            let body = &bodies(doc)[b];
            let at = peet_model::face_sketch_plane(&body.solid, id).ok_or_else(|| {
                format!(
                    "{} is curved: a plane is needed here, so pick a flat face.",
                    describe_face(doc, b, id)
                )
            })?;
            Ok((PlaneRef::Face(face_ref(doc, f)?), at))
        }
        PlaneSel::Feature(f) => {
            let id = f.resolve(doc)?;
            match doc.evaluation().output(id) {
                Output::Plane(p) => Ok((PlaneRef::Feature(id), p)),
                Output::Frame(frame) => Ok((PlaneRef::Feature(id), Plane { frame })),
                _ => Err(format!(
                    "{} is not a plane. Use \"top\", \"front\", \"right\", a reference plane or a face selector.",
                    doc.model.name_of(id)
                )),
            }
        }
    }
}

/// A direction: a standard axis, a reference axis or an edge.
pub(crate) fn axis(doc: &Document, sel: &AxisSel) -> Result<AxisRef, String> {
    match sel {
        AxisSel::Standard(a) => Ok(AxisRef::Standard(*a)),
        AxisSel::Edge(e) => Ok(AxisRef::Edge(edge_ref(doc, e)?)),
        AxisSel::Feature(f) => {
            let id = f.resolve(doc)?;
            match doc.evaluation().output(id) {
                Output::Axis(_) | Output::Frame(_) => Ok(AxisRef::Feature(id)),
                _ => Err(format!(
                    "{} is not an axis. Use \"x\", \"y\", \"z\", a reference axis or an edge selector.",
                    doc.model.name_of(id)
                )),
            }
        }
    }
}

/// A position: the origin, a reference point or a vertex.
pub(crate) fn point(doc: &Document, sel: &PointSel) -> Result<PointRef, String> {
    match sel {
        PointSel::Origin => Ok(PointRef::Origin),
        PointSel::Vertex(v) => Ok(PointRef::Vertex(vertex_ref(doc, v)?)),
        PointSel::Feature(f) => {
            let id = f.resolve(doc)?;
            match doc.evaluation().output(id) {
                Output::Point(_) | Output::Frame(_) => Ok(PointRef::Feature(id)),
                _ => Err(format!(
                    "{} is not a point. Use \"origin\", a reference point or a vertex selector.",
                    doc.model.name_of(id)
                )),
            }
        }
    }
}
