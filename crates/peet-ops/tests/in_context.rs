//! In-context references, as SolidWorks' external references: a component's part's sketch
//! following another component's geometry in the assembly it was made in, updated when that
//! assembly rebuilds, kept as it last was elsewhere, locked and broken.

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
    let dir = std::env::temp_dir().join(format!("peet-ic-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A plate 100 × 60 × 5 and a cover 40 × 20 × 2, saved in `dir`.
fn parts(dir: &std::path::Path) -> (String, String) {
    let plate = dir.join("plate.peet").to_string_lossy().into_owned();
    let cover = dir.join("cover.peet").to_string_lossy().into_owned();
    for (path, w, h, t) in [(&plate, 100, 60, 5), (&cover, 40, 20, 2)] {
        let mut doc = Document::default();
        ok(
            &mut doc,
            json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [w, h]}]}),
        );
        ok(&mut doc, json!({"op": "extrude", "sketch": "Sketch1", "depth": t}));
        ok(&mut doc, json!({"op": "save", "path": path}));
    }
    (plate, cover)
}

fn component<'a>(reply: &'a Value, name: &str) -> &'a Value {
    reply["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("{name} in {reply}"))
}

fn max_x(doc: &mut Document, name: &str) -> f64 {
    let all = ok(doc, json!({"op": "components"}));
    component(&all, name)["max"][0].as_f64().unwrap()
}

fn statuses(doc: &mut Document) -> Vec<String> {
    let listed = ok(doc, json!({"op": "external_references"}));
    listed["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["status"].as_str().unwrap().to_owned())
        .collect()
}

/// The plate fixed at the origin and the cover on it, its sketch's right side (line 4)
/// following the plate's right edge.
fn assembly(link: bool) -> (Document, std::path::PathBuf) {
    let dir = temp(if link { "linked" } else { "own" });
    let (plate, cover) = parts(&dir);
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(&mut doc, json!({"op": "insert", "path": plate, "name": "Plate-1", "fixed": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": cover, "name": "Cover-1", "at": [0, 0, 5], "fixed": true, "link": link}),
    );
    assert_eq!(max_x(&mut doc, "Cover-1"), 40.0);
    let made = ok(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "sketch": "Sketch1", "entity": 4,
            "from": {"component": "Plate-1", "edge": {"at": [100, 30, 5]}}}),
    );
    assert_eq!(made["entity"], 4, "{made}");
    assert_eq!(made["in_context"]["status"], "in_context", "{made}");
    assert_eq!(made["in_context"]["follows"], json!(["Plate-1"]), "{made}");
    (doc, dir)
}

#[test]
fn a_cover_follows_the_plate_it_was_made_on() {
    let (mut doc, _) = assembly(false);
    assert_eq!(max_x(&mut doc, "Cover-1"), 100.0, "the cover reaches the plate's edge");
    // The plate moves: the cover's sketch follows it in the cover's own coordinates.
    ok(&mut doc, json!({"op": "place", "component": "Plate-1", "at": [30, 0, 0]}));
    assert_eq!(max_x(&mut doc, "Cover-1"), 130.0);
    assert_eq!(statuses(&mut doc), ["in_context"]);

    // The part on its own keeps the geometry it last had, and says it is out of context.
    let part = doc
        .model
        .assembly()
        .unwrap()
        .components()
        .find(|c| c.name == "Cover-1")
        .map(|c| doc.model.assembly().unwrap().definition_of(c.id).unwrap().model.clone())
        .unwrap();
    let mut alone = Document::from_model((*part).clone(), None);
    assert_eq!(statuses(&mut alone), ["out_of_context"]);
    let bodies = ok(&mut alone, json!({"op": "bodies"}));
    assert_eq!(bodies["bodies"][0]["max"][0], 130.0, "{bodies}");

    // Locked: it stays where it is, in its context too, until unlocked.
    let locked = ok(&mut doc, json!({"op": "lock_references", "component": "Cover-1"}));
    assert_eq!(locked["changed"], 1);
    ok(&mut doc, json!({"op": "place", "component": "Plate-1", "at": [60, 0, 0]}));
    assert_eq!(max_x(&mut doc, "Cover-1"), 130.0);
    assert_eq!(statuses(&mut doc), ["locked"]);
    ok(&mut doc, json!({"op": "lock_references", "component": "Cover-1", "on": false}));
    assert_eq!(max_x(&mut doc, "Cover-1"), 160.0);

    // Undone, the cover is where it was before the unlock.
    ok(&mut doc, json!({"op": "undo"}));
    assert_eq!(max_x(&mut doc, "Cover-1"), 130.0);
    ok(&mut doc, json!({"op": "redo"}));
    assert_eq!(max_x(&mut doc, "Cover-1"), 160.0);

    // Broken: plain fixed geometry, which follows nothing.
    let broken = ok(&mut doc, json!({"op": "break_references", "component": "Cover-1"}));
    assert_eq!(broken["changed"], 1);
    ok(&mut doc, json!({"op": "place", "component": "Plate-1", "at": [0, 0, 0]}));
    assert_eq!(max_x(&mut doc, "Cover-1"), 160.0);
    assert!(statuses(&mut doc).is_empty());

    // Saved and opened again, everything is as it was.
    let bytes = peet_io::document::save(&doc.model, &Default::default(), None).unwrap();
    assert_eq!(peet_io::document::open(&bytes).unwrap().model, doc.model);
}

#[test]
fn the_reference_belongs_to_the_instance_it_was_made_in() {
    let (mut doc, _) = assembly(false);
    // A second cover, another instance of the same part, somewhere else.
    ok(
        &mut doc,
        json!({"op": "insert", "component": "Cover-1", "name": "Cover-2", "at": [0, 100, 5], "fixed": true}),
    );
    let listed = ok(&mut doc, json!({"op": "external_references"}));
    let by: Vec<(String, String)> = listed["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["component"].as_str().unwrap().to_owned(), r["status"].as_str().unwrap().to_owned()))
        .collect();
    assert!(by.contains(&("Cover-1".to_owned(), "in_context".to_owned())), "{listed}");
    assert!(by.contains(&("Cover-2".to_owned(), "out_of_context".to_owned())), "{listed}");
    // Moving the second cover changes nothing; moving the plate changes both (one part).
    ok(&mut doc, json!({"op": "place", "component": "Cover-2", "at": [-50, 100, 5]}));
    assert_eq!(max_x(&mut doc, "Cover-1"), 100.0);
    ok(&mut doc, json!({"op": "place", "component": "Plate-1", "at": [10, 0, 0]}));
    assert_eq!(max_x(&mut doc, "Cover-1"), 110.0);
    assert_eq!(max_x(&mut doc, "Cover-2"), 60.0);
}

#[test]
fn a_mated_part_follows_and_the_mates_are_solved_again() {
    // The cover is mated onto the plate's top face, and its right side follows the plate's
    // right edge; a block, mated against the cover's right face, moves with it.
    let dir = temp("mated");
    let (plate, cover) = parts(&dir);
    let block = dir.join("block.peet").to_string_lossy().into_owned();
    let mut b = Document::default();
    ok(
        &mut b,
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [10, 10]}]}),
    );
    ok(&mut b, json!({"op": "extrude", "sketch": "Sketch1", "depth": 10}));
    ok(&mut b, json!({"op": "save", "path": block}));
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(&mut doc, json!({"op": "insert", "path": plate, "name": "Plate-1", "fixed": true}));
    ok(&mut doc, json!({"op": "insert", "path": cover, "name": "Cover-1", "at": [0, 0, 5], "fixed": true}));
    ok(&mut doc, json!({"op": "insert", "path": block, "name": "Block-1", "at": [50, 0, 7]}));
    ok(
        &mut doc,
        json!({"op": "mate", "type": "coincident",
            "a": {"component": "Cover-1", "face": {"normal": [1, 0, 0]}},
            "b": {"component": "Block-1", "face": {"normal": [-1, 0, 0]}}}),
    );
    assert_eq!(component(&ok(&mut doc, json!({"op": "components"})), "Block-1")["min"][0], 40.0);
    ok(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "sketch": "Sketch1", "entity": 4,
            "from": {"component": "Plate-1", "edge": {"at": [100, 30, 5]}}}),
    );
    // The cover grew to the plate's edge, and the block's mate was solved again after it.
    assert_eq!(max_x(&mut doc, "Cover-1"), 100.0);
    assert_eq!(component(&ok(&mut doc, json!({"op": "components"})), "Block-1")["min"][0], 100.0);
}

#[test]
fn a_linked_part_is_written_when_the_assembly_is_saved() {
    let (mut doc, dir) = assembly(true);
    let file = dir.join("frame.peet");
    ok(&mut doc, json!({"op": "save", "path": file.to_string_lossy()}));
    // Saved under its own name: the context follows its title.
    ok(&mut doc, json!({"op": "place", "component": "Plate-1", "at": [20, 0, 0]}));
    assert_eq!(max_x(&mut doc, "Cover-1"), 120.0);
    let saved = ok(&mut doc, json!({"op": "save"}));
    let written = saved["parts_written"].to_string();
    assert!(written.contains("cover.peet"), "{saved}");
    // The cover's file has the new geometry: opened on its own, it is 120 wide.
    let bytes = std::fs::read(dir.join("cover.peet")).unwrap();
    let cover = peet_io::document::open(&bytes).unwrap().model;
    let mut alone = Document::from_model(cover, None);
    assert_eq!(ok(&mut alone, json!({"op": "bodies"}))["bodies"][0]["max"][0], 120.0);
    assert_eq!(statuses(&mut alone), ["out_of_context"]);
}

#[test]
fn what_is_refused() {
    let (mut doc, _) = assembly(false);
    let e = error(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "sketch": "Sketch1",
            "from": {"component": "Cover-1", "edge": {"at": [100, 30, 2]}}}),
    );
    assert!(e.contains("itself"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "sketch": "Sketch1", "entity": 1,
            "from": {"component": "Plate-1", "vertex": {"at": [100, 60, 5]}}}),
    );
    assert!(e.contains("point"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "sketch": "Extrude1",
            "from": {"component": "Plate-1", "vertex": {"at": [100, 60, 5]}}}),
    );
    assert!(e.contains("not a sketch"), "{e}");
}

#[test]
fn a_fixed_entity_made_to_follow_gives_up_its_fix() {
    // As the SolidWorks importer leaves an in-context relation's copy: fixed where it last was.
    let dir = temp("fixed");
    let (plate, _) = parts(&dir);
    let cover = dir.join("fixed.peet").to_string_lossy().into_owned();
    let mut c = Document::default();
    ok(
        &mut c,
        json!({"op": "sketch", "on": "top", "draw": [
            {"type": "rectangle", "from": [0, 0], "to": [40, 20], "as": "r"},
            {"type": "fix", "of": ["r.right"]}
        ]}),
    );
    ok(&mut c, json!({"op": "extrude", "sketch": "Sketch1", "depth": 2}));
    ok(&mut c, json!({"op": "save", "path": cover}));
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(&mut doc, json!({"op": "insert", "path": plate, "name": "Plate-1", "fixed": true}));
    ok(&mut doc, json!({"op": "insert", "path": cover, "name": "Cover-1", "at": [0, 0, 5], "fixed": true}));
    ok(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "sketch": "Sketch1", "entity": 4,
            "from": {"component": "Plate-1", "edge": {"at": [100, 30, 5]}}}),
    );
    assert_eq!(max_x(&mut doc, "Cover-1"), 100.0);
    assert!(
        ok(&mut doc, json!({"op": "status"}))["failures"].as_array().is_none_or(Vec::is_empty),
        "nothing fails"
    );
}

#[test]
fn a_dimension_of_the_part_followed_moves_the_in_context_sketch() {
    let dir = temp("dimension");
    let (_, cover) = parts(&dir);
    let plate = dir.join("wide.peet").to_string_lossy().into_owned();
    let mut p = Document::default();
    ok(
        &mut p,
        json!({"op": "sketch", "on": "top", "draw": [
            {"type": "rectangle", "from": [0, 0], "to": [100, 60]},
            {"type": "horizontal_distance", "of": [2, 3], "value": 100, "name": "Width"}
        ]}),
    );
    ok(&mut p, json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}));
    ok(&mut p, json!({"op": "save", "path": plate}));
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(&mut doc, json!({"op": "insert", "path": plate, "name": "Plate-1", "fixed": true}));
    ok(&mut doc, json!({"op": "insert", "path": cover, "name": "Cover-1", "at": [0, 0, 5], "fixed": true}));
    ok(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "sketch": "Sketch1", "entity": 4,
            "from": {"component": "Plate-1", "edge": {"at": [100, 30, 5]}}}),
    );
    assert_eq!(max_x(&mut doc, "Cover-1"), 100.0);
    // SolidWorks: the plate made wider, the cover's in-context sketch follows its edge.
    ok(&mut doc, json!({"op": "set", "name": "Width@Sketch1@Plate-1", "value": 150}));
    // (The plate's rectangle is held by nothing else: it grows both ways.)
    let edge = max_x(&mut doc, "Plate-1");
    assert!(edge > 120.0, "{edge}");
    assert_eq!(max_x(&mut doc, "Cover-1"), edge);
    assert_eq!(statuses(&mut doc), ["in_context"]);
}

fn max_z(doc: &mut Document, name: &str) -> f64 {
    let all = ok(doc, json!({"op": "components"}));
    component(&all, name)["max"][2].as_f64().unwrap()
}

#[test]
fn a_part_inside_a_sub_assembly_follows_in_the_top_assembly() {
    // The cover is in a sub-assembly; its reference is made in the top assembly, to the
    // plate there: SolidWorks' Edit Part of a part in a sub-assembly, in the top's context.
    let dir = temp("nested-top");
    let (plate, cover) = parts(&dir);
    let sub = dir.join("sub.peet").to_string_lossy().into_owned();
    let mut s = Document::default();
    ok(&mut s, json!({"op": "new", "assembly": true}));
    ok(
        &mut s,
        json!({"op": "insert", "path": cover, "name": "Cover-1", "fixed": true}),
    );
    ok(&mut s, json!({"op": "save", "path": sub}));
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Plate-1", "fixed": true}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": sub, "name": "Sub-1", "at": [0, 0, 5], "fixed": true}),
    );
    let made = ok(
        &mut doc,
        json!({"op": "in_context", "component": ["Sub-1", "Cover-1"], "sketch": "Sketch1", "entity": 4,
            "from": {"component": "Plate-1", "edge": {"at": [100, 30, 5]}}}),
    );
    assert_eq!(made["in_context"]["status"], "in_context", "{made}");
    assert_eq!(made["in_context"]["component"], "Sub-1 / Cover-1", "{made}");
    assert_eq!(max_x(&mut doc, "Sub-1"), 100.0);
    ok(
        &mut doc,
        json!({"op": "place", "component": "Plate-1", "at": [25, 0, 0]}),
    );
    assert_eq!(max_x(&mut doc, "Sub-1"), 125.0);
    assert_eq!(statuses(&mut doc), ["in_context"]);
    // Locked and broken through the path, as for a component of the assembly.
    let locked = ok(
        &mut doc,
        json!({"op": "lock_references", "component": ["Sub-1", "Cover-1"]}),
    );
    assert_eq!(locked["changed"], 1);
    assert_eq!(statuses(&mut doc), ["locked"]);
}

#[test]
fn a_part_follows_in_its_sub_assembly_when_the_top_assembly_rebuilds() {
    // The reference is made in the sub-assembly (its context): the top assembly rebuilds the
    // sub-assembly first, which updates it, as SolidWorks does.
    let dir = temp("nested-sub");
    let (_, cover) = parts(&dir);
    // A plate whose width is a dimension.
    let plate = dir.join("wide.peet").to_string_lossy().into_owned();
    let mut p = Document::default();
    ok(
        &mut p,
        json!({"op": "sketch", "on": "top", "draw": [
            {"type": "rectangle", "from": [0, 0], "to": [100, 60]},
            {"type": "horizontal_distance", "of": [2, 3], "value": 100, "name": "Width"}
        ]}),
    );
    ok(
        &mut p,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 5}),
    );
    ok(&mut p, json!({"op": "save", "path": plate}));
    let frame = dir.join("frame.peet").to_string_lossy().into_owned();
    let mut s = Document::default();
    ok(&mut s, json!({"op": "new", "assembly": true}));
    ok(
        &mut s,
        json!({"op": "insert", "path": plate, "name": "Plate-1", "fixed": true}),
    );
    ok(
        &mut s,
        json!({"op": "insert", "path": cover, "name": "Cover-1", "at": [0, 0, 5], "fixed": true}),
    );
    ok(&mut s, json!({"op": "save", "path": frame}));
    ok(
        &mut s,
        json!({"op": "in_context", "component": "Cover-1", "sketch": "Sketch1", "entity": 4,
            "from": {"component": "Plate-1", "edge": {"at": [100, 30, 5]}}}),
    );
    ok(&mut s, json!({"op": "save"}));
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": frame, "name": "Frame-1", "fixed": true}),
    );
    let listed = ok(&mut doc, json!({"op": "external_references"}));
    let r = &listed["references"][0];
    assert_eq!(r["component"], "Frame-1 / Cover-1", "{listed}");
    assert_eq!(
        r["status"], "in_context",
        "worked out by the sub-assembly: {listed}"
    );
    assert_eq!(r["context"], "frame.peet", "{listed}");
    assert_eq!(r["at"][0], 100.0, "{listed}");
    // The plate made wider in the sub-assembly: the cover follows when the top rebuilds.
    ok(
        &mut doc,
        json!({"op": "set", "name": "Width@Sketch1@Plate-1@Frame-1", "value": 150}),
    );
    let listed = ok(&mut doc, json!({"op": "external_references"}));
    let r = &listed["references"][0];
    let at = r["at"][0].as_f64().unwrap();
    assert!(
        at > 120.0,
        "the cover's side is the plate's new edge: {listed}"
    );
    assert_eq!(max_x(&mut doc, "Frame-1"), at, "{listed}");
    assert_eq!(r["status"], "in_context", "{listed}");
}

#[test]
fn a_sketch_follows_the_assemblys_own_plane() {
    // SolidWorks relates a part's sketch to the assembly's planes as to another part's.
    let (mut doc, _) = assembly(false);
    ok(
        &mut doc,
        json!({"op": "break_references", "component": "Cover-1"}),
    );
    let made = ok(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "sketch": "Sketch1", "entity": 10,
            "from": {"plane": "right"}}),
    );
    assert_eq!(made["in_context"]["on_assembly"], true, "{made}");
    // The cover moves 30 along: its left side stays on the assembly's right plane (x = 0).
    ok(
        &mut doc,
        json!({"op": "place", "component": "Cover-1", "at": [30, 0, 5]}),
    );
    let all = ok(&mut doc, json!({"op": "components"}));
    assert_eq!(component(&all, "Cover-1")["min"][0], 0.0, "{all}");
    assert_eq!(component(&all, "Cover-1")["max"][0], 130.0, "{all}");
}

#[test]
fn an_extrude_runs_up_to_another_parts_face_and_a_sketch_sits_on_one() {
    // The cover stands 10 below the plate; its extrusion runs up to the plate's top face.
    let dir = temp("planes");
    let (plate, cover) = parts(&dir);
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Plate-1", "fixed": true}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": cover, "name": "Cover-1", "at": [0, 0, -10], "fixed": true}),
    );
    let made = ok(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "feature": "Extrude1",
            "from": {"component": "Plate-1", "face": {"normal": [0, 0, 1]}}}),
    );
    assert_eq!(made["in_context"]["use"], "up_to", "{made}");
    assert_eq!(max_z(&mut doc, "Cover-1"), 5.0);
    ok(
        &mut doc,
        json!({"op": "place", "component": "Plate-1", "at": [0, 0, 20]}),
    );
    assert_eq!(max_z(&mut doc, "Cover-1"), 25.0);
    assert_eq!(statuses(&mut doc), ["in_context"]);
    // Broken, the end stays where it is.
    ok(
        &mut doc,
        json!({"op": "break_references", "component": "Cover-1"}),
    );
    assert_eq!(statuses(&mut doc), ["broken"]);
    ok(
        &mut doc,
        json!({"op": "place", "component": "Plate-1", "at": [0, 0, 0]}),
    );
    assert_eq!(max_z(&mut doc, "Cover-1"), 25.0);

    // A sketch on another part's face: the plate's top face, wherever the plate is.
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "new", "assembly": true}));
    ok(
        &mut doc,
        json!({"op": "insert", "path": plate, "name": "Plate-1", "fixed": true}),
    );
    ok(
        &mut doc,
        json!({"op": "insert", "path": cover, "name": "Cover-1", "fixed": true}),
    );
    ok(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "feature": "Sketch1",
            "from": {"component": "Plate-1", "face": {"normal": [0, 0, 1]}}}),
    );
    assert_eq!(max_z(&mut doc, "Cover-1"), 7.0);
    ok(
        &mut doc,
        json!({"op": "place", "component": "Plate-1", "at": [0, 0, 3]}),
    );
    assert_eq!(max_z(&mut doc, "Cover-1"), 10.0);
    let e = error(
        &mut doc,
        json!({"op": "in_context", "component": "Cover-1", "feature": "Sketch1",
            "from": {"component": "Plate-1", "edge": {"at": [100, 30, 5]}}}),
    );
    assert!(e.contains("flat face or a plane"), "{e}");
}
