//! Configurations, through the operations.
//!
//! The first test is the exit criterion of stage 1 of the configurations plan: the sample
//! enclosure panel with three configurations (two thicknesses with their flange lengths,
//! and one with its cutouts suppressed). Each configuration's flat pattern DXF matches
//! its hand calculation, and the part survives save and reopen with every configuration.
//!
//! **Hand calculation** (90° bends, inner radius R = 2, K = 0.44, thickness t, flange L):
//!
//! - bend allowance BA = π/2 · (R + K·t)
//! - outside setback OSSB = R + t
//! - bend deduction BD = 2·OSSB − BA
//! - flat size = (200 + 2·L − 2·BD) × (150 + 2·L − 2·BD)
//!
//! | configuration | t   | L  | BA       | BD       | flat size               |
//! |---------------|-----|----|----------|----------|-------------------------|
//! | Default       | 1.5 | 25 | 4.178318 | 2.821682 | 244.356636 × 194.356636 |
//! | Thick         | 2   | 30 | 4.523893 | 3.476107 | 253.047787 × 203.047787 |
//! | Blank         | 1.5 | 25 | 4.178318 | 2.821682 | 244.356636 × 194.356636 |

use std::f64::consts::FRAC_PI_2;

use peet_document::Document;
use peet_io::dxf::{self, layer, read::Raw};
use peet_io::step::StepSchema;
use peet_model::{Model, Status};
use peet_ops::{Configs, Format, Headless, Op, Undo, apply, apply_json, apply_model, export_bytes};
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

fn enclosure() -> Document {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "open_sample", "sample": "enclosure"}),
    );
    doc
}

/// The enclosure panel with the three configurations of the exit criterion.
fn family() -> Document {
    let mut doc = enclosure();
    ok(
        &mut doc,
        json!({"op": "add_configuration", "name": "Thick", "comment": "2 mm sheet"}),
    );
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "thickness", "value": "2mm"}),
    );
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "flange", "value": "30mm"}),
    );
    ok(
        &mut doc,
        json!({"op": "add_configuration", "name": "Blank", "copy": "Default"}),
    );
    for cut in ["Sheet-Cut1", "Sheet-Cut2"] {
        ok(&mut doc, json!({"op": "suppress", "feature": cut}));
    }
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Default"}),
    );
    doc
}

fn assert_builds(doc: &Document) {
    for f in doc.model.features() {
        if let Some(Status::Failed(m)) = doc.status(f.id) {
            panic!(
                "{} failed in {}: {m}",
                f.name,
                doc.model.active_configuration().name
            );
        }
    }
}

/// The size of the flat pattern's outline and the number of cutout entities, read back
/// from the DXF of the active configuration.
fn flat(doc: &Document) -> ((f64, f64), usize) {
    let (bytes, _) = export_bytes(doc, Format::Dxf, None, StepSchema::default()).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let entities = dxf::read::entities(&text).expect("a well-formed DXF");
    let outline: Vec<&Raw> = entities
        .iter()
        .filter(|e| e.layer() == layer::OUTLINE)
        .collect();
    assert!(outline.iter().all(|e| e.kind == "LINE"));
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for e in &outline {
        for (x, y) in [(e.num(10), e.num(20)), (e.num(11), e.num(21))] {
            lo = [lo[0].min(x), lo[1].min(y)];
            hi = [hi[0].max(x), hi[1].max(y)];
        }
    }
    let cutouts = entities
        .iter()
        .filter(|e| e.layer() == layer::CUTOUTS)
        .count();
    ((hi[0] - lo[0], hi[1] - lo[1]), cutouts)
}

fn flat_size(t: f64, l: f64) -> (f64, f64) {
    let ba = FRAC_PI_2 * (2.0 + 0.44 * t);
    let bd = 2.0 * (2.0 + t) - ba;
    (200.0 + 2.0 * l - 2.0 * bd, 150.0 + 2.0 * l - 2.0 * bd)
}

/// A DXF holds six decimals, so a size read back from one is good to 1e-6.
#[track_caller]
fn assert_size(got: (f64, f64), want: (f64, f64)) {
    assert!(
        (got.0 - want.0).abs() < 2e-6 && (got.1 - want.1).abs() < 2e-6,
        "{got:?} != {want:?}"
    );
}

#[test]
fn enclosure_family_exports_each_configuration_to_its_hand_calculation() {
    let mut doc = family();
    // Five round holes, and four lines each for the window and the slot.
    let expected = [
        ("Default", (1.5, 25.0), (244.356636, 194.356636), 13),
        ("Thick", (2.0, 30.0), (253.047787, 203.047787), 13),
        ("Blank", (1.5, 25.0), (244.356636, 194.356636), 0),
    ];
    // Twice round, so every configuration is also arrived at from another one.
    for (name, (t, l), by_hand, cutouts) in expected.iter().chain(&expected) {
        ok(
            &mut doc,
            json!({"op": "configuration", "configuration": name}),
        );
        assert_builds(&doc);
        let (size, cut) = flat(&doc);
        assert_size(size, flat_size(*t, *l));
        assert_size(size, *by_hand);
        assert_eq!(cut, *cutouts, "{name}");
        // The folded panel keeps its outside size: 200 × 150, as high as its flanges.
        let bounds = doc.evaluation().bodies[0].solid.bounds();
        let size = bounds.size();
        assert!(
            (size.x - 200.0).abs() < 1e-9
                && (size.y - 150.0).abs() < 1e-9
                && (size.z - l).abs() < 1e-9,
            "{name}: {size:?}"
        );
    }

    // Saved and opened again, every configuration is there and builds the same.
    let bytes = doc.save_bytes(true).unwrap();
    let opened = peet_io::document::open(&bytes).unwrap();
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    assert_eq!(opened.model, doc.model);
    assert!(
        opened.bodies.is_some(),
        "the cache is the active configuration's"
    );
    let mut again = Document::from_opened(opened, None);
    again.finish_loading();
    assert_eq!(again.model.active_configuration().name, "Blank");
    for (name, (t, l), _, cutouts) in &expected {
        ok(
            &mut again,
            json!({"op": "configuration", "configuration": name}),
        );
        assert_builds(&again);
        let (size, cut) = flat(&again);
        assert_size(size, flat_size(*t, *l));
        assert_eq!(cut, *cutouts, "{name}");
    }
}

#[test]
fn a_configuration_is_built_without_making_it_active() {
    let doc = family();
    let thick = doc.model.configuration_named("Thick").unwrap().id;
    let other = Document::from_model(doc.model.with_configuration(thick), None);
    assert_builds(&other);
    assert_size(flat(&other).0, flat_size(2.0, 30.0));
    assert_eq!(doc.model.active_configuration().name, "Default");
}

#[test]
fn what_is_built_on_a_suppressed_feature_is_suppressed_with_it() {
    let mut doc = family();
    ok(
        &mut doc,
        json!({"op": "add_configuration", "name": "No Front"}),
    );
    // The front flange carries a sketch and the hole cut from it.
    let reply = ok(
        &mut doc,
        json!({"op": "suppress", "feature": "Edge-Flange1"}),
    );
    assert!(reply.get("failures").is_none(), "{reply}");
    let features = ok(&mut doc, json!({"op": "features"}));
    let by: Vec<(&str, &str)> = features["features"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| Some((f["name"].as_str()?, f["suppressed_by"].as_str()?)))
        .collect();
    assert_eq!(
        by,
        [("Sketch3", "Edge-Flange1"), ("Sheet-Cut2", "Sketch3")],
        "{features}"
    );
    let ((_, height), cutouts) = flat(&doc);
    let (_, both) = flat_size(1.5, 25.0);
    let bd = 2.0 * 3.5 - FRAC_PI_2 * (2.0 + 0.44 * 1.5);
    assert!((height - (both - 25.0 + bd)).abs() < 1e-6, "{height}");
    assert_eq!(cutouts, 12, "the flange's hole went with it");

    // Elsewhere nothing changed, and it comes back with the flange.
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Default"}),
    );
    assert_builds(&doc);
    assert_eq!(flat(&doc).1, 13);
    ok(
        &mut doc,
        json!({"op": "suppress", "feature": "Edge-Flange1", "on": false, "configurations": "No Front"}),
    );
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "No Front"}),
    );
    assert_builds(&doc);
    assert_eq!(flat(&doc).1, 13);
}

#[test]
fn scopes_and_queries() {
    let mut doc = family();
    let q = ok(&mut doc, json!({"op": "configurations"}));
    assert_eq!(q["active"], "Default");
    assert_eq!(
        q["configurations"],
        json!([
            {"name": "Default", "active": true},
            {"name": "Thick", "comment": "2 mm sheet"},
            {"name": "Blank"},
        ])
    );
    assert_eq!(
        q["suppressed_in"],
        json!({"Sheet-Cut1": ["Blank"], "Sheet-Cut2": ["Blank"]})
    );
    assert_eq!(
        q["parameters"],
        json!({
            "thickness": {"Default": "1.5mm", "Thick": "2mm", "Blank": "1.5mm"},
            "flange": {"Default": "25mm", "Thick": "30mm", "Blank": "25mm"},
        })
    );
    assert_eq!(
        ok(&mut doc, json!({"op": "status"}))["configuration"],
        "Default"
    );
    let p = ok(&mut doc, json!({"op": "parameters"}));
    assert_eq!(p["parameters"][0]["expression"], "1.5mm");
    assert_eq!(p["parameters"][0]["configurations"]["Thick"], "2mm");

    // Named configurations, without making them active.
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "flange", "value": "20mm", "configurations": ["Thick", "Blank"]}),
    );
    assert_eq!(doc.model.parameters.get("flange"), Some(25.0));
    // All of them: it no longer differs.
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "flange", "value": "22mm", "configurations": "all"}),
    );
    let q = ok(&mut doc, json!({"op": "configurations"}));
    assert!(q["parameters"].get("flange").is_none(), "{q}");
    ok(
        &mut doc,
        json!({"op": "suppress", "feature": "Sheet-Cut1", "on": false, "configurations": "all"}),
    );
    let q = ok(&mut doc, json!({"op": "configurations"}));
    assert_eq!(q["suppressed_in"], json!({"Sheet-Cut2": ["Blank"]}));
    // A new parameter exists everywhere.
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "gap", "value": "1mm"}),
    );
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Thick"}),
    );
    assert_eq!(doc.model.parameters.get("gap"), Some(1.0));
    assert_eq!(doc.model.parameters.get("flange"), Some(22.0));

    // Renaming, commenting and deleting.
    ok(
        &mut doc,
        json!({"op": "edit_configuration", "configuration": "Thick", "name": "2 mm", "comment": ""}),
    );
    let q = ok(&mut doc, json!({"op": "configurations"}));
    assert_eq!(
        q["configurations"][1],
        json!({"name": "2 mm", "active": true})
    );
    let reply = ok(
        &mut doc,
        json!({"op": "delete_configuration", "configuration": "2 mm"}),
    );
    assert_eq!(reply["configuration"], "Default", "its neighbour is active");
    assert_eq!(doc.model.parameters.get("thickness"), Some(1.5));
    assert!(!doc.model.parameter_differs("thickness"));

    // What can't be done says why, and changes nothing.
    let e = error(
        &mut doc,
        json!({"op": "configuration", "configuration": "Thin"}),
    );
    assert!(e.contains("Default, Blank"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "add_configuration", "name": "Blank"}),
    );
    assert!(e.contains("already called"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "suppress", "feature": "Sheet-Cut1", "configurations": ["Blank", "Thin"]}),
    );
    assert!(e.contains("no configuration called 'Thin'"), "{e}");
    let e = error(
        &mut doc,
        json!({"op": "set_parameter", "name": "gap", "value": "2 * nothing", "configurations": "Blank"}),
    );
    assert!(e.contains("configuration Blank"), "{e}");
    ok(
        &mut doc,
        json!({"op": "delete_configuration", "configuration": "Blank"}),
    );
    let e = error(
        &mut doc,
        json!({"op": "delete_configuration", "configuration": "Default"}),
    );
    assert!(e.contains("at least one"), "{e}");
}

#[test]
fn switching_is_not_an_undo_step_and_undo_returns_to_where_the_change_was_made() {
    let mut doc = enclosure();
    assert!(!doc.can_undo());
    ok(
        &mut doc,
        json!({"op": "add_configuration", "name": "Thick"}),
    );
    assert_eq!(doc.undo_label(), Some("Add Configuration Thick"));
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "thickness", "value": "2mm"}),
    );
    let thick = doc.model.active_configuration().id;
    let reply = apply_json(
        &mut doc,
        &json!({"op": "configuration", "configuration": "Default"}),
        Undo::Step,
    );
    assert!(reply.ok && reply.changed);
    assert_eq!(doc.undo_label(), Some("Set thickness"));
    assert!(doc.is_modified());
    // The active one again: nothing to do.
    let reply = apply_json(
        &mut doc,
        &json!({"op": "configuration", "configuration": "Default"}),
        Undo::Step,
    );
    assert!(reply.ok && !reply.changed);

    // A change made in Default, then one made in Thick, undone from Default.
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "flange", "value": "20mm", "configurations": "all"}),
    );
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Thick"}),
    );
    ok(&mut doc, json!({"op": "suppress", "feature": "Sheet-Cut1"}));
    ok(
        &mut doc,
        json!({"op": "configuration", "configuration": "Default"}),
    );
    ok(&mut doc, json!({"op": "undo"}));
    assert_eq!(doc.model.active_configuration().name, "Thick");
    assert!(!doc.model.features().any(|f| f.suppressed));
    ok(&mut doc, json!({"op": "undo"}));
    assert_eq!(doc.model.active_configuration().name, "Default");
    assert_eq!(doc.model.parameters.get("flange"), Some(25.0));
    assert_eq!(doc.model.parameter_in("thickness", thick), Some("2mm"));
    // Back to the part before it had configurations.
    ok(&mut doc, json!({"op": "undo"}));
    ok(&mut doc, json!({"op": "undo"}));
    assert_eq!(doc.model.configurations().len(), 1);
    assert!(!doc.can_undo());
    assert!(!doc.is_modified());
}

/// A change a tool makes to a copy of the model, applied through the operations.
fn tool(doc: &mut Document, change: impl FnOnce(&mut Model)) -> Vec<Op> {
    let mut new = doc.model.clone();
    change(&mut new);
    let t = apply_model(&mut Headless::default(), doc, new, "Tool", 1);
    doc.seal_history();
    assert_eq!(t.untranslated, None, "{:?}", t.ops);
    t.ops
}

#[test]
fn a_tools_change_is_to_this_configuration_where_it_differs_and_to_all_where_not() {
    let mut doc = family();
    let cut = doc
        .model
        .features()
        .find(|f| f.name == "Sheet-Cut1")
        .unwrap()
        .id;
    let flange = doc
        .model
        .features()
        .find(|f| f.name == "Edge-Flange1")
        .unwrap()
        .id;
    let thick = doc.model.configuration_named("Thick").unwrap().id;
    let blank = doc.model.configuration_named("Blank").unwrap().id;

    // Something that differs: the change stays in the active configuration.
    let ops = tool(&mut doc, |m| {
        m.feature_mut(cut).unwrap().suppressed = true;
        m.parameters.set("thickness", "1mm").unwrap();
    });
    assert!(ops.iter().all(|op| matches!(
        op,
        Op::Suppress {
            configurations: Configs::This,
            ..
        } | Op::SetParameter {
            configurations: Configs::This,
            ..
        }
    )));
    assert_eq!(doc.model.suppressed_in(cut, thick), Some(false));
    assert_eq!(doc.model.parameter_in("thickness", thick), Some("2mm"));

    // Something that doesn't: the change is to the part, in every configuration.
    let ops = tool(&mut doc, |m| {
        m.feature_mut(flange).unwrap().suppressed = true;
        m.parameters.set("gap", "1mm").unwrap();
    });
    assert!(ops.iter().all(|op| matches!(
        op,
        Op::Suppress {
            configurations: Configs::All,
            ..
        } | Op::SetParameter {
            configurations: Configs::All,
            ..
        }
    )));
    assert_eq!(doc.model.suppressed_in(flange, blank), Some(true));
    assert_eq!(doc.model.parameter_in("gap", blank), Some("1mm"));

    // Deleting what differs takes what was kept about it along.
    tool(&mut doc, |m| {
        m.remove(cut);
        m.parameters.remove("flange");
    });
    assert!(!doc.model.suppression_differs(cut));
    assert!(!doc.model.parameter_differs("flange"));

    // The typed operation, as the application sends it.
    let reply = apply(
        &mut doc,
        &Op::Suppress {
            feature: flange.into(),
            on: false,
            configurations: Configs::Named(vec!["Thick".to_owned()]),
        },
        Undo::Step,
    );
    assert!(reply.ok);
    assert_eq!(doc.model.suppressed_in(flange, thick), Some(false));
    assert!(doc.model.feature(flange).unwrap().suppressed);
}
