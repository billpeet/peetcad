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
            "fixed": true, "freedom": 0, "status": "ok",
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
            link: false,
            absolute: false,
        },
        Undo::Step,
    );
    assert!(reply.ok, "{}", reply.json);
    assert_eq!(reply.json["component"]["at"], json!([0.0, 0.0, 80.0]));
}

/// A 40 x 30 x 5 plate with a hole of radius 4 through its middle, and a pin of radius 4
/// and length 20, as files in `dir`.
fn plate_and_pin(dir: &std::path::Path) -> (String, String) {
    let plate = dir.join("plate.peet").to_string_lossy().into_owned();
    let pin = dir.join("pin.peet").to_string_lossy().into_owned();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [
            {"type": "rectangle", "from": [0, 0], "to": [40, 30]},
            {"type": "circle", "center": [20, 15], "radius": 4},
        ]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}),
    );
    ok(&mut doc, json!({"op": "save", "path": plate}));
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "circle", "center": [0, 0], "radius": 4}]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 20}),
    );
    ok(&mut doc, json!({"op": "save", "path": pin}));
    (plate, pin)
}

fn at(reply: &Value) -> [f64; 3] {
    let p = reply["at"].as_array().unwrap_or_else(|| panic!("{reply}"));
    [0, 1, 2].map(|i| p[i].as_f64().unwrap())
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
}

#[test]
fn components_are_mated_by_faces_of_their_parts() {
    let dir = temp("mates");
    let (plate, pin) = plate_and_pin(&dir);
    let mut doc = assembly();
    ok(&mut doc, json!({"op": "insert", "path": plate}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Top", "at": [70, -20, 33],
               "rotate": {"axis": [1, 2, 3], "angle": 25}}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": pin, "name": "Pin", "at": [-30, 10, 40]}),
    );
    let up = json!({"normal": [0, 0, 1]});
    let down = json!({"normal": [0, 0, -1]});

    // The top plate's underside on the bottom plate's top: it comes down and lies flat.
    let made = ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident",
               "a": {"component": "plate-1", "face": up},
               "b": {"component": "Top", "face": down}}),
    );
    assert_eq!(
        made["mate"],
        json!({
            "id": 1, "name": "Coincident1", "type": "coincident",
            "a": {"component": "plate-1", "on": "face"},
            "b": {"component": "Top", "on": "face"},
            "status": "ok",
        })
    );
    assert_eq!(
        made["freedom"], 9,
        "the top plate slides and turns; the pin is free"
    );
    let moved = &made["moved"][0];
    assert_eq!(moved["name"], "Top");
    assert_eq!(moved["mates"], json!(["Coincident1"]));
    assert_eq!(moved["freedom"], 3, "it slides two ways and turns one");
    assert!(
        (moved["min"][2].as_f64().unwrap() - 5.0).abs() < 1e-6,
        "{moved}"
    );
    assert!(
        (moved["max"][2].as_f64().unwrap() - 10.0).abs() < 1e-6,
        "{moved}"
    );
    assert_eq!(doc.undo_label(), Some("Add Coincident1"));

    // The pin in the bottom plate's hole, its end flush with the underside.
    let hole = json!({"at": [24, 15, 2.5]});
    let round = json!({"at": [4, 0, 10]});
    let in_hole = ok(
        &mut doc,
        json!({"op": "mate", "type": "concentric", "name": "In hole",
               "a": {"component": "plate-1", "face": hole},
               "b": {"component": "Pin", "face": round}}),
    );
    assert_eq!(in_hole["freedom"], 5);
    let flush = ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "flip": true,
               "a": {"component": "plate-1", "face": down},
               "b": {"component": "Pin", "face": down}}),
    );
    assert_eq!(flush["mate"]["flip"], true);
    assert!(near(at(&flush["moved"][0]), [20.0, 15.0, 0.0]), "{flush}");
    assert_eq!(flush["freedom"], 4);

    // Square it up: two side faces the same way, then a gap between two others.
    ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "flip": true,
               "a": {"component": "plate-1", "face": {"normal": [-1, 0, 0]}},
               "b": {"component": "Top", "face": {"normal": [-1, 0, 0]}}}),
    );
    let gap = ok(
        &mut doc,
        json!({"op": "mate", "type": "distance", "distance": 12,
               "a": {"component": "plate-1", "face": {"normal": [0, -1, 0]}},
               "b": {"component": "Top", "face": {"normal": [0, 1, 0]}}}),
    );
    assert_eq!(gap["mate"]["distance"], 12.0);
    assert!(near(at(&gap["moved"][0]), [0.0, -42.0, 5.0]), "{gap}");
    assert_eq!(
        gap["freedom"], 1,
        "only the turn of the pin about its axis is left"
    );

    // A value changed moves the component; so does a parameter the value is made of.
    let edited = ok(
        &mut doc,
        json!({"op": "edit_mate", "mate": "Distance1", "distance": 2}),
    );
    assert!(near(at(&edited["moved"][0]), [0.0, -32.0, 5.0]), "{edited}");
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "gap", "value": "7mm"}),
    );
    let edited = ok(
        &mut doc,
        json!({"op": "edit_mate", "mate": "Distance1", "distance": "gap + 1"}),
    );
    assert_eq!(
        edited["mate"]["distance"],
        json!({"expression": "gap + 1", "value": 8.0})
    );
    let follows = ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "gap", "value": "9mm"}),
    );
    assert!(
        near(at(&follows["moved"][0]), [0.0, -40.0, 5.0]),
        "{follows}"
    );

    // Placing a mated component is a suggestion: it goes to the nearest place its
    // mates allow.
    let placed = ok(
        &mut doc,
        json!({"op": "place", "component": "Top", "at": [300, 200, 100]}),
    );
    assert!(
        near(at(&placed["component"]), [0.0, -40.0, 5.0]),
        "{placed}"
    );
    let slid = ok(
        &mut doc,
        json!({"op": "place", "component": "Pin", "rotate": {"axis": "z", "angle": 40}}),
    );
    assert_eq!(slid["component"]["rotate"]["angle"], 40.0);
    assert!(near(at(&slid["component"]), [20.0, 15.0, 0.0]));

    let list = ok(&mut doc, json!({"op": "mates"}));
    assert_eq!(list["mates"].as_array().unwrap().len(), 5);
    assert_eq!(list["freedom"], 1);
    // Which component it is that can still move: the pin, turning in its hole.
    let loose: Vec<(String, u64)> = ok(&mut doc, json!({"op": "components"}))["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_owned(),
                c["freedom"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        loose,
        [
            ("plate-1".to_owned(), 0),
            ("Top".to_owned(), 0),
            ("Pin".to_owned(), 1)
        ]
    );
    let status = ok(&mut doc, json!({"op": "status"}));
    assert_eq!(
        (status["mates"].as_u64(), status["freedom"].as_u64()),
        (Some(5), Some(1))
    );
    assert_eq!(status["failures"], json!([]));

    // A mate that can not hold with the others is a failure, and they still hold.
    let clash = ok(
        &mut doc,
        json!({"op": "mate", "type": "distance", "distance": 30,
               "a": {"component": "plate-1", "face": up},
               "b": {"component": "Top", "face": down}}),
    );
    assert_eq!(clash["mate"]["status"], "failed");
    assert!(
        clash["mate"]["message"]
            .as_str()
            .unwrap()
            .contains("hold together")
    );
    assert_eq!(clash["failures"][0]["name"], "Distance2");
    assert!(clash.get("moved").is_none());
    ok(&mut doc, json!({"op": "suppress", "mate": "Distance2"}));
    assert_eq!(ok(&mut doc, json!({"op": "status"}))["failures"], json!([]));
    ok(
        &mut doc,
        json!({"op": "rename", "mate": "Distance2", "name": "Later"}),
    );
    let gone = ok(&mut doc, json!({"op": "delete", "mate": "Later"}));
    assert_eq!(gone["deleted"], json!(["Later"]));

    // Saved and opened again, the mates are there and nothing moves.
    let file = dir.join("mated.peet").to_string_lossy().into_owned();
    ok(&mut doc, json!({"op": "save", "path": file}));
    let mut again = Document::default();
    ok(&mut again, json!({"op": "open", "path": file}));
    assert_eq!(again.model, doc.model);
    assert!(!again.is_modified());

    // A component deleted takes its mates along; undone, they are back and hold.
    ok(&mut doc, json!({"op": "delete", "component": "Pin"}));
    assert_eq!(
        ok(&mut doc, json!({"op": "mates"}))["mates"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    ok(&mut doc, json!({"op": "undo"}));
    let list = ok(&mut doc, json!({"op": "mates"}));
    assert_eq!(list["mates"].as_array().unwrap().len(), 5);
    assert!(
        list["mates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["status"] == "ok")
    );

    // Fastened: the pin keeps its place on the top plate, wherever that goes.
    ok(&mut doc, json!({"op": "delete", "mate": "In hole"}));
    ok(&mut doc, json!({"op": "delete", "mate": "Coincident2"}));
    let held = ok(
        &mut doc,
        json!({"op": "mate", "type": "fasten", "a": "Top", "b": {"component": "Pin"}}),
    );
    assert_eq!(
        held["mate"]["a"],
        json!({"component": "Top", "on": "component"})
    );
    assert_eq!(held["freedom"], 0);
    let carried = ok(
        &mut doc,
        json!({"op": "edit_mate", "mate": "Distance1", "distance": 0}),
    );
    let moved: Vec<&str> = carried["moved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(moved, ["Top", "Pin"]);
    assert!(
        near(at(&carried["moved"][1]), [20.0, 25.0, 0.0]),
        "{carried}"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn mistakes_with_mates_are_explained() {
    let dir = temp("mate-errors");
    let (plate, pin) = plate_and_pin(&dir);
    let mut doc = assembly();
    ok(&mut doc, json!({"op": "insert", "path": plate}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": pin, "at": [0, 0, 50]}),
    );
    let top = json!({"component": "plate-1", "face": {"normal": [0, 0, 1]}});
    let end = json!({"component": "pin-1", "face": {"normal": [0, 0, -1]}});
    for (op, says) in [
        (json!({"op": "mate", "a": top, "b": end}), "'type'"),
        (
            json!({"op": "mate", "type": "welded", "a": top, "b": end}),
            "coincident, concentric",
        ),
        (
            json!({"op": "mate", "type": "distance", "a": top, "b": end}),
            "'distance' field",
        ),
        (json!({"op": "mate", "type": "coincident", "a": top}), "'b'"),
        (
            json!({"op": "mate", "type": "coincident", "a": top, "b": top}),
            "Both ends are on plate-1",
        ),
        (
            json!({"op": "mate", "type": "coincident", "a": top, "b": "pin-1"}),
            "only fasten",
        ),
        (
            json!({"op": "mate", "type": "coincident", "a": top, "b": {"component": "Nut-1", "face": {"normal": [0, 0, 1]}}}),
            "pin-1",
        ),
        (
            json!({"op": "mate", "type": "coincident", "a": top,
                   "b": {"component": "pin-1", "face": {"normal": [1, 0, 0]}}}),
            "In pin-1 (its part, pin)",
        ),
        (
            json!({"op": "mate", "type": "coincident", "a": top,
                   "b": {"component": "pin-1", "face": {}, "edge": {}}}),
            "expected",
        ),
        (
            json!({"op": "mate", "type": "distance", "distance": "tall", "a": top, "b": end}),
            "distance",
        ),
        (
            json!({"op": "edit_mate", "mate": "Coincident1", "flip": true}),
            "no mates yet",
        ),
    ] {
        let e = error(&mut doc, op.clone());
        assert!(e.contains(says), "{op}: {e}");
    }
    // What the solve refuses is not an error of the operation: the mate is added and
    // says why, as a feature that fails to build does.
    let wrong = ok(
        &mut doc,
        json!({"op": "mate", "type": "concentric", "a": top, "b": end}),
    );
    assert_eq!(wrong["mate"]["status"], "failed");
    assert!(
        wrong["mate"]["message"]
            .as_str()
            .unwrap()
            .contains("two axes")
    );
    let e = error(
        &mut doc,
        json!({"op": "edit_mate", "mate": "Concentric1", "distance": 3}),
    );
    assert!(e.contains("no distance or angle"), "{e}");
    let e = error(&mut doc, json!({"op": "edit_mate", "mate": "Concentric1"}));
    assert!(e.contains("needs a"), "{e}");
    let e = error(&mut doc, json!({"op": "delete", "mate": "Weld"}));
    assert!(e.contains("Concentric1"), "{e}");
    let mut part = Document::default();
    let e = error(&mut part, json!({"op": "mates"}));
    assert!(e.contains("is a part"), "{e}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_applications_mates_are_made_by_operations() {
    let dir = temp("mate-diff");
    let (plate, _) = plate_and_pin(&dir);
    let part = Arc::new(
        peet_io::document::open(&std::fs::read(&plate).unwrap())
            .unwrap()
            .model,
    );
    // The faces, as a click in the application gives them.
    let built = Document::from_model((*part).clone(), None);
    let body = &built.evaluation().bodies[0];
    let face = |z: f64| {
        let id = body
            .solid
            .face_ids()
            .find(|f| {
                matches!(body.solid.face(*f).surface, peet_kernel::Surface::Plane(_))
                    && (body.face_center(*f).z - z).abs() < 1e-9
            })
            .unwrap();
        peet_model::MateGeom::Face(body.face_ref(id))
    };
    let end = |c: u32, z: f64| peet_model::MateEnd {
        path: vec![peet_model::CompId(c)],
        geom: Some(face(z)),
    };
    let whole = |c: u32| peet_model::MateEnd {
        path: vec![peet_model::CompId(c)],
        geom: None,
    };

    let mut host = Headless::default();
    let mut direct = Document::from_model(Model::new_assembly(), None);
    let mut through = Document::from_model(Model::new_assembly(), None);
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
    change("Insert", &|m| {
        let a = m.assembly_mut().unwrap();
        let d = a.define(part.clone());
        a.insert(d, Frame::WORLD);
        a.insert(
            d,
            Frame {
                origin: DVec3::new(5.0, 9.0, 60.0),
                ..Frame::WORLD
            },
        );
        a.insert(
            d,
            Frame {
                origin: DVec3::new(90.0, 0.0, 0.0),
                ..Frame::WORLD
            },
        );
    });
    use peet_model::{MateId, MateKind, Scalar};
    let ops = change("Mate", &|m| {
        let a = m.assembly_mut().unwrap();
        a.add_mate(
            MateKind::Distance(Scalar::new(3.0)),
            end(1, 5.0),
            end(2, 0.0),
        );
        let relative = Frame {
            origin: DVec3::new(90.0, 0.0, 0.0),
            ..Frame::WORLD
        };
        let held = a.add_mate(MateKind::Fasten(relative), whole(1), whole(3));
        a.mate_mut(held).unwrap().suppressed = true;
    });
    assert_eq!(ops.len(), 3, "{ops:?}");
    let ops = change("Change", &|m| {
        let mate = m.assembly_mut().unwrap().mate_mut(MateId(1)).unwrap();
        mate.kind = MateKind::Distance(Scalar::new(8.0));
        mate.flip = true;
        mate.name = "Gap".to_owned();
    });
    assert_eq!(ops.len(), 2, "{ops:?}");
    let ops = change("Delete", &|m| {
        let a = m.assembly_mut().unwrap();
        a.remove_mate(MateId(1));
        // A component goes, and its mate with it: one operation.
        a.remove(peet_model::CompId(3));
    });
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(through.model.assembly().unwrap().mates().count(), 0);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_component_is_dragged_as_far_as_its_mates_allow() {
    let dir = temp("drag");
    let (plate, pin) = plate_and_pin(&dir);
    let mut doc = assembly();
    // A plate on a pin through its hole: it can only swing round the pin.
    ok(
        &mut doc,
        json!({"op": "insert", "path": pin, "name": "Post"}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Arm", "at": [-20, -15, 0]}),
    );
    ok(
        &mut doc,
        json!({"op": "mate", "type": "concentric",
               "a": {"component": "Post", "face": {"at": [4, 0, 10]}},
               "b": {"component": "Arm", "face": {"at": [24, 15, 2.5]}}}),
    );
    let held = ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident", "flip": true,
               "a": {"component": "Post", "face": {"normal": [0, 0, -1]}},
               "b": {"component": "Arm", "face": {"normal": [0, 0, -1]}}}),
    );
    assert_eq!(held["freedom"], 1);

    // The middle of its short edge, 20 from the pin along X, pulled to 20 along Y: a
    // quarter turn.
    let swung = ok(
        &mut doc,
        json!({"op": "drag", "component": "Arm", "point": [40, 15, 0], "to": [0, 20, 0]}),
    );
    let turn = &swung["component"]["rotate"];
    assert!(
        (turn["angle"].as_f64().unwrap() - 90.0).abs() < 0.01,
        "{swung}"
    );
    assert_eq!(turn["axis"], json!([0.0, 0.0, 1.0]));
    assert!(swung.get("short_by").is_none(), "{swung}");
    assert_eq!(swung["freedom"], 1);
    assert_eq!(doc.undo_label(), Some("Drag Arm"));
    assert_eq!(ok(&mut doc, json!({"op": "status"}))["failures"], json!([]));

    // Pulled to where it can't go, it gets as near as it can and says how far that is.
    let short = ok(
        &mut doc,
        json!({"op": "drag", "component": "Arm", "point": [40, 15, 0], "to": [0, 50, 0]}),
    );
    assert!(
        (short["short_by"].as_f64().unwrap() - 30.0).abs() < 0.01,
        "{short}"
    );
    // Nothing to do is not a change.
    let again = apply_json(
        &mut doc,
        &json!({"op": "drag", "component": "Arm", "point": [40, 15, 0], "to": [0, 50, 0]}),
        Undo::Step,
    );
    assert!(again.ok && !again.changed, "{}", again.json);

    // A fixed component stays; one with no mates just goes there (its origin, if no
    // point is given), without turning.
    let e = error(
        &mut doc,
        json!({"op": "drag", "component": "Post", "to": [1, 2, 3]}),
    );
    assert!(e.contains("is fixed") && e.contains("\"on\": false"), "{e}");
    let e = error(&mut doc, json!({"op": "drag", "component": "Arm"}));
    assert!(e.contains("'to'"), "{e}");
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Loose", "at": [100, 0, 0],
               "rotate": {"axis": "x", "angle": 30}}),
    );
    let moved = ok(
        &mut doc,
        json!({"op": "drag", "component": "Loose", "to": [5, 6, 7]}),
    );
    assert!(near(at(&moved["component"]), [5.0, 6.0, 7.0]), "{moved}");
    assert_eq!(moved["component"]["rotate"]["angle"], 30.0);

    // The steps of one drag with the mouse are one undo step.
    let before = doc.model.clone();
    for step in 1..=5 {
        let reply = apply_json(
            &mut doc,
            &json!({"op": "drag", "component": "Loose", "to": [5 + step * 4, 6, 7]}),
            Undo::Group(77),
        );
        assert!(reply.ok && reply.changed, "{}", reply.json);
    }
    doc.seal_history();
    ok(&mut doc, json!({"op": "undo"}));
    assert_eq!(doc.model, before);
    std::fs::remove_dir_all(dir).ok();
}

/// Changes the depth of the plate in a part file, as another program (or another
/// session) would: the file is read, edited and written back.
fn set_depth(path: &str, depth: f64) {
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "open", "path": path}));
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": depth}),
    );
    ok(&mut doc, json!({"op": "save"}));
}

fn top_of(doc: &mut Document, component: usize) -> f64 {
    ok(doc, json!({"op": "components"}))["components"][component]["max"][2]
        .as_f64()
        .unwrap()
}

#[test]
fn a_linked_part_follows_its_file() {
    let dir = temp("links");
    let (plate, _) = plate_and_pin(&dir);
    let file = dir.join("stack.peet").to_string_lossy().into_owned();
    let mut doc = assembly();
    ok(&mut doc, json!({"op": "save", "path": file}));

    // Two components of one linked part: the assembly follows the file.
    let first = ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "link": true}),
    );
    assert_eq!(first["component"]["part"], "plate");
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "link": true, "at": [0, 0, 20]}),
    );
    let listed = ok(&mut doc, json!({"op": "components"}));
    let parts = listed["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1, "one part, linked once");
    assert_eq!(parts[0]["link_status"], "current");
    // Relative to the assembly's folder, where the plate's file also is.
    assert_eq!(parts[0]["link"], "plate.peet");
    assert!(std::path::Path::new(parts[0]["link_file"].as_str().unwrap()).is_absolute());
    // The same part inserted without a link is the assembly's own copy: another part.
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "at": [100, 0, 0]}),
    );
    assert_eq!(
        doc.model.assembly().unwrap().definitions().count(),
        2,
        "a copy is not the linked part"
    );
    ok(&mut doc, json!({"op": "save"}));
    assert_eq!(top_of(&mut doc, 0), 5.0);

    // The file changes behind the assembly's back: it says so, and reads it when asked.
    set_depth(&plate, 9.0);
    let stale = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(stale["parts"][0]["link_status"], "changed");
    assert!(
        stale["parts"][0]["link_message"]
            .as_str()
            .unwrap()
            .contains("update_links")
    );
    assert_eq!(top_of(&mut doc, 0), 5.0, "not until it is read again");
    let updated = apply_json(&mut doc, &json!({"op": "update_links"}), Undo::Step);
    assert!(updated.ok && updated.changed);
    assert_eq!(updated.json["updated"], json!(["plate"]));
    assert_eq!(doc.undo_label(), Some("Update Links"));
    assert_eq!((top_of(&mut doc, 0), top_of(&mut doc, 1)), (9.0, 29.0));
    assert_eq!(top_of(&mut doc, 2), 5.0, "the copy is not the file's");
    let again = apply_json(&mut doc, &json!({"op": "update_links"}), Undo::Step);
    assert!(again.ok && !again.changed, "{}", again.json);
    assert_eq!(again.json["updated"], json!([]));
    ok(&mut doc, json!({"op": "save"}));

    // Opening the assembly reads the linked parts as they are now, and that is not an
    // unsaved change.
    set_depth(&plate, 12.0);
    let mut opened = Document::default();
    let reply = ok(&mut opened, json!({"op": "open", "path": file}));
    assert_eq!(reply["updated"], json!(["plate"]));
    assert!(!opened.is_modified() && !opened.can_undo());
    assert_eq!(top_of(&mut opened, 0), 12.0);

    // The file gone: the assembly opens with the part as it last read it, and says so.
    let away = dir.join("elsewhere.peet");
    std::fs::rename(&plate, &away).unwrap();
    let mut alone = Document::default();
    let reply = ok(&mut alone, json!({"op": "open", "path": file}));
    assert!(
        reply["warnings"][0]
            .as_str()
            .unwrap()
            .contains("can't be found"),
        "{reply}"
    );
    assert_eq!(alone.bodies.len(), 3);
    assert_eq!(top_of(&mut alone, 0), 9.0, "as saved");
    let parts = ok(&mut alone, json!({"op": "components"}));
    assert_eq!(parts["parts"][0]["link_status"], "missing");
    assert!(
        parts["parts"][0]["link_message"]
            .as_str()
            .unwrap()
            .contains("unlink")
    );
    let again = apply_json(&mut alone, &json!({"op": "update_links"}), Undo::Step);
    assert!(again.ok && !again.changed);
    assert!(
        again.json["warnings"][0]
            .as_str()
            .unwrap()
            .contains("plate.peet")
    );
    // Made the assembly's own, it is a part like any other.
    // (By its id: the assembly has two parts called plate, the linked one and the copy.)
    let e = error(&mut alone, json!({"op": "unlink", "part": "plate"}));
    assert!(
        e.contains("More than one part") && e.contains("1 (plate), 2 (plate)"),
        "{e}"
    );
    ok(&mut alone, json!({"op": "unlink", "part": 1}));
    assert_eq!(alone.undo_label(), Some("Unlink plate"));
    let parts = ok(&mut alone, json!({"op": "components"}));
    assert!(parts["parts"][0].get("link").is_none());
    let e = error(&mut alone, json!({"op": "unlink", "part": 1}));
    assert!(e.contains("not linked"), "{e}");

    // Moved together to another folder, the part is found next to the assembly.
    let moved = dir.join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    std::fs::rename(&away, moved.join("plate.peet")).unwrap();
    let there = moved.join("stack.peet");
    std::fs::copy(&file, &there).unwrap();
    let mut found = Document::default();
    let reply = ok(&mut found, json!({"op": "open", "path": there}));
    assert!(reply.get("warnings").is_none(), "{reply}");
    assert_eq!(top_of(&mut found, 0), 12.0);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn parts_are_linked_to_files_and_mistakes_are_explained() {
    let dir = temp("link-ops");
    let (plate, pin) = plate_and_pin(&dir);
    let file = dir.join("asm.peet").to_string_lossy().into_owned();
    let mut doc = assembly();
    ok(&mut doc, json!({"op": "save", "path": file}));
    ok(&mut doc, json!({"op": "insert", "sample": "bracket"}));

    // The assembly's own part, written to a file of its own and linked to it.
    let out = dir.join("bracket.peet").to_string_lossy().into_owned();
    ok(
        &mut doc,
        json!({"op": "link", "part": "Bracket", "path": out}),
    );
    assert_eq!(doc.undo_label(), Some("Link Bracket"));
    let mut written = Document::default();
    let opened = ok(&mut written, json!({"op": "open", "path": out}));
    assert_eq!(
        (opened["kind"].as_str(), opened["bodies"].as_u64()),
        (Some("part"), Some(1))
    );
    let parts = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(parts["parts"][0]["link_status"], "current");
    // Linked to a file that is there, a part becomes what is in the file.
    ok(&mut doc, json!({"op": "unlink", "part": 1}));
    let swapped = ok(&mut doc, json!({"op": "link", "part": 1, "path": pin}));
    assert_eq!(swapped["failures"], Value::Null);
    let parts = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(parts["parts"][0]["name"], "pin");
    assert_eq!(parts["components"][0]["max"], json!([4.0, 4.0, 20.0]));

    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "at": [50, 0, 0]}),
    );
    for (op, says) in [
        (
            json!({"op": "insert", "sample": "bracket", "link": true}),
            "needs the part's 'path'",
        ),
        (
            json!({"op": "insert", "component": "pin-1", "link": true}),
            "needs the part's 'path'",
        ),
        (
            json!({"op": "insert", "path": file, "link": true}),
            "of itself",
        ),
        (
            json!({"op": "insert", "path": "nowhere.peet", "link": true}),
            "nowhere.peet",
        ),
        (json!({"op": "link", "part": "plate"}), "'path'"),
        (
            json!({"op": "link", "part": "plate", "path": file}),
            "own file",
        ),
        (
            json!({"op": "link", "part": "plate", "path": pin}),
            "already linked",
        ),
        (json!({"op": "link", "part": "lid", "path": out}), "1 (pin)"),
        (json!({"op": "unlink", "part": "plate"}), "not linked"),
    ] {
        let e = error(&mut doc, op.clone());
        assert!(e.contains(says), "{op}: {e}");
    }
    let mut part = Document::default();
    let e = error(&mut part, json!({"op": "update_links"}));
    assert!(e.contains("is a part"), "{e}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_linked_part_is_edited_in_its_own_file() {
    let dir = temp("link-session");
    let (plate, _) = plate_and_pin(&dir);
    let file = dir.join("asm.peet").to_string_lossy().into_owned();
    let mut host = Headless::default();
    let mut session = Session::default();
    let mut run =
        |session: &mut Session, op: Value| apply_session_json(&mut host, session, &op, Undo::Step);
    let ok = |reply: peet_ops::Reply| {
        assert!(reply.ok, "{}", reply.json);
        reply.json
    };
    ok(run(&mut session, json!({"op": "new", "assembly": true})));
    ok(run(&mut session, json!({"op": "save", "path": file})));
    ok(run(
        &mut session,
        json!({"op": "insert", "path": plate, "link": true}),
    ));
    ok(run(
        &mut session,
        json!({"op": "insert", "component": "plate-1", "at": [0, 0, 30]}),
    ));
    ok(run(&mut session, json!({"op": "save"})));
    let asm = session.current_id();

    // Its part opens as the file it is: a document with a path, not a working copy.
    let opened = ok(run(
        &mut session,
        json!({"op": "open_component", "component": "plate-2"}),
    ));
    assert_eq!(opened["part"], "plate.peet");
    assert!(opened["note"].as_str().unwrap().contains("its own file"));
    assert!(session.embedded.is_none());
    assert!(session.file.as_ref().unwrap().path.is_some());
    let part = session.current_id();
    // Again, from the other component: the same document.
    ok(run(
        &mut session,
        json!({"op": "open_component", "component": "plate-1", "document": asm.0}),
    ));
    assert_eq!((session.count(), session.current_id()), (2, part));

    // Saved, the file is written and the assembly that links to it follows, as a step
    // of its own that can be undone there.
    ok(run(
        &mut session,
        json!({"op": "edit", "feature": "Extrude1", "depth": 8}),
    ));
    let saved = ok(run(&mut session, json!({"op": "save"})));
    assert_eq!(saved["updated_in"], json!(["asm.peet"]));
    let assembly = session.get(asm).unwrap();
    assert_eq!(assembly.undo_label(), Some("Update plate"));
    assert!(assembly.is_modified());
    let tops: Vec<f64> = assembly
        .model
        .assembly()
        .unwrap()
        .components()
        .map(|c| assembly.component_bounds(c.id).max.z)
        .collect();
    assert_eq!(tops, [8.0, 38.0]);
    // Saving it again changes nothing there.
    let saved = ok(run(&mut session, json!({"op": "save"})));
    assert!(saved.get("updated_in").is_none(), "{saved}");
    std::fs::remove_dir_all(dir).ok();
}

/// The links of an assembly's file, as the file has them.
fn links_in_file(path: &std::path::Path) -> Vec<(String, bool)> {
    let model = peet_io::document::open(&std::fs::read(path).unwrap())
        .unwrap()
        .model;
    model
        .assembly()
        .unwrap()
        .definitions()
        .filter_map(|d| d.link.as_ref().map(|l| (l.path.clone(), l.relative)))
        .collect()
}

#[test]
fn links_are_relative_to_the_assemblys_folder() {
    let dir = temp("relative");
    let (made, pin) = plate_and_pin(&dir);
    // A job folder with the assembly in one folder and its parts in another.
    let job = dir.join("job");
    std::fs::create_dir_all(job.join("asm")).unwrap();
    std::fs::create_dir_all(job.join("parts")).unwrap();
    let plate = job.join("parts").join("plate.peet");
    std::fs::rename(&made, &plate).unwrap();
    let file = job.join("asm").join("stack.peet");

    // Linked before the assembly has a file: there is nothing to be relative to yet.
    let mut doc = assembly();
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "link": true}),
    );
    let before = ok(&mut doc, json!({"op": "components"}));
    assert!(std::path::Path::new(before["parts"][0]["link"].as_str().unwrap()).is_absolute());
    // Saved, the link is written from the assembly's folder; an absolute one as it is.
    ok(
        &mut doc,
        json!({"op": "insert", "path": pin, "link": true, "absolute": true, "at": [80, 0, 0]}),
    );
    ok(&mut doc, json!({"op": "save", "path": file}));
    let pin_full = std::path::absolute(&pin)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        links_in_file(&file),
        [
            ("../parts/plate.peet".to_owned(), true),
            (pin_full.clone(), false)
        ]
    );
    let listed = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(listed["parts"][0]["link"], "../parts/plate.peet");
    assert_eq!(listed["parts"][1]["link"], pin_full);
    assert_eq!(listed["parts"][0]["link_status"], "current");
    assert!(!doc.is_modified());

    // Opened again it is the same assembly, and not changed by being opened.
    let mut again = Document::default();
    let reply = ok(&mut again, json!({"op": "open", "path": file}));
    assert!(
        reply.get("warnings").is_none() && reply.get("updated").is_none(),
        "{reply}"
    );
    assert_eq!(again.model, doc.model);
    assert!(!again.is_modified());

    // The whole job copied elsewhere: the copy's assembly follows the copy's parts, not
    // the originals, though the full path stays where it pointed.
    let copy = dir.join("backup");
    for folder in ["asm", "parts"] {
        std::fs::create_dir_all(copy.join(folder)).unwrap();
    }
    std::fs::copy(&file, copy.join("asm").join("stack.peet")).unwrap();
    std::fs::copy(&plate, copy.join("parts").join("plate.peet")).unwrap();
    set_depth(
        &copy.join("parts").join("plate.peet").to_string_lossy(),
        11.0,
    );
    let mut copied = Document::default();
    let reply = ok(
        &mut copied,
        json!({"op": "open", "path": copy.join("asm").join("stack.peet")}),
    );
    assert_eq!(reply["updated"], json!(["plate"]));
    assert_eq!(top_of(&mut copied, 0), 11.0);
    let parts = ok(&mut copied, json!({"op": "components"}));
    assert!(
        parts["parts"][0]["link_file"]
            .as_str()
            .unwrap()
            .contains("backup"),
        "{parts}"
    );
    assert_eq!(parts["parts"][1]["link_file"], pin_full);
    assert_eq!(top_of(&mut doc, 0), 5.0, "the original is as it was");

    // Saved somewhere else, the assembly still means the same part files: its relative
    // link is written from the new place.
    let elsewhere = dir.join("stack-copy.peet");
    ok(&mut doc, json!({"op": "save", "path": elsewhere}));
    assert_eq!(links_in_file(&elsewhere)[0].0, "job/parts/plate.peet");
    let mut moved = Document::default();
    ok(&mut moved, json!({"op": "open", "path": elsewhere}));
    assert_eq!(moved.model, doc.model);

    // A part of the assembly's own, linked: relative by default, or a full path.
    ok(
        &mut doc,
        json!({"op": "insert", "sample": "bracket", "at": [0, 100, 0]}),
    );
    let out = dir.join("bracket.peet");
    ok(
        &mut doc,
        json!({"op": "link", "part": "Bracket", "path": out}),
    );
    ok(&mut doc, json!({"op": "save"}));
    assert_eq!(
        links_in_file(&elsewhere)[2],
        ("bracket.peet".to_owned(), true)
    );
    ok(&mut doc, json!({"op": "unlink", "part": "Bracket"}));
    ok(
        &mut doc,
        json!({"op": "link", "part": "Bracket", "path": out, "absolute": true}),
    );
    ok(&mut doc, json!({"op": "save"}));
    assert!(!links_in_file(&elsewhere)[2].1);
    let e = error(
        &mut doc,
        json!({"op": "insert", "sample": "bracket", "absolute": true}),
    );
    assert!(e.contains("\"link\": true"), "{e}");
    std::fs::remove_dir_all(dir).ok();
}
