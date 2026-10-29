//! Display states through the operations: named looks of an assembly (each component
//! shown or hidden, coloured, see-through), linked to configurations or not, and changes
//! made in the application translated into operations.

use peet_document::Document;
use peet_ops::{Headless, Undo, apply_json, apply_model};
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

/// Two brackets and a housing.
fn assembly() -> Document {
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(&mut doc, json!({"op": "insert", "sample": "bracket"}));
    ok(
        &mut doc,
        json!({"op": "insert", "component": "Bracket-1", "at": [0, 200, 0]}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "sample": "housing", "at": [300, 0, 0]}),
    );
    doc
}

#[test]
fn display_states_hold_each_components_look() {
    let mut doc = assembly();
    // A component made see-through without display states: it is the component's own.
    let clear = ok(
        &mut doc,
        json!({"op": "set_transparency", "component": "Housing-1", "transparency": 0.5}),
    );
    assert_eq!(clear["component"]["transparency"], 0.5);
    assert_eq!(doc.undo_label(), Some("Change Transparency of Housing-1"));
    assert_eq!(doc.placed[2].transparency, 0.5);
    let e = error(
        &mut doc,
        json!({"op": "set_transparency", "component": "Housing-1", "transparency": 2}),
    );
    assert!(e.contains("from 0 (opaque)"), "{e}");

    // The first display state names the look as it is.
    let first = ok(
        &mut doc,
        json!({"op": "add_display_state", "name": "Working"}),
    );
    assert_eq!(first["display_state"]["name"], "Working");
    assert_eq!(first["display_state"]["shown"], true);
    let looks = &first["display_state"]["looks"];
    assert_eq!(looks[0]["component"], "Housing-1");
    assert_eq!(looks[0]["transparency"], 0.5);
    // A second starts as a copy, and is shown.
    ok(
        &mut doc,
        json!({"op": "add_display_state", "name": "Presentation"}),
    );
    ok(
        &mut doc,
        json!({"op": "set_transparency", "component": "Housing-1", "transparency": 0}),
    );
    ok(
        &mut doc,
        json!({"op": "set_color", "component": "Bracket-1", "color": "#2060c0"}),
    );
    ok(
        &mut doc,
        json!({"op": "show", "component": "Bracket-2", "on": false}),
    );
    // In a display state that is not shown: the components stay as they are.
    ok(
        &mut doc,
        json!({"op": "show", "component": "Bracket-1", "on": false, "display_states": "Working"}),
    );
    assert!(!doc.is_hidden(0));
    let listed = ok(&mut doc, json!({"op": "display_states"}));
    assert_eq!(listed["shown"], "Presentation");
    assert_eq!(listed["linked"], false);
    let working = &listed["display_states"][0];
    assert_eq!(working["shown"], false);
    assert_eq!(
        working["looks"],
        json!([
            {"component": "Bracket-1", "hidden": true},
            {"component": "Housing-1", "transparency": 0.5}
        ])
    );

    // Showing another: every component takes its look there. Not an undo step.
    let label = doc.undo_label().map(str::to_owned);
    ok(
        &mut doc,
        json!({"op": "display_state", "display_state": "Working"}),
    );
    assert_eq!(doc.undo_label().map(str::to_owned), label);
    assert!(doc.is_hidden(0) && !doc.is_hidden(1));
    assert_eq!(doc.placed[2].transparency, 0.5);
    assert_eq!(doc.placed[0].color, doc.placed[1].color);
    ok(
        &mut doc,
        json!({"op": "display_state", "display_state": "Presentation"}),
    );
    assert!(!doc.is_hidden(0) && doc.is_hidden(1));
    assert_eq!(doc.placed[0].color, Some([0x20, 0x60, 0xc0]));
    assert_eq!(doc.placed[2].transparency, 0.0);
    // `isolate` and `show_all` work on the one shown.
    ok(
        &mut doc,
        json!({"op": "isolate", "components": ["Housing-1"]}),
    );
    ok(
        &mut doc,
        json!({"op": "display_state", "display_state": "Working"}),
    );
    assert!(doc.is_hidden(0) && !doc.is_hidden(2));
    // In every display state at once.
    ok(
        &mut doc,
        json!({"op": "set_transparency", "component": "Bracket-2", "transparency": 0.25, "display_states": "all"}),
    );
    let listed = ok(&mut doc, json!({"op": "display_states"}));
    for s in listed["display_states"].as_array().unwrap() {
        assert!(
            s["looks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["component"] == "Bracket-2" && l["transparency"] == 0.25),
            "{s}"
        );
    }

    // Renamed, deleted; one always stays.
    ok(
        &mut doc,
        json!({"op": "rename", "display_state": "Presentation", "name": "Shiny"}),
    );
    let e = error(
        &mut doc,
        json!({"op": "add_display_state", "name": "Shiny"}),
    );
    assert!(e.contains("already called"), "{e}");
    ok(
        &mut doc,
        json!({"op": "delete", "display_state": "Working"}),
    );
    let listed = ok(&mut doc, json!({"op": "display_states"}));
    assert_eq!(listed["shown"], "Shiny");
    let e = error(&mut doc, json!({"op": "delete", "display_state": "Shiny"}));
    assert!(e.contains("only display state"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "display_state", "display_state": "Nope"}),
    );
    assert!(e.contains("Shiny"), "{e}");

    // Saved and opened again.
    let bytes = peet_io::document::save(&doc.model, &Default::default(), None).unwrap();
    let opened = peet_io::document::open(&bytes).unwrap().model;
    assert_eq!(opened, doc.model);

    // A part has none.
    let mut part = Document::default();
    let e = error(&mut part, json!({"op": "add_display_state", "name": "X"}));
    assert!(e.contains("works on an assembly"), "{e}");
}

#[test]
fn linked_display_states_follow_the_configuration() {
    let mut doc = assembly();
    ok(&mut doc, json!({"op": "link_display_states"}));
    ok(
        &mut doc,
        json!({"op": "add_display_state", "name": "Plain"}),
    );
    ok(&mut doc, json!({"op": "add_configuration", "name": "Open"}));
    // The new configuration got one of its own when it was made active.
    let listed = ok(&mut doc, json!({"op": "display_states"}));
    assert_eq!(listed["linked"], true);
    assert_eq!(listed["display_states"][1]["configuration"], "Open");
    ok(&mut doc, json!({"op": "add_display_state", "name": "Red"}));
    ok(
        &mut doc,
        json!({"op": "set_color", "component": "Housing-1", "color": "#ff0000"}),
    );
    let default = listed["display_states"][0]["configuration"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": default}),
    );
    let listed = ok(&mut doc, json!({"op": "display_states"}));
    assert_eq!(listed["shown"], "Plain");
    assert!(
        doc.model
            .assembly()
            .unwrap()
            .components()
            .all(|c| c.color.is_none())
    );
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Open"}),
    );
    let listed = ok(&mut doc, json!({"op": "display_states"}));
    assert_eq!(listed["shown"], "Red");
    assert_eq!(doc.placed[2].color, Some([255, 0, 0]));
    // A display state for another configuration, named there.
    ok(
        &mut doc,
        json!({"op": "add_display_state", "name": "Ghost", "configuration": default}),
    );
    assert_eq!(
        ok(&mut doc, json!({"op": "display_states"}))["shown"],
        "Red"
    );
}

#[test]
fn looks_changed_in_the_application_are_operations() {
    let mut doc = assembly();
    ok(&mut doc, json!({"op": "add_display_state", "name": "One"}));
    ok(
        &mut doc,
        json!({"op": "add_configuration", "name": "Other"}),
    );
    let mut changed = doc.model.clone();
    let id = changed.assembly().unwrap().components().nth(2).unwrap().id;
    let c = changed.assembly_mut().unwrap().component_mut(id).unwrap();
    c.transparency = 0.75;
    c.color = Some([1, 2, 3]);
    c.visible = false;
    let mut host = Headless::default();
    let done = apply_model(&mut host, &mut doc, changed.clone(), "Look", 1);
    assert_eq!(done.untranslated, None, "{:?}", done.ops);
    assert_eq!(doc.model, changed);
    // The display states themselves are changed by operations only.
    let mut changed = doc.model.clone();
    changed.add_display_state("Two", None).unwrap();
    let done = apply_model(&mut host, &mut doc, changed, "Add", 2);
    assert!(done.untranslated.is_some());
}

#[test]
fn a_sub_assembly_is_drawn_as_its_own_display_state_has_it() {
    use std::sync::Arc;
    let (part, _) = peet_model::samples::bracket();
    let mut sub = peet_model::Model::new_assembly();
    let a = sub.assembly_mut().unwrap();
    let def = a.define(Arc::new(part));
    let one = a.insert(def, peet_math::Frame::WORLD).unwrap();
    let two = a.insert(def, peet_math::Frame::WORLD).unwrap();
    sub.add_display_state("Inside", None).unwrap();
    sub.set_look(one, None, |l| l.transparency = 0.4).unwrap();
    sub.set_look(two, None, |l| l.visible = false).unwrap();
    let mut top = peet_model::Model::new_assembly();
    let a = top.assembly_mut().unwrap();
    let def = a.define(Arc::new(sub));
    let c = a.insert(def, peet_math::Frame::WORLD).unwrap();
    let mut doc = Document::from_model(top, None);
    doc.finish_loading();
    assert_eq!(doc.placed.len(), 2);
    assert_eq!(doc.placed[0].transparency, 0.4);
    assert!(!doc.is_hidden(0) && doc.is_hidden(1));
    // The sub-assembly's own transparency goes for everything in it.
    doc.change("See through", |m| {
        m.set_look(c, None, |l| l.transparency = 0.8).unwrap();
    });
    assert_eq!(doc.placed[0].transparency, 0.8);
    assert_eq!(doc.placed[1].transparency, 0.8);
}
