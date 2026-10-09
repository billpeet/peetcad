use peet_document::Document;
use peet_ops::{InsertSource, Op, Undo, apply, apply_json};
use serde_json::{Value, json};
use std::sync::Arc;

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op}: {}", reply.json);
    assert!(reply.json.get("failures").is_none(), "{}", reply.json);
    reply.json
}
fn error(doc: &mut Document, op: Value) -> String {
    let before = doc.model.clone();
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(!reply.ok, "{op}: {}", reply.json);
    assert_eq!(before, doc.model);
    reply.json["error"].as_str().unwrap().to_owned()
}
fn block() -> Document {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"sketch","on":"top","draw":[{"type":"rectangle","from":[0,0],"to":[60,40]}]}),
    );
    ok(
        &mut doc,
        json!({"op":"extrude","sketch":"Sketch1","depth":20}),
    );
    doc
}
fn named(doc: &mut Document, query: &str, name: &str) -> Value {
    ok(doc, json!({"op":query}))[query]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["names"].as_array().unwrap().contains(&json!(name)))
        .unwrap()
        .clone()
}

#[test]
fn names_follow_rebuild_save_open_and_undo() {
    let mut doc = block();
    ok(
        &mut doc,
        json!({"op":"name_face","face":{"feature":"Extrude1","side":"end"},"name":"Front"}),
    );
    ok(
        &mut doc,
        json!({"op":"name_edge","edge":{"between":[[0,0,0],[0,0,20]]},"name":"Corner"}),
    );
    ok(
        &mut doc,
        json!({"op":"edit","feature":"Extrude1","depth":30}),
    );
    assert_eq!(named(&mut doc, "faces", "Front")["center"][2], 30.0);
    assert_eq!(named(&mut doc, "edges", "Corner")["middle"][2], 15.0);
    let bytes = peet_io::document::save(&doc.model, &Default::default(), None).unwrap();
    let text = peet_io::document::to_text(&bytes).unwrap();
    let opened = peet_io::document::open(&peet_io::document::from_text(&text).unwrap()).unwrap();
    assert_eq!(opened.model, doc.model);
    let mut reopened = Document::from_model(opened.model, None);
    named(&mut reopened, "faces", "Front");
    ok(
        &mut reopened,
        json!({"op":"measure","a":{"edge":{"name":"Corner"}}}),
    );
    ok(
        &mut doc,
        json!({"op":"sketch","on":{"name":"Front"},"draw":[]}),
    );
    ok(&mut doc, json!({"op":"delete_name","name":"Front"}));
    assert!(!doc.model.geometry_names.contains_key("Front"));
    ok(&mut doc, json!({"op":"undo"}));
    assert!(doc.model.geometry_names.contains_key("Front"));
    ok(&mut doc, json!({"op":"redo"}));
    assert!(!doc.model.geometry_names.contains_key("Front"));
}

#[test]
fn names_refuse_collisions_wrong_kinds_and_missing_geometry() {
    let mut doc = block();
    ok(
        &mut doc,
        json!({"op":"name_face","face":{"normal":[0,0,1]},"name":"Front"}),
    );
    for name in ["", " Front", "Front "] {
        error(
            &mut doc,
            json!({"op":"name_face","face":{"normal":[0,0,1]},"name":name}),
        );
    }
    assert!(
        error(
            &mut doc,
            json!({"op":"name_edge","edge":{"at":[0,0,10]},"name":"Front"})
        )
        .contains("already")
    );
    assert!(
        error(
            &mut doc,
            json!({"op":"measure","a":{"edge":{"name":"Front"}}})
        )
        .contains("not an edge")
    );
    error(
        &mut doc,
        json!({"op":"sketch","on":{"name":"front"},"draw":[]}),
    );
    error(
        &mut doc,
        json!({"op":"sketch","on":{"name":"Front","normal":[0,0,-1]},"draw":[]}),
    );
    ok(&mut doc, json!({"op":"suppress","feature":"Extrude1"}));
    assert!(
        error(
            &mut doc,
            json!({"op":"sketch","on":{"name":"Front"},"draw":[]})
        )
        .contains("no longer exists")
    );
    ok(
        &mut doc,
        json!({"op":"suppress","feature":"Extrude1","on":false}),
    );
    named(&mut doc, "faces", "Front");
    ok(&mut doc, json!({"op":"delete_name","name":"Front"}));
    error(&mut doc, json!({"op":"delete_name","name":"Front"}));
}

#[test]
fn cylindrical_names_work_in_mates_and_survive_nested_assemblies() {
    let mut part = block();
    ok(
        &mut part,
        json!({"op":"sketch","on":{"feature":"Extrude1","side":"end"},"name":"Centres","draw":[{"type":"point","at":[30,20]}]}),
    );
    ok(
        &mut part,
        json!({"op":"hole","sketch":"Centres","diameter":8}),
    );
    let faces = ok(&mut part, json!({"op":"faces"}));
    let cylinder = faces["faces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["surface"] == "cylinder")
        .unwrap();
    ok(
        &mut part,
        json!({"op":"name_face","face":{"body":0,"index":cylinder["index"]},"name":"HingeHole"}),
    );
    assert_eq!(
        named(&mut part, "faces", "HingeHole")["surface"],
        "cylinder"
    );
    error(
        &mut part,
        json!({"op":"sketch","on":{"name":"HingeHole"},"draw":[]}),
    );
    let mut assembly = Document::from_model(peet_model::Model::new_assembly(), None);
    let r = apply(
        &mut assembly,
        &Op::Insert {
            from: InsertSource::Model(Arc::new(part.model)),
            name: Some("First".into()),
            placing: None,
            fixed: Some(true),
            link: false,
            absolute: false,
        },
        Undo::Step,
    );
    assert!(r.ok, "{}", r.json);
    ok(
        &mut assembly,
        json!({"op":"insert","component":"First","name":"Second","at":[100,0,0]}),
    );
    ok(
        &mut assembly,
        json!({"op":"mate","type":"concentric","a":{"component":"First","face":{"name":"HingeHole"}},"b":{"component":"Second","face":{"name":"HingeHole"}}}),
    );
    let mut outer = peet_model::Model::new_assembly();
    outer
        .assembly_mut()
        .unwrap()
        .define(Arc::new(assembly.model));
    let bytes = peet_io::document::save(&outer, &Default::default(), None).unwrap();
    assert_eq!(peet_io::document::open(&bytes).unwrap().model, outer);
}

#[test]
fn model_edits_translate_names_to_operations() {
    let mut doc = block();
    let mut edited = doc.model.clone();
    let body = &doc.evaluation().bodies[0];
    let face = body.solid.face_ids().next().unwrap();
    edited
        .name_geometry(
            "Face",
            peet_model::naming::NamedGeometry::Face(body.face_ref(face)),
        )
        .unwrap();
    let translated = peet_ops::apply_model(
        &mut peet_ops::Headless::default(),
        &mut doc,
        edited.clone(),
        "Name face",
        99,
    );
    assert_eq!(translated.untranslated, None);
    assert_eq!(doc.model, edited);
}

#[test]
fn names_are_shared_across_configurations_and_rollback() {
    let mut doc = block();
    ok(
        &mut doc,
        json!({"op":"name_face","face":{"normal":[0,0,1]},"name":"Front"}),
    );
    ok(
        &mut doc,
        json!({"op":"name_face","face":{"name":"Front"},"name":"MateFace"}),
    );
    assert_eq!(
        named(&mut doc, "faces", "Front")["names"],
        json!(["Front", "MateFace"])
    );
    ok(&mut doc, json!({"op":"add_configuration","name":"Tall"}));
    ok(
        &mut doc,
        json!({"op":"edit","feature":"Extrude1","depth":40,"configurations":"this"}),
    );
    assert_eq!(named(&mut doc, "faces", "Front")["center"][2], 40.0);
    ok(
        &mut doc,
        json!({"op":"configuration","configuration":"Default"}),
    );
    assert_eq!(named(&mut doc, "faces", "MateFace")["center"][2], 20.0);
    ok(&mut doc, json!({"op":"rollback","to":"Sketch1"}));
    error(
        &mut doc,
        json!({"op":"measure","a":{"face":{"name":"Front"}}}),
    );
    ok(&mut doc, json!({"op":"rollback","to":"end"}));
    named(&mut doc, "faces", "Front");
    ok(&mut doc, json!({"op":"delete","features":["Extrude1"]}));
    error(
        &mut doc,
        json!({"op":"measure","a":{"face":{"name":"Front"}}}),
    );
}

#[test]
fn named_sheet_faces_and_edges_survive_a_flange() {
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op":"sketch","on":"top","draw":[{"type":"rectangle","from":[0,0],"to":[80,50]}]}),
    );
    ok(
        &mut doc,
        json!({"op":"base_flange","sketch":"Sketch1","thickness":2,"radius":1}),
    );
    ok(
        &mut doc,
        json!({"op":"name_face","face":{"feature":"Base-Flange1","side":"top"},"name":"Panel"}),
    );
    ok(
        &mut doc,
        json!({"op":"name_edge","edge":{"between":[[0,0,2],[80,0,2]]},"name":"BendEdge"}),
    );
    ok(
        &mut doc,
        json!({"op":"edge_flange","edge":{"name":"BendEdge"},"length":20}),
    );
    named(&mut doc, "faces", "Panel");
}
