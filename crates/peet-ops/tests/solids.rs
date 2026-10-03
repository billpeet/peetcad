//! Scripts for general solid modelling: revolves, sweeps, fillets and chamfers, shells,
//! draft, holes, imported bodies, mass properties and measurements.

use std::f64::consts::PI;

use peet_document::Document;
use peet_kernel::validate::measure;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
    reply.json
}

fn error(doc: &mut Document, op: Value) -> String {
    let before = doc.model.clone();
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(!reply.ok, "{op} should have failed: {}", reply.json);
    assert_eq!(doc.model, before, "a failed operation changes nothing");
    reply.json["error"].as_str().unwrap().to_owned()
}

fn volume(doc: &Document) -> f64 {
    doc.evaluation()
        .bodies
        .iter()
        .map(|b| measure::volume(&b.solid))
        .sum()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6 * b.abs().max(1.0), "{a} is not {b}");
}

fn built(reply: &Value) {
    assert_eq!(reply["created"][0]["status"], "ok", "{reply}");
    assert!(reply.get("failures").is_none(), "{reply}");
}

/// A 60 × 40 × 20 block.
fn block(doc: &mut Document) {
    ok(
        doc,
        json!({"op": "sketch", "on": "top", "draw": [
            {"type": "rectangle", "from": [0, 0], "to": [60, 40]},
        ]}),
    );
    ok(
        doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 20}),
    );
}

const FULL: f64 = 60.0 * 40.0 * 20.0;

#[test]
fn fillets_and_chamfers() {
    let mut doc = Document::default();
    block(&mut doc);
    let fillet = ok(
        &mut doc,
        json!({"op": "fillet", "size": 5, "edges": [{"between": [[0, 0, 0], [0, 0, 20]]}]}),
    );
    built(&fillet);
    assert_eq!(fillet["created"][0]["type"], "Fillet");
    let rounded = FULL - (25.0 - PI * 25.0 / 4.0) * 20.0;
    close(volume(&doc), rounded);
    let chamfer = ok(
        &mut doc,
        json!({"op": "chamfer", "size": 3, "edges": [{"between": [[60, 40, 0], [60, 40, 20]]}]}),
    );
    built(&chamfer);
    assert_eq!(chamfer["created"][0]["name"], "Chamfer1");
    close(volume(&doc), rounded - 4.5 * 20.0);

    // The fillet's own face is found by what made it, and measured exactly.
    let measured = ok(
        &mut doc,
        json!({"op": "measure", "a": {"face": {"feature": "Fillet1", "side": "blend"}}}),
    );
    assert_eq!(measured["radius"], 5.0);
    let faces = ok(&mut doc, json!({"op": "faces"}));
    let round = faces["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["surface"] == "cylinder")
        .expect("the fillet");
    assert_eq!(round["radius"], 5.0);

    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Fillet1", "size": 8}),
    );
    close(
        volume(&doc),
        FULL - (64.0 - PI * 64.0 / 4.0) * 20.0 - 4.5 * 20.0,
    );
    let read = ok(&mut doc, json!({"op": "feature", "feature": "Fillet1"}));
    assert_eq!(read["fields"]["kind"], "fillet");
    assert_eq!(read["fields"]["chain"], true);
    let e = error(&mut doc, json!({"op": "fillet", "size": 2}));
    assert!(e.contains("Missing: edges"), "{e}");
}

#[test]
fn standard_holes() {
    let mut doc = Document::default();
    block(&mut doc);
    ok(
        &mut doc,
        json!({"op": "sketch", "on": {"normal": [0, 0, 1]}, "name": "Centres", "draw": [
            {"type": "point", "at": [15, 20]},
            {"type": "point", "at": [45, 20]},
        ]}),
    );
    let hole = ok(
        &mut doc,
        json!({"op": "hole", "sketch": "Centres", "standard": {"size": "M8", "fit": "close"}}),
    );
    built(&hole);
    let read = ok(&mut doc, json!({"op": "feature", "feature": "Hole1"}));
    assert_eq!(read["fields"]["diameter"], 8.4);
    assert_eq!(
        read["fields"]["standard"],
        json!({"size": "M8", "fit": "close"})
    );
    assert_eq!(read["fields"]["end"], "through_all");
    close(volume(&doc), FULL - 2.0 * PI * 4.2 * 4.2 * 20.0);

    // A size given with the standard wins over it.
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Hole1", "standard": {"size": "M6"}, "diameter": 7}),
    );
    close(volume(&doc), FULL - 2.0 * PI * 3.5 * 3.5 * 20.0);
    let e = error(
        &mut doc,
        json!({"op": "edit", "feature": "Hole1", "standard": {"size": "M7"}}),
    );
    assert!(e.contains("M6") && e.contains("M8"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "edit", "feature": "Hole1", "kind": "threaded"}),
    );
    assert!(e.contains("simple, counterbore, countersink"), "{e}");
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Hole1", "end": "blind", "depth": 5, "tip_angle": 180}),
    );
    close(volume(&doc), FULL - 2.0 * PI * 3.5 * 3.5 * 5.0);
}

#[test]
fn shells_and_drafts() {
    let mut doc = Document::default();
    block(&mut doc);
    let draft = ok(
        &mut doc,
        json!({"op": "draft", "faces": [{"normal": [1, 0, 0]}], "neutral": "top", "angle": 5}),
    );
    built(&draft);
    assert!(volume(&doc) < FULL);
    let e = error(&mut doc, json!({"op": "draft", "angle": 5}));
    assert!(e.contains("Missing: faces, neutral"), "{e}");
    ok(&mut doc, json!({"op": "undo"}));

    let shell = ok(
        &mut doc,
        json!({"op": "shell", "open": [{"normal": [0, 0, 1]}], "thickness": 2}),
    );
    built(&shell);
    close(volume(&doc), FULL - 56.0 * 36.0 * 18.0);
    // The inside is a face of the shell.
    let floor = ok(
        &mut doc,
        json!({"op": "measure", "a": {"face": {"feature": "Shell1", "side": "inner", "normal": [0, 0, 1]}}}),
    );
    close(floor["area_mm2"].as_f64().unwrap(), 56.0 * 36.0);
}

#[test]
fn revolves_and_mass() {
    let mut doc = Document::default();
    // A ring: a rectangle off the axis, turned about the sketch's vertical axis.
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "front", "draw": [
            {"type": "rectangle", "from": [10, 0], "to": [20, 30]},
        ]}),
    );
    built(&ok(&mut doc, json!({"op": "revolve", "sketch": "Sketch1"})));
    close(volume(&doc), PI * 300.0 * 30.0);
    let read = ok(&mut doc, json!({"op": "feature", "feature": "Revolve1"}));
    assert_eq!(read["fields"]["axis"], "sketch_y");
    assert_eq!(
        read["fields"]["operation"], "new_body",
        "the first body of a part"
    );
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Revolve1", "angle": 90}),
    );
    close(volume(&doc), PI * 300.0 * 30.0 / 4.0);

    let mass = ok(&mut doc, json!({"op": "mass"}));
    assert_eq!(mass["bodies"].as_array().unwrap().len(), 1);
    close(
        mass["total"]["volume_mm3"].as_f64().unwrap(),
        PI * 300.0 * 30.0 / 4.0,
    );
    assert_eq!(
        mass["total"]["center_of_gravity"].as_array().unwrap().len(),
        3
    );

    // A centreline in the sketch is the axis without being asked for.
    let mut doc = Document::default();
    let sketch = ok(
        &mut doc,
        json!({"op": "sketch", "on": "front", "draw": [
            {"type": "rectangle", "from": [0, 5], "to": [40, 12]},
            {"type": "line", "from": [0, 0], "to": [40, 0], "construction": true},
        ]}),
    );
    let line = sketch["drawn"][1]["id"].clone();
    built(&ok(&mut doc, json!({"op": "revolve", "sketch": "Sketch1"})));
    let read = ok(&mut doc, json!({"op": "feature", "feature": "Revolve1"}));
    assert_eq!(read["fields"]["axis"], json!({"line": line}));
    let tube = PI * (144.0 - 25.0) * 40.0;
    close(volume(&doc), tube);
    // The same axis, as the model's X axis.
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Revolve1", "axis": "x"}),
    );
    close(volume(&doc), tube);
    let read = ok(&mut doc, json!({"op": "feature", "feature": "Revolve1"}));
    assert_eq!(read["fields"]["axis"], "x");
}

#[test]
fn sweeps_and_measurements() {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "name": "Profile", "draw": [
            {"type": "circle", "center": [0, 0], "radius": 3},
        ]}),
    );
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "front", "name": "Path", "draw": [
            {"type": "line", "from": [0, 0], "to": [0, 50]},
        ]}),
    );
    let e = error(&mut doc, json!({"op": "sweep", "profile": "Profile"}));
    assert!(e.contains("Missing: path"), "{e}");
    let sweep = ok(
        &mut doc,
        json!({"op": "sweep", "profile": "Profile", "path": "Path"}),
    );
    built(&sweep);
    close(volume(&doc), PI * 9.0 * 50.0);
    // Both sketches are hidden, as in the application.
    let tree = ok(&mut doc, json!({"op": "features"}));
    assert_eq!(tree["features"][0]["hidden"], true);
    assert_eq!(tree["features"][1]["hidden"], true);

    let ends = ok(
        &mut doc,
        json!({"op": "measure", "a": {"face": {"normal": [0, 0, 1]}}, "b": {"face": {"normal": [0, 0, -1]}}}),
    );
    assert_eq!(ends["distance"], 50.0);
    assert_eq!(ends["angle"], 0.0);
    let end = ok(
        &mut doc,
        json!({"op": "measure", "a": {"face": {"normal": [0, 0, 1]}}}),
    );
    close(end["area_mm2"].as_f64().unwrap(), PI * 9.0);
    let e = error(
        &mut doc,
        json!({"op": "measure", "a": {"normal": [0, 0, 1]}}),
    );
    assert!(
        e.contains("face") && e.contains("edge") && e.contains("vertex"),
        "{e}"
    );
}

#[test]
fn a_step_file_is_imported_as_a_body() {
    let dir = std::env::temp_dir().join(format!("peet-ops-step-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("block.step").to_string_lossy().into_owned();
    let mut doc = Document::default();
    block(&mut doc);
    ok(&mut doc, json!({"op": "export", "path": path}));

    let mut other = Document::default();
    let imported = ok(&mut other, json!({"op": "import_step", "path": path}));
    assert_eq!(imported["created"][0]["name"], "block");
    assert_eq!(imported["created"][0]["type"], "Imported body");
    assert_eq!(imported["bodies"], 1);
    close(volume(&other), FULL);
    // It can be built on like any body, and has no fields of its own.
    built(&ok(
        &mut other,
        json!({"op": "fillet", "size": 2, "edges": [{"between": [[0, 0, 0], [0, 0, 20]]}]}),
    ));
    let e = error(
        &mut other,
        json!({"op": "edit", "feature": "block", "size": 3}),
    );
    assert!(e.contains("no fields to edit"), "{e}");
    let missing = dir.join("none.step").to_string_lossy().into_owned();
    let e = error(&mut other, json!({"op": "import_step", "path": missing}));
    assert!(e.contains("Couldn't read"), "{e}");
    assert_eq!(ok(&mut other, json!({"op": "undo"}))["undo"], "Add Fillet1");
    assert_eq!(ok(&mut other, json!({"op": "undo"}))["undo"], "Import STEP");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_housing_sample_reads_back() {
    let (model, _) = peet_model::samples::housing();
    let mut doc = Document::from_model(model, None);
    let ids: Vec<u32> = doc.model.features().map(|f| f.id.0).collect();
    for id in ids {
        let read = ok(&mut doc, json!({"op": "feature", "feature": id}));
        assert!(read["fields"].is_object(), "{read}");
        assert_eq!(read["status"], "ok", "{read}");
    }
    ok(&mut doc, json!({"op": "faces"}));
    ok(&mut doc, json!({"op": "edges"}));
    let mass = ok(&mut doc, json!({"op": "mass"}));
    assert!(mass["total"]["volume_mm3"].as_f64().unwrap() > 0.0);
}
