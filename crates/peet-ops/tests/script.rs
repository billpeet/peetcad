//! Scripts against a headless document: the same operations an agent sends.

use peet_document::Document;
use peet_kernel::validate::measure;
use peet_ops::{Reply, Undo, apply_all, apply_json, parse_script};
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
    assert!((a - b).abs() < 1e-6, "{a} is not {b}");
}

/// A dimensioned plate with a hole through it.
fn plate(doc: &mut Document) {
    ok(
        doc,
        json!({"op": "set_parameter", "name": "width", "value": "80mm"}),
    );
    let sketch = ok(
        doc,
        json!({"op": "sketch", "on": "top", "draw": [
            {"type": "rectangle", "from": [0, 0], "to": [80, 50], "as": "r"},
            {"type": "coincident", "of": ["r.bottom.start", "origin"]},
            {"type": "length", "of": ["r.bottom"], "value": "width"},
            {"type": "length", "of": ["r.right"], "value": 50, "name": "height"},
        ]}),
    );
    assert_eq!(sketch["created"][0]["name"], "Sketch1");
    assert_eq!(sketch["definition"], "fully_defined");
    assert_eq!(sketch["degrees_of_freedom"], 0);
    assert_eq!(sketch["drawn"][2]["dimension"], "d1");
    assert_eq!(sketch["drawn"][3]["dimension"], "height");
    let extrude = ok(
        doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 8}),
    );
    assert_eq!(extrude["created"][0]["status"], "ok");
    ok(
        doc,
        json!({"op": "sketch", "on": {"feature": "Extrude1", "side": "end"}, "name": "Hole", "draw": [
            {"type": "circle", "center": [40, 25], "radius": 5},
        ]}),
    );
    let cut = ok(
        doc,
        json!({"op": "cut", "sketch": "Hole", "end": "through_all"}),
    );
    assert_eq!(cut["created"][0]["name"], "Cut-Extrude1");
    assert!(cut.get("failures").is_none());
}

#[test]
fn a_plate_is_built_edited_and_undone() {
    let mut doc = Document::default();
    plate(&mut doc);
    let hole = std::f64::consts::PI * 25.0 * 8.0;
    close(volume(&doc), 80.0 * 50.0 * 8.0 - hole);

    // A parameter drives the sketch; a dimension and a feature are edited by name.
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "width", "value": 100}),
    );
    close(volume(&doc), 100.0 * 50.0 * 8.0 - hole);
    ok(
        &mut doc,
        json!({"op": "set_dimension", "sketch": "Sketch1", "name": "height", "value": 60}),
    );
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": "width / 10"}),
    );
    close(
        volume(&doc),
        100.0 * 60.0 * 10.0 - std::f64::consts::PI * 25.0 * 10.0,
    );
    let read = ok(&mut doc, json!({"op": "feature", "feature": "Extrude1"}));
    assert_eq!(
        read["fields"]["depth"],
        json!({"expression": "width / 10", "value": 10.0})
    );
    assert_eq!(read["fields"]["end"], "blind");

    // Every change is an undo step with a name.
    let undone = ok(&mut doc, json!({"op": "undo"}));
    assert_eq!(undone["undo"], "Edit Extrude1");
    close(volume(&doc), 100.0 * 60.0 * 8.0 - hole);
    ok(&mut doc, json!({"op": "redo"}));

    // The hole follows the face it was sketched on, and suppressing it fills it in.
    ok(
        &mut doc,
        json!({"op": "suppress", "feature": "Cut-Extrude1"}),
    );
    close(volume(&doc), 100.0 * 60.0 * 10.0);
    let tree = ok(&mut doc, json!({"op": "features"}));
    assert_eq!(tree["features"][3]["status"], "suppressed");
    assert_eq!(tree["features"][1]["uses"], json!(["Sketch1"]));
    ok(
        &mut doc,
        json!({"op": "suppress", "feature": "Cut-Extrude1", "on": false}),
    );
    ok(
        &mut doc,
        json!({"op": "delete", "features": ["Cut-Extrude1", "Hole"]}),
    );
    assert_eq!(doc.model.len(), 2);
}

#[test]
fn selectors_find_geometry_and_say_what_is_ambiguous() {
    let mut doc = Document::default();
    plate(&mut doc);
    let faces = ok(&mut doc, json!({"op": "faces"}));
    let faces = faces["faces"].as_array().unwrap();
    assert_eq!(faces.len(), 7, "six sides and the hole");
    let top = faces
        .iter()
        .find(|f| f["what"] == "the end face of Extrude1")
        .unwrap();
    assert_eq!(top["normal"], json!([0.0, 0.0, 1.0]));

    // The same face four ways.
    for on in [
        json!({"at": [10, 10, 8]}),
        json!({"normal": [0, 0, 1]}),
        json!({"feature": "Extrude1", "side": "end"}),
        json!({"body": 0, "index": top["index"]}),
    ] {
        let made = ok(&mut doc, json!({"op": "plane", "from": on, "distance": 5}));
        let read = ok(
            &mut doc,
            json!({"op": "feature", "feature": made["created"][0]["id"]}),
        );
        assert_eq!(read["plane"]["origin"], json!([0.0, 0.0, 13.0]), "{on}");
        ok(&mut doc, json!({"op": "undo"}));
    }

    // A point on an edge is on two faces: the message lists them.
    let e = error(&mut doc, json!({"op": "plane", "from": {"at": [10, 0, 8]}}));
    assert!(e.contains("2 faces match"), "{e}");
    assert!(e.contains("the end face of Extrude1"), "{e}");
    // Adding the normal settles it.
    ok(
        &mut doc,
        json!({"op": "plane", "from": {"at": [10, 0, 8], "normal": [0, 0, 1]}}),
    );

    let e = error(
        &mut doc,
        json!({"op": "plane", "from": {"at": [500, 0, 0]}}),
    );
    assert!(e.contains("No face matches"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "sketch", "on": {"feature": "Cut-Extrude1", "side": "side"}}),
    );
    assert!(e.contains("curved"), "{e}");

    // Edges: by their ends, by a point on them, by the faces they join.
    let edge = json!({"between": [[0, 0, 8], [80, 0, 8]]});
    let axis = ok(&mut doc, json!({"op": "axis", "edge": edge}));
    let read = ok(
        &mut doc,
        json!({"op": "feature", "feature": axis["created"][0]["id"]}),
    );
    assert_eq!(read["axis"]["direction"].as_array().unwrap()[1], 0.0);
    ok(&mut doc, json!({"op": "axis", "edge": {"at": [40, 0, 8]}}));
    ok(
        &mut doc,
        json!({"op": "axis", "edge": {"faces": [{"normal": [0, 0, 1]}, {"normal": [0, -1, 0]}]}}),
    );
    ok(
        &mut doc,
        json!({"op": "axis", "face": {"feature": "Cut-Extrude1", "side": "side"}}),
    );
    ok(
        &mut doc,
        json!({"op": "point", "vertex": {"at": [80, 50, 8]}}),
    );
    let status = ok(&mut doc, json!({"op": "status"}));
    assert_eq!(status["failures"], json!([]));
}

#[test]
fn mistakes_are_explained_and_change_nothing() {
    let mut doc = Document::default();
    plate(&mut doc);
    let e = error(&mut doc, json!({"op": "extrud", "sketch": "Sketch1"}));
    assert!(
        e.contains("no operation 'extrud'") && e.contains("extrude"),
        "{e}"
    );
    let e = error(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depht": 3}),
    );
    assert!(e.contains("no field 'depht'") && e.contains("depth"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": "nonsense * 2"}),
    );
    assert!(e.starts_with("depth:"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "operation": "remove"}),
    );
    assert!(e.contains("add, new_body, cut"), "{e}");
    let e = error(&mut doc, json!({"op": "extrude", "depth": 3}));
    assert!(e.contains("Missing: sketch"), "{e}");
    let e = error(&mut doc, json!({"op": "extrude", "sketch": "Extrude1"}));
    assert!(e.contains("not a sketch"), "{e}");
    let e = error(&mut doc, json!({"op": "extrude", "sketch": "Sketch9"}));
    assert!(e.contains("Sketch1, Extrude1, Hole, Cut-Extrude1"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "draw", "sketch": "Hole", "draw": [{"type": "circle", "center": [1, 2]}]}),
    );
    assert!(e.contains("draw item 1") && e.contains("radius"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "draw", "sketch": "Hole", "draw": [
            {"type": "line", "from": [0, 0], "to": [5, 0], "as": "a"},
            {"type": "tangent", "of": ["a", "b"]},
        ]}),
    );
    assert!(e.contains("nothing is labelled 'b'"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "move", "feature": "Extrude1", "before": "Sketch1"}),
    );
    assert!(e.contains("Sketch1"), "{e}");
    let e = error(&mut doc, json!({"op": "undo", "extra": 1}));
    assert!(e.contains("no field 'extra'"), "{e}");
    let e = error(&mut doc, json!({"sketch": "Sketch1"}));
    assert!(e.contains("'op'"), "{e}");

    // A feature that can't be built is added and reported, as in the application.
    let reply = ok(
        &mut doc,
        json!({"op": "cut", "sketch": "Sketch1", "depth": 0}),
    );
    assert_eq!(reply["created"][0]["status"], "failed");
    assert_eq!(reply["failures"][0]["name"], "Cut-Extrude2");
    assert!(reply["failures"][0]["message"].is_string());
}

/// The enclosure panel sample, as a script.
const ENCLOSURE: &str = r#"
# Parameters, then the base plate.
{"op": "set_parameter", "name": "thickness", "value": "1.5mm"}
{"op": "set_parameter", "name": "flange", "value": "25mm"}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 150], "as": "r"}, {"type": "coincident", "of": ["r.bottom.start", "origin"]}, {"type": "length", "of": ["r.bottom"], "value": 200}, {"type": "length", "of": ["r.right"], "value": 150}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": "thickness", "radius": 2}

// A flange on each edge of the top face, set back from the corners.
{"op": "edge_flange", "length": "flange", "offset_start": 10, "offset_end": 10, "edges": [{"between": [[0, 0, 1.5], [200, 0, 1.5]]}, {"between": [[200, 0, 1.5], [200, 150, 1.5]]}, {"between": [[200, 150, 1.5], [0, 150, 1.5]]}, {"between": [[0, 150, 1.5], [0, 0, 1.5]]}]}

{"op": "sketch", "on": {"at": [50, 50, 1.5], "normal": [0, 0, 1]}, "draw": [{"type": "rectangle", "from": [60, 55], "to": [140, 95]}, {"type": "circle", "center": [20, 20], "radius": 2.5}, {"type": "circle", "center": [180, 20], "radius": 2.5}, {"type": "circle", "center": [20, 130], "radius": 2.5}, {"type": "circle", "center": [180, 130], "radius": 2.5}, {"type": "rectangle", "from": [190, 70], "to": [210, 80]}]}
{"op": "sheet_cut", "sketch": "Sketch2"}
{"op": "sketch", "on": {"at": [100, 0, 15], "normal": [0, -1, 0]}, "draw": [{"type": "circle", "center": [100, 15], "radius": 4}]}
{"op": "sheet_cut", "sketch": "Sketch3"}
"#;

#[test]
fn the_enclosure_script_matches_the_sample() {
    let ops = parse_script(ENCLOSURE).unwrap();
    assert_eq!(ops.len(), 9);
    let mut doc = Document::default();
    let replies = apply_all(&mut doc, &ops, Undo::Group(1));
    doc.seal_history();
    for (op, r) in ops.iter().zip(&replies) {
        assert!(r.ok, "{op}: {}", r.json["error"]);
        assert!(
            r.json.get("failures").is_none(),
            "{op}: {}",
            r.json["failures"]
        );
    }
    assert_eq!(replies.len(), ops.len());
    assert_eq!(replies[4].json["created"].as_array().unwrap().len(), 4);

    let (sample, engine) = peet_model::samples::enclosure();
    assert_eq!(doc.model.len(), sample.len());
    let expected = &engine.evaluation().bodies[0];
    let built = &doc.evaluation().bodies[0];
    close(
        measure::volume(&built.solid),
        measure::volume(&expected.solid),
    );
    assert_eq!(built.solid.faces.len(), expected.solid.faces.len());

    // The flat size of the hand calculation.
    let table = ok(&mut doc, json!({"op": "bend_table"}));
    assert_eq!(table["flat_size"], json!([244.356636, 194.356636]));
    assert_eq!(table["bends"].as_array().unwrap().len(), 4);
    assert_eq!(table["bends"][0]["feature"], "Edge-Flange1");
    assert_eq!(table["cutouts"], 7);
    let bodies = ok(&mut doc, json!({"op": "bodies"}));
    assert_eq!(bodies["bodies"][0]["sheet_metal"], true);
    let checks = ok(&mut doc, json!({"op": "checks"}));
    assert!(checks["findings"].is_array());

    // The parameter still drives it.
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "flange", "value": "30mm"}),
    );
    let table = ok(&mut doc, json!({"op": "bend_table"}));
    assert_eq!(table["flat_size"], json!([254.356636, 204.356636]));
    ok(&mut doc, json!({"op": "undo"}));

    // The whole script was one undo step.
    assert_eq!(doc.undo().as_deref(), Some("Set thickness"));
    assert!(doc.model.is_empty());
    assert!(!doc.can_undo());
}

#[test]
fn sketches_take_relations_dimensions_and_edits() {
    let mut doc = Document::default();
    let reply = ok(
        &mut doc,
        json!({"op": "sketch", "on": "front", "name": "Profile", "draw": [
            {"type": "polyline", "points": [[0, 0], [60, 0], [60, 40], [0, 40]], "closed": true, "as": "p"},
            {"type": "horizontal", "of": ["p.0"]},
            {"type": "vertical", "of": ["p.1"]},
            {"type": "horizontal", "of": ["p.2"]},
            {"type": "vertical", "of": ["p.3"]},
            {"type": "fix", "of": ["p.0.start"]},
            {"type": "length", "of": ["p.0"], "value": 60},
            {"type": "length", "of": ["p.1"], "value": 40},
            {"type": "fillet", "corner": "p.1.end", "radius": 6},
            {"type": "line", "from": [30, -5], "to": [30, 45], "as": "axis", "construction": true},
            {"type": "circle", "center": [15, 20], "radius": 4, "as": "c"},
            {"type": "mirror", "of": ["c"], "axis": "axis"},
            {"type": "radius", "of": ["c"], "value": "3mm + 1"},
        ]}),
    );
    assert_eq!(reply["drawn"][0]["curves"].as_array().unwrap().len(), 4);
    assert_eq!(reply["drawn"][8]["arc"]["type"], "arc");
    assert_eq!(reply["drawn"][11]["copies"].as_array().unwrap().len(), 1);
    let read = ok(&mut doc, json!({"op": "feature", "feature": "Profile"}));
    let kinds: Vec<&str> = read["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "circle").count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == "arc").count(), 1);
    assert!(read["dimensions"].as_array().unwrap().len() >= 4);
    assert_eq!(read["plane"]["normal"], json!([0.0, -1.0, 0.0]));

    // Later operations refer to entities by id.
    let line = read["entities"][0]["id"].clone();
    ok(
        &mut doc,
        json!({"op": "draw", "sketch": "Profile", "draw": [{"type": "construction", "of": [line]}]}),
    );
    let read = ok(&mut doc, json!({"op": "feature", "feature": "Profile"}));
    assert_eq!(read["entities"][0]["construction"], true);
    // With one side construction the outline is open: only the two holes are regions.
    let ex = ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Profile", "end": "mid_plane", "depth": 10}),
    );
    assert_eq!(ex["created"][0]["status"], "ok", "{ex}");
    close(volume(&doc), 2.0 * std::f64::consts::PI * 16.0 * 10.0);
}

#[test]
fn files_are_saved_and_exported() {
    let dir = std::env::temp_dir().join(format!("peet-ops-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
    let mut doc = Document::default();
    for r in apply_all(&mut doc, &parse_script(ENCLOSURE).unwrap(), Undo::Step) {
        assert!(r.ok);
    }
    let e = error(&mut doc, json!({"op": "save"}));
    assert!(e.contains("path"), "{e}");
    assert!(doc.is_modified());
    let saved = ok(&mut doc, json!({"op": "save", "path": path("panel.peet")}));
    assert!(saved["bytes"].as_u64().unwrap() > 100);
    assert!(!doc.is_modified());
    assert_eq!(doc.title(), "panel.peet");
    let bytes = std::fs::read(dir.join("panel.peet")).unwrap();
    assert_eq!(peet_io::document::open(&bytes).unwrap().model, doc.model);
    // Once it has a file, save needs no path.
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Edge-Flange1", "length": 30}),
    );
    ok(&mut doc, json!({"op": "save"}));

    let dxf = ok(&mut doc, json!({"op": "export", "path": path("flat.dxf")}));
    assert_eq!(dxf["format"], "dxf");
    assert!(
        std::fs::read_to_string(dir.join("flat.dxf"))
            .unwrap()
            .contains("ENTITIES")
    );
    let stl = ok(&mut doc, json!({"op": "export", "path": path("panel.stl")}));
    assert!(stl["triangles"].as_u64().unwrap() > 100);
    let step = ok(
        &mut doc,
        json!({"op": "export", "path": path("panel.out"), "format": "step", "schema": "ap242"}),
    );
    assert_eq!(step["bodies"], 1);
    assert!(
        std::fs::read_to_string(dir.join("panel.out"))
            .unwrap()
            .starts_with("ISO-10303-21")
    );
    let e = error(&mut doc, json!({"op": "export", "path": path("panel.obj")}));
    assert!(e.contains("stl, dxf, step or csv"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn every_feature_of_the_samples_reads_back_and_help_covers_every_operation() {
    for (model, _) in [
        peet_model::samples::bracket(),
        peet_model::samples::enclosure(),
        peet_model::samples::chassis(),
    ] {
        let mut doc = Document::from_model(model, None);
        let ids: Vec<u32> = doc.model.features().map(|f| f.id.0).collect();
        for id in ids {
            let read = ok(&mut doc, json!({"op": "feature", "feature": id}));
            assert!(read["fields"].is_object(), "{read}");
            assert_eq!(read["status"], "ok", "{read}");
        }
        ok(&mut doc, json!({"op": "edges"}));
    }
    let mut doc = Document::default();
    let help = ok(&mut doc, json!({"op": "help"}));
    let ops = help["operations"].as_object().unwrap();
    for name in [
        "extrude",
        "edge_flange",
        "linear_pattern",
        "sketch",
        "export",
        "checks",
    ] {
        assert!(ops.contains_key(name), "{name}");
    }
    assert_eq!(
        ops["edge_flange"]["fields"]["position"],
        "material_inside | material_outside | bend_outside"
    );
    assert_eq!(
        ops["edge_flange"]["fields"]["edge"],
        "edge, or null to pick it afterwards (required)"
    );
    // Every operation help lists exists: none answers "no operation".
    for name in ops.keys() {
        let reply: Reply = apply_json(&mut doc, &json!({"op": name}), Undo::Step);
        let error = reply.json["error"].as_str().unwrap_or_default();
        assert!(!error.contains("There is no operation"), "{name}: {error}");
    }
}

#[test]
fn patterns_mirrors_and_reference_geometry() {
    let mut doc = Document::default();
    plate(&mut doc);
    let pattern = ok(
        &mut doc,
        json!({"op": "linear_pattern", "features": ["Cut-Extrude1"], "direction": "x",
               "spacing": 20, "count": 2, "direction2": "y", "spacing2": 15, "count2": 2, "flip2": true}),
    );
    assert_eq!(pattern["created"][0]["status"], "ok", "{pattern}");
    close(
        volume(&doc),
        80.0 * 50.0 * 8.0 - 4.0 * std::f64::consts::PI * 25.0 * 8.0,
    );
    let e = error(
        &mut doc,
        json!({"op": "mirror", "features": ["Sketch1"], "plane": "right"}),
    );
    assert!(e.contains("can't be copied"), "{e}");
    let mid = ok(
        &mut doc,
        json!({"op": "plane", "a": {"normal": [1, 0, 0]}, "b": {"normal": [-1, 0, 0]}, "name": "Middle"}),
    );
    assert_eq!(mid["created"][0]["name"], "Middle");
    let read = ok(&mut doc, json!({"op": "feature", "feature": "Middle"}));
    assert_eq!(read["plane"]["origin"].as_array().unwrap()[0], 40.0);
    ok(&mut doc, json!({"op": "rollback", "to": "Extrude1"}));
    close(volume(&doc), 80.0 * 50.0 * 8.0);
    ok(&mut doc, json!({"op": "rollback", "to": "end"}));
    ok(&mut doc, json!({"op": "set_units", "length": "in"}));
    let bodies = ok(&mut doc, json!({"op": "bodies"}));
    assert_eq!(bodies["bodies"][0]["size"][2], round6(8.0 / 25.4));
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}
