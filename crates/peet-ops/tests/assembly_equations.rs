//! An assembly reaching into its components, as SolidWorks' assembly equations and FabSys
//! do: dimensions read and set by `NAME@FEATURE@COMPONENT`, equations, suppression by an
//! expression, reference dimensions, and a component given a part of its own.

use peet_document::Document;
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

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("peet-eq-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A 100 × 100 plate 5 thick, and a pipe of radius 5, 300 long, saved in `dir`.
fn plate_and_pipe(dir: &std::path::Path) -> (String, String) {
    let plate = dir.join("plate.peet").to_string_lossy().into_owned();
    let pipe = dir.join("pipe.peet").to_string_lossy().into_owned();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 100]}]}),
    );
    ok(&mut doc, json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}));
    ok(&mut doc, json!({"op": "save", "path": plate}));
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 5}]}),
    );
    ok(&mut doc, json!({"op": "extrude", "sketch": "Sketch1", "depth": 300}));
    ok(&mut doc, json!({"op": "save", "path": pipe}));
    (plate, pipe)
}

fn component<'a>(reply: &'a Value, name: &str) -> &'a Value {
    reply["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("{name} in {reply}"))
}

/// Two plates 308 apart, and a pipe standing on the lower one.
fn plinth(link: bool) -> (Document, std::path::PathBuf) {
    let dir = temp(if link { "linked" } else { "own" });
    let (plate, pipe) = plate_and_pipe(&dir);
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(&mut doc, json!({"op": "insert", "path": plate, "name": "Base-1"}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Top-1", "at": [0, 0, 313], "fixed": true}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": pipe, "name": "Pipe-1", "at": [50, 50, 10], "link": link}),
    );
    ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident",
            "a": {"component": "Base-1", "face": {"normal": [0, 0, 1]}},
            "b": {"component": "Pipe-1", "face": {"normal": [0, 0, -1]}}}),
    );
    (doc, dir)
}

#[test]
fn a_pipe_is_as_long_as_the_gap_a_reference_dimension_measures() {
    let (mut doc, _) = plinth(false);
    let added = ok(
        &mut doc,
        json!({"op": "reference_dimension",
            "a": {"component": "Base-1", "face": {"normal": [0, 0, 1]}},
            "b": {"component": "Top-1", "face": {"normal": [0, 0, -1]}}}),
    );
    assert_eq!(added["reference"]["name"], "RD1");
    assert_eq!(added["reference"]["value"], 308.0);
    // SolidWorks' own form of the equation (Plinth Folded Assembly's)
    ok(
        &mut doc,
        json!({"op": "set", "name": "D1@Extrude1@Pipe<1>.Part", "value": "\"RD1@Annotations\""}),
    );
    let pipe = component(&ok(&mut doc, json!({"op": "components"})), "Pipe-1").clone();
    assert_eq!(pipe["min"][2], 5.0, "{pipe}");
    assert_eq!(pipe["max"][2], 313.0, "{pipe}");
    // The top plate moves: the pipe follows the gap, the mates solved again.
    ok(&mut doc, json!({"op": "place", "component": "Top-1", "at": [0, 0, 405]}));
    let listed = ok(&mut doc, json!({"op": "dimensions"}));
    assert_eq!(listed["references"][0]["value"], 400.0, "{listed}");
    let pipe = component(&ok(&mut doc, json!({"op": "components"})), "Pipe-1").clone();
    assert_eq!(pipe["max"][2], 405.0, "{pipe}");
    // A reference dimension measures; it can't be set.
    let e = error(&mut doc, json!({"op": "set", "name": "RD1@Annotations", "value": 3}));
    assert!(e.contains("reference dimension"), "{e}");
    // Saved and opened again, the equation and the reference are there.
    let bytes = peet_io::document::save(&doc.model, &Default::default(), None).unwrap();
    let opened = peet_io::document::open(&bytes).unwrap().model;
    let assembly = opened.assembly().unwrap();
    assert_eq!((assembly.equations().len(), assembly.references().len()), (1, 1));
    ok(&mut doc, json!({"op": "delete", "reference_dimension": "RD1"}));
    let failures = ok(&mut doc, json!({"op": "status"}))["failures"].to_string();
    assert!(failures.contains("RD1@Annotations"), "the equation can't read it now: {failures}");
}

#[test]
fn a_components_dimension_is_set_in_its_part_and_its_file() {
    let (mut doc, dir) = plinth(true);
    ok(&mut doc, json!({"op": "insert", "component": "Pipe-1", "name": "Pipe-2", "at": [20, 20, 10]}));
    let set = ok(&mut doc, json!({"op": "set", "name": "depth@Extrude1@Pipe-2", "value": 250}));
    // The part is shared: both instances change, and the linked file is written, as
    // SolidWorks saves the part it changed.
    assert!(set["written"].as_str().unwrap().ends_with("pipe.peet"), "{set}");
    let listed = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(component(&listed, "Pipe-1")["max"][2], 255.0);
    let file = peet_io::document::open(&std::fs::read(dir.join("pipe.peet")).unwrap())
        .unwrap()
        .model;
    assert_eq!(
        file.find_dimension("d1@Extrude1").map(|d| d.value.value),
        Some(250.0)
    );
    // Made independent, one changes alone; a linked part's copy is a file of its own.
    let made = ok(&mut doc, json!({"op": "make_independent", "component": "Pipe-2", "name": "Short pipe"}));
    assert_eq!(made["component"]["name"], "Short pipe-1");
    assert!(dir.join("Short pipe.peet").is_file());
    ok(&mut doc, json!({"op": "set", "name": "depth@Extrude1@Short pipe-1", "value": 100}));
    let listed = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(component(&listed, "Pipe-1")["max"][2], 255.0);
    assert_eq!(component(&listed, "Short pipe-1")["max"][2], 110.0);
    // A parameter of a component's part, and what can't be found.
    let e = error(&mut doc, json!({"op": "set", "name": "Nothing@Extrude1@Pipe-1", "value": 3}));
    assert!(e.contains("has no dimension or parameter"), "{e}");
    let e = error(&mut doc, json!({"op": "set", "name": "d1@Extrude1@Nobody-1", "value": 3}));
    assert!(e.contains("D1@MATE"), "{e}");
}

#[test]
fn a_component_is_suppressed_by_an_expression_and_a_feature_of_its_part_by_name() {
    let (mut doc, _) = plinth(false);
    ok(&mut doc, json!({"op": "set_parameter", "name": "tall", "value": "1"}));
    ok(
        &mut doc,
        json!({"op": "set_suppression_expression", "component": "Top-1",
            "value": "iif(\"d1@Extrude1@Pipe-1\" > 299, 0, 1)"}),
    );
    let status = |doc: &mut Document, name: &str| {
        component(&ok(doc, json!({"op": "components"})), name)["status"].clone()
    };
    assert_eq!(status(&mut doc, "Top-1"), "ok");
    ok(&mut doc, json!({"op": "set", "name": "depth@Extrude1@Pipe-1", "value": 200}));
    assert_eq!(status(&mut doc, "Top-1"), "suppressed");
    // The equation decides over 'suppress' until it is cleared.
    ok(&mut doc, json!({"op": "suppress", "component": "Top-1", "on": false}));
    assert_eq!(status(&mut doc, "Top-1"), "suppressed");
    ok(&mut doc, json!({"op": "set_suppression_expression", "component": "Top-1", "value": null}));
    ok(&mut doc, json!({"op": "suppress", "component": "Top-1", "on": false}));
    assert_eq!(status(&mut doc, "Top-1"), "ok");
    let e = error(
        &mut doc,
        json!({"op": "set_suppression_expression", "component": "Top-1", "value": "5mm"}),
    );
    assert!(e.contains("plain number"), "{e}");
    // A feature of a component's part, as FabSys's SuppressFeature("NAME@COMPONENT")
    ok(&mut doc, json!({"op": "suppress", "feature": "Extrude1@Pipe-1"}));
    assert_eq!(status(&mut doc, "Pipe-1"), "warning");
    ok(&mut doc, json!({"op": "suppress", "feature": "Extrude1@Pipe-1", "on": false}));
    assert_eq!(status(&mut doc, "Pipe-1"), "ok");
    // A component's name in any case, as SolidWorks finds it, where only one matches.
    ok(&mut doc, json!({"op": "suppress", "component": "top-1"}));
    assert_eq!(status(&mut doc, "Top-1"), "suppressed");
    let listed = ok(&mut doc, json!({"op": "dimensions", "component": "Pipe-1"}));
    assert!(listed.to_string().contains("depth@Extrude1@Pipe-1"), "{listed}");
}
