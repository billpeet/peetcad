//! Operations built in Rust: the compiler checks their fields, and they can carry
//! references that are already resolved, as the application has after a click.

use peet_document::Document;
use peet_kernel::validate::measure;
use peet_model::{Operation, StdPlane};
use peet_ops::{
    Draw, EdgeFlange, EdgeSel, End, Extrude, FaceQuery, FeatureArgs, Input, Measure, New, Op,
    PlaneSel, Query, Relation, SheetCut, Side, Undo, apply, parse,
};
use serde_json::json;

fn ok(doc: &mut Document, op: Op) -> serde_json::Value {
    let reply = apply(doc, &op, Undo::Step);
    assert!(reply.ok, "{op:?} failed: {}", reply.json["error"]);
    reply.json
}

fn plate(doc: &mut Document) {
    ok(
        doc,
        Op::Sketch {
            on: StdPlane::Top.into(),
            name: None,
            draw: vec![
                Draw::Rectangle {
                    from: [0.0, 0.0],
                    to: [80.0, 50.0],
                }
                .labelled("r"),
                Draw::Relation {
                    kind: Relation::Coincident,
                    of: vec!["r.bottom.start".into(), "origin".into()],
                }
                .into(),
                Draw::Dimension {
                    kind: Measure::Length,
                    of: vec!["r.bottom".into()],
                    value: Some(80.into()),
                    name: Some("width".to_owned()),
                    driven: false,
                }
                .into(),
            ],
        },
    );
    let reply = ok(
        doc,
        Op::add(FeatureArgs::Extrude(Extrude {
            sketch: Some("Sketch1".into()),
            depth: Some(8.into()),
            ..Default::default()
        })),
    );
    assert_eq!(reply["op"], "extrude");
    assert_eq!(reply["created"][0]["status"], "ok");
}

#[test]
fn typed_operations_build_and_edit_a_part() {
    let mut doc = Document::default();
    plate(&mut doc);
    ok(
        &mut doc,
        Op::Sketch {
            on: FaceQuery {
                feature: Some("Extrude1".into()),
                side: Some(Side::End),
                ..Default::default()
            }
            .into(),
            name: Some("Hole".to_owned()),
            draw: vec![
                Draw::Circle {
                    center: [40.0, 25.0],
                    radius: 5.0,
                }
                .into(),
            ],
        },
    );
    ok(
        &mut doc,
        Op::add(FeatureArgs::Cut(Extrude {
            sketch: Some("Hole".into()),
            end: Some(End::ThroughAll),
            ..Default::default()
        })),
    );
    let volume = |doc: &Document| measure::volume(&doc.evaluation().bodies[0].solid);
    let hole = std::f64::consts::PI * 25.0;
    assert!((volume(&doc) - (80.0 * 50.0 - hole) * 8.0).abs() < 1e-6);

    // An edit takes the same typed fields; only the ones given change.
    ok(
        &mut doc,
        Op::SetParameter {
            name: "deep".to_owned(),
            value: "10mm".into(),
            configurations: Default::default(),
        },
    );
    let id = doc.model.features().nth(1).unwrap().id;
    ok(
        &mut doc,
        Op::Edit {
            feature: id.into(),
            fields: FeatureArgs::Extrude(Extrude {
                depth: Some(Input::Expr("deep".to_owned())),
                ..Default::default()
            }),
            configurations: None,
        },
    );
    assert!((volume(&doc) - (80.0 * 50.0 - hole) * 10.0).abs() < 1e-6);
    let e = doc.feature(id).unwrap().extrude().unwrap();
    assert_eq!(
        e.params.operation,
        Operation::Add,
        "untouched fields keep their values"
    );

    // Fields of another kind of feature are refused, and say why.
    let reply = apply(
        &mut doc,
        &Op::Edit {
            feature: id.into(),
            fields: FeatureArgs::SheetCut(SheetCut::default()),
            configurations: None,
        },
        Undo::Step,
    );
    assert!(!reply.ok);
    let error = reply.json["error"].as_str().unwrap();
    assert_eq!(
        error,
        "Extrude1 (Extrude) doesn't have the fields of 'sheet_cut'."
    );

    let status = ok(&mut doc, Op::Query(Query::Status));
    assert_eq!(status["features"], 4);
    assert_eq!(status["failures"], json!([]));
}

#[test]
fn a_resolved_reference_is_used_as_it_is() {
    // What the application has after the user clicks an edge: a persistent reference.
    let (model, _) = peet_model::samples::enclosure();
    let mut doc = Document::from_model(model, None);
    let flange = doc
        .model
        .features()
        .find(|f| f.name == "Edge-Flange1")
        .unwrap();
    let peet_model::FeatureKind::EdgeFlange(existing) = &flange.kind else {
        panic!("an edge flange");
    };
    let clicked = existing.edge.clone().unwrap();
    let id = flange.id;
    ok(
        &mut doc,
        Op::Delete {
            features: vec![id.into()],
        },
    );
    let before = doc.evaluation().bodies[0].solid.faces.len();

    let reply = ok(
        &mut doc,
        Op::Add(vec![New {
            name: Some("Front".to_owned()),
            feature: FeatureArgs::EdgeFlange(EdgeFlange {
                edge: Some(Some(clicked.clone().into())),
                length: Some("flange".into()),
                offset_start: Some(10.into()),
                offset_end: Some(10.into()),
                ..Default::default()
            }),
        }]),
    );
    assert_eq!(reply["created"][0]["name"], "Front");
    assert_eq!(reply["created"][0]["status"], "ok");
    assert!(doc.evaluation().bodies[0].solid.faces.len() > before);
    // The stored reference is the clicked one, not a re-resolved copy of it.
    let added = doc.model.features().find(|f| f.name == "Front").unwrap();
    let peet_model::FeatureKind::EdgeFlange(f) = &added.kind else {
        panic!("an edge flange");
    };
    assert_eq!(f.edge.as_ref(), Some(&clicked));
}

#[test]
fn json_reads_into_the_same_operations() {
    let mut doc = Document::default();
    plate(&mut doc);
    let op = parse(
        &doc,
        &json!({"op": "edge_flange", "name": "A", "length": "flange", "angle": 45, "radius": null,
                "edge": {"between": [[0, 0, 8], [80, 0, 8]]}}),
    )
    .unwrap();
    assert_eq!(
        op,
        Op::Add(vec![New {
            name: Some("A".to_owned()),
            feature: FeatureArgs::EdgeFlange(EdgeFlange {
                edge: Some(Some(EdgeSel::between([0.0, 0.0, 8.0], [80.0, 0.0, 8.0]))),
                length: Some("flange".into()),
                angle: Some(45.into()),
                radius: Some(None),
                ..Default::default()
            }),
        }])
    );
    // An edit is read as the fields of the feature it is about.
    let op = parse(
        &doc,
        &json!({"op": "edit", "feature": "Extrude1", "end": {"up_to": "front"}}),
    )
    .unwrap();
    assert_eq!(
        op,
        Op::Edit {
            feature: "Extrude1".into(),
            fields: FeatureArgs::Extrude(Extrude {
                end: Some(End::UpTo(PlaneSel::Standard(StdPlane::Front))),
                ..Default::default()
            }),
            configurations: None,
        }
    );
    assert!(
        parse(
            &doc,
            &json!({"op": "edit", "feature": "Extrude1", "length": 3})
        )
        .is_err()
    );
}
