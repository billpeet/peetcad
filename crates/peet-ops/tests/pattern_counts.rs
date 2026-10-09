//! Pattern counts as numeric expressions, including configuration overrides.
use peet_document::Document;
use peet_kernel::validate::measure;
use peet_model::Status;
use peet_ops::{Undo, apply_json};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    reply.json
}

fn plate() -> Document {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"width", "value":"299mm"}),
    );
    ok(
        &mut doc,
        json!({"op":"sketch", "on":"top", "draw":[{"type":"rectangle", "from":[-50,-50], "to":[50,50]}]}),
    );
    ok(
        &mut doc,
        json!({"op":"extrude", "sketch":"Sketch1", "depth":4}),
    );
    ok(
        &mut doc,
        json!({"op":"sketch", "name":"Seed", "on":{"feature":"Extrude1", "side":"end"}, "draw":[{"type":"circle", "center":[15,0], "radius":1}]}),
    );
    ok(
        &mut doc,
        json!({"op":"cut", "sketch":"Seed", "end":"through_all"}),
    );
    doc
}

fn row(doc: &mut Document, count: Value) {
    ok(
        doc,
        json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "spacing":5, "count":count}),
    );
}

fn volume(doc: &Document, holes: u32) {
    let expected = 40000.0 - f64::from(holes) * 4.0 * std::f64::consts::PI;
    let actual: f64 = doc.bodies.iter().map(|b| measure::volume(&b.solid)).sum();
    assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
}

fn status(doc: &Document, name: &str) -> Status {
    let id = doc.model.features().find(|f| f.name == name).unwrap().id;
    doc.status(id).unwrap().clone()
}

#[test]
fn conditional_linear_count_rebuilds_at_the_threshold_and_accepts_edits() {
    let mut doc = plate();
    row(&mut doc, json!("iif(width > 299mm, 3, 2)"));
    volume(&doc, 2);
    for width in ["300mm", "299mm", "400mm"] {
        ok(
            &mut doc,
            json!({"op":"set_parameter", "name":"width", "value":width}),
        );
        assert_eq!(status(&doc, "LPattern1"), Status::Ok);
        volume(&doc, if width == "299mm" { 2 } else { 3 });
    }
    let query = ok(&mut doc, json!({"op":"feature", "feature":"LPattern1"}));
    assert_eq!(
        query["fields"]["count"]["expression"],
        "iif(width > 299mm, 3, 2)"
    );
    assert_eq!(query["fields"]["count"]["value"], 3.0);
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "count":"2 + int(width / 100mm)"}),
    );
    volume(&doc, 6);
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "count":4}),
    );
    volume(&doc, 4);
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"width", "value":"300mm"}),
    );
    volume(&doc, 4);
}

#[test]
fn both_grid_counts_and_circular_counts_follow_parameters() {
    let mut doc = plate();
    ok(
        &mut doc,
        json!({"op":"linear_pattern", "features":["Cut-Extrude1"], "direction":"x", "spacing":5,
        "count":"if(width > 299mm, 3, 2)", "direction2":"y", "spacing2":5, "count2":"if(width > 299mm, 3, 2)"}),
    );
    volume(&doc, 4);
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"width", "value":"300mm"}),
    );
    volume(&doc, 9);
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "count2":2}),
    );
    volume(&doc, 6);
    let mut doc = plate();
    ok(
        &mut doc,
        json!({"op":"circular_pattern", "features":["Cut-Extrude1"], "axis":"z", "count":"if(width > 299mm, 6, 4)", "angle":360}),
    );
    volume(&doc, 4);
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"width", "value":"300mm"}),
    );
    volume(&doc, 6);
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"CirPattern1", "count":"int(width / 100mm) + 2"}),
    );
    volume(&doc, 5);
}

#[test]
fn count_expressions_survive_save_reopen_and_configuration_overrides() {
    let mut doc = plate();
    row(&mut doc, json!("if(width > 299mm, 3, 2)"));
    ok(&mut doc, json!({"op":"add_configuration", "name":"Wide"}));
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"width", "value":"300mm"}),
    );
    volume(&doc, 3);
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "count":"int(width / 100mm) + 2", "configurations":"this"}),
    );
    volume(&doc, 5);
    let configs = ok(&mut doc, json!({"op":"configurations"}));
    assert!(configs.to_string().contains("int(width / 100mm) + 2"));
    let bytes = doc.save_bytes(true).unwrap();
    let opened = peet_io::document::open(&bytes).unwrap();
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    let mut doc = Document::from_opened(opened, None);
    doc.finish_loading();
    volume(&doc, 5);
    ok(
        &mut doc,
        json!({"op":"configuration", "configuration":"Default"}),
    );
    volume(&doc, 2);
    ok(
        &mut doc,
        json!({"op":"configuration", "configuration":"Wide"}),
    );
    volume(&doc, 5);
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "count":4, "configurations":"all"}),
    );
    volume(&doc, 4);
    ok(
        &mut doc,
        json!({"op":"configuration", "configuration":"Default"}),
    );
    volume(&doc, 4);
}

#[test]
fn invalid_count_results_fail_without_rounding_or_making_copies_and_recover() {
    let mut doc = plate();
    row(&mut doc, json!("if(width > 0, 3, 2)"));
    for value in [json!(2.5), json!(1), json!(0), json!(-2), json!(10001)] {
        ok(
            &mut doc,
            json!({"op":"edit", "feature":"LPattern1", "count":value}),
        );
        assert!(matches!(status(&doc, "LPattern1"), Status::Failed(_)));
        volume(&doc, 1);
    }
    for value in ["1mm", "1deg", "unknown", "1 / 0"] {
        let before = doc.model.clone();
        let reply = apply_json(
            &mut doc,
            &json!({"op":"edit", "feature":"LPattern1", "count":value}),
            Undo::Step,
        );
        assert!(!reply.ok, "{value}: {}", reply.json);
        assert_eq!(doc.model, before);
    }
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "count":"if(width > 299mm, 2.5, 2)"}),
    );
    volume(&doc, 2);
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"width", "value":"300mm"}),
    );
    assert!(matches!(status(&doc, "LPattern1"), Status::Failed(_)));
    volume(&doc, 1);
    ok(
        &mut doc,
        json!({"op":"set_parameter", "name":"width", "value":"299mm"}),
    );
    volume(&doc, 2);
    ok(
        &mut doc,
        json!({"op":"edit", "feature":"LPattern1", "direction2":"y", "spacing2":5, "count":101, "count2":100}),
    );
    assert!(matches!(status(&doc,"LPattern1"),Status::Failed(ref m) if m.contains("in total")));
    volume(&doc, 1);
}

#[test]
fn model_diff_replays_expression_counts() {
    let mut doc = plate();
    row(&mut doc, json!(2));
    let id = doc
        .model
        .features()
        .find(|f| f.name == "LPattern1")
        .unwrap()
        .id;
    let mut wanted = doc.model.clone();
    if let peet_model::FeatureKind::Pattern(p) = &mut wanted.feature_mut(id).unwrap().kind
        && let peet_model::PatternDef::Linear { first, .. } = &mut p.def
    {
        first
            .count
            .set_input(
                "if(width > 299mm, 3, 2)",
                peet_model::ScalarKind::Number,
                &doc.model.parameters,
            )
            .unwrap();
    }
    let ops = peet_ops::diff(&doc, &wanted).unwrap();
    assert_eq!(ops.len(), 1);
    assert!(peet_ops::apply(&mut doc, &ops[0], Undo::Step).ok);
    assert_eq!(doc.model, wanted);
}
