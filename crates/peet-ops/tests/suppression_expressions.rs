//! Expression-driven suppression through the same operations as the CLI and UI.
use peet_document::Document;
use peet_model::Status;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    reply.json
}

fn part() -> Document {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"set_parameter","name":"width","value":"300mm"}),
    );
    ok(
        &mut doc,
        json!({"op":"sketch","on":"top","draw":[{"type":"rectangle","from":[0,0],"to":[20,10]}]}),
    );
    ok(
        &mut doc,
        json!({"op":"extrude","sketch":"Sketch1","depth":"if(width > 300mm, 8mm, 4mm)"}),
    );
    ok(
        &mut doc,
        json!({"op":"sketch","name":"Vent","on":{"feature":"Extrude1","side":"end"},"draw":[{"type":"circle","center":[5,5],"radius":1}]}),
    );
    ok(
        &mut doc,
        json!({"op":"cut","sketch":"Vent","end":"through_all"}),
    );
    doc
}

fn rule(doc: &mut Document, feature: &str, value: Value) -> Value {
    ok(
        doc,
        json!({"op":"set_suppression_expression","feature":feature,"value":value}),
    )
}

fn status(doc: &Document, feature: &str) -> Status {
    doc.status(doc.model.features().find(|f| f.name == feature).unwrap().id)
        .unwrap()
        .clone()
}

#[test]
fn threshold_changes_suppress_dependents_and_restore_cached_geometry() {
    let mut doc = part();
    rule(&mut doc, "Vent", json!("iif(width > 300mm, 0, 1)"));
    assert_eq!(status(&doc, "Vent"), Status::Suppressed);
    assert!(matches!(
        status(&doc, "Cut-Extrude1"),
        Status::SuppressedBy(_)
    ));
    assert!((peet_kernel::validate::measure::volume(&doc.bodies[0].solid) - 800.0).abs() < 1e-6);
    for width in ["301mm", "300mm", "400mm"] {
        ok(
            &mut doc,
            json!({"op":"set_parameter","name":"width","value":width}),
        );
        if width == "300mm" {
            assert_eq!(status(&doc, "Vent"), Status::Suppressed);
            assert!(
                (peet_kernel::validate::measure::volume(&doc.bodies[0].solid) - 800.0).abs() < 1e-6
            );
        } else {
            assert!(matches!(status(&doc, "Cut-Extrude1"), Status::Ok));
            let expected = 1600.0 - 8.0 * std::f64::consts::PI;
            assert!(
                (peet_kernel::validate::measure::volume(&doc.bodies[0].solid) - expected).abs()
                    < 1e-6
            );
        }
    }
    let query = ok(&mut doc, json!({"op":"feature","feature":"Vent"}));
    assert_eq!(query["suppressed"], false);
    assert_eq!(query["suppression_expression"], "iif(width > 300mm, 0, 1)");
}

#[test]
fn rules_follow_configuration_parameters_and_preserve_manual_flags_after_reopen() {
    let mut doc = part();
    ok(&mut doc, json!({"op":"suppress","feature":"Vent"}));
    rule(&mut doc, "Vent", json!("width <= 300mm"));
    ok(&mut doc, json!({"op":"add_configuration","name":"Wide"}));
    ok(
        &mut doc,
        json!({"op":"set_parameter","name":"width","value":"400mm"}),
    );
    assert!(matches!(status(&doc, "Cut-Extrude1"), Status::Ok));
    let bytes = doc.save_bytes(true).unwrap();
    let opened = peet_io::document::open(&bytes).unwrap();
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    let mut doc = Document::from_opened(opened, None);
    doc.finish_loading();
    assert!(matches!(status(&doc, "Cut-Extrude1"), Status::Ok));
    ok(
        &mut doc,
        json!({"op":"configuration","configuration":"Default"}),
    );
    assert_eq!(status(&doc, "Vent"), Status::Suppressed);
    ok(
        &mut doc,
        json!({"op":"configuration","configuration":"Wide"}),
    );
    assert!(matches!(status(&doc, "Cut-Extrude1"), Status::Ok));
    rule(&mut doc, "Vent", Value::Null);
    assert_eq!(status(&doc, "Vent"), Status::Suppressed);
    ok(&mut doc, json!({"op":"undo"}));
    assert!(matches!(status(&doc, "Cut-Extrude1"), Status::Ok));
    ok(&mut doc, json!({"op":"redo"}));
    assert_eq!(status(&doc, "Vent"), Status::Suppressed);
}

#[test]
fn invalid_rules_report_failures_and_recover_without_using_stale_geometry() {
    let mut doc = part();
    let before = doc.model.clone();
    let reply = apply_json(
        &mut doc,
        &json!({"op":"set_suppression_expression","feature":"Vent","value":"if("}),
        Undo::Step,
    );
    assert!(!reply.ok);
    assert_eq!(doc.model, before);
    for source in ["unknown > 0", "1mm", "1 / 0"] {
        rule(&mut doc, "Vent", json!(source));
        assert!(matches!(status(&doc, "Vent"), Status::Failed(_)));
        assert!(matches!(status(&doc, "Cut-Extrude1"), Status::Failed(_)));
    }
    rule(&mut doc, "Vent", json!("if(width > 0, 0, 1 / 0)"));
    assert!(matches!(status(&doc, "Cut-Extrude1"), Status::Ok));
    rule(&mut doc, "Vent", json!("width / (width - 300mm)"));
    assert!(matches!(status(&doc, "Vent"), Status::Failed(_)));
    ok(
        &mut doc,
        json!({"op":"set_parameter","name":"width","value":"400mm"}),
    );
    assert_eq!(status(&doc, "Vent"), Status::Suppressed);
    rule(&mut doc, "Vent", Value::Null);
    assert!(matches!(status(&doc, "Cut-Extrude1"), Status::Ok));
}

#[test]
fn model_diff_replays_rule_changes_and_clearing() {
    let mut doc = part();
    let id = doc.model.features().find(|f| f.name == "Vent").unwrap().id;
    let mut wanted = doc.model.clone();
    wanted.feature_mut(id).unwrap().suppression_expression = Some("width <= 300mm".into());
    let ops = peet_ops::diff(&doc, &wanted).unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].word(), "set_suppression_expression");
    let reply = peet_ops::apply(&mut doc, &ops[0], Undo::Step);
    assert!(reply.ok);
    assert_eq!(doc.model, wanted);
    wanted.feature_mut(id).unwrap().suppression_expression = None;
    for op in peet_ops::diff(&doc, &wanted).unwrap() {
        assert!(peet_ops::apply(&mut doc, &op, Undo::Step).ok);
    }
    assert_eq!(doc.model, wanted);
}

#[test]
fn conditional_sketch_dimensions_rebuild_with_parameter_changes() {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"width", "value":"300mm"}),
    );
    ok(
        &mut doc,
        json!({"op":"sketch", "on":"top", "draw":[
            {"type":"rectangle", "from":[0,0], "to":[10,10], "as":"r"},
            {"type":"coincident", "of":["r.bottom.start","origin"]},
            {"type":"length", "of":["r.bottom"], "name":"span", "value":"if(width > 300mm, 20mm, 10mm)"},
            {"type":"length", "of":["r.right"], "value":10}
        ]}),
    );
    ok(
        &mut doc,
        json!({"op":"extrude", "sketch":"Sketch1", "depth":4}),
    );
    assert!((peet_kernel::validate::measure::volume(&doc.bodies[0].solid) - 400.0).abs() < 1e-6);
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"width", "value":"301mm"}),
    );
    assert!((peet_kernel::validate::measure::volume(&doc.bodies[0].solid) - 800.0).abs() < 1e-6);
}
