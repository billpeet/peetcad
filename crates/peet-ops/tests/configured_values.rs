//! Configurations stage 2, through the operations: any numeric value of a feature and any
//! sketch dimension can differ between configurations, without a parameter in between.
//!
//! The first test is the stage's exit criterion: a plate with a hole, in three
//! configurations that differ in two sketch dimensions, the hole's diameter and the
//! extrusion's depth. Each configuration's volume matches its hand calculation, and the
//! part survives save and reopen.
//!
//! **Hand calculation**: volume = (width · 50 − π · (diameter / 2)²) · depth
//!
//! | configuration | width | diameter | depth | volume (mm³) |
//! |---------------|-------|----------|-------|--------------|
//! | Default       | 80    | 12       | 8     | 31 095.22    |
//! | Long          | 120   | 12       | 8     | 47 095.22    |
//! | Thick         | 80    | 20       | 12    | 44 230.09    |

use std::f64::consts::PI;

use peet_document::Document;
use peet_kernel::validate::measure;
use peet_model::{Model, Slot, Status};
use peet_ops::{Configs, Headless, Op, Undo, apply_json, apply_model_scoped};
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

/// An 80 × 50 plate, 8 thick, with a Ø12 hole: `width` and `hole` are sketch dimensions.
fn plate() -> Document {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "sketch", "on": "top", "draw": [
            {"type": "rectangle", "from": [0, 0], "to": [80, 50], "as": "r"},
            {"type": "coincident", "of": ["r.bottom.start", "origin"]},
            {"type": "length", "of": ["r.bottom"], "value": 80, "name": "width"},
            {"type": "length", "of": ["r.right"], "value": 50},
            {"type": "circle", "center": [25, 25], "radius": 6, "as": "c"},
            {"type": "diameter", "of": ["c"], "value": 12, "name": "hole"},
        ]}),
    );
    ok(
        &mut doc,
        json!({"op": "extrude", "sketch": "Sketch1", "depth": 8}),
    );
    doc
}

/// The plate in the three configurations of the exit criterion, with `Default` active.
fn family() -> Document {
    let mut doc = plate();
    ok(&mut doc, json!({"op": "add_configuration", "name": "Long"}));
    ok(
        &mut doc,
        json!({"op": "set_dimension", "sketch": "Sketch1", "name": "width", "value": 120, "configurations": "this"}),
    );
    // Without making it active, and from another configuration.
    ok(
        &mut doc,
        json!({"op": "add_configuration", "name": "Thick", "copy": "Default"}),
    );
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Default"}),
    );
    ok(
        &mut doc,
        json!({"op": "set_dimension", "sketch": "Sketch1", "name": "hole", "value": 20, "configurations": "Thick"}),
    );
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": 12, "configurations": ["Thick"]}),
    );
    doc
}

fn volume(doc: &Document) -> f64 {
    for f in doc.model.features() {
        if let Some(Status::Failed(m)) = doc.status(f.id) {
            panic!("{} failed: {m}", f.name);
        }
    }
    let bodies = &doc.evaluation().bodies;
    bodies.iter().map(|b| measure::volume(&b.solid)).sum()
}

fn by_hand(width: f64, diameter: f64, depth: f64) -> f64 {
    (width * 50.0 - PI * (diameter / 2.0).powi(2)) * depth
}

#[track_caller]
fn assert_close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

#[test]
fn a_plate_family_differs_in_dimensions_and_depth_without_parameters() {
    let mut doc = family();
    let expected = [
        ("Default", by_hand(80.0, 12.0, 8.0), 31_095.22),
        ("Long", by_hand(120.0, 12.0, 8.0), 47_095.22),
        ("Thick", by_hand(80.0, 20.0, 12.0), 44_230.09),
    ];
    assert_close(volume(&doc), expected[0].1);
    // Twice round, so every configuration is also arrived at from another one.
    for (name, exact, rounded) in expected.iter().chain(&expected) {
        ok(
            &mut doc,
            json!({"op": "configuration", "configuration": name}),
        );
        assert_close(volume(&doc), *exact);
        assert!((exact - rounded).abs() < 0.005, "{name}: {exact}");
    }

    let q = ok(&mut doc, json!({"op": "configurations"}));
    assert_eq!(
        q["values"],
        json!({
            "Sketch1": {
                "width": {"Default": "80", "Long": "120", "Thick": "80"},
                "hole": {"Default": "12", "Long": "12", "Thick": "20"},
            },
            "Extrude1": {"depth": {"Default": "8", "Long": "8", "Thick": "12"}},
        })
    );

    // Saved and opened again, every configuration builds the same.
    let bytes = doc.save_bytes(true).unwrap();
    let opened = peet_io::document::open(&bytes).unwrap();
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    assert_eq!(opened.model, doc.model);
    let mut again = Document::from_opened(opened, None);
    again.finish_loading();
    for (name, exact, _) in &expected {
        ok(
            &mut again,
            json!({"op": "configuration", "configuration": name}),
        );
        assert_close(volume(&again), *exact);
    }
    // And as text, for diffs.
    let text = peet_io::document::to_text(&bytes).unwrap();
    let back = peet_io::document::open(&peet_io::document::from_text(&text).unwrap()).unwrap();
    assert_eq!(back.model, doc.model);
}

#[test]
fn left_out_a_value_changes_here_if_it_differs_and_everywhere_if_not() {
    let mut doc = family();
    let extrude = doc.model.features().nth(1).unwrap().id;
    let sketch = doc.model.features().next().unwrap().id;
    let depth = Slot::Field("depth".to_owned());
    let in_config = |doc: &Document, name: &str, feature, slot: &Slot| {
        let c = doc.model.configuration_named(name).unwrap().id;
        doc.model.value_in(feature, slot, c).unwrap().value
    };

    // The depth differs (Thick has its own): an edit is to this configuration.
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": 9}),
    );
    assert_eq!(in_config(&doc, "Default", extrude, &depth), 9.0);
    assert_eq!(in_config(&doc, "Long", extrude, &depth), 8.0);
    assert_eq!(in_config(&doc, "Thick", extrude, &depth), 12.0);
    // The 50 mm side doesn't: it changes everywhere.
    ok(
        &mut doc,
        json!({"op": "set_dimension", "sketch": "Sketch1", "name": "d2", "value": 60}),
    );
    let side = doc.model.slot_named(sketch, "d2").unwrap().slot;
    assert!(!doc.model.value_differs(sketch, &side));
    assert_eq!(in_config(&doc, "Thick", sketch, &side), 60.0);
    // "all" makes a value the same everywhere again.
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": 10, "configurations": "all"}),
    );
    assert!(!doc.model.value_differs(extrude, &depth));
    assert_eq!(in_config(&doc, "Thick", extrude, &depth), 10.0);
    // An expression can differ too, and follows that configuration's parameters.
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "t", "value": "5mm", "configurations": "all"}),
    );
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "t", "value": "7mm", "configurations": "Long"}),
    );
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": "2 * t", "configurations": ["Long", "Thick"]}),
    );
    assert_eq!(in_config(&doc, "Default", extrude, &depth), 10.0);
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Long"}),
    );
    assert_close(volume(&doc), (120.0 * 60.0 - PI * 36.0) * 14.0);
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Thick"}),
    );
    assert_close(volume(&doc), (80.0 * 60.0 - PI * 100.0) * 10.0);

    // A value given is set where it was asked for, even if it is what the active
    // configuration has already.
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": "2 * t", "configurations": "Default"}),
    );
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": "2 * t", "configurations": "all"}),
    );
    assert!(!doc.model.value_differs(extrude, &depth));
    let hole = doc.model.slot_named(sketch, "hole").unwrap().slot;
    ok(
        &mut doc,
        json!({"op": "set_dimension", "sketch": "Sketch1", "name": "hole", "value": 20, "configurations": "all"}),
    );
    assert!(!doc.model.value_differs(sketch, &hole));
    assert_eq!(in_config(&doc, "Default", sketch, &hole), 20.0);
    assert_close(volume(&doc), (80.0 * 60.0 - PI * 100.0) * 10.0);

    // Only numeric values can differ: the rest is the part's.
    let e = error(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "reverse": true, "configurations": "this"}),
    );
    assert!(e.contains("Only the numeric values"), "{e}");
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "reverse": true}),
    );
    let e = error(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": 3, "configurations": "Short"}),
    );
    assert!(e.contains("no configuration called 'Short'"), "{e}");
    // Undo takes a scoped change back whole.
    ok(
        &mut doc,
        json!({"op": "edit", "feature": "Extrude1", "depth": 4, "configurations": "this"}),
    );
    assert!(doc.model.value_differs(extrude, &depth));
    ok(&mut doc, json!({"op": "undo"}));
    assert!(!doc.model.value_differs(extrude, &depth));
}

/// A change a tool makes to a copy of the model, with the scope the application's
/// "this configuration only" gives its values.
fn tool(doc: &mut Document, this_only: bool, change: impl FnOnce(&mut Model)) -> Vec<Op> {
    let mut new = doc.model.clone();
    change(&mut new);
    let scope = this_only.then_some(Configs::This);
    let t = apply_model_scoped(
        &mut Headless::default(),
        doc,
        new,
        "Tool",
        1,
        scope.as_ref(),
    );
    doc.seal_history();
    assert_eq!(t.untranslated, None, "{:?}", t.ops);
    t.ops
}

#[test]
fn a_tools_change_to_values_takes_the_scope_it_is_given() {
    let mut doc = plate();
    ok(
        &mut doc,
        json!({"op": "add_configuration", "name": "Other"}),
    );
    let sketch = doc.model.features().next().unwrap().id;
    let extrude = doc.model.features().nth(1).unwrap().id;
    let default = doc.model.configuration_named("Default").unwrap().id;
    let depth = Slot::Field("depth".to_owned());
    let width = doc.model.slot_named(sketch, "width").unwrap().slot;
    let set_depth = |m: &mut Model, v: f64| {
        let e = m.feature_mut(extrude).unwrap().extrude_mut().unwrap();
        e.params.depth = peet_model::Scalar::new(v);
    };

    // This configuration only: the depth here, the direction everywhere.
    let ops = tool(&mut doc, true, |m| {
        set_depth(m, 10.0);
        m.feature_mut(extrude)
            .unwrap()
            .extrude_mut()
            .unwrap()
            .params
            .reverse = true;
    });
    assert!(
        matches!(
            &ops[..],
            [
                Op::Edit {
                    configurations: Some(Configs::This),
                    ..
                },
                Op::Edit {
                    configurations: None,
                    ..
                }
            ]
        ),
        "{ops:?}"
    );
    assert_eq!(
        doc.model.value_in(extrude, &depth, default).unwrap().value,
        8.0
    );
    assert!(
        doc.model
            .feature(extrude)
            .unwrap()
            .extrude()
            .unwrap()
            .params
            .reverse
    );

    // A sketch committed by the editor: its changed dimension, here only.
    let ops = tool(&mut doc, true, |m| {
        let s = &mut m.feature_mut(sketch).unwrap().sketch_mut().unwrap().sketch;
        let Slot::Dimension(id) = &width else {
            unreachable!()
        };
        s.constraint_mut(*id)
            .unwrap()
            .dimension
            .as_mut()
            .unwrap()
            .value = 100.0;
    });
    assert!(
        matches!(
            &ops[..],
            [Op::SetSketch {
                configurations: Some(Configs::This),
                ..
            }]
        ),
        "{ops:?}"
    );
    assert_eq!(
        doc.model.value_in(sketch, &width, default).unwrap().value,
        80.0
    );
    assert_close(volume(&doc), by_hand(100.0, 12.0, 10.0));

    // Without a scope: here where it differs already, everywhere where it doesn't.
    let ops = tool(&mut doc, false, |m| set_depth(m, 11.0));
    assert!(matches!(
        &ops[..],
        [Op::Edit {
            configurations: None,
            ..
        }]
    ));
    assert_eq!(
        doc.model.value_in(extrude, &depth, default).unwrap().value,
        8.0
    );
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Default"}),
    );
    assert_close(volume(&doc), by_hand(80.0, 12.0, 8.0));
}
