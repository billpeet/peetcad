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
