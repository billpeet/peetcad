use peet_document::Document;
use peet_ops::{Undo, apply_json};
use peet_sketch::{EntityId, Geometry};
use serde_json::{Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    reply.json
}
fn base(doc: &mut Document) {
    ok(
        doc,
        json!({"op":"sketch","on":"top","name":"Base","draw":[
            {"type":"rectangle","from":[0,0],"to":[40,30],"as":"r"},
            {"type":"coincident","of":["r.bottom.start","origin"]},
            {"type":"length","of":["r.bottom"],"value":40,"name":"width"},
            {"type":"length","of":["r.right"],"value":30}
        ]}),
    );
    ok(
        doc,
        json!({"op":"extrude","sketch":"Base","depth":5,"name":"Stock"}),
    );
}
fn sketch<'a>(doc: &'a Document, name: &str) -> &'a peet_model::SketchFeature {
    doc.model
        .features()
        .find(|f| f.name == name)
        .unwrap()
        .sketch()
        .unwrap()
}
#[test]
fn references_follow_dimensions_and_survive_save() {
    let mut doc = Document::default();
    base(&mut doc);
    let reply = ok(
        &mut doc,
        json!({"op":"sketch","on":{"feature":"Stock","side":"end"},"name":"Located","draw":[
            {"type":"project","edge":{"between":[[40,0,5],[40,30,5]]},"as":"edge"},
            {"type":"project","vertex":{"at":[40,30,5]},"as":"corner"},
            {"type":"point","at":[35,27],"as":"p"},
            {"type":"distance","of":["p","edge"],"value":5},
            {"type":"vertical_distance","of":["p","corner"],"value":3}
        ]}),
    );
    assert!(reply.get("failures").is_none(), "{reply}");
    let point = EntityId(reply["drawn"][2]["id"].as_u64().unwrap() as u32);
    let s = sketch(&doc, "Located");
    assert_eq!(s.projections.len(), 2);
    assert!(s.sketch.entity(s.projections[0].entity).unwrap().locked);
    ok(
        &mut doc,
        json!({"op":"set_dimension","sketch":"Base","name":"width","value":60}),
    );
    assert!(
        sketch(&doc, "Located")
            .sketch
            .point(point)
            .abs_diff_eq(peet_math::DVec2::new(55., 27.), 1e-6)
    );
    let bytes = peet_io::document::save(&doc.model, &Default::default(), None).unwrap();
    let mut model = peet_io::document::open(&bytes).unwrap().model;
    assert_eq!(model, doc.model);
    let mut engine = peet_model::Engine::new();
    engine.regenerate(&mut model);
    assert_eq!(model, doc.model);
    assert!(engine.evaluation().failures().next().is_none());
}
#[test]
fn converted_face_is_a_profile_and_refreshes() {
    let mut doc = Document::default();
    base(&mut doc);
    let reply = ok(
        &mut doc,
        json!({"op":"sketch","on":{"feature":"Stock","side":"end"},"name":"Converted","draw":[
            {"type":"project","face":{"feature":"Stock","side":"end"},"convert":true,"as":"outline"}
        ]}),
    );
    assert_eq!(reply["drawn"][0]["curves"].as_array().unwrap().len(), 4);
    assert_eq!(sketch(&doc, "Converted").projections.len(), 4);
    let reply = ok(
        &mut doc,
        json!({"op":"extrude","sketch":"Converted","depth":2}),
    );
    assert!(reply.get("failures").is_none(), "{reply}");
    ok(
        &mut doc,
        json!({"op":"set_dimension","sketch":"Base","name":"width","value":50}),
    );
    let volume: f64 = doc
        .evaluation()
        .bodies
        .iter()
        .map(|b| peet_kernel::validate::measure::volume(&b.solid))
        .sum();
    assert!((volume - 50. * 30. * 7.).abs() < 1e-5, "{volume}");
}
#[test]
fn reference_plane_and_doubled_distance_follow_offset() {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"plane","name":"Datum","from":"right","distance":10}),
    );
    let reply = ok(
        &mut doc,
        json!({"op":"sketch","on":"top","name":"Located","draw":[
            {"type":"project","plane":"Datum","as":"axis"},
            {"type":"point","at":[14,0],"as":"p"},
            {"type":"doubled_distance","of":["p","axis"],"value":8,"name":"across"},
            {"type":"horizontal","of":["p","origin"]}
        ]}),
    );
    let point = EntityId(reply["drawn"][1]["id"].as_u64().unwrap() as u32);
    ok(
        &mut doc,
        json!({"op":"edit","feature":"Datum","distance":20}),
    );
    assert!((sketch(&doc, "Located").sketch.point(point).x - 24.).abs() < 1e-6);
    ok(
        &mut doc,
        json!({"op":"set_dimension","sketch":"Located","name":"across","value":12}),
    );
    assert!((sketch(&doc, "Located").sketch.point(point).x - 26.).abs() < 1e-6);
    let s = sketch(&doc, "Located");
    assert!(
        (s.sketch
            .measure(
                &s.sketch
                    .constraints()
                    .find(|(_, c)| c.dimension.is_some())
                    .unwrap()
                    .1
                    .kind
            )
            .unwrap()
            - 12.)
            .abs()
            < 1e-6
    );
}
#[test]
fn locked_circle_radius_is_constant_and_coradial_works() {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"sketch","on":"top","draw":[{"type":"circle","center":[0,0],"radius":10}]}),
    );
    ok(
        &mut doc,
        json!({"op":"extrude","sketch":"Sketch1","depth":5}),
    );
    let r = ok(
        &mut doc,
        json!({"op":"sketch","on":"top","name":"Round","draw":[
            {"type":"project","edge":{"at":[0,10,5]},"as":"ring"},
            {"type":"circle","center":[1,2],"radius":7,"as":"c"},
            {"type":"coradial","of":["ring","c"]}
        ]}),
    );
    assert!(r.get("failures").is_none(), "{r}");
    assert_eq!(r["degrees_of_freedom"], 0);
    let circle = EntityId(r["drawn"][1]["id"].as_u64().unwrap() as u32);
    assert!(
        matches!(sketch(&doc,"Round").sketch.entity(circle).unwrap().geometry,Geometry::Circle{radius,..} if (radius-10.).abs()<1e-6)
    );
}
#[test]
fn bad_projection_is_atomic_and_can_be_deleted() {
    let mut doc = Document::default();
    base(&mut doc);
    let before = doc.model.clone();
    let r = apply_json(
        &mut doc,
        &json!({"op":"sketch","on":"top","draw":[{"type":"project","plane":"top"}]}),
        Undo::Step,
    );
    assert!(!r.ok);
    assert_eq!(before, doc.model);
    let r = ok(
        &mut doc,
        json!({"op":"sketch","on":"top","name":"Ref","draw":[{"type":"project","vertex":{"at":[40,30,5]}}]}),
    );
    let id = r["drawn"][0]["id"].clone();
    ok(
        &mut doc,
        json!({"op":"draw","sketch":"Ref","draw":[{"type":"delete","of":[id]}]}),
    );
    assert!(sketch(&doc, "Ref").projections.is_empty());
}

#[test]
fn suppression_and_missing_sources_do_not_use_stale_geometry() {
    let mut doc = Document::default();
    base(&mut doc);
    ok(
        &mut doc,
        json!({"op":"sketch","on":"top","name":"Ref","draw":[{"type":"project","edge":{"between":[[40,0,5],[40,30,5]]}}]}),
    );
    let id = doc.model.features().find(|f| f.name == "Ref").unwrap().id;
    let stock = doc.model.features().find(|f| f.name == "Stock").unwrap().id;
    ok(
        &mut doc,
        json!({"op":"suppress","feature":"Stock","on":true}),
    );
    assert!(doc.evaluation().state(id).unwrap().status.is_suppressed());
    ok(
        &mut doc,
        json!({"op":"suppress","feature":"Stock","on":false}),
    );
    assert!(doc.evaluation().state(id).unwrap().status.is_built());
    let mut model = doc.model.clone();
    model.feature_mut(stock).unwrap().kind =
        peet_model::FeatureKind::Point(peet_model::PointDef::Coordinates {
            x: peet_model::Scalar::new(0.0),
            y: peet_model::Scalar::new(0.0),
            z: peet_model::Scalar::new(0.0),
        });
    let mut engine = peet_model::Engine::new();
    engine.regenerate(&mut model);
    let status = &engine.evaluation().state(id).unwrap().status;
    assert!(status.is_failed(), "{status:?}");
    assert!(status.message().unwrap().contains("projected edge"));
}
#[test]
fn collinear_and_midpoint_against_projected_edge() {
    let mut doc = Document::default();
    base(&mut doc);
    let r = ok(
        &mut doc,
        json!({"op":"sketch","on":"top","name":"Refs","draw":[
            {"type":"project","edge":{"between":[[40,0,5],[40,30,5]]},"as":"edge"},
            {"type":"line","from":[38,3],"to":[39,12],"as":"l"},
            {"type":"collinear","of":["l","edge"]},
            {"type":"point","at":[30,10],"as":"p"},
            {"type":"midpoint","of":["p","edge"]}
        ]}),
    );
    assert_eq!(r["drawn"][2]["relations"].as_array().unwrap().len(), 2);
    let p = EntityId(r["drawn"][3]["id"].as_u64().unwrap() as u32);
    assert!(
        sketch(&doc, "Refs")
            .sketch
            .point(p)
            .abs_diff_eq(peet_math::DVec2::new(40., 15.), 1e-6)
    );
    ok(
        &mut doc,
        json!({"op":"set_dimension","sketch":"Base","name":"width","value":60}),
    );
    assert!(
        sketch(&doc, "Refs")
            .sketch
            .point(p)
            .abs_diff_eq(peet_math::DVec2::new(60., 15.), 1e-6)
    );
}

#[test]
fn projected_references_refresh_when_switching_configurations() {
    let mut doc = Document::default();
    base(&mut doc);
    let r = ok(
        &mut doc,
        json!({"op":"sketch","on":"top","name":"Ref","draw":[
            {"type":"project","vertex":{"at":[40,30,5]},"as":"corner"},
            {"type":"point","at":[40,30],"as":"p"},
            {"type":"coincident","of":["corner","p"]}
        ]}),
    );
    let p = EntityId(r["drawn"][1]["id"].as_u64().unwrap() as u32);
    ok(&mut doc, json!({"op":"add_configuration","name":"Wide"}));
    ok(
        &mut doc,
        json!({"op":"set_dimension","sketch":"Base","name":"width","value":60,"configurations":"Wide"}),
    );
    for (config, width) in [("Wide", 60.), ("Default", 40.), ("Wide", 60.)] {
        ok(
            &mut doc,
            json!({"op":"configuration","configuration":config}),
        );
        assert!(
            sketch(&doc, "Ref")
                .sketch
                .point(p)
                .abs_diff_eq(peet_math::DVec2::new(width, 30.), 1e-6)
        );
    }
}
