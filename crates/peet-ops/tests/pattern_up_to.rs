//! Linear patterns that run up to a reference (SolidWorks' "up to reference").
use peet_document::Document;
use peet_kernel::validate::measure;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    reply.json
}

/// A 100 x 100 x 4 plate on the top plane with a 2 mm hole at x = 15, z = 0, and a
/// reference plane `End` square to x at x = 40.
fn plate() -> Document {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"sketch", "on":"top", "draw":[{"type":"rectangle", "from":[-50,-50], "to":[50,50]}]}),
    );
    ok(
        &mut doc,
        json!({"op":"extrude", "sketch":"Sketch1", "depth":4}),
    );
    ok(
        &mut doc,
        json!({"op":"sketch", "name":"Seed", "on":"top", "draw":[{"type":"circle", "center":[15,0], "radius":1}]}),
    );
    ok(
        &mut doc,
        json!({"op":"cut", "sketch":"Seed", "end":"mid_plane", "depth":20}),
    );
    ok(
        &mut doc,
        json!({"op":"plane", "from":"right", "distance":40, "name":"End"}),
    );
    doc
}

fn holes(doc: &Document) -> f64 {
    let actual: f64 = doc.bodies.iter().map(|b| measure::volume(&b.solid)).sum();
    (40000.0 - actual) / (4.0 * std::f64::consts::PI)
}

fn assert_holes(doc: &Document, n: u32) {
    let got = holes(doc);
    assert!((got - f64::from(n)).abs() < 1e-6, "{got} holes, not {n}");
}

#[test]
fn the_count_follows_the_reference() {
    let mut doc = plate();
    assert_holes(&doc, 1);
    // From x = 15 to the plane at 40, less 2: 23 mm, room for copies at 20, 25, 30 and 35
    ok(
        &mut doc,
        json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "spacing":5,
               "up_to":{"plane":"End"}, "up_to_offset":2}),
    );
    assert_holes(&doc, 5);
    // A copy exactly at the end counts: 25 mm is five spacings
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "up_to_offset":0}),
    );
    assert_holes(&doc, 6);
    // Moving the reference moves the end of the row
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"End", "distance":31}),
    );
    assert_holes(&doc, 4);
    // Past the reference by the offset
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "up_to_offset":5, "up_to_reverse":true}),
    );
    assert_holes(&doc, 5);
    let query = ok(&mut doc, json!({"op":"feature", "feature":"LPattern1"}));
    assert_eq!(query["fields"]["up_to"]["plane"], "End");
    assert_eq!(query["fields"]["up_to_keeps"], "spacing");
    // Back to the count as given
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "up_to":null, "count":2}),
    );
    assert_holes(&doc, 2);
}

#[test]
fn the_spacing_follows_the_reference_when_the_count_is_given() {
    let mut doc = plate();
    // Three holes from x = 15 to a vertex of the plate at x = 50, less 5: spaced 15 apart
    ok(
        &mut doc,
        json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "count":3,
               "up_to":{"point":{"at":[50,50,0]}}, "up_to_offset":5, "up_to_keeps":"count"}),
    );
    assert_holes(&doc, 3);
    let faces = ok(&mut doc, json!({"op":"faces"}));
    let round: Vec<f64> = faces["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["surface"] == "cylinder")
        .filter_map(|f| f["center"][0].as_f64())
        .collect();
    for x in [15.0, 30.0, 45.0] {
        assert!(
            round.iter().any(|r| (r - x).abs() < 1e-6),
            "a hole at x = {x}: {round:?}"
        );
    }
}

#[test]
fn a_reference_behind_the_seed_says_so() {
    let mut doc = plate();
    let reply = apply_json(
        &mut doc,
        &json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "flip":true,
                "spacing":5, "up_to":{"plane":"End"}}),
        Undo::Step,
    );
    assert!(
        reply.json.to_string().contains("flip the direction"),
        "{}",
        reply.json
    );
}

#[test]
fn up_to_fields_need_up_to() {
    let mut doc = plate();
    let reply = apply_json(
        &mut doc,
        &json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "spacing":5,
                "count":2, "up_to_offset":3}),
        Undo::Step,
    );
    assert!(!reply.ok);
    assert!(
        reply.json.to_string().contains("give 'up_to'"),
        "{}",
        reply.json
    );
}
