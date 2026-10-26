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
