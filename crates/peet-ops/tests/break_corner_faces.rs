//! A break corner on faces breaks every corner of each face, found again at each rebuild:
//! a notch's corners are broken where the notch is, and not where it is suppressed.

use std::f64::consts::FRAC_PI_4;

use peet_document::Document;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    reply.json
}

fn volume(doc: &mut Document) -> f64 {
    ok(doc, json!({"op": "mass"}))["total"]["volume_mm3"]
        .as_f64()
        .unwrap()
}

#[test]
fn every_corner_of_a_face_follows_the_part() {
    let mut doc = Document::default();
    for op in [
        json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [120, 80]}]}),
        json!({"op": "base_flange", "sketch": "Sketch1", "thickness": 2}),
        json!({"op": "sketch", "on": {"at": [60, 40, 2], "normal": [0, 0, 1]}, "name": "Notch", "draw": [{"type": "rectangle", "from": [50, 70], "to": [70, 90]}]}),
        json!({"op": "sheet_cut", "sketch": "Notch", "name": "Notch cut"}),
        json!({"op": "break_corner", "name": "All round", "size": 4, "edges": [], "faces": [{"at": [10, 10, 2], "normal": [0, 0, 1]}]}),
    ] {
        ok(&mut doc, op);
    }
    // What a 4 mm round takes from an outside corner, and gives an inside one, 2 mm thick
    let round = 16.0 * (1.0 - FRAC_PI_4) * 2.0;
    // Four outside corners of the plate, two of the notch's mouth, and its two inside
    // corners, which gain what the others lose
    let notched = 120.0 * 80.0 * 2.0 - 20.0 * 10.0 * 2.0 - 4.0 * round;
    assert!(
        (volume(&mut doc) - notched).abs() < 1e-3,
        "{}",
        volume(&mut doc)
    );

    ok(
        &mut doc,
        json!({"op": "add_configuration", "name": "Plain"}),
    );
    ok(&mut doc, json!({"op": "suppress", "feature": "Notch cut"}));
    let plain = 120.0 * 80.0 * 2.0 - 4.0 * round;
    assert!(
        (volume(&mut doc) - plain).abs() < 1e-3,
        "{}",
        volume(&mut doc)
    );
}

/// The x of the middle of each round a break corner makes.
fn rounds(doc: &mut Document, name: &str) -> Vec<f64> {
    let faces = ok(doc, json!({"op": "faces"}))["faces"].clone();
    faces
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["surface"] == "cylinder")
        .filter(|f| {
            f["made_by"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m["feature"] == name))
        })
        .map(|f| f["center"][0].as_f64().unwrap())
        .collect()
}

/// A face picked whole and cut in two later (here, by unsuppressing a cut before it, as a
/// configuration does) is the piece it was picked at: SolidWorks keeps each piece picked.
#[test]
fn a_face_picked_whole_is_the_piece_it_was_picked_at() {
    for (x, side) in [(10.0, -1.0), (110.0, 1.0)] {
        let mut doc = Document::default();
        for op in [
            json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [120, 80]}]}),
            json!({"op": "base_flange", "sketch": "Sketch1", "thickness": 2}),
            json!({"op": "edge_flange", "name": "Lip", "length": 30, "edge": {"at": [60, 80, 2]}}),
            // A hole in the lip's right end, which the right piece has around it and the
            // left not: by what is around them, the right piece is more like the whole face
            json!({"op": "sketch", "on": {"at": [60, 40, 2], "normal": [0, 0, 1]}, "name": "Hole", "draw": [{"type": "circle", "center": [100, 100], "radius": 3}]}),
            json!({"op": "sheet_cut", "sketch": "Hole", "name": "Hole cut"}),
            // A notch across the lip, which cuts its faces in two
            json!({"op": "sketch", "on": {"at": [60, 40, 2], "normal": [0, 0, 1]}, "name": "Notch", "draw": [{"type": "rectangle", "from": [40, 60], "to": [60, 200]}]}),
            json!({"op": "sheet_cut", "sketch": "Notch", "name": "Notch cut"}),
            json!({"op": "suppress", "feature": "Notch cut"}),
            json!({"op": "break_corner", "name": "Corners", "size": 3, "edges": [], "faces": [{"at": [x, 80, 20], "normal": [0, 1, 0]}]}),
        ] {
            ok(&mut doc, op);
        }
        // Whole: the lip's corners at both ends
        let whole = rounds(&mut doc, "Corners");
        assert!(
            whole.iter().any(|&c| c < 10.0) && whole.iter().any(|&c| c > 110.0),
            "{whole:?}"
        );
        ok(
            &mut doc,
            json!({"op": "suppress", "feature": "Notch cut", "on": false}),
        );
        // In two: the corners of the piece it was picked at, and none of the other
        let piece = rounds(&mut doc, "Corners");
        assert!(!piece.is_empty(), "no corners broken");
        assert!(
            piece.iter().all(|&c| (c - 50.0) * side > 10.0),
            "picked at x = {x}: {piece:?}"
        );
    }
}
