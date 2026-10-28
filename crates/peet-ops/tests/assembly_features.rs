//! Assembly features through the operations: component patterns whose count and spacing
//! are expressions with SolidWorks' addresses (`D1@LocalLPattern1`, `D3@LocalLPattern1`),
//! and assembly cuts, which cut the components in the assembly and not their parts.

use peet_document::Session;
use peet_ops::{Headless, Undo, apply_session_json};
use serde_json::{Value, json};

fn run(ops: &[Value]) -> (Headless, Session, Vec<Value>) {
    let mut host = Headless::default();
    let mut session = Session::default();
    let mut replies = Vec::new();
    for op in ops {
        let reply = apply_session_json(&mut host, &mut session, op, Undo::Step);
        assert!(reply.ok, "{op}: {}", reply.json);
        replies.push(reply.json);
    }
    (host, session, replies)
}

fn apply(host: &mut Headless, session: &mut Session, op: Value) -> Value {
    let reply = apply_session_json(host, session, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    reply.json
}

/// A tray `Length` long (named, in Sketch1) and a bracket, in an assembly with the bracket
/// patterned along the tray.
fn tray() -> Vec<Value> {
    vec![
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [1200, 40], "as": "r"}, {"type": "coincident", "of": ["r.bottom.start", "origin"]}, {"type": "length", "of": ["r.bottom"], "value": 1200, "name": "Length"}, {"type": "length", "of": ["r.left"], "value": 40}]}),
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 2}),
        json!({"op": "new", "keep": true}),
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [20, 40]}]}),
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 20}),
        json!({"op": "new", "assembly": true, "keep": true}),
        json!({"op": "insert", "part": 1, "name": "Tray-1", "fixed": true}),
        json!({"op": "insert", "part": 2, "name": "Bracket-1", "at": [0, 0, 2], "fixed": true}),
    ]
}

fn copies(reply: &Value) -> Vec<f64> {
    reply["components"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c.get("pattern").is_some())
        .map(|c| c["at"][0].as_f64().unwrap())
        .collect()
}

#[test]
fn a_patterns_count_and_spacing_follow_expressions_as_solidworks_equations_drive_them() {
    let mut ops = tray();
    ops.push(json!({"op": "component_pattern", "components": ["Bracket-1"], "type": "linear", "direction": "x", "spacing": 100, "count": 2, "name": "LocalLPattern1"}));
    let (mut host, mut session, _) = run(&ops);
    let h = &mut host;
    let s = &mut session;
    // Gear Tray Assembly's equations, as SolidWorks writes them.
    apply(h, s, json!({"op": "set", "name": "D1@LocalLPattern1", "value": "IIF(\"Length@Sketch1@Tray<1>.Part\" > 1100, 3, 2)"}));
    apply(h, s, json!({"op": "set", "name": "D3@LocalLPattern1", "value": "(\"Length@Sketch1@Tray-1\" - 20) / (\"D1@LocalLPattern1\" - 1)"}));
    let dims = apply(h, s, json!({"op": "dimensions", "name": "D3@LocalLPattern1"}));
    assert_eq!(dims["dimensions"][0]["value"], json!(590.0));
    let c = apply(h, s, json!({"op": "components"}));
    assert_eq!(copies(&c), vec![590.0, 1180.0]);
    // A shorter tray: one copy fewer, spread over the length.
    apply(h, s, json!({"op": "set", "name": "Length@Sketch1@Tray-1", "value": 900}));
    let c = apply(h, s, json!({"op": "components"}));
    assert_eq!(copies(&c), vec![880.0]);
    assert_eq!(c["patterns"][0]["count"]["value"], json!(2));
    // A plain number again.
    apply(h, s, json!({"op": "set", "name": "D1@LocalLPattern1", "value": 4}));
    let c = apply(h, s, json!({"op": "components"}));
    assert_eq!(c["patterns"][0]["count"], json!(4));
    // A count that is not a whole number is refused.
    let bad = apply_session_json(h, s, &json!({"op": "set", "name": "D1@LocalLPattern1", "value": 2.5}), Undo::Step);
    assert!(!bad.ok);
    let none = apply_session_json(h, s, &json!({"op": "set", "name": "D7@LocalLPattern1", "value": 2}), Undo::Step);
    assert!(!none.ok && none.json["error"].as_str().unwrap().contains("D3@LocalLPattern1"), "{}", none.json);
}

fn volumes(reply: &Value) -> Vec<f64> {
    reply["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["volume_mm3"].as_f64().unwrap())
        .collect()
}

#[test]
fn an_assembly_cut_cuts_the_components_in_its_scope_and_not_their_parts() {
    let (mut host, mut session, _) = run(&[
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}),
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}),
        json!({"op": "new", "assembly": true, "keep": true}),
        json!({"op": "insert", "part": 1, "name": "Base", "fixed": true}),
        json!({"op": "insert", "part": 1, "name": "Cover", "at": [0, 0, 5]}),
    ]);
    let (h, s) = (&mut host, &mut session);
    let made = apply(h, s, json!({"op": "assembly_cut", "on": "top", "draw": [{"type": "circle", "center": [50, 30], "radius": 10}], "end": "through_all", "flip": true, "components": ["Cover"]}));
    assert_eq!(made["cut"]["status"], json!("ok"), "{made}");
    let hole = std::f64::consts::PI * 100.0 * 5.0;
    let v = volumes(&apply(h, s, json!({"op": "mass"})));
    assert!((v[0] - 30000.0).abs() < 1e-6 && (v[1] - (30000.0 - hole)).abs() < 1e-6, "{v:?}");
    // Every component it reaches.
    apply(h, s, json!({"op": "edit_assembly_cut", "cut": "Cut-Extrude1", "components": null}));
    let v = volumes(&apply(h, s, json!({"op": "mass"})));
    assert!(v.iter().all(|x| (x - (30000.0 - hole)).abs() < 1e-6), "{v:?}");
    // The part is whole: opened by itself it has no hole.
    apply(h, s, json!({"op": "open_component", "component": "Base"}));
    let part = apply(h, s, json!({"op": "mass"}));
    assert!((part["total"]["volume_mm3"].as_f64().unwrap() - 30000.0).abs() < 1e-6, "{part}");
    apply(h, s, json!({"op": "close"}));
    // A sketch on a component's face, with an edge of another component converted: it
    // follows the cover when the cover moves.
    let made = apply(h, s, json!({"op": "assembly_cut", "on": {"component": "Cover", "face": {"normal": [0, 0, 1]}}, "convert": [{"component": "Base", "edge": {"at": [100, 30, 5]}}], "draw": [{"type": "rectangle", "from": [0, 0], "to": [20, 10]}], "depth": 2}));
    assert_eq!(made["cut"]["converted"][0]["component"], json!("Base"), "{made}");
    let v = volumes(&apply(h, s, json!({"op": "mass"})));
    assert!((v[1] - (30000.0 - hole - 400.0)).abs() < 1e-6, "{v:?}");
    // Suppressed: the components are whole again.
    apply(h, s, json!({"op": "edit_assembly_cut", "cut": "Cut-Extrude1", "suppressed": true}));
    apply(h, s, json!({"op": "delete", "cut": "Cut-Extrude2"}));
    let v = volumes(&apply(h, s, json!({"op": "mass"})));
    assert!(v.iter().all(|x| (x - 30000.0).abs() < 1e-6), "{v:?}");
    // One that reaches nothing says so.
    let made = apply(h, s, json!({"op": "assembly_cut", "on": "top", "draw": [{"type": "circle", "center": [500, 30], "radius": 10}], "depth": 5}));
    assert_eq!(made["cut"]["status"], json!("warning"), "{made}");
}

#[test]
fn an_assembly_cut_is_kept_in_the_file() {
    let dir = std::env::temp_dir().join(format!("peet-asmcut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("cut.peet").to_string_lossy().into_owned();
    let (mut host, mut session, _) = run(&[
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60]}]}),
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}),
        json!({"op": "new", "assembly": true, "keep": true}),
        json!({"op": "insert", "part": 1, "name": "Base", "fixed": true}),
        json!({"op": "assembly_cut", "on": "top", "draw": [{"type": "circle", "center": [50, 30], "radius": 10}], "end": "through_all", "flip": true, "name": "Hole"}),
        json!({"op": "save", "path": file}),
    ]);
    let (h, s) = (&mut host, &mut session);
    apply(h, s, json!({"op": "open", "path": file}));
    let c = apply(h, s, json!({"op": "components"}));
    assert_eq!(c["cuts"][0]["name"], json!("Hole"), "{c}");
    let v = volumes(&apply(h, s, json!({"op": "mass"})));
    assert!((v[0] - (30000.0 - std::f64::consts::PI * 500.0)).abs() < 1e-6, "{v:?}");
    // A second instance the cut doesn't reach: in a STEP file the cut one is a part of its
    // own.
    apply(h, s, json!({"op": "insert", "component": "Base", "at": [300, 0, 0], "name": "Other"}));
    let step = dir.join("cut.step").to_string_lossy().into_owned();
    let out = apply(h, s, json!({"op": "export", "path": step}));
    assert_eq!((out["bodies"].as_u64(), out["parts"].as_u64()), (Some(2), Some(2)), "{out}");
}
