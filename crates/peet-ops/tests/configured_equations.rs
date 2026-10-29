//! What C1's assembly configurations left: equations that differ between an assembly's
//! configurations, derived configurations, and opening a component's part in the
//! configuration the component shows.

use peet_document::Session;
use peet_ops::{Headless, Undo, apply_session_json};
use serde_json::{Value, json};

fn status(reply: &Value, name: &str) -> String {
    reply["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {reply}"))["status"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn session_ok(host: &mut Headless, session: &mut Session, op: Value) -> Value {
    let reply = apply_session_json(host, session, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    reply.json
}

fn plate_ops() -> Vec<Value> {
    vec![
        json!({"op": "new"}),
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}),
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 6}),
        json!({"op": "add_configuration", "name": "Thick"}),
        json!({"op": "edit", "feature": "Extrude1", "depth": 20, "configurations": "this"}),
        json!({"op": "configuration", "configuration": "Default"}),
    ]
}

#[test]
fn equations_differ_between_an_assemblys_configurations() {
    let mut host = Headless::default();
    let mut session = Session::default();
    for op in plate_ops() {
        session_ok(&mut host, &mut session, op);
    }
    for op in [
        json!({"op": "new", "assembly": true, "keep": true}),
        json!({"op": "insert", "part": 1, "name": "Base"}),
        json!({"op": "insert", "part": 1, "name": "Spare", "at": [100, 0, 0]}),
        json!({"op": "set_parameter", "name": "wide", "value": 0}),
        json!({"op": "edit_configuration", "configuration": "Default", "name": "Narrow"}),
        json!({"op": "add_configuration", "name": "Wide"}),
        json!({"op": "set_parameter", "name": "wide", "value": 1, "configurations": "this"}),
        // The spare is suppressed in Wide only, by an equation that is Wide's alone.
        json!({"op": "set_suppression_expression", "component": "Spare", "value": "wide", "configurations": "this"}),
        // The width: one equation, another expression in each configuration.
        json!({"op": "set", "name": "depth@Extrude1@Base", "value": "6 + 4 * wide"}),
        json!({"op": "set", "name": "depth@Extrude1@Base", "value": "12mm", "configurations": "this"}),
    ] {
        session_ok(&mut host, &mut session, op);
    }
    let width = |session: &mut Session, host: &mut Headless| {
        let d = session_ok(
            host,
            session,
            json!({"op": "dimensions", "name": "depth@Extrude1@Base"}),
        );
        d["dimensions"][0]["value"].as_f64().unwrap()
    };
    let doc_status = |session: &mut Session, host: &mut Headless| {
        let c = session_ok(host, session, json!({"op": "components"}));
        status(&c, "Spare")
    };
    assert_eq!(width(&mut session, &mut host), 12.0);
    assert_eq!(doc_status(&mut session, &mut host), "suppressed");
    let listed = session_ok(&mut host, &mut session, json!({"op": "configurations"}));
    assert_eq!(
        listed["equations"]["Spare"],
        json!({"Narrow": null, "Wide": "wide"}),
        "{listed}"
    );
    assert_eq!(
        listed["equations"]["depth@Extrude1@Base"],
        json!({"Narrow": "6 + 4 * wide", "Wide": "12mm"}),
        "{listed}"
    );
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "configuration", "configuration": "Narrow"}),
    );
    assert_eq!(width(&mut session, &mut host), 6.0);
    assert_ne!(doc_status(&mut session, &mut host), "suppressed");
    // Left out, `configurations` is the active one where the equation already differs.
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "set", "name": "depth@Extrude1@Base", "value": "70"}),
    );
    let listed = session_ok(&mut host, &mut session, json!({"op": "configurations"}));
    assert_eq!(
        listed["equations"]["depth@Extrude1@Base"],
        json!({"Narrow": "70", "Wide": "12mm"})
    );
    // null clears an equation where it is given.
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "set", "name": "depth@Extrude1@Base", "value": null, "configurations": "Narrow"}),
    );
    let d = session_ok(&mut host, &mut session, json!({"op": "dimensions"}));
    let eq = d["equations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["target"] == "depth@Extrude1@Base")
        .unwrap();
    assert_eq!(eq["expression"], Value::Null, "{d}");
    assert_eq!(eq["configurations"]["Wide"], "12mm", "{d}");
}

#[test]
fn derived_configurations_of_an_assembly_follow_their_parent() {
    let mut host = Headless::default();
    let mut session = Session::default();
    for op in plate_ops() {
        session_ok(&mut host, &mut session, op);
    }
    for op in [
        json!({"op": "new", "assembly": true, "keep": true}),
        json!({"op": "insert", "part": 1, "name": "Base"}),
        json!({"op": "insert", "part": 1, "name": "Spare", "at": [100, 0, 0]}),
        json!({"op": "add_configuration", "name": "Big"}),
        json!({"op": "add_configuration", "name": "Big painted", "parent": "Big"}),
    ] {
        session_ok(&mut host, &mut session, op);
    }
    let listed = session_ok(&mut host, &mut session, json!({"op": "configurations"}));
    assert_eq!(listed["active"], "Big painted");
    assert_eq!(listed["configurations"][2]["parent"], "Big", "{listed}");
    // A change to the parent reaches the derived configuration...
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "component_configuration", "component": "Base", "configuration": "Thick", "configurations": "Big"}),
    );
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "suppress", "component": "Spare", "configurations": "Big"}),
    );
    let c = session_ok(&mut host, &mut session, json!({"op": "components"}));
    assert_eq!(status(&c, "Spare"), "suppressed", "{c}");
    // ... unless it has a value of its own there.
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "component_configuration", "component": "Base", "configuration": "Default"}),
    );
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "suppress", "component": "Spare", "on": false, "configurations": "Big"}),
    );
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "component_configuration", "component": "Base", "configuration": "Thick", "configurations": "Big"}),
    );
    let listed = session_ok(&mut host, &mut session, json!({"op": "configurations"}));
    assert_eq!(
        listed["components"]["Base"]["Big painted"]["configuration"], "Default",
        "{listed}"
    );
    assert_eq!(
        listed["components"]["Base"]["Big"]["configuration"],
        "Thick"
    );
    let c = session_ok(&mut host, &mut session, json!({"op": "components"}));
    assert_ne!(status(&c, "Spare"), "suppressed", "{c}");
    // Deleting the parent deletes what is derived from it.
    let reply = apply_session_json(
        &mut host,
        &mut session,
        &json!({"op": "add_configuration", "name": "X", "parent": "Big", "copy": "Big"}),
        Undo::Step,
    );
    assert!(!reply.ok);
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "delete_configuration", "configuration": "Big"}),
    );
    let listed = session_ok(&mut host, &mut session, json!({"op": "configurations"}));
    assert_eq!(
        listed["configurations"].as_array().unwrap().len(),
        1,
        "{listed}"
    );
}

#[test]
fn a_component_opens_in_the_configuration_it_shows() {
    let mut host = Headless::default();
    let mut session = Session::default();
    for op in plate_ops() {
        session_ok(&mut host, &mut session, op);
    }
    for op in [
        json!({"op": "new", "assembly": true, "keep": true}),
        json!({"op": "insert", "part": 1, "name": "Base"}),
        json!({"op": "insert", "part": 1, "name": "Lid", "configuration": "Thick", "at": [0, 0, 50]}),
    ] {
        session_ok(&mut host, &mut session, op);
    }
    let opened = session_ok(
        &mut host,
        &mut session,
        json!({"op": "open_component", "component": "Lid"}),
    );
    assert_eq!(opened["configuration"], "Thick", "{opened}");
    let shown = session_ok(&mut host, &mut session, json!({"op": "configurations"}));
    assert_eq!(shown["active"], "Thick");
    // A change made there, in that configuration, and stored back.
    session_ok(
        &mut host,
        &mut session,
        json!({"op": "edit", "feature": "Extrude1", "depth": 30, "configurations": "this"}),
    );
    session_ok(&mut host, &mut session, json!({"op": "save"}));
    session_ok(&mut host, &mut session, json!({"op": "close"}));
    // The part keeps its own active configuration: Base (which names none) is as it was.
    let c = session_ok(&mut host, &mut session, json!({"op": "components"}));
    let z = |name: &str| {
        let x = c["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == name)
            .unwrap();
        x["max"][2].as_f64().unwrap() - x["min"][2].as_f64().unwrap()
    };
    assert_eq!(z("Base"), 6.0, "{c}");
    assert_eq!(z("Lid"), 30.0, "{c}");
    // Opened from Base, it shows the part's active configuration.
    let opened = session_ok(
        &mut host,
        &mut session,
        json!({"op": "open_component", "component": "Base"}),
    );
    assert_eq!(opened["configuration"], "Default", "{opened}");
}
