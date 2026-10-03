//! The command line, end to end: arguments in, files and replies out.

use std::path::{Path, PathBuf};

use peet_cli::{FAILED, OK, USAGE};
use serde_json::{Value, json};

/// A folder of its own for one test, removed at the end.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("peet-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, text: &str) -> String {
        std::fs::write(self.0.join(name), text).unwrap();
        self.path(name)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What a run printed and returned.
struct Ran {
    code: i32,
    /// The replies: one JSON value per line of standard output.
    replies: Vec<Value>,
    out: String,
    err: String,
}

fn peet_with(args: &[&str], stdin: &str) -> Ran {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = peet_cli::run(&args, &mut stdin.as_bytes(), &mut out, &mut err);
    let out = String::from_utf8(out).unwrap();
    Ran {
        code,
        replies: out
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
        out,
        err: String::from_utf8(err).unwrap(),
    }
}

fn peet(args: &[&str]) -> Ran {
    peet_with(args, "")
}

/// The enclosure panel sample, as a script.
const PANEL: &str = r#"
# Parameters, then the base plate and a flange on two edges.
{"op": "set_parameter", "name": "thickness", "value": "1.5mm"}
{"op": "set_parameter", "name": "flange", "value": "25mm"}
{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 150], "as": "r"}, {"type": "coincident", "of": ["r.bottom.start", "origin"]}, {"type": "length", "of": ["r.bottom"], "value": 200}, {"type": "length", "of": ["r.right"], "value": 150}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": "thickness", "radius": 2}
{"op": "edge_flange", "length": "flange", "offset_start": 10, "offset_end": 10, "edges": [{"between": [[0, 0, 1.5], [200, 0, 1.5]]}, {"between": [[200, 0, 1.5], [200, 150, 1.5]]}, {"between": [[200, 150, 1.5], [0, 150, 1.5]]}, {"between": [[0, 150, 1.5], [0, 0, 1.5]]}]}
"#;

fn model(path: &str) -> peet_model::Model {
    peet_io::document::open(&std::fs::read(path).unwrap())
        .unwrap()
        .model
}

#[test]
fn a_part_is_built_changed_asked_about_and_exported() {
    let dir = Folder::new("build");
    let script = dir.write("panel.jsonl", PANEL);
    let part = dir.path("panel.peet");

    // Build it from the script.
    let built = peet(&["run", &script, "--new", "--file", &part]);
    assert_eq!(built.code, OK, "{}", built.err);
    assert_eq!(built.replies.len(), 5);
    assert!(built.replies.iter().all(|r| r["ok"] == true));
    assert_eq!(built.replies[4]["created"].as_array().unwrap().len(), 4);
    assert!(built.err.contains("Saved"), "{}", built.err);
    assert_eq!(model(&part).len(), 6);

    // Ask about it: nothing changes, so nothing is saved.
    let before = std::fs::read(&part).unwrap();
    let table = peet(&["bend_table", "-f", &part]);
    assert_eq!(table.code, OK);
    assert_eq!(
        table.replies[0]["flat_size"],
        json!([244.356636, 194.356636])
    );
    assert_eq!(table.err, "");
    assert_eq!(std::fs::read(&part).unwrap(), before);

    // Change it by name: the file is updated.
    let changed = peet(&[
        "set_parameter",
        "name=flange",
        "value=30mm",
        "--file",
        &part,
    ]);
    assert_eq!(changed.code, OK, "{}", changed.err);
    assert_eq!(changed.replies[0]["parameter"]["expression"], "30mm");
    assert_eq!(model(&part).parameters.get("flange"), Some(30.0));
    let table = peet(&["bend_table", "-f", &part, "--pretty"]);
    assert!(table.out.contains("\n  "), "indented");
    let pretty: Value = serde_json::from_str(&table.out).unwrap();
    assert_eq!(pretty["flat_size"], json!([254.356636, 204.356636]));

    // Operations as JSON, and a field that is JSON.
    let edited = peet(&[
        "op",
        r#"{"op": "edit", "feature": "Edge-Flange1", "length": 40}"#,
        r#"{"op": "features"}"#,
        "-f",
        &part,
    ]);
    assert_eq!(edited.code, OK);
    assert_eq!(edited.replies[1]["features"].as_array().unwrap().len(), 6);
    let measured = peet(&[
        "measure",
        r#"a={"face":{"feature":"Base-Flange1","side":"top"}}"#,
        "-f",
        &part,
    ]);
    assert_eq!(measured.code, OK, "{}", measured.out);

    // Export it.
    let dxf = dir.path("flat.dxf");
    let exported = peet(&["export", &format!("path={dxf}"), "-f", &part, "-q"]);
    assert_eq!(exported.code, OK, "{}", exported.out);
    assert_eq!(exported.replies[0]["format"], "dxf");
    assert!(std::fs::read_to_string(&dxf).unwrap().contains("ENTITIES"));
    assert_eq!(exported.err, "");
}

#[test]
fn a_failed_operation_stops_the_run_and_nothing_is_saved() {
    let dir = Folder::new("fail");
    let part = dir.path("panel.peet");
    let script = dir.write("panel.jsonl", PANEL);
    assert_eq!(peet(&["run", &script, "--new", "-f", &part, "-q"]).code, OK);
    let before = std::fs::read(&part).unwrap();

    let bad = dir.write(
        "bad.jsonl",
        r#"{"op": "set_parameter", "name": "flange", "value": "40mm"}
{"op": "edit", "feature": "Edge-Flange1", "lenght": 3}
{"op": "set_parameter", "name": "thickness", "value": "2mm"}
"#,
    );
    let ran = peet(&["run", &bad, "-f", &part]);
    assert_eq!(ran.code, FAILED);
    assert_eq!(ran.replies.len(), 2, "it stopped at the failure");
    assert_eq!(ran.replies[1]["ok"], false);
    assert!(
        ran.replies[1]["error"].as_str().unwrap().contains("lenght"),
        "{}",
        ran.out
    );
    assert!(ran.err.contains("not saved"), "{}", ran.err);
    assert_eq!(std::fs::read(&part).unwrap(), before);

    // --keep-going applies the rest, and still doesn't save.
    let ran = peet(&["run", &bad, "-f", &part, "--keep-going"]);
    assert_eq!(ran.code, FAILED);
    assert_eq!(ran.replies.len(), 3);
    assert_eq!(ran.replies[2]["ok"], true);
    assert_eq!(std::fs::read(&part).unwrap(), before);

    // A feature that can't be built is not a failed operation, unless --strict.
    let short = r#"{"op": "edit", "feature": "Edge-Flange1", "angle": 400}"#;
    let strict = peet(&["op", short, "-f", &part, "--strict"]);
    assert_eq!(strict.code, FAILED, "{}", strict.out);
    assert_eq!(strict.replies[0]["ok"], true);
    assert!(
        strict.err.contains("--strict") && strict.err.contains("Edge-Flange1"),
        "{}",
        strict.err
    );
    assert_eq!(std::fs::read(&part).unwrap(), before);
    let lenient = peet(&["op", short, "-f", &part, "-o", &dir.path("broken.peet")]);
    assert_eq!(lenient.code, OK);
    assert!(lenient.replies[0]["failures"].is_array());
    assert_eq!(
        std::fs::read(&part).unwrap(),
        before,
        "--out leaves --file alone"
    );
    assert!(Path::new(&dir.path("broken.peet")).exists());
}

#[test]
fn scripts_come_from_standard_input_and_parts_can_stay_in_memory() {
    let dir = Folder::new("stdin");
    // No file: a new part in memory, which the script exports itself.
    let step = dir.path("block.step");
    let script = format!(
        r#"[{{"op": "sketch", "on": "top", "draw": [{{"type": "rectangle", "from": [0, 0], "to": [60, 40]}}]}},
            {{"op": "extrude", "sketch": "Sketch1", "depth": 20}},
            {{"op": "export", "path": {}}}]"#,
        json!(step)
    );
    let ran = peet_with(&["run"], &script);
    assert_eq!(ran.code, OK, "{}{}", ran.out, ran.err);
    assert_eq!(ran.replies.len(), 3);
    assert!(Path::new(&step).exists());
    assert_eq!(
        ran.err, "",
        "it exported, so there is nothing to warn about"
    );

    // Changed, not saved and not exported: said so.
    let ran = peet_with(
        &["run", "-"],
        r#"{"op": "set_parameter", "name": "a", "value": 1}"#,
    );
    assert_eq!(ran.code, OK);
    assert!(ran.err.contains("not saved"), "{}", ran.err);
    // --no-save says nothing and writes nothing.
    let part = dir.path("kept.peet");
    let ran = peet_with(
        &["run", "--new", "-f", &part, "--no-save"],
        r#"{"op": "set_parameter", "name": "a", "value": 1}"#,
    );
    assert_eq!(ran.code, OK);
    assert!(!Path::new(&part).exists());

    // An empty part file, and the readable text of one.
    assert_eq!(peet(&["new", &part]).code, OK);
    assert!(model(&part).is_empty());
    let again = peet(&["new", &part]);
    assert_eq!(again.code, USAGE);
    assert!(again.err.contains("already exists"), "{}", again.err);
    let text = peet(&["dump", &part]);
    assert_eq!(text.code, OK);
    assert!(text.out.contains("model"), "{}", text.out);
    let ron = dir.write("part.ron", &text.out);
    assert_eq!(peet(&["pack", &ron, &dir.path("packed.peet")]).code, OK);
    assert_eq!(model(&dir.path("packed.peet")), model(&part));
}

#[test]
fn problems_with_the_command_line_and_files_are_told_apart() {
    let dir = Folder::new("usage");
    let none = dir.path("none.peet");
    let ran = peet(&["features", "-f", &none]);
    assert_eq!(ran.code, USAGE);
    assert!(ran.err.contains("--new"), "{}", ran.err);
    assert_eq!(ran.out, "");

    let junk = dir.write("junk.peet", "this is not a part");
    let ran = peet(&["features", "-f", &junk]);
    assert_eq!(ran.code, USAGE);
    assert!(ran.err.contains("junk.peet"), "{}", ran.err);

    let ran = peet(&["run", &dir.path("none.jsonl")]);
    assert_eq!(ran.code, USAGE);
    let broken = dir.write("broken.jsonl", "{\"op\": \"features\"}\n{not json}\n");
    let ran = peet(&["run", &broken]);
    assert_eq!(ran.code, USAGE);
    assert!(ran.err.contains("Line 2"), "{}", ran.err);
    assert_eq!(
        ran.out, "",
        "nothing is applied from a script that can't be read"
    );

    assert_eq!(peet(&["op", "{nope"]).code, USAGE);
    assert_eq!(peet(&["extrude", "depth"]).code, USAGE);
    assert_eq!(peet(&["run", "--frobnicate"]).code, USAGE);

    // An unknown operation is an operation that failed: the reply lists the real ones.
    let ran = peet(&["extrud", "depth=3"]);
    assert_eq!(ran.code, FAILED);
    assert!(
        ran.replies[0]["error"]
            .as_str()
            .unwrap()
            .contains("extrude")
    );
    // What only a running application can do says so.
    let ran = peet(&["view", "to=front"]);
    assert_eq!(ran.code, FAILED);
    assert!(
        ran.replies[0]["error"]
            .as_str()
            .unwrap()
            .contains("running PeetCAD")
    );

    let help = peet(&["--help"]);
    assert_eq!(help.code, OK);
    assert!(help.out.contains("USAGE") && help.out.contains("EXIT CODE"));
    assert_eq!(peet(&[]).out, help.out);
    let ops = peet(&["ops"]);
    assert_eq!(ops.code, OK);
    let listed: Value = serde_json::from_str(&ops.out).unwrap();
    assert!(listed["operations"]["edge_flange"].is_object());
    let one = peet(&["ops", "edge_flange"]);
    assert!(one.out.contains("material_inside"), "{}", one.out);
    assert_eq!(peet(&["ops", "nothing"]).code, USAGE);
}

#[test]
fn a_run_can_open_documents_beside_the_part() {
    let dir = Folder::new("documents");
    let part = dir.path("plate.peet");
    let other = dir.path("panel.peet");
    let script = dir.write(
        "two.jsonl",
        &format!(
            r#"{{"op": "sketch", "on": "top", "draw": [{{"type": "rectangle", "from": [0, 0], "to": [40, 30]}}]}}
{{"op": "extrude", "sketch": "Sketch1", "depth": 5}}
{{"op": "open_sample", "sample": "enclosure", "keep": true}}
{{"op": "set_parameter", "name": "extra", "value": "3mm"}}
{{"op": "save", "path": {}}}
{{"op": "open_sample", "sample": "housing", "keep": true}}
{{"op": "set_parameter", "name": "extra", "value": "4mm"}}
{{"op": "set_parameter", "name": "t", "value": "5mm", "document": 1}}
{{"op": "documents"}}
"#,
            json!(other)
        ),
    );
    let ran = peet(&["run", &script, "--new", "-f", &part]);
    assert_eq!(ran.code, OK, "{}", ran.err);
    let list = ran.replies.last().unwrap()["documents"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(list[2]["current"], true);
    // The part of --file is saved by the run, though it is not the current document
    // at the end; the others are the script's to save.
    assert!(ran.err.contains(&format!("Saved {part}")), "{}", ran.err);
    assert!(
        ran.err.contains("Housing") && ran.err.contains("not saved"),
        "{}",
        ran.err
    );
    assert!(
        !ran.err.contains("Enclosure Panel was changed"),
        "{}",
        ran.err
    );
    let saved = model(&part);
    assert_eq!(saved.len(), 2);
    assert!(saved.parameters.entries.iter().any(|p| p.name == "t"));
    assert!(
        model(&other)
            .parameters
            .entries
            .iter()
            .any(|p| p.name == "extra")
    );
}

#[test]
fn an_assembly_is_built_and_changed_across_calls() {
    let dir = Folder::new("assembly");
    let asm = dir.path("pair.peet");
    let script = dir.write(
        "build.jsonl",
        r#"{"op": "new", "assembly": true}
{"op": "insert", "sample": "bracket"}
{"op": "insert", "component": "Bracket-1", "at": [0, 120, 0]}
"#,
    );
    let ran = peet(&["run", &script, "--new", "-f", &asm]);
    assert_eq!(ran.code, OK, "{}", ran.err);
    assert!(model(&asm).is_assembly());

    // Later calls work on the file: one operation by name, then a part changed inside.
    let moved = peet(&["place", "component=Bracket-2", "at=[0,200,0]", "-f", &asm]);
    assert_eq!(moved.code, OK, "{}", moved.err);
    assert_eq!(
        moved.replies[0]["component"]["at"],
        json!([0.0, 200.0, 0.0])
    );
    let edit = dir.write(
        "edit.jsonl",
        r#"{"op": "open_component", "component": "Bracket-1"}
{"op": "set_parameter", "name": "extra", "value": "2mm"}
{"op": "save"}
{"op": "close"}
"#,
    );
    let ran = peet(&["run", &edit, "-f", &asm]);
    assert_eq!(ran.code, OK, "{}", ran.err);
    let saved = model(&asm);
    let assembly = saved.assembly().unwrap();
    assert_eq!(assembly.components().count(), 2);
    let part = assembly.definitions().next().unwrap();
    assert!(
        part.model
            .parameters
            .entries
            .iter()
            .any(|p| p.name == "extra")
    );
    let listed = peet(&["components", "-f", &asm]);
    assert_eq!(listed.replies[0]["components"][1]["status"], "ok");
    assert_eq!(peet(&["status", "-f", &asm]).replies[0]["kind"], "assembly");

    // A part left open with changes that were not stored is said, not lost silently.
    let forgot = dir.write(
        "forgot.jsonl",
        r#"{"op": "open_component", "component": "Bracket-1"}
{"op": "set_parameter", "name": "more", "value": "3mm"}
"#,
    );
    let ran = peet(&["run", &forgot, "-f", &asm]);
    assert_eq!(ran.code, OK);
    assert!(
        ran.err.contains("Bracket was changed and not saved"),
        "{}",
        ran.err
    );
    // Part operations on the assembly say what to do.
    let wrong = peet(&["bodies", "-f", &asm]);
    assert_eq!(wrong.code, FAILED);
    assert!(
        wrong.replies[0]["error"]
            .as_str()
            .unwrap()
            .contains("open_component")
    );
}

#[test]
fn what_the_assemblies_skill_says_about_mates_is_true() {
    // The skill's own script: a pin, placed roughly, ends up in the hole of a plate.
    let skill = peet_cli::SKILLS
        .iter()
        .find(|(name, _)| *name == "assemblies")
        .map(|(_, text)| *text)
        .unwrap();
    let script = scripts(skill)
        .into_iter()
        .find(|s| s.contains("\"concentric\""))
        .expect("the mates script");
    let ran = peet_with(&["run", "-q"], &script);
    assert_eq!(ran.code, OK, "{}", ran.out);
    let replies = &ran.replies;
    let in_hole = &replies[replies.len() - 3];
    assert_eq!(in_hole["mate"]["status"], "ok");
    assert_eq!(in_hole["moved"][0]["name"], "Pin");
    let flush = &replies[replies.len() - 2];
    let pin = &flush["moved"][0];
    // In the hole at (30, 20), its end flush with the underside, standing up through it.
    assert_eq!(pin["at"], json!([30.0, 20.0, 0.0]));
    assert_eq!(pin["max"][2], 25.0);
    let mates = replies.last().unwrap();
    assert_eq!(mates["freedom"], 1, "it can still turn in the hole");

    // Without flip the two undersides are put against each other: the pin hangs below.
    let against = script.replace("\"flip\": true, ", "");
    assert_ne!(against, script);
    let ran = peet_with(&["run", "-q"], &against);
    let flush = &ran.replies[ran.replies.len() - 2];
    assert_eq!(flush["mate"]["status"], "ok");
    assert_eq!(flush["moved"][0]["min"][2], -25.0);
    assert_eq!(flush["moved"][0]["max"][2], 0.0);

    // A part's faces are described in the part's own coordinates, wherever it is.
    let moved = script.replace(
        "\"at\": [100, 0, 50]",
        "\"at\": [-300, 80, 7], \"rotate\": {\"axis\": \"x\", \"angle\": 70}",
    );
    assert_ne!(moved, script);
    let ran = peet_with(&["run", "-q"], &moved);
    assert_eq!(ran.code, OK, "{}", ran.out);
    assert_eq!(ran.replies.last().unwrap()["freedom"], 1);
}

#[test]
fn what_the_assemblies_skill_says_about_dragging_is_true() {
    let skill = peet_cli::SKILLS
        .iter()
        .find(|(name, _)| *name == "assemblies")
        .map(|(_, text)| *text)
        .unwrap();
    let script = scripts(skill)
        .into_iter()
        .find(|s| s.contains("\"drag\""))
        .expect("the drag script");
    // The arm swings a quarter turn about the post: its end goes from +X to +Y.
    let ran = peet_with(&["run", "-q"], &script);
    assert_eq!(ran.code, OK, "{}", ran.out);
    let swung = ran.replies.last().unwrap();
    assert!(swung.get("short_by").is_none(), "{swung}");
    let turn = &swung["component"]["rotate"];
    assert_eq!(turn["axis"], json!([0.0, 0.0, 1.0]));
    assert!(
        (turn["angle"].as_f64().unwrap() - 90.0).abs() < 0.01,
        "{swung}"
    );
    assert_eq!(swung["freedom"], 1);

    // Out of reach, it says by how much; a fixed component is refused.
    let far = script.replace("\"to\": [0, 70, 0]", "\"to\": [0, 100, 0]");
    let ran = peet_with(&["run", "-q"], &far);
    let short = ran.replies.last().unwrap()["short_by"].as_f64().unwrap();
    assert!((short - 30.0).abs() < 0.01, "{short}");
    let fixed = script.replace(
        "\"drag\", \"component\": \"Arm\"",
        "\"drag\", \"component\": \"Post\"",
    );
    let ran = peet_with(&["run", "-q"], &fixed);
    assert_eq!(ran.code, FAILED);
    assert!(
        ran.replies.last().unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("is fixed")
    );
}

#[test]
fn what_the_assemblies_skill_says_about_linked_parts_is_true() {
    // The skill's commands, in a folder of their own.
    let dir = Folder::new("linked");
    let plate = dir.path("plate.peet");
    let frame = dir.path("frame.peet");
    let script = dir.write(
        "plate.jsonl",
        r#"{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [40, 30]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 5}
"#,
    );
    assert_eq!(peet(&["run", &script, "--new", "-f", &plate]).code, OK);
    let insert = json!({"op": "insert", "path": plate, "link": true}).to_string();
    let made = peet(&[
        "op",
        r#"{"op": "new", "assembly": true}"#,
        &insert,
        "--new",
        "-f",
        &frame,
    ]);
    assert_eq!(made.code, OK, "{}", made.err);
    assert_eq!(made.replies[1]["component"]["max"][2], 5.0);
    assert_eq!(
        peet(&["insert", "component=plate-1", "at=[0,0,20]", "-f", &frame]).code,
        OK
    );

    // The part changed in its own file: the assembly has it at the next call.
    assert_eq!(
        peet(&["edit", "feature=Extrude1", "depth=9", "-f", &plate]).code,
        OK
    );
    let listed = peet(&["components", "-f", &frame]);
    assert_eq!(listed.code, OK, "{}", listed.err);
    let reply = &listed.replies[0];
    assert_eq!(reply["components"][0]["max"][2], 9.0);
    assert_eq!(reply["components"][1]["max"][2], 29.0);
    assert_eq!(reply["parts"].as_array().unwrap().len(), 1);
    assert_eq!(reply["parts"][0]["link_status"], "current");
    // The link is relative to the assembly's folder, in the file too.
    assert_eq!(reply["parts"][0]["link"], "plate.peet");
    let link = model(&frame)
        .assembly()
        .unwrap()
        .definitions()
        .next()
        .unwrap()
        .link
        .clone()
        .unwrap();
    assert_eq!((link.path.as_str(), link.relative), ("plate.peet", true));

    // From the assembly: its part opens as the file, and saving it there carries over.
    let edit = dir.write(
        "edit.jsonl",
        r#"{"op": "open_component", "component": "plate-1"}
{"op": "edit", "feature": "Extrude1", "depth": 3}
{"op": "save"}
{"op": "close"}
{"op": "components"}
"#,
    );
    let ran = peet(&["run", &edit, "-f", &frame]);
    assert_eq!(ran.code, OK, "{}", ran.err);
    assert_eq!(ran.replies[2]["updated_in"], json!(["frame.peet"]));
    assert_eq!(ran.replies[4]["components"][1]["max"][2], 23.0);
    assert_eq!(model(&plate).len(), 2);

    // The file gone: a warning, and the assembly whole as it last read the part.
    std::fs::remove_file(&plate).unwrap();
    let alone = peet(&["components", "-f", &frame]);
    assert_eq!(alone.code, OK);
    assert!(alone.err.contains("can't be found"), "{}", alone.err);
    assert_eq!(alone.replies[0]["parts"][0]["link_status"], "missing");
    assert_eq!(alone.replies[0]["components"][1]["max"][2], 23.0);
    assert_eq!(peet(&["unlink", "part=plate", "-f", &frame]).code, OK);
    let own = peet(&["components", "-f", &frame]);
    assert!(own.err.is_empty(), "{}", own.err);
    assert!(own.replies[0]["parts"][0].get("link").is_none());
}

#[test]
fn what_the_assemblies_skill_says_about_step_is_true() {
    // The skill's commands, in a folder of their own.
    let dir = Folder::new("step");
    let gearbox = dir.path("gearbox.peet");
    let step = dir.path("gearbox.step");
    let received = dir.path("received.peet");
    let script = dir.write(
        "gearbox.jsonl",
        r#"{"op": "new", "assembly": true}
{"op": "insert", "sample": "bracket"}
{"op": "insert", "component": "Bracket-1", "at": [0, 120, 0]}
{"op": "insert", "sample": "housing", "name": "Bearing", "at": [60, 40, 30], "rotate": {"axis": "x", "angle": 90}}
"#,
    );
    assert_eq!(peet(&["run", &script, "--new", "-f", &gearbox]).code, OK);
    let was = peet(&["components", "-f", &gearbox]);

    let out = peet(&["export", &format!("path={step}"), "-f", &gearbox]);
    assert_eq!(out.code, OK, "{}", out.err);
    let reply = &out.replies[0];
    assert_eq!(
        (
            reply["parts"].as_u64(),
            reply["components"].as_u64(),
            reply["bodies"].as_u64()
        ),
        (Some(2), Some(3), Some(3)),
        "{reply}"
    );
    let import = json!({"op": "import_step", "path": step}).to_string();
    let made = peet(&[
        "op",
        r#"{"op": "new", "assembly": true}"#,
        &import,
        "--new",
        "-f",
        &received,
    ]);
    assert_eq!(made.code, OK, "{}", made.err);
    assert_eq!(
        made.replies[1]["components"],
        json!(["Bracket-1", "Bracket-2", "Bearing"])
    );
    assert_eq!(made.replies[1]["parts"], 2);

    // The same components in the same places, fixed; two parts.
    let now = peet(&["components", "-f", &received]);
    assert_eq!(now.code, OK, "{}", now.err);
    let (now, was) = (&now.replies[0], &was.replies[0]);
    assert_eq!(now["parts"].as_array().unwrap().len(), 2);
    let pairs = now["components"]
        .as_array()
        .unwrap()
        .iter()
        .zip(was["components"].as_array().unwrap());
    for (now, was) in pairs {
        assert_eq!(now["fixed"], true);
        assert_eq!(now["status"], "ok");
        for key in ["min", "max"] {
            for axis in 0..3 {
                let (a, b) = (
                    now[key][axis].as_f64().unwrap(),
                    was[key][axis].as_f64().unwrap(),
                );
                assert!((a - b).abs() < 1e-4, "{key}: {now} / {was}");
            }
        }
    }
    // "To let mates move it": unfixed, it is free.
    let freed = peet(&["fix", "component=Bearing", "on=false", "-f", &received]);
    assert_eq!(freed.code, OK, "{}", freed.err);
    assert_eq!(freed.replies[0]["component"]["freedom"], 6);
}

#[test]
fn what_the_assemblies_skill_says_about_showing_and_exploding_is_true() {
    let skill = peet_cli::SKILLS
        .iter()
        .find(|(name, _)| *name == "assemblies")
        .map(|(_, text)| *text)
        .unwrap();
    let script = scripts(skill)
        .into_iter()
        .find(|s| s.contains("\"explode_step\""))
        .expect("the script that explodes an assembly");
    // Asked while it is shown exploded: the components are where they were put.
    let script = format!("{script}{{\"op\": \"components\"}}\n{{\"op\": \"bom\"}}\n");
    let ran = peet_with(&["run", "-q"], &script);
    assert_eq!(ran.code, OK, "{}", ran.out);
    let n = ran.replies.len();
    let (steps, components, bom) = (
        &ran.replies[n - 3],
        &ran.replies[n - 2],
        &ran.replies[n - 1],
    );
    assert_eq!(steps["exploded"], true);
    assert_eq!(steps["steps"].as_array().unwrap().len(), 2);
    // "Bearing ends 110 above where it is."
    assert_eq!(
        steps["moved"],
        json!([
            {"component": "Top", "by": [0.0, 0.0, 60.0], "at": [0.0, 0.0, 100.0]},
            {"component": "Bearing", "by": [0.0, 0.0, 110.0], "at": [60.0, 40.0, 190.0]},
        ])
    );
    let listed = components["components"].as_array().unwrap();
    assert_eq!(listed[2]["at"], json!([60.0, 40.0, 80.0]));
    assert_eq!(listed[1]["color"], "#c82828");
    assert!(listed[0].get("color").is_none());
    // Isolated and shown again: nothing is hidden, and everything is counted.
    assert!(listed.iter().all(|c| c.get("hidden").is_none()));
    assert_eq!(bom["quantity"], 3);
    let isolated = ran
        .replies
        .iter()
        .find(|r| r["op"] == "isolate")
        .expect("the isolate reply");
    assert_eq!(isolated["hidden"], json!(["Base", "Top"]));
}

#[test]
fn what_the_assemblies_skill_says_about_patterns_is_true() {
    let skill = peet_cli::SKILLS
        .iter()
        .find(|(name, _)| *name == "assemblies")
        .map(|(_, text)| *text)
        .unwrap();
    let script = scripts(skill)
        .into_iter()
        .find(|s| s.contains("\"component_pattern\""))
        .expect("the script that patterns a component");
    // "place ... on a copy fails and says which pattern made it and its original."
    let script = format!(
        "{script}{{\"op\": \"bom\"}}\n{{\"op\": \"place\", \"component\": \"Screw M4-1\", \"at\": [0, 0, 0]}}\n"
    );
    let ran = peet_with(&["run", "-q"], &script);
    let n = ran.replies.len();
    let (pattern, listed, bom, refused) = (
        &ran.replies[n - 4],
        &ran.replies[n - 3],
        &ran.replies[n - 2],
        &ran.replies[n - 1],
    );
    // "count 2 with a second direction of 2 makes three copies."
    assert_eq!(pattern["pattern"]["name"], "Screws");
    assert_eq!(pattern["pattern"]["copies"].as_array().unwrap().len(), 3);
    assert_eq!(pattern["pattern"]["status"], "ok");
    // Mated once: the original is in its hole, and the copies are in the other three.
    let components = listed["components"].as_array().unwrap();
    assert_eq!(components.len(), 6);
    let at: Vec<&serde_json::Value> = components.iter().skip(2).map(|c| &c["at"]).collect();
    assert_eq!(
        at,
        [
            &json!([20.0, 25.0, 1.5]),
            &json!([220.0, 25.0, 1.5]),
            &json!([20.0, 135.0, 1.5]),
            &json!([220.0, 135.0, 1.5])
        ]
    );
    assert_eq!(components[1]["at"], json!([0.0, 0.0, 40.0]));
    for copy in &components[3..] {
        assert_eq!(copy["pattern"], "Screws");
        assert_eq!(copy["status"], "ok");
    }
    assert_eq!(listed["patterns"][0]["direction"]["component"], "Chassis");
    // "The bill of materials counts them."
    let screws = bom["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["part"] == "Screw M4")
        .expect("a line for the screws");
    assert_eq!(screws["quantity"], 4);
    assert_eq!(refused["ok"], false);
    let why = refused["error"].as_str().unwrap();
    assert!(why.contains("Screws") && why.contains("Screw"), "{why}");

    // The finished example opens fully held.
    let dir = Folder::new("sample-assembly");
    let file = dir.path("enclosure.peet");
    let made = peet(&[
        "op",
        r#"{"op": "open_sample", "sample": "assembly"}"#,
        r#"{"op": "components"}"#,
        "--new",
        "-f",
        &file,
    ]);
    assert_eq!(made.code, OK, "{}", made.err);
    assert_eq!(made.replies[1]["freedom"], 0);
    assert_eq!(made.replies[1]["components"].as_array().unwrap().len(), 13);
    let again = peet(&["interference", "-f", &file]);
    assert_eq!(again.replies[0]["clear"], true);
}

#[test]
fn what_the_assemblies_skill_says_about_checking_is_true() {
    let skill = peet_cli::SKILLS
        .iter()
        .find(|(name, _)| *name == "assemblies")
        .map(|(_, text)| *text)
        .unwrap();
    let script = scripts(skill)
        .into_iter()
        .find(|s| s.contains("\"interference\""))
        .expect("the script that checks an assembly");
    let ran = peet_with(&["run", "-q"], &script);
    assert_eq!(ran.code, OK, "{}", ran.out);
    let n = ran.replies.len();
    let (found, bom, mass) = (
        &ran.replies[n - 3],
        &ran.replies[n - 2],
        &ran.replies[n - 1],
    );
    // A pin in a hole of exactly its size touches, and does not interfere.
    assert_eq!(found["clear"], true, "{found}");
    assert_eq!(found["compared"], 1);
    // Two parts, each with its material: the whole is weighed.
    let pi = std::f64::consts::PI;
    let plate = (60.0 * 40.0 * 6.0 - pi * 25.0 * 6.0) * 7850e-9;
    let pin = pi * 25.0 * 25.0 * 2680e-9;
    assert_eq!(bom["rows"].as_array().unwrap().len(), 2);
    assert_eq!(bom["rows"][1]["material"], "Aluminium 5052-H32");
    assert!(
        (bom["mass_kg"].as_f64().unwrap() - (plate + pin)).abs() < 1e-5,
        "{bom}"
    );
    assert!((mass["total"]["mass_kg"].as_f64().unwrap() - (plate + pin)).abs() < 1e-5);
    assert_eq!(mass["total"]["center_of_gravity_of"], "mass");

    // The pin moved off the hole goes through the plate: the slug it would cut out.
    let off = script.replace("\"at\": [30, 20, 0]", "\"at\": [12, 20, 0]");
    assert_ne!(off, script);
    let ran = peet_with(&["run", "-q"], &off);
    let found = &ran.replies[ran.replies.len() - 3];
    assert_eq!(found["clear"], false);
    let hit = &found["interferences"][0];
    assert_eq!(
        (hit["a"].as_str(), hit["b"].as_str()),
        (Some("Plate"), Some("Pin"))
    );
    assert!(
        (hit["volume_mm3"].as_f64().unwrap() - pi * 25.0 * 6.0).abs() < 1e-4,
        "{hit}"
    );

    // A part with no material: the whole has no mass, and the replies say which part.
    let bare = script.replacen(
        "{\"op\": \"set_material\", \"material\": \"Aluminium 5052-H32\"}
",
        "",
        1,
    );
    assert_ne!(bare, script);
    let ran = peet_with(&["run", "-q"], &bare);
    let n = ran.replies.len();
    assert_eq!(ran.replies[n - 2]["without_mass"], json!(["Part1"]));
    assert_eq!(ran.replies[n - 1]["without_material"], json!(["Part1"]));
    assert!(ran.replies[n - 1]["total"].get("mass_kg").is_none());
}

#[test]
fn materials_come_from_a_file_for_the_run() {
    let dir = Folder::new("materials");
    let part = dir.path("panel.peet");
    let script = dir.write("panel.jsonl", PANEL);
    assert_eq!(peet(&["run", &script, "--new", "-f", &part, "-q"]).code, OK);

    // Write the built-in tables out, add a material, and use the file.
    let csv = dir.path("gauges.csv");
    let ran = peet(&[
        "op",
        r#"{"op": "set_gauge", "material": "Titanium", "gauge": "2 mm", "thickness": 2, "radius": 4}"#,
        &json!({"op": "export_materials", "path": csv}).to_string(),
    ]);
    assert_eq!(ran.code, OK, "{}", ran.out);
    let unknown = peet(&[
        "apply_material",
        "material=Titanium",
        "gauge=2 mm",
        "-f",
        &part,
    ]);
    assert_eq!(unknown.code, FAILED, "the built-in tables have no titanium");
    let applied = peet(&[
        "apply_material",
        "material=Titanium",
        "gauge=2 mm",
        "-f",
        &part,
        "--materials",
        &csv,
    ]);
    assert_eq!(applied.code, OK, "{}{}", applied.out, applied.err);
    assert_eq!(applied.replies[0]["applied"]["thickness"], 2.0);
    let bad = peet(&["features", "-f", &part, "--materials", &part]);
    assert_eq!(bad.code, USAGE);
}

#[test]
fn the_binary_runs() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_peet"))
        .args(["status", "--pretty"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["ok"], true);
    assert_eq!(status["features"], 0);

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_peet"))
        .args(["extrude", "sketch=Nothing"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(FAILED));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_peet"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("peet "));
}

#[test]
fn skills_are_listed_and_printed() {
    let list = peet(&["skills"]);
    assert_eq!(list.code, OK);
    for (name, _) in peet_cli::SKILLS {
        assert!(
            list.out.contains(&format!("\n{name}\n")),
            "{name}: {}",
            list.out
        );
        let one = peet(&["skills", name]);
        assert_eq!(one.code, OK);
        assert!(
            one.out.starts_with("---\nname: "),
            "{name} starts with its front matter"
        );
        assert!(one.out.contains("\ndescription: "), "{name}");
    }
    assert!(list.out.contains("peet skills core"));
    let core = peet(&["skills", "core"]).out;
    // Every skill the core one points to exists.
    for (name, _) in peet_cli::SKILLS.iter().skip(1) {
        assert!(
            core.contains(&format!("peet skills {name}")),
            "core points to {name}"
        );
    }
    let none = peet(&["skills", "welding"]);
    assert_eq!(none.code, USAGE);
    assert!(none.err.contains("core, sketching"), "{}", none.err);
    assert!(peet(&["--help"]).out.contains("peet skills"));
}

/// The scripts in a skill: the contents of its ```jsonl blocks.
fn scripts(skill: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current: Option<String> = None;
    for line in skill.lines() {
        match &mut current {
            None if line.trim() == "```jsonl" => current = Some(String::new()),
            None => {}
            Some(script) if line.trim() == "```" => {
                found.push(std::mem::take(script));
                current = None;
            }
            Some(script) => {
                script.push_str(line);
                script.push('\n');
            }
        }
    }
    found
}

#[test]
fn every_script_in_the_skills_runs_and_builds() {
    let mut count = 0;
    for (name, skill) in peet_cli::SKILLS {
        for script in scripts(skill) {
            count += 1;
            // Each is a whole script: it starts from an empty part, every operation is
            // applied, and every feature it makes is built.
            let ran = peet_with(&["run", "--strict", "-q"], &script);
            assert_eq!(ran.code, OK, "{name}:\n{script}\n{}\n{}", ran.out, ran.err);
            for reply in &ran.replies {
                assert_eq!(reply["ok"], true, "{name}: {reply}");
                for made in reply["created"].as_array().into_iter().flatten() {
                    assert_eq!(made["status"], "ok", "{name}: {reply}");
                }
                if let Some(feature) = reply.get("feature") {
                    assert_eq!(feature["status"], "ok", "{name}: {reply}");
                }
                assert_ne!(reply["definition"], "over_defined", "{name}: {reply}");
            }
        }
    }
    assert!(count >= 5, "each skill has a script");
}

#[test]
fn what_the_skills_say_about_operations_is_true() {
    // An over-defined sketch is a warning, and `remove` fixes it.
    let ran = peet_with(
        &["run", "-q"],
        r#"{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [100, 60], "as": "r"}, {"type": "length", "of": ["r.bottom"], "value": 100}, {"type": "length", "of": ["r.top"], "value": 90}]}
{"op": "draw", "sketch": "Sketch1", "draw": [{"type": "remove", "dimensions": ["d2"]}]}
{"op": "draw", "sketch": "Sketch1", "draw": [{"type": "remove", "dimensions": ["d9"]}]}
"#,
    );
    assert_eq!(ran.code, FAILED);
    assert_eq!(ran.replies[0]["definition"], "over_defined");
    assert_eq!(ran.replies[0]["created"][0]["status"], "warning");
    assert_eq!(ran.replies[1]["definition"], "under_defined");
    assert_eq!(ran.replies[1]["feature"]["status"], "ok");
    assert!(ran.replies[2]["error"].as_str().unwrap().contains("d9"));

    // A flange on an edge of the top face goes up; on one of the bottom face, down.
    let ran = peet_with(
        &["run", "-q"],
        r#"{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [200, 150]}]}
{"op": "base_flange", "sketch": "Sketch1"}
{"op": "edge_flange", "edge": {"between": [[0, 0, 1.5], [200, 0, 1.5]]}}
{"op": "bodies"}
{"op": "edge_flange", "edge": {"between": [[0, 150, 0], [200, 150, 0]]}}
{"op": "bodies"}
{"op": "sketch", "on": {"feature": "Base-Flange1", "side": "top"}, "draw": [{"type": "circle", "center": [50, 50], "radius": 3}]}
{"op": "cut", "sketch": "Sketch2", "end": "through_all"}
"#,
    );
    assert_eq!(ran.code, OK, "{}", ran.out);
    assert_eq!(ran.replies[3]["bodies"][0]["min"][2], 0.0);
    assert!(ran.replies[3]["bodies"][0]["max"][2].as_f64().unwrap() > 10.0);
    assert!(ran.replies[5]["bodies"][0]["min"][2].as_f64().unwrap() < -10.0);
    // A plain cut on a sheet metal body is a warning that says to use a sheet metal cut.
    let cut = &ran.replies[7]["created"][0];
    assert_eq!(cut["status"], "warning");
    assert!(cut["message"].as_str().unwrap().contains("sheet metal cut"));

    // The planes a sketch can be on point where the skill says.
    for (plane, x, y) in [
        ("top", [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ("front", [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ("right", [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    ] {
        let ran = peet(&["sketch", &format!("on={plane}"), "-q"]);
        assert_eq!(ran.replies[0]["plane"]["x"], json!(x), "{plane}");
        assert_eq!(ran.replies[0]["plane"]["y"], json!(y), "{plane}");
    }
    // `peet ops draw` includes what goes in a draw list.
    let draw = peet(&["ops", "draw"]);
    assert!(draw.out.contains("draw items") && draw.out.contains("polyline"));
    assert_eq!(peet(&["ops", "selectors"]).code, OK);
}

#[test]
fn every_operation_that_adds_a_feature_is_named_in_a_skill() {
    // AGENTS.md asks for this: a feature an agent isn't told about is not finished.
    let ops = peet(&["ops"]);
    let listed: Value = serde_json::from_str(&ops.out).unwrap();
    let all: String = peet_cli::SKILLS.iter().map(|(_, text)| *text).collect();
    let mut features = 0;
    for (name, entry) in listed["operations"].as_object().unwrap() {
        // The operations that add a feature list their fields one by one (or have
        // several forms that do); the others describe theirs in a sentence.
        if entry["fields"].is_object() || entry["forms"].is_object() {
            features += 1;
            assert!(
                all.contains(&format!("`{name}`")),
                "the operation '{name}' adds a feature, and no skill in crates/peet-cli/skills names it (see AGENTS.md)"
            );
        }
    }
    assert!(features >= 30, "{features}");
}

#[test]
fn what_the_skills_say_about_lofts_freeform_faces_and_conversion_is_true() {
    let status = |reply: &Value| reply["created"][0]["status"].clone();
    let message = |reply: &Value| {
        reply["created"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    // Profiles with different numbers of sides don't loft.
    let ran = peet_with(
        &["run", "-q"],
        r#"{"op": "sketch", "on": "top", "name": "Base", "draw": [{"type": "rectangle", "from": [-20, -15], "to": [20, 15]}]}
{"op": "plane", "from": "top", "distance": 30, "name": "Up"}
{"op": "sketch", "on": "Up", "name": "Tri", "draw": [{"type": "polygon", "center": [0, 0], "vertex": [8, 0], "sides": 3}]}
{"op": "loft", "profiles": ["Base", "Tri"]}
"#,
    );
    assert_eq!(status(&ran.replies[3]), "failed");
    assert!(message(&ran.replies[3]).contains("same number"));

    // A loft's sides are freeform; fillets and shells are refused there, and `normal`
    // alone doesn't find them.
    let ran = peet_with(
        &["run", "-q", "--keep-going"],
        r#"{"op": "sketch", "on": "top", "name": "Base", "draw": [{"type": "rectangle", "from": [-20, -15], "to": [20, 15]}]}
{"op": "plane", "from": "top", "distance": 30, "name": "Up"}
{"op": "sketch", "on": "Up", "name": "Neck", "draw": [{"type": "circle", "center": [0, 0], "radius": 8}]}
{"op": "loft", "profiles": ["Base", "Neck"]}
{"op": "faces"}
{"op": "fillet", "size": 2, "edges": [{"body": 0, "index": 0}]}
{"op": "shell", "thickness": 1}
{"op": "measure", "a": {"face": {"feature": "Loft1", "side": "side", "normal": [1, 0, 0]}}}
"#,
    );
    assert_eq!(status(&ran.replies[3]), "ok");
    let freeform = ran.replies[4]["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["surface"] == "freeform")
        .count();
    assert!(freeform > 0);
    assert_eq!(status(&ran.replies[5]), "failed");
    assert_eq!(status(&ran.replies[6]), "failed");
    assert!(message(&ran.replies[6]).contains("freeform"));
    assert_eq!(ran.replies[7]["ok"], false);

    // A solid of two thicknesses is not converted, and says why.
    let ran = peet_with(
        &["run", "-q"],
        r#"{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 2}
{"op": "sketch", "on": {"normal": [0, 0, 1]}, "draw": [{"type": "rectangle", "from": [0, 0], "to": [20, 40]}]}
{"op": "extrude", "sketch": "Sketch2", "depth": 5}
{"op": "convert_to_sheet"}
"#,
    );
    assert_eq!(status(&ran.replies[4]), "failed");
    assert!(message(&ran.replies[4]).contains("one thickness"));
}
