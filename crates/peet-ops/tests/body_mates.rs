//! A body placed by mates (SolidWorks' Move/Copy Body with constraints) follows the part
//! when it changes.

use peet_document::Document;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
    reply.json
}

/// The pin's box: min and max.
fn pin(doc: &mut Document) -> ([f64; 3], [f64; 3]) {
    let bodies = ok(doc, json!({"op": "bodies"}));
    let b = bodies["bodies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["made_by"] == "Extrude2")
        .unwrap_or_else(|| panic!("no pin in {bodies}"))
        .clone();
    let p = |v: &Value| [0, 1, 2].map(|k| v[k].as_f64().unwrap());
    (p(&b["min"]), p(&b["max"]))
}

fn near(a: [f64; 3], b: [f64; 3]) {
    assert!(
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-6),
        "{a:?} is not {b:?}"
    );
}

/// A plate `t` thick with a hole, and a pin drawn standing on the front plane.
fn plate_and_pin(doc: &mut Document) {
    for op in [
        json!({"op": "set_parameter", "name": "t", "value": "5mm"}),
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 20]}]}),
        json!({"op": "extrude", "sketch": "Sketch1", "depth": "t"}),
        json!({"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [10, 10], "radius": 3}]}),
        json!({"op": "extrude", "sketch": "Sketch2", "operation": "cut", "end": "through_all", "reverse": true}),
        json!({"op": "sketch", "on": "front", "name": "Pin", "draw": [{"type": "circle", "center": [30, 30], "radius": 3}]}),
        json!({"op": "extrude", "sketch": "Pin", "depth": 12, "operation": "new_body"}),
    ] {
        ok(doc, op);
    }
}

#[test]
fn a_mated_body_follows_the_part() {
    let mut doc = Document::default();
    plate_and_pin(&mut doc);
    let r = ok(
        &mut doc,
        json!({"op": "move_body", "bodies": [{"feature": "Extrude2", "side": "end"}], "rx": -90,
        "mates": [
            {"type": "concentric", "moved": {"face": {"feature": "Extrude2", "side": "side"}},
             "to": {"face": {"feature": "Cut-Extrude1", "side": "side"}}},
            {"type": "distance", "distance": 2, "name": "Gap",
             "moved": {"face": {"feature": "Extrude2", "side": "start"}},
             "to": {"face": {"feature": "Extrude1", "side": "end"}}},
        ]}),
    );
    assert_eq!(r["created"][0]["status"], "ok", "{r}");
    // In the hole, its start face 2 above the plate, standing up from it
    near(pin(&mut doc).0, [7.0, 7.0, 7.0]);
    near(pin(&mut doc).1, [13.0, 13.0, 19.0]);
    // A thicker plate lifts it
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "t", "value": "8mm"}),
    );
    near(pin(&mut doc).0, [7.0, 7.0, 10.0]);
    // The mate's distance drives it, and flip puts it under the top face, in the hole
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Body-Move1", "mates": [
            {"type": "concentric", "moved": {"face": {"feature": "Extrude2", "side": "side"}},
             "to": {"face": {"feature": "Cut-Extrude1", "side": "side"}}},
            {"type": "distance", "distance": 3, "flip": true,
             "moved": {"face": {"feature": "Extrude2", "side": "start"}},
             "to": {"face": {"feature": "Extrude1", "side": "end"}}},
        ]}),
    );
    near(pin(&mut doc).0, [7.0, 7.0, 5.0]);
    let f = ok(&mut doc, json!({"op": "feature", "feature": "Body-Move1"}));
    assert_eq!(f["fields"]["mates"][1]["flip"], true, "{f}");
}

#[test]
fn a_mate_that_cannot_hold_warns() {
    let mut doc = Document::default();
    plate_and_pin(&mut doc);
    let r = ok(
        &mut doc,
        json!({"op": "move_body", "bodies": [{"feature": "Extrude2", "side": "end"}], "rx": -90,
        "mates": [
            {"type": "coincident", "name": "OnTop",
             "moved": {"face": {"feature": "Extrude2", "side": "start"}},
             "to": {"face": {"feature": "Extrude1", "side": "end"}}},
            {"type": "distance", "distance": 4, "name": "Apart",
             "moved": {"face": {"feature": "Extrude2", "side": "start"}},
             "to": {"face": {"feature": "Extrude1", "side": "end"}}},
        ]}),
    );
    assert_eq!(r["created"][0]["status"], "warning", "{r}");
    let status = r["created"][0]["message"].to_string();
    assert!(status.contains("Apart"), "{r}");
    // The first still holds
    assert!((pin(&mut doc).0[2] - 5.0).abs() < 1e-6);
}

#[test]
fn a_mate_value_has_solidworks_address_and_differs_by_configuration() {
    let mut doc = Document::default();
    plate_and_pin(&mut doc);
    ok(
        &mut doc,
        json!({"op": "move_body", "bodies": [{"feature": "Extrude2", "side": "end"}], "rx": -90,
        "mates": [
            {"type": "concentric", "moved": {"face": {"feature": "Extrude2", "side": "side"}},
             "to": {"face": {"feature": "Cut-Extrude1", "side": "side"}}},
            {"type": "distance", "distance": 2, "name": "Distance2",
             "moved": {"face": {"feature": "Extrude2", "side": "start"}},
             "to": {"face": {"feature": "Extrude1", "side": "end"}}},
        ]}),
    );
    // SolidWorks' name for a mate's value: D1@ the mate
    let dims = ok(
        &mut doc,
        json!({"op": "dimensions", "feature": "Body-Move1"}),
    );
    let mate = dims["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["address"] == "D1@Distance2")
        .unwrap_or_else(|| panic!("no D1@Distance2 in {dims}"))
        .clone();
    assert_eq!(mate["field"], "mate2", "{mate}");
    ok(
        &mut doc,
        json!({"op": "set", "name": "D1@Distance2", "value": 4}),
    );
    near(pin(&mut doc).0, [7.0, 7.0, 9.0]);
    // Another configuration with a value of its own
    ok(&mut doc, json!({"op": "add_configuration", "name": "Low"}));
    ok(
        &mut doc,
        json!({"op": "set", "name": "D1@Distance2", "value": 1, "configurations": "this"}),
    );
    near(pin(&mut doc).0, [7.0, 7.0, 6.0]);
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Default"}),
    );
    near(pin(&mut doc).0, [7.0, 7.0, 9.0]);
    // And it can be named as another feature's value is
    ok(
        &mut doc,
        json!({"op": "name_dimension", "feature": "Body-Move1", "dimension": "mate2", "name": "Lift"}),
    );
    ok(
        &mut doc,
        json!({"op": "set", "name": "Lift@Body-Move1", "value": 3}),
    );
    near(pin(&mut doc).0, [7.0, 7.0, 8.0]);
}
