//! Operations beyond one part: opening and starting documents, the material tables and
//! check limits of the host, drawings brought in, and the application's own commands.

use peet_document::Document;
use peet_ops::{
    AppCommand, Headless, Host, Op, Toggle, Undo, View, apply, apply_all, apply_json,
    apply_json_in, parse,
};
use peet_sheetmetal::{CheckRules, MaterialLibrary};
use serde_json::{Map, Value, json};

fn ok(doc: &mut Document, op: Value) -> Value {
    let reply = apply_json(doc, &op, Undo::Step);
    assert!(reply.ok, "{op} failed: {}", reply.json["error"]);
    reply.json
}

fn ok_in(host: &mut dyn Host, doc: &mut Document, op: Value) -> Value {
    let reply = apply_json_in(host, doc, &op, Undo::Step);
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
    let dir = std::env::temp_dir().join(format!("peet-ops-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn documents_are_started_opened_and_guarded() {
    let dir = temp("docs");
    let file = dir.join("panel.peet").to_string_lossy().into_owned();
    let mut doc = Document::default();
    let opened = apply_json(
        &mut doc,
        &json!({"op": "open_sample", "sample": "enclosure"}),
        Undo::Step,
    );
    assert!(opened.ok && opened.replaced && !opened.changed);
    assert_eq!(opened.json["name"], "Enclosure Panel");
    assert_eq!(opened.json["bodies"], 1);
    assert!(!doc.is_modified());
    assert!(!doc.can_undo(), "a new document has no history");

    // A document with unsaved changes is not thrown away by accident.
    ok(
        &mut doc,
        json!({"op": "set_parameter", "name": "flange", "value": "30mm"}),
    );
    for op in [
        json!({"op": "new"}),
        json!({"op": "open_sample", "sample": "bracket"}),
        json!({"op": "open", "path": file}),
    ] {
        let e = error(&mut doc, op);
        assert!(
            e.contains("unsaved changes") && e.contains("discard"),
            "{e}"
        );
    }
    ok(&mut doc, json!({"op": "save", "path": file}));
    let fresh = ok(&mut doc, json!({"op": "new"}));
    assert_eq!(fresh["features"], 0);
    assert!(doc.model.is_empty());

    let back = ok(&mut doc, json!({"op": "open", "path": file}));
    assert_eq!(back["name"], "panel.peet");
    assert_eq!(back["failures"], json!([]));
    assert_eq!(doc.model.parameters.get("flange"), Some(30.0));
    ok(&mut doc, json!({"op": "delete", "feature": "Edge-Flange1"}));
    ok(
        &mut doc,
        json!({"op": "open_sample", "sample": "housing", "discard": true}),
    );
    assert_eq!(doc.title(), "Housing");

    let e = error(
        &mut doc,
        json!({"op": "open", "path": dir.join("none.peet")}),
    );
    assert!(e.contains("Couldn't read"), "{e}");
    let e = error(&mut doc, json!({"op": "open_sample", "sample": "gearbox"}));
    assert!(e.contains("bracket, enclosure, chassis, housing"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_flat_pattern_and_the_planes_are_shown_and_hidden() {
    let mut doc = Document::default();
    let e = error(&mut doc, json!({"op": "flat_pattern", "on": true}));
    assert!(e.contains("no sheet metal"), "{e}");
    ok(
        &mut doc,
        json!({"op": "open_sample", "sample": "enclosure"}),
    );
    let flat = apply_json(&mut doc, &json!({"op": "flat_pattern"}), Undo::Step);
    assert_eq!(flat.json["flat"], true);
    assert!(!flat.changed, "a view is not a change to the part");
    assert!(doc.is_flat() && doc.bodies[0].flat);
    assert!(!doc.can_undo());
    // Selectors and exports still mean the folded part.
    ok(
        &mut doc,
        json!({"op": "measure", "a": {"face": {"at": [30, 30, 1.5], "normal": [0, 0, 1]}}}),
    );
    assert_eq!(
        ok(&mut doc, json!({"op": "flat_pattern", "on": false}))["flat"],
        false
    );

    assert!(doc.planes_visible());
    let hidden = ok(
        &mut doc,
        json!({"op": "show", "datum": "planes", "on": false}),
    );
    assert_eq!(hidden["op"], "show");
    assert!(!doc.planes_visible());
    assert_eq!(doc.undo_label(), Some("Hide Planes"));
    ok(&mut doc, json!({"op": "show", "datum": "top"}));
    assert!(
        doc.model
            .datum_visible(peet_model::Datum::Plane(peet_model::StdPlane::Top))
    );
    assert!(!doc.planes_visible());
    ok(
        &mut doc,
        json!({"op": "show", "datum": "origin", "on": false}),
    );
    assert!(!doc.model.datum_visible(peet_model::Datum::Origin));
}

#[test]
fn materials_are_listed_applied_and_edited() {
    let dir = temp("materials");
    let mut host = Headless::default();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "open_sample", "sample": "enclosure"}),
    );
    let all = ok_in(&mut host, &mut doc, json!({"op": "materials"}));
    let tables = all["materials"].as_array().unwrap();
    assert!(tables.len() >= 3);
    let material = tables[0]["material"].as_str().unwrap().to_owned();
    let gauge = tables[0]["gauges"][0].clone();

    // By gauge name, then by the nearest thickness.
    let applied = ok_in(
        &mut host,
        &mut doc,
        json!({"op": "apply_material", "material": material, "gauge": gauge["gauge"]}),
    );
    assert_eq!(applied["feature"]["name"], "Base-Flange1");
    assert_eq!(applied["applied"]["thickness"], gauge["thickness"]);
    let table = ok(&mut doc, json!({"op": "bend_table"}));
    assert_eq!(table["thickness"], gauge["thickness"]);
    assert!(doc.undo_label().unwrap().starts_with("Apply "));
    let near = ok_in(
        &mut host,
        &mut doc,
        json!({"op": "apply_material", "material": material, "thickness": 1.4}),
    );
    let t = near["applied"]["thickness"].as_f64().unwrap();
    assert!((t - 1.4).abs() < 0.3, "{t}");

    // The tables are the host's: an added gauge is there for the next operation.
    let added = ok_in(
        &mut host,
        &mut doc,
        json!({"op": "set_gauge", "material": "Titanium", "gauge": "2 mm", "thickness": 2,
               "radius": 4, "bend": {"k_factor": 0.4}, "notes": "grade 2"}),
    );
    assert_eq!(added["gauge"]["radius"], 4.0);
    ok_in(
        &mut host,
        &mut doc,
        json!({"op": "set_gauge", "material": "titanium", "gauge": "2mm", "radius": 5}),
    );
    let ti = ok_in(
        &mut host,
        &mut doc,
        json!({"op": "materials", "material": "Titanium"}),
    );
    assert_eq!(ti["materials"][0]["gauges"].as_array().unwrap().len(), 1);
    assert_eq!(ti["materials"][0]["gauges"][0]["radius"], 5.0);
    assert_eq!(
        ti["materials"][0]["gauges"][0]["bend"],
        json!({"k_factor": 0.4})
    );
    ok_in(
        &mut host,
        &mut doc,
        json!({"op": "apply_material", "material": "Titanium", "gauge": "2 mm"}),
    );
    assert_eq!(ok(&mut doc, json!({"op": "bend_table"}))["thickness"], 2.0);

    // Out to CSV and back.
    let csv = dir.join("gauges.csv").to_string_lossy().into_owned();
    ok_in(
        &mut host,
        &mut doc,
        json!({"op": "export_materials", "path": csv}),
    );
    let before = host.materials.clone();
    ok_in(
        &mut host,
        &mut doc,
        json!({"op": "delete_gauge", "material": "Titanium"}),
    );
    assert!(host.materials.table("Titanium").is_none());
    let read = ok_in(
        &mut host,
        &mut doc,
        json!({"op": "import_materials", "path": csv}),
    );
    assert_eq!(read["materials"], before.tables.len());
    assert!(host.materials.find("Titanium", "2 mm").is_some());

    // Mistakes.
    let reply = apply_json_in(
        &mut host,
        &mut doc,
        &json!({"op": "apply_material", "material": "Unobtainium", "gauge": "1 mm"}),
        Undo::Step,
    );
    assert!(reply.json["error"].as_str().unwrap().contains("Titanium"));
    let reply = apply_json_in(
        &mut host,
        &mut doc,
        &json!({"op": "set_gauge", "material": "Titanium", "gauge": "3 mm"}),
        Undo::Step,
    );
    assert!(reply.json["error"].as_str().unwrap().contains("thickness"));
    // A fresh host has the built-in tables again.
    let e = error(
        &mut doc,
        json!({"op": "apply_material", "material": "Titanium", "gauge": "2 mm"}),
    );
    assert!(e.contains("no material 'Titanium'"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_limits_are_the_hosts_and_can_be_changed() {
    let mut host = Headless::default();
    let mut doc = Document::default();
    ok(&mut doc, json!({"op": "open_sample", "sample": "chassis"}));
    let checks = ok_in(&mut host, &mut doc, json!({"op": "checks"}));
    assert_eq!(checks["passed"], true, "{checks}");
    assert_eq!(checks["rules"]["min_flange"]["thickness"], 4.0);

    // A much longer minimum flange fails the chassis's 12 mm rim.
    let set = ok_in(
        &mut host,
        &mut doc,
        json!({"op": "set_check_rule", "rule": "min_flange", "thickness": 0, "constant": 30}),
    );
    assert_eq!(set["rules"]["min_flange"]["constant_mm"], 30.0);
    let checks = ok_in(&mut host, &mut doc, json!({"op": "checks"}));
    assert_eq!(checks["passed"], false);
    assert!(
        checks["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["check"] == "Flange too short")
    );
    let reply = apply_json_in(
        &mut host,
        &mut doc,
        &json!({"op": "set_check_rule", "rule": "min_flange", "constant": -1}),
        Undo::Step,
    );
    assert!(!reply.ok);
    assert_eq!(host.check_rules.min_flange.constant, 30.0);
}

#[test]
fn a_dxf_drawing_becomes_a_sketch() {
    let dir = temp("dxf");
    let file = dir.join("flat.dxf").to_string_lossy().into_owned();
    let mut doc = Document::default();
    ok(
        &mut doc,
        json!({"op": "open_sample", "sample": "enclosure"}),
    );
    let flat = ok(&mut doc, json!({"op": "export", "path": file}));

    // The flat pattern, read back, is a blank ready for a base flange.
    let mut other = Document::default();
    let imported = ok(
        &mut other,
        json!({"op": "import_dxf", "path": file, "name": "Blank", "placement": "lower_left"}),
    );
    assert_eq!(imported["created"][0]["name"], "Blank");
    assert!(imported["curves"].as_u64().unwrap() > 20);
    assert_eq!(imported["unit"], "millimetres");
    assert_eq!(other.undo_label(), Some("Import DXF"));
    let plate = ok(
        &mut other,
        json!({"op": "base_flange", "sketch": "Blank", "thickness": 1.5}),
    );
    assert_eq!(plate["created"][0]["status"], "ok", "{plate}");
    let body = ok(&mut other, json!({"op": "bodies"}));
    assert_eq!(body["bodies"][0]["size"][0], flat["flat_size"][0]);
    assert_eq!(body["bodies"][0]["size"][1], flat["flat_size"][1]);

    // Into a sketch the part already has.
    ok(
        &mut other,
        json!({"op": "sketch", "on": "front", "name": "More"}),
    );
    let more = ok(
        &mut other,
        json!({"op": "import_dxf", "path": file, "sketch": "More", "unit": "in"}),
    );
    assert_eq!(more["feature"]["name"], "More");
    assert_eq!(more["unit"], "inches");
    let e = error(
        &mut other,
        json!({"op": "import_dxf", "path": file, "sketch": "More", "on": "top"}),
    );
    assert!(e.contains("not both"), "{e}");
    let e = error(
        &mut other,
        json!({"op": "import_dxf", "path": dir.join("none.dxf")}),
    );
    assert!(e.contains("Couldn't read"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A stand-in for the application: it notes the commands it is given.
#[derive(Default)]
struct Recorder {
    materials: MaterialLibrary,
    rules: CheckRules,
    seen: Vec<AppCommand>,
}

impl Host for Recorder {
    fn materials(&mut self) -> &mut MaterialLibrary {
        &mut self.materials
    }

    fn check_rules(&mut self) -> &mut CheckRules {
        &mut self.rules
    }

    fn app(&mut self, command: &AppCommand) -> Result<Map<String, Value>, String> {
        self.seen.push(command.clone());
        Ok(Map::new())
    }
}

#[test]
fn application_commands_go_to_the_application() {
    let mut doc = Document::default();
    let commands = [
        json!({"op": "view", "to": "front"}),
        json!({"op": "zoom_to_fit"}),
        json!({"op": "toggle", "what": "grid", "on": false}),
        json!({"op": "toggle", "what": "perspective"}),
        json!({"op": "window", "open": "settings"}),
        json!({"op": "edit_sketch", "sketch": "Sketch1"}),
        json!({"op": "tool", "tool": "line"}),
        json!({"op": "exit_sketch"}),
        json!({"op": "check_for_updates"}),
        json!({"op": "install_update"}),
        json!({"op": "quit"}),
    ];
    // With no application, each says so and nothing happens.
    for op in &commands {
        let e = error(&mut doc, op.clone());
        assert!(e.contains("needs a running PeetCAD"), "{e}");
    }
    // In an application, each arrives as a typed command.
    let mut app = Recorder::default();
    for op in &commands {
        let reply = apply_json_in(&mut app, &mut doc, op, Undo::Step);
        assert!(reply.ok && !reply.changed, "{op}: {}", reply.json);
        assert_eq!(reply.json["op"], op["op"]);
    }
    assert_eq!(app.seen.len(), commands.len());
    assert_eq!(app.seen[0], AppCommand::View(View::Front));
    assert_eq!(
        app.seen[2],
        AppCommand::Toggle {
            what: Toggle::Grid,
            on: Some(false)
        }
    );
    assert_eq!(
        parse(&doc, &json!({"op": "toggle", "what": "perspective"})).unwrap(),
        Op::App(AppCommand::Toggle {
            what: Toggle::Perspective,
            on: None
        })
    );
    let e = error(&mut doc, json!({"op": "view", "to": "sideways"}));
    assert!(e.contains("isometric, front, back"), "{e}");
    let reply = apply(&mut doc, &Op::App(AppCommand::ZoomToFit), Undo::Step);
    assert!(!reply.ok);
}

#[test]
fn a_script_keeps_its_material_tables_for_the_run() {
    let mut doc = Document::default();
    let replies = apply_all(
        &mut doc,
        &[
            json!({"op": "open_sample", "sample": "enclosure"}),
            json!({"op": "set_gauge", "material": "Brass", "gauge": "1 mm", "thickness": 1}),
            json!({"op": "apply_material", "material": "Brass", "gauge": "1 mm"}),
        ],
        Undo::Step,
    );
    assert_eq!(replies.len(), 3);
    assert!(
        replies.iter().all(|r| r.ok),
        "{:?}",
        replies.last().map(|r| &r.json)
    );
    assert_eq!(ok(&mut doc, json!({"op": "bend_table"}))["thickness"], 1.0);
}
