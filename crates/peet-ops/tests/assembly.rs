//! Assemblies through the operations: components inserted, placed, changed and replaced,
//! parts opened and stored back, and what is refused in which kind of document.

use std::sync::Arc;

use peet_document::{Document, Session};
use peet_math::{DVec3, Frame};
use peet_model::Model;
use peet_ops::{
    CompSel, ComponentChange, Headless, InsertSource, Op, Placing, Undo, apply, apply_json,
    apply_model, apply_session_json,
};
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
    let dir = std::env::temp_dir().join(format!("peet-asm-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn assembly() -> Document {
    let mut doc = Document::default();
    let made = ok(&mut doc, json!({"op": "new", "assembly": true}));
    assert_eq!(made["kind"], "assembly");
    assert_eq!(made["components"], 0);
    doc
}

#[test]
fn components_are_inserted_placed_and_changed() {
    let mut doc = assembly();
    let first = ok(&mut doc, json!({"op": "insert", "sample": "bracket"}));
    assert_eq!(
        first["component"],
        json!({
            "id": 1, "name": "Bracket-1", "part": "Bracket", "at": [0.0, 0.0, 0.0],
            "fixed": true, "status": "ok",
            "min": first["component"]["min"], "max": first["component"]["max"],
        })
    );
    assert_eq!(doc.undo_label(), Some("Insert Bracket-1"));
    let size = |c: &Value| {
        let (lo, hi) = (c["min"].as_array().unwrap(), c["max"].as_array().unwrap());
        [0, 1, 2].map(|i| hi[i].as_f64().unwrap() - lo[i].as_f64().unwrap())
    };
    let own = size(&first["component"]);

    // A second instance, placed and turned: a quarter turn about Z swaps its X and Y.
    let second = ok(
        &mut doc,
        json!({"op": "insert", "component": "Bracket-1", "name": "Left",
               "at": [200, 0, 0], "rotate": {"axis": "z", "angle": 90}}),
    );
    let c = &second["component"];
    assert_eq!(
        (c["name"].as_str(), c["fixed"].as_bool()),
        (Some("Left"), Some(false))
    );
    assert_eq!(c["rotate"], json!({"axis": [0.0, 0.0, 1.0], "angle": 90.0}));
    let turned = size(c);
    assert!((turned[0] - own[1]).abs() < 1e-6 && (turned[1] - own[0]).abs() < 1e-6);
    assert!((turned[2] - own[2]).abs() < 1e-6);

    let list = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(list["components"].as_array().unwrap().len(), 2);
    assert_eq!(
        list["parts"],
        json!([{"id": 1, "name": "Bracket", "kind": "part", "components": 2}]),
        "one part, however many instances"
    );
    assert_eq!(doc.bodies.len(), 2);
    assert!(Arc::ptr_eq(&doc.bodies[0], &doc.bodies[1]));

    // Placing: the parts left out stay.
    let moved = ok(
        &mut doc,
        json!({"op": "place", "component": "Left", "at": [0, 300, 10]}),
    );
    assert_eq!(moved["component"]["at"], json!([0.0, 300.0, 10.0]));
    assert_eq!(moved["component"]["rotate"]["angle"], 90.0);
    assert_eq!(doc.undo_label(), Some("Move Left"));
    let turned = ok(
        &mut doc,
        json!({"op": "place", "component": 2, "rotate": {"axis": [0, 0, 2], "angle": 0}}),
    );
    assert!(turned["component"].get("rotate").is_none());
    assert_eq!(turned["component"]["at"], json!([0.0, 300.0, 10.0]));

    // The other changes.
    ok(&mut doc, json!({"op": "fix", "component": "Left"}));
    ok(
        &mut doc,
        json!({"op": "rename", "component": "Left", "name": "Second"}),
    );
    let hidden = ok(
        &mut doc,
        json!({"op": "show", "component": "Second", "on": false}),
    );
    assert_eq!(hidden["component"]["hidden"], true);
    let off = ok(&mut doc, json!({"op": "suppress", "component": "Second"}));
    assert_eq!(off["component"]["status"], "suppressed");
    assert_eq!(doc.bodies.len(), 1);
    ok(
        &mut doc,
        json!({"op": "suppress", "component": "Second", "on": false}),
    );
    let c = doc.model.assembly().unwrap().components().nth(1).unwrap();
    assert!(c.fixed && !c.visible && !c.suppressed && c.name == "Second");

    // Replaced where it is: another part, the same place.
    let replaced = ok(
        &mut doc,
        json!({"op": "replace", "component": "Second", "sample": "housing"}),
    );
    assert_eq!(replaced["component"]["part"], "Housing");
    assert_eq!(replaced["component"]["at"], json!([0.0, 300.0, 10.0]));
    let status = ok(&mut doc, json!({"op": "status"}));
    assert_eq!(
        (status["components"].as_u64(), status["parts"].as_u64()),
        (Some(2), Some(2))
    );
    assert_eq!(status["failures"], json!([]));

    // Deleting the last instance of a part takes the part along; undo brings both back.
    let deleted = ok(&mut doc, json!({"op": "delete", "component": "Second"}));
    assert_eq!(deleted["deleted"], json!(["Second"]));
    assert_eq!(doc.model.assembly().unwrap().definitions().count(), 1);
    ok(&mut doc, json!({"op": "undo"}));
    assert_eq!(doc.model.assembly().unwrap().definitions().count(), 2);
    assert_eq!(doc.bodies.len(), 2);
}

#[test]
fn mistakes_are_explained_and_change_nothing() {
    let mut doc = assembly();
    ok(&mut doc, json!({"op": "insert", "sample": "bracket"}));
    for (op, says) in [
        (json!({"op": "insert"}), "needs one of"),
        (
            json!({"op": "insert", "sample": "bracket", "path": "a.peet"}),
            "only one of",
        ),
        (json!({"op": "insert", "sample": "gearbox"}), "bracket"),
        (
            json!({"op": "insert", "path": "no-such.peet"}),
            "no-such.peet",
        ),
        (json!({"op": "insert", "component": "Nut-1"}), "Bracket-1"),
        (
            json!({"op": "insert", "sample": "bracket", "name": "Bracket-1"}),
            "already called",
        ),
        (
            json!({"op": "insert", "sample": "bracket", "at": [1, 2]}),
            "3 coordinates",
        ),
        (
            json!({"op": "insert", "sample": "bracket", "rotate": {"axis": [0, 0, 0], "angle": 9}}),
            "no direction",
        ),
        (
            json!({"op": "insert", "sample": "bracket", "rotate": {"axis": "up", "angle": 9}}),
            "axis",
        ),
        (
            json!({"op": "insert", "sample": "bracket", "rotate": {"axis": "z"}}),
            "angle",
        ),
        (
            json!({"op": "place", "component": "Bracket-1"}),
            "'at' or a 'rotate'",
        ),
        (
            json!({"op": "place", "component": 7, "at": [0, 0, 0]}),
            "id 7",
        ),
        (
            json!({"op": "replace", "component": "Bracket-1", "component2": 1}),
            "needs one of",
        ),
        (
            json!({"op": "rename", "component": "Bracket-1", "name": " "}),
            "empty",
        ),
        (
            json!({"op": "fix", "component": "Bracket-1", "off": true}),
            "no field 'off'",
        ),
        // Part operations are for parts.
        (json!({"op": "sketch", "on": "top"}), "is an assembly"),
        (json!({"op": "bodies"}), "open_component"),
        (
            json!({"op": "set_material", "material": "Mild steel"}),
            "is an assembly",
        ),
        // A session is needed for these.
        (json!({"op": "insert", "part": "Bracket"}), "open together"),
        (
            json!({"op": "open_component", "component": "Bracket-1"}),
            "open together",
        ),
    ] {
        let e = error(&mut doc, op.clone());
        assert!(e.contains(says), "{op}: {e}");
    }
    // And assembly operations are for assemblies.
    let mut part = Document::default();
    for op in [
        json!({"op": "insert", "sample": "bracket"}),
        json!({"op": "components"}),
        json!({"op": "place", "component": 1, "at": [0, 0, 0]}),
    ] {
        let e = error(&mut part, op);
        assert!(
            e.contains("is a part") && e.contains("\"assembly\": true"),
            "{e}"
        );
    }
}

#[test]
fn an_assembly_is_saved_opened_and_exported() {
    let dir = temp("files");
    let part = dir.join("plate.peet").to_string_lossy().into_owned();
    let file = dir.join("pair.peet").to_string_lossy().into_owned();
    // A part file of our own: a 40 x 30 x 5 plate.
    let mut plate = Document::default();
    ok(
        &mut plate,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 30]}]}),
    );
    ok(
        &mut plate,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}),
    );
    ok(&mut plate, json!({"op": "save", "path": part}));

    let mut doc = assembly();
    let a = ok(&mut doc, json!({"op": "insert", "path": part}));
    // Called after its file.
    assert_eq!(a["component"]["name"], "plate-1");
    let b = ok(
        &mut doc,
        json!({"op": "insert", "path": part, "at": [0, 0, 5]}),
    );
    assert_eq!(b["component"]["max"], json!([40.0, 30.0, 10.0]));
    assert_eq!(doc.model.assembly().unwrap().definitions().count(), 1);

    ok(&mut doc, json!({"op": "save", "path": file}));
    assert!(!doc.is_modified());
    let mut again = Document::default();
    let opened = ok(&mut again, json!({"op": "open", "path": file}));
    assert_eq!(
        (opened["kind"].as_str(), opened["components"].as_u64()),
        (Some("assembly"), Some(2))
    );
    assert_eq!(again.model, doc.model);
    // The file is whole: the part's own file is not needed.
    std::fs::remove_file(&part).unwrap();
    let mut alone = Document::default();
    ok(&mut alone, json!({"op": "open", "path": file}));
    assert_eq!(alone.bodies.len(), 2);

    // An assembly in an assembly: a sub-assembly, placed as one thing.
    let mut top = assembly();
    let sub = ok(
        &mut top,
        json!({"op": "insert", "path": file, "at": [100, 0, 0]}),
    );
    assert_eq!(sub["component"]["part"], "pair");
    assert_eq!(sub["component"]["max"], json!([140.0, 30.0, 10.0]));
    let parts = ok(&mut top, json!({"op": "components"}));
    assert_eq!(parts["parts"][0]["kind"], "assembly");
    assert_eq!(top.bodies.len(), 2);

    // Exported where they are: two boxes stacked, 2 x 6000 mm³.
    let step = dir.join("pair.step").to_string_lossy().into_owned();
    let out = ok(&mut doc, json!({"op": "export", "path": step}));
    assert_eq!(
        (out["format"].as_str(), out["bodies"].as_u64()),
        (Some("step"), Some(2))
    );
    let mut read = Document::default();
    ok(&mut read, json!({"op": "import_step", "path": step}));
    let bodies = ok(&mut read, json!({"op": "bodies"}));
    let boxes = bodies["bodies"].as_array().unwrap();
    assert_eq!(boxes.len(), 2);
    assert_eq!(boxes[1]["max"], json!([40.0, 30.0, 10.0]));
    assert!((boxes[1]["volume_mm3"].as_f64().unwrap() - 6000.0).abs() < 1e-6);
    let stl = dir.join("pair.stl").to_string_lossy().into_owned();
    let out = ok(&mut doc, json!({"op": "export", "path": stl}));
    assert_eq!(out["triangles"], 24);
    let e = error(&mut doc, json!({"op": "export", "path": step, "body": 0}));
    assert!(e.contains("exported whole"), "{e}");
    let e = error(&mut doc, json!({"op": "export", "path": dir.join("a.dxf")}));
    assert!(e.contains("open_component"), "{e}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_part_is_opened_from_its_assembly_and_stored_back() {
    let mut host = Headless::default();
    let mut session = Session::default();
    let mut run =
        |session: &mut Session, op: Value| apply_session_json(&mut host, session, &op, Undo::Step);
    let ok = |reply: peet_ops::Reply| {
        assert!(reply.ok, "{}", reply.json);
        reply.json
    };
    let err = |reply: peet_ops::Reply| {
        assert!(!reply.ok, "{}", reply.json);
        reply.json["error"].as_str().unwrap().to_owned()
    };

    // A part open in the session, made a component of a new assembly beside it.
    ok(run(
        &mut session,
        json!({"op": "open_sample", "sample": "bracket"}),
    ));
    let part = session.current_id();
    let made = ok(run(
        &mut session,
        json!({"op": "new", "assembly": true, "keep": true}),
    ));
    let asm = session.current_id();
    assert_eq!(made["kind"], "assembly");
    ok(run(
        &mut session,
        json!({"op": "insert", "part": "Bracket"}),
    ));
    ok(run(
        &mut session,
        json!({"op": "insert", "part": part.0, "at": [0, 0, 50]}),
    ));
    assert_eq!(session.model.assembly().unwrap().definitions().count(), 1);
    let e = err(run(&mut session, json!({"op": "insert", "part": asm.0})));
    assert!(e.contains("of itself"), "{e}");
    let e = err(run(&mut session, json!({"op": "insert", "part": "Lid"})));
    assert!(e.contains("'Lid'"), "{e}");
    // The assembly has its own copy: the document it came from can go.
    ok(run(
        &mut session,
        json!({"op": "close", "document": part.0}),
    ));
    assert_eq!(session.bodies.len(), 2);
    let height = |doc: &Document| doc.visible_body_bounds().size().z;
    let before = height(&session);
    let stamp = session.bodies[0].stamp;

    // Open the part of a component: a document of its own, which is a part.
    let opened = run(
        &mut session,
        json!({"op": "open_component", "component": "Bracket-2"}),
    );
    assert!(opened.replaced);
    let opened = ok(opened);
    assert_eq!(
        (opened["part"].as_str(), opened["kind"].as_str()),
        (Some("Bracket"), Some("part"))
    );
    let editing = session.current_id();
    assert!(session.embedded.is_some() && !session.is_modified());
    assert_eq!(session.count(), 2);
    // Opening it again, from the other instance, goes to the same document.
    ok(run(
        &mut session,
        json!({"op": "open_component", "component": "Bracket-1", "document": asm.0}),
    ));
    assert_eq!((session.count(), session.current_id()), (2, editing));

    // Change it. The assembly doesn't change until the part is saved.
    ok(run(
        &mut session,
        json!({"op": "edit", "feature": "Extrude1", "depth": 12}),
    ));
    assert!(session.is_modified());
    assert_eq!(session.get(asm).unwrap().bodies[0].stamp, stamp);
    let e = err(run(&mut session, json!({"op": "close"})));
    assert!(e.contains("unsaved changes"), "{e}");
    let e = err(run(&mut session, json!({"op": "close", "document": asm.0})));
    assert!(e.contains("Bracket has unsaved changes"), "{e}");
    let e = err(run(
        &mut session,
        json!({"op": "save", "path": "bracket.peet"}),
    ));
    assert!(e.contains("leave 'path' out"), "{e}");
    let stored = run(&mut session, json!({"op": "save"}));
    assert!(stored.changed);
    assert_eq!(
        stored.json,
        json!({"ok": true, "op": "save", "stored": "Bracket", "in": "Assembly1"})
    );
    assert!(!session.is_modified());
    let assembly = session.get(asm).unwrap();
    assert_ne!(
        assembly.bodies[0].stamp, stamp,
        "both instances are rebuilt"
    );
    assert_eq!(assembly.undo_label(), Some("Edit Bracket"));
    assert!(Arc::ptr_eq(&assembly.bodies[0], &assembly.bodies[1]));
    // Nothing changed since: saving again is not another step.
    assert!(!run(&mut session, json!({"op": "save"})).changed);

    // Undone in the assembly, the instances are as before; the part document keeps its
    // edit and can be stored again.
    ok(run(&mut session, json!({"op": "undo", "document": asm.0})));
    assert_eq!(session.get(asm).unwrap().bodies[0].stamp, stamp);
    assert_eq!(height(session.get(asm).unwrap()), before);

    // A part whose components are gone can't be stored; closing the assembly closes
    // the parts opened from it.
    ok(run(
        &mut session,
        json!({"op": "edit", "feature": "Extrude1", "depth": 13}),
    ));
    for name in ["Bracket-1", "Bracket-2"] {
        ok(run(
            &mut session,
            json!({"op": "delete", "component": name, "document": asm.0}),
        ));
    }
    let e = err(run(&mut session, json!({"op": "save"})));
    assert!(e.contains("no longer in the assembly"), "{e}");
    let closed = ok(run(
        &mut session,
        json!({"op": "close", "document": asm.0, "discard": true}),
    ));
    assert_eq!(closed["closed"], "Assembly1");
    assert_eq!(session.count(), 1);
    assert!(session.embedded.is_none() && session.model.is_empty());
}

#[test]
fn the_applications_changes_to_an_assembly_are_made_by_operations() {
    // As the application's tools change an assembly (on a copy of the model), applied
    // as operations: the same model must come out.
    let mut host = Headless::default();
    let mut direct = Document::from_model(Model::new_assembly(), None);
    let mut through = Document::from_model(Model::new_assembly(), None);
    let bracket = Arc::new(peet_model::samples::bracket().0);
    let housing = Arc::new(peet_model::samples::housing().0);
    let far = Frame {
        origin: DVec3::new(0.0, 0.0, 80.0),
        ..Frame::WORLD
    };
    let mut key = 0;
    let mut change = |label: &str, f: &dyn Fn(&mut Model)| {
        direct.change(label, f);
        let mut new = through.model.clone();
        f(&mut new);
        key += 1;
        let done = apply_model(&mut host, &mut through, new, label, key);
        through.seal_history();
        assert_eq!(done.untranslated, None, "{label}: {:?}", done.ops);
        assert_eq!(through.model, direct.model, "{label}");
        done.ops
    };
    let ops = change("Insert", &|m| {
        let a = m.assembly_mut().unwrap();
        let d = a.define(bracket.clone());
        a.insert(d, Frame::WORLD);
        a.insert(d, far);
    });
    assert_eq!(ops.len(), 2);
    let second = peet_model::CompId(2);
    let ops = change("Change", &|m| {
        let c = m.assembly_mut().unwrap().component_mut(second).unwrap();
        c.placement.origin.x = 25.0;
        c.fixed = true;
        c.visible = false;
        c.suppressed = true;
        c.name = "Top".to_owned();
    });
    assert_eq!(ops.len(), 5, "{ops:?}");
    let ops = change("Replace", &|m| {
        let a = m.assembly_mut().unwrap();
        let d = a.define(housing.clone());
        a.replace(second, d);
    });
    assert!(matches!(
        &ops[..],
        [Op::Component {
            change: ComponentChange::Replace(InsertSource::Model(_)),
            ..
        }]
    ));
    let mut thicker = (*bracket).clone();
    thicker.name = "Thick bracket".to_owned();
    let thicker = Arc::new(thicker);
    let ops = change("Edit part", &|m| {
        let a = m.assembly_mut().unwrap();
        let d = a.components().next().unwrap().definition;
        a.set_model(d, thicker.clone());
    });
    assert!(matches!(&ops[..], [Op::SetPart { .. }]));
    let ops = change("Delete", &|m| {
        m.assembly_mut().unwrap().remove(second);
    });
    assert_eq!(
        ops,
        [Op::Component {
            component: CompSel::Id(second),
            change: ComponentChange::Delete
        }]
    );

    // Typed, as the application builds them.
    let reply = apply(
        &mut through,
        &Op::Insert {
            from: InsertSource::Model(housing),
            name: None,
            placing: Some(Placing::Frame(far)),
            fixed: None,
        },
        Undo::Step,
    );
    assert!(reply.ok, "{}", reply.json);
    assert_eq!(reply.json["component"]["at"], json!([0.0, 0.0, 80.0]));
}
