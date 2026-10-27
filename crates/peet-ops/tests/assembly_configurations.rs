//! Configurations in assemblies through the operations: a component showing a
//! configuration of its part, an assembly's own configurations differing in components and
//! mates, and both through a file.

use peet_document::{Document, Session};
use peet_ops::{Headless, Undo, apply_json, apply_session_json};
use serde_json::{Value, json};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("peet-asmcfg-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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

/// A 60 x 40 plate, 6 thick, and 20 thick in its configuration "Thick"; saved in `dir`.
fn plate_file(dir: &std::path::Path) -> String {
    let file = dir.join("plate.peet").to_string_lossy().into_owned();
    let mut plate = Document::default();
    for op in [
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}),
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 6}),
        json!({"op": "add_configuration", "name": "Thick"}),
        json!({"op": "edit", "feature": "Extrude1", "depth": 20, "configurations": "this"}),
        json!({"op": "configuration", "configuration": "Default"}),
        json!({"op": "save", "path": file}),
    ] {
        ok(&mut plate, op);
    }
    file
}

/// The component called `name` in a `components` reply.
fn listed<'a>(reply: &'a Value, name: &str) -> &'a Value {
    reply["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {reply}"))
}

fn z(reply: &Value, name: &str) -> (f64, f64) {
    let c = listed(reply, name);
    (c["min"][2].as_f64().unwrap(), c["max"][2].as_f64().unwrap())
}

#[test]
fn a_component_shows_a_configuration_of_its_part() {
    let dir = temp("component");
    let plate = plate_file(&dir);
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    let base = ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Base"}),
    );
    assert_eq!(base["component"]["configuration"], "Default");
    let lid = ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Lid", "configuration": "Thick", "at": [0, 0, 50]}),
    );
    assert_eq!(lid["component"]["configuration"], "Thick");
    assert_eq!(lid["component"]["max"][2], 70.0);
    // One part, two configurations: the bill of materials has a line for each.
    let parts = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(parts["parts"].as_array().unwrap().len(), 1);
    assert_eq!(
        parts["parts"][0]["configurations"],
        json!(["Default", "Thick"])
    );
    let bom = ok(&mut doc, json!({"op": "bom"}));
    let rows: Vec<(&str, &str, u64)> = bom["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["part"].as_str().unwrap(),
                r["configuration"].as_str().unwrap(),
                r["quantity"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [("plate", "Default", 1), ("plate", "Thick", 1)]);
    // Another instance of a component shows what it shows.
    let copy = ok(
        &mut doc,
        json!({"op": "insert", "component": "Lid", "at": [100, 0, 0]}),
    );
    assert_eq!(copy["component"]["configuration"], "Thick");
    // Changed, and back to the part's active one.
    let changed = ok(
        &mut doc,
        json!({"op": "component_configuration", "component": "Lid", "configuration": "Default"}),
    );
    assert_eq!(changed["component"]["max"][2], 56.0);
    let e = error(
        &mut doc,
        json!({"op": "component_configuration", "component": "Lid", "configuration": "Wide"}),
    );
    assert!(e.contains("Default, Thick"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "insert", "path": plate, "configuration": "Wide"}),
    );
    assert!(e.contains("no configuration called 'Wide'"), "{e}");
    ok(
        &mut doc,
        json!({"op": "component_configuration", "component": "Lid", "configuration": null}),
    );
    // A mate on a face that is where it is in the configuration shown.
    ok(
        &mut doc,
        json!({"op": "component_configuration", "component": "Lid", "configuration": "Thick"}),
    );
    let mated = ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "a": {"component": "Base", "face": {"normal": [0, 0, 1]}}, "b": {"component": "Lid", "face": {"normal": [0, 0, -1]}}}),
    );
    assert_eq!(mated["mate"]["status"], "ok", "{mated}");
    let now = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(z(&now, "Lid"), (6.0, 26.0));
    // Saved and opened again: the same.
    let file = dir.join("asm.peet");
    ok(&mut doc, json!({"op": "save", "path": file}));
    let mut again = Document::default();
    ok(&mut again, json!({"op": "open", "path": file}));
    let reopened = ok(&mut again, json!({"op": "components"}));
    assert_eq!(listed(&reopened, "Lid")["configuration"], "Thick");
    assert_eq!(z(&reopened, "Lid"), (6.0, 26.0));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn an_assemblys_configurations_differ_in_components_and_mates() {
    let dir = temp("assembly");
    let plate = plate_file(&dir);
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Base"}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Lid", "at": [0, 0, 40], "configuration": "Thick"}),
    );
    ok(
        &mut doc,
        json!({"op": "mate", "type": "distance", "distance": 10, "a": {"component": "Base", "face": {"normal": [0, 0, 1]}}, "b": {"component": "Lid", "face": {"normal": [0, 0, -1]}}}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Spare", "at": [100, 0, 0]}),
    );
    ok(
        &mut doc,
        json!({"op": "edit_configuration", "configuration": "Default", "name": "Closed", "comment": "shut"}),
    );
    ok(&mut doc, json!({"op": "add_configuration", "name": "Open"}));
    // In "Open": the lid 50 up and thin, the spare there; in "Closed" the spare suppressed.
    ok(
        &mut doc,
        json!({"op": "set", "name": "D1@Distance1", "value": 50, "configurations": "this"}),
    );
    ok(
        &mut doc,
        json!({"op": "component_configuration", "component": "Lid", "configuration": "Default"}),
    );
    ok(
        &mut doc,
        json!({"op": "suppress", "component": "Spare", "configurations": ["Closed"]}),
    );
    ok(&mut doc, json!({"op": "fix", "component": "Spare"}));
    let open = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(z(&open, "Lid"), (56.0, 62.0));
    assert_eq!(listed(&open, "Spare")["status"], "ok");
    let differs = ok(&mut doc, json!({"op": "configurations"}));
    assert_eq!(differs["mates"]["Distance1"]["Closed"]["distance"], 10.0);
    assert_eq!(differs["mates"]["Distance1"]["Open"]["distance"], 50.0);
    assert_eq!(
        differs["components"]["Lid"]["Closed"]["configuration"],
        "Thick"
    );
    assert_eq!(differs["components"]["Spare"]["Closed"]["suppressed"], true);
    assert_eq!(differs["components"]["Spare"]["Open"]["fixed"], true);

    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Closed"}),
    );
    let closed = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(z(&closed, "Lid"), (16.0, 36.0));
    assert_eq!(listed(&closed, "Spare")["status"], "suppressed");
    assert_eq!(listed(&closed, "Spare")["fixed"], false);
    let mates = ok(&mut doc, json!({"op": "mates"}));
    assert_eq!(mates["mates"][0]["distance"], 10.0);

    // A mate suppressed in one configuration only, and a value set in all of them.
    ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "flip": true, "a": {"component": "Base", "face": {"normal": [1, 0, 0]}}, "b": {"component": "Lid", "face": {"normal": [1, 0, 0]}}}),
    );
    ok(
        &mut doc,
        json!({"op": "suppress", "mate": "Coincident1", "configurations": "Open"}),
    );
    ok(
        &mut doc,
        json!({"op": "edit_mate", "mate": "Distance1", "distance": 12, "configurations": "all"}),
    );
    let differs = ok(&mut doc, json!({"op": "configurations"}));
    assert_eq!(differs["mates"]["Coincident1"]["Open"]["suppressed"], true);
    assert!(differs["mates"]["Distance1"].is_null(), "{differs}");
    // Names and colours are the same in all of them.
    let e = error(
        &mut doc,
        json!({"op": "rename", "component": "Spare", "name": "Extra", "configurations": "this"}),
    );
    assert!(e.contains("every configuration"), "{e}");

    // Through a file, and every configuration as it was.
    let file = dir.join("lid.peet");
    ok(&mut doc, json!({"op": "save", "path": file}));
    let mut again = Document::default();
    ok(&mut again, json!({"op": "open", "path": file}));
    let reopened = ok(&mut again, json!({"op": "configurations"}));
    assert_eq!(reopened["active"], "Closed");
    assert_eq!(reopened["components"], differs["components"]);
    assert_eq!(reopened["mates"], differs["mates"]);
    ok(
        &mut again,
        json!({"op": "configuration", "configuration": "Open"}),
    );
    let open = ok(&mut again, json!({"op": "components"}));
    assert_eq!(z(&open, "Lid"), (18.0, 24.0));
    assert_eq!(listed(&open, "Spare")["fixed"], true);
    // FabSys's SetConfiguration with a name that isn't there: an error that lists them.
    let e = error(
        &mut again,
        json!({"op": "configuration", "configuration": "Ajar"}),
    );
    assert!(e.contains("Closed, Open"), "{e}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_sub_assembly_shows_one_of_its_configurations() {
    let dir = temp("sub");
    let plate = plate_file(&dir);
    // A sub-assembly whose configuration "Two" has a second plate.
    let sub = dir.join("pair.peet").to_string_lossy().into_owned();
    let mut host = Headless::default();
    let mut session = Session::default();
    for op in [
        json!({"op": "new", "assembly": true}),
        json!({"op": "insert", "path": plate, "name": "One"}),
        json!({"op": "insert", "path": plate, "name": "Other", "at": [0, 100, 0]}),
        json!({"op": "suppress", "component": "Other"}),
        json!({"op": "add_configuration", "name": "Two"}),
        json!({"op": "suppress", "component": "Other", "on": false}),
        json!({"op": "configuration", "configuration": "Default"}),
        json!({"op": "save", "path": sub}),
    ] {
        let reply = apply_session_json(&mut host, &mut session, &op, Undo::Step);
        assert!(reply.ok, "{op}: {}", reply.json);
    }
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(&mut doc, json!({"op": "insert", "path": sub, "name": "A"}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": sub, "name": "B", "configuration": "Two", "at": [200, 0, 0]}),
    );
    let bom = ok(&mut doc, json!({"op": "bom"}));
    assert_eq!(bom["quantity"], 3, "{bom}");
    let top = ok(&mut doc, json!({"op": "bom", "level": "top"}));
    assert_eq!(top["rows"].as_array().unwrap().len(), 2, "{top}");
    let mass = ok(&mut doc, json!({"op": "mass"}));
    let volume = |name: &str| {
        mass["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["component"] == name)
            .unwrap()["volume_mm3"]
            .as_f64()
            .unwrap()
    };
    assert!((volume("B") - 2.0 * volume("A")).abs() < 1e-6, "{mass}");
    let step = dir.join("pair.step");
    let exported = ok(&mut doc, json!({"op": "export", "path": step}));
    assert_eq!(exported["bodies"], 3, "{exported}");
    std::fs::remove_dir_all(dir).ok();
}
