//! SolidWorks' path mate: a point of one component kept on a path of edges or sketch curves
//! of another, anywhere on it or at a length or percentage along it.

use std::sync::Arc;

use peet_document::Document;
use peet_math::{DVec3, Frame};
use peet_model::Model;
use peet_ops::{Headless, Undo, apply_json, apply_model};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
    reply.json
}

/// A plate with a path sketched on it (a line from (10, 20) to (60, 20), then a quarter
/// arc round (60, 40) to (80, 40)), and a shoe whose origin is to run along it.
fn fixture() -> Document {
    let mut plate = Document::default();
    ok(
        &mut plate,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}),
    );
    ok(
        &mut plate,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}),
    );
    ok(
        &mut plate,
        json!({"op": "sketch", "on": "top", "name": "Track", "draw": [
            {"type": "line", "from": [10, 20], "to": [60, 20]},
            {"type": "arc", "center": [60, 40], "start": [60, 20], "end": [80, 40]}
        ]}),
    );
    let mut shoe = Document::default();
    ok(
        &mut shoe,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [-3, -2], "to": [12, 2]}]}),
    );
    ok(
        &mut shoe,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 4}),
    );
    let mut model = Model::new_assembly();
    let a = model.assembly_mut().unwrap();
    let p = a.define(Arc::new(plate.model));
    let s = a.define(Arc::new(shoe.model));
    let c = a.insert(p, Frame::WORLD).unwrap();
    a.component_mut(c).unwrap().name = "Plate".to_owned();
    let c = a
        .insert(
            s,
            Frame {
                origin: DVec3::new(30.0, 25.0, 3.0),
                ..Frame::WORLD
            },
        )
        .unwrap();
    a.component_mut(c).unwrap().name = "Shoe".to_owned();
    Document::from_model(model, None)
}

fn path_op() -> Value {
    json!({"op": "mate", "type": "path", "a": {"component": "Shoe", "origin": true},
        "b": [{"component": "Plate", "sketch": "Track", "entity": 1},
              {"component": "Plate", "sketch": "Track", "entity": 4}]})
}

fn shoe(doc: &mut Document) -> (DVec3, usize) {
    let reply = ok(doc, json!({"op": "components"}));
    let c = reply["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Shoe")
        .unwrap()
        .clone();
    let at: Vec<f64> = c["at"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    (
        DVec3::new(at[0], at[1], at[2]),
        c["freedom"].as_u64().unwrap() as usize,
    )
}

fn near(p: DVec3, x: f64, y: f64, z: f64) -> bool {
    p.distance(DVec3::new(x, y, z)) < 1e-6
}

#[test]
fn a_point_runs_on_a_path_of_sketch_curves() {
    let mut doc = fixture();
    let reply = ok(&mut doc, path_op());
    assert_eq!(reply["mate"]["status"], "ok", "{reply}");
    // Free: the nearest place on the path, and it slides along it and turns
    let (at, freedom) = shoe(&mut doc);
    assert!(near(at, 30.0, 20.0, 0.0), "{at}");
    assert_eq!(freedom, 4);
    // A length from the path's start, then from its end
    ok(
        &mut doc,
        json!({"op": "edit_mate", "mate": "PathMate1", "mode": "dimension", "distance": 20}),
    );
    assert!(near(shoe(&mut doc).0, 30.0, 20.0, 0.0));
    ok(
        &mut doc,
        json!({"op": "edit_mate", "mate": "PathMate1", "flip_dimension": true}),
    );
    // 20 back from the arc's end: 20 of its 31.4 long quarter, from (80, 40)
    let angle: f64 = 20.0 / 20.0;
    let (at, freedom) = shoe(&mut doc);
    assert!(
        near(at, 60.0 + 20.0 * angle.cos(), 40.0 - 20.0 * angle.sin(), 0.0),
        "{at}"
    );
    assert_eq!(freedom, 3);
    ok(
        &mut doc,
        json!({"op": "edit_mate", "mate": "PathMate1", "flip_dimension": false}),
    );
    // The whole way along: the arc's end, not the other point of its circle level with it
    ok(
        &mut doc,
        json!({"op": "edit_mate", "mate": "PathMate1", "mode": "percent", "percent": 100}),
    );
    assert!(near(shoe(&mut doc).0, 80.0, 40.0, 0.0));
}

#[test]
fn a_path_mate_follows_the_path_and_rolls_to_an_up_vector() {
    let mut doc = fixture();
    let mut op = path_op();
    op["follow"] = json!("x");
    op["roll"] = json!({"axis": "z", "up": {"component": "Plate", "face": {"normal": [0, 0, 1]}}});
    op["mode"] = json!("percent");
    op["percent"] = json!(80);
    let reply = ok(&mut doc, op);
    assert_eq!(reply["mate"]["status"], "ok", "{reply}");
    let (_, freedom) = shoe(&mut doc);
    assert_eq!(freedom, 0);
    // The shoe's x along the arc's tangent there
    let a = doc.model.assembly().unwrap();
    let c = a.components().find(|c| c.name == "Shoe").unwrap();
    let x = c.placement.vector_to_world(DVec3::X);
    let s = 0.8 * (50.0 + 10.0 * std::f64::consts::PI) - 50.0;
    let t = DVec3::new((s / 20.0).cos(), (s / 20.0).sin(), 0.0);
    assert!(x.cross(t).length() < 1e-6, "{x} {t}");
    assert!(c.placement.vector_to_world(DVec3::Z).z.abs() > 1.0 - 1e-9);
}

#[test]
fn path_mates_are_refused_off_one_component_and_translate_to_operations() {
    let mut doc = fixture();
    let mut bad = path_op();
    bad["b"][1] = json!({"component": "Shoe", "edge": {"at": [12, 0, 4]}});
    let reply = apply_json(&mut doc, &bad, Undo::Step);
    assert!(!reply.ok);
    assert!(
        reply.json["error"]
            .as_str()
            .unwrap()
            .contains("all on one component")
    );
    let mut replay = Document::from_model(doc.model.clone(), None);
    let mut host = Headless::default();
    for (index, op) in [
        path_op(),
        json!({"op": "edit_mate", "mate": "PathMate1", "mode": "percent", "percent": 25}),
    ]
    .into_iter()
    .enumerate()
    {
        ok(&mut doc, op);
        let result = apply_model(
            &mut host,
            &mut replay,
            doc.model.clone(),
            "Path edit",
            index as u64 + 1,
        );
        assert_eq!(result.untranslated, None, "{:?}", result.ops);
        assert_eq!(replay.model, doc.model);
    }
    let mates = ok(&mut doc, json!({"op": "mates"}));
    let m = &mates["mates"][0];
    assert_eq!(m["type"], "path");
    assert_eq!(m["b"].as_array().unwrap().len(), 2);
    assert_eq!(m["mode"], "percent");
}

#[test]
fn a_path_can_be_edges() {
    // The plate's top front edge, then its top right edge: round a corner
    let mut doc = fixture();
    let reply = ok(
        &mut doc,
        json!({"op": "mate", "type": "path", "mode": "dimension", "distance": 110,
            "a": {"component": "Shoe", "origin": true},
            "b": [{"component": "Plate", "edge": {"at": [50, 0, 5]}},
                  {"component": "Plate", "edge": {"at": [100, 30, 5]}}]}),
    );
    assert_eq!(reply["mate"]["status"], "ok", "{reply}");
    let (at, _) = shoe(&mut doc);
    // The path starts at the front edge's far end from the joint: 100 along it, then 10
    // up the right edge
    assert!(near(at, 100.0, 10.0, 5.0), "{at}");
}
