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
                    // A feature that was just suppressed is suppressed, not built.
                    let wanted = if reply["op"] == "suppress" {
                        "suppressed"
                    } else {
                        "ok"
                    };
                    assert_eq!(feature["status"], wanted, "{name}: {reply}");
                }
                assert_ne!(reply["definition"], "over_defined", "{name}: {reply}");
            }
        }
    }
    assert!(count >= 6, "each skill has a script");
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

#[test]
fn what_the_demo_found_is_fixed() {
    let status = |reply: &Value, i: usize| reply["created"][i]["status"].clone();
    // A plain tray: four walls set back a little from the corners, with the default
    // relief. The corners are notched and the sheet stays in one piece; the walls are
    // named from one name.
    let tray = r#"{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [180, 120]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 1.5, "radius": 2}
{"op": "edge_flange", "name": "Wall", "length": 30, "offset_start": 5, "offset_end": 5, "edges": [{"between": [[0, 0, 1.5], [180, 0, 1.5]]}, {"between": [[180, 0, 1.5], [180, 120, 1.5]]}, {"between": [[180, 120, 1.5], [0, 120, 1.5]]}, {"between": [[0, 120, 1.5], [0, 0, 1.5]]}]}
"#;
    let ran = peet_with(
        &["run", "-q", "--strict"],
        &format!("{tray}{{\"op\": \"bend_table\"}}\n"),
    );
    assert_eq!(ran.code, OK, "{}", ran.out);
    for i in 0..4 {
        assert_eq!(status(&ran.replies[2], i), "ok", "{}", ran.replies[2]);
        assert_eq!(
            ran.replies[2]["created"][i]["name"],
            format!("Wall{}", i + 1)
        );
    }
    assert_eq!(ran.replies[3]["pieces"], 1);

    // A wall's `top` face looks into the tray; a hem on its edge folds inside, and on
    // the outside face's edge, outside.
    for (y, outside) in [(1.5, false), (0.0, true)] {
        let script = format!(
            "{tray}{{\"op\": \"hem\", \"length\": 8, \"edge\": {{\"between\": [[5, {y}, 30], [175, {y}, 30]]}}}}\n{{\"op\": \"bodies\"}}\n{{\"op\": \"measure\", \"a\": {{\"face\": {{\"feature\": \"Wall1\", \"side\": \"top\"}}}}}}\n"
        );
        let ran = peet_with(&["run", "-q", "--strict"], &script);
        assert_eq!(ran.code, OK, "{}", ran.out);
        let min_y = ran.replies[4]["bodies"][0]["min"][1].as_f64().unwrap();
        assert_eq!(min_y < -1.0, outside, "hem on the y = {y} edge: {min_y}");
    }

    // A full revolve has only side faces: the error says which faces it has.
    let ring = r#"{"op": "sketch", "on": "front", "draw": [{"type": "rectangle", "from": [11, 58], "to": [22, 63]}]}
{"op": "revolve", "sketch": "Sketch1", "name": "Collar"}
"#;
    let ran = peet_with(
        &["run", "-q", "--keep-going"],
        &format!(
            "{ring}{}\n{}\n{}\n{}\n",
            r#"{"op": "measure", "a": {"face": {"feature": "Collar", "side": "end"}}}"#,
            r#"{"op": "measure", "a": {"face": {"feature": "Collar", "normal": [0, 0, 1]}}}"#,
            r#"{"op": "chamfer", "size": 1, "edges": [{"at": [22, 0, 63]}]}"#,
            r#"{"op": "chamfer", "size": 1, "edges": [{"at": [0, 22, 63]}]}"#,
        ),
    );
    let none = ran.replies[2]["error"].as_str().unwrap();
    assert!(none.contains("The faces Collar made are"), "{none}");
    assert!(
        none.contains("side \"side\", normal [0.0,0.0,1.0]"),
        "{none}"
    );
    assert_eq!(ran.replies[3]["ok"], true);
    // A point on the seam is on two edges: the error says to move along the edge.
    let seam = ran.replies[4]["error"].as_str().unwrap();
    assert!(
        seam.contains("2 edges match") && seam.contains("further along"),
        "{seam}"
    );
    assert_eq!(status(&ran.replies[5], 0), "ok");

    // A point on an edge between two faces: the error says to add the normal.
    let ran = peet_with(
        &["run", "-q", "--keep-going"],
        r#"{"op": "sketch", "on": "top", "draw": [{"type": "center_rectangle", "center": [0, 0], "corner": [50, 40], "as": "r"}, {"type": "coincident", "of": ["r.center", "origin"]}, {"type": "vertical_distance", "of": ["r.center", "origin"], "value": 0}]}
{"op": "sketch", "on": "top", "draw": [{"type": "center_rectangle", "center": [0, 0], "corner": [50, 40], "as": "r"}, {"type": "coincident", "of": ["r.center", "origin"]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 8}
{"op": "plane", "from": {"at": [0, 40, 8]}}
"#,
    );
    // A dimension of 0 is refused, and the message gives the alternative.
    let zero = ran.replies[0]["error"].as_str().unwrap();
    assert!(
        zero.contains("can't be 0") && zero.contains("coincident"),
        "{zero}"
    );
    assert_eq!(ran.replies[1]["ok"], true);
    let edge = ran.replies[3]["error"].as_str().unwrap();
    assert!(
        edge.contains("2 faces match") && edge.contains("add \"normal\""),
        "{edge}"
    );

    // `peet ops edge_flange` lists `edges`.
    assert!(peet(&["ops", "edge_flange"]).out.contains("\"edges\""));
    assert!(peet(&["ops", "hem"]).out.contains("\"edges\""));
}

#[test]
fn what_the_second_demo_found_is_fixed() {
    // Walls set back 5 from each end of their edges have bends that long, whatever
    // order they are added in, and the sheet is one piece.
    let ran = peet_with(
        &["run", "-q", "--strict"],
        r#"{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [180, 120]}]}
{"op": "base_flange", "sketch": "Sketch1", "thickness": 1.5, "radius": 2}
{"op": "edge_flange", "name": "Wall", "length": 30, "offset_start": 5, "offset_end": 5, "edges": [{"between": [[0, 0, 1.5], [180, 0, 1.5]]}, {"between": [[180, 0, 1.5], [180, 120, 1.5]]}, {"between": [[180, 120, 1.5], [0, 120, 1.5]]}, {"between": [[0, 120, 1.5], [0, 0, 1.5]]}]}
{"op": "bend_table"}
{"op": "faces"}
"#,
    );
    assert_eq!(ran.code, OK, "{}", ran.out);
    let lengths: Vec<f64> = ran.replies[3]["bends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["length"].as_f64().unwrap())
        .collect();
    assert_eq!(lengths, [170.0, 110.0, 170.0, 110.0]);
    assert_eq!(ran.replies[3]["pieces"], 1);
    // `faces` says what made each face as selector values.
    let faces = ran.replies[4]["faces"].as_array().unwrap();
    let inside = faces
        .iter()
        .find(|f| f["made_by"] == json!([{"feature": "Wall3", "side": "top"}]))
        .expect("the back wall's inside face");
    assert_eq!(inside["normal"], json!([0.0, -1.0, 0.0]));
    assert_eq!(inside["what"], "the top face of Wall3");

    // A lofted body's size is its real size, and a face of a feature made at several
    // places says so once.
    let ran = peet_with(
        &["run", "-q", "--strict"],
        r#"{"op": "sketch", "on": "top", "name": "Base", "draw": [{"type": "rectangle", "from": [-20, -15], "to": [20, 15]}]}
{"op": "plane", "from": "top", "distance": 30, "name": "Up"}
{"op": "sketch", "on": "Up", "name": "Neck", "draw": [{"type": "circle", "center": [0, 0], "radius": 8}]}
{"op": "loft", "profiles": ["Base", "Neck"]}
{"op": "bodies"}
"#,
    );
    assert_eq!(ran.code, OK, "{}", ran.out);
    assert_eq!(
        ran.replies[4]["bodies"][0]["size"],
        json!([40.0, 30.0, 30.0])
    );

    let ran = peet_with(
        &["run", "-q", "--strict"],
        r#"{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [-50, -40], "to": [50, 40]}]}
{"op": "extrude", "sketch": "Sketch1", "depth": 8}
{"op": "sketch", "on": {"feature": "Extrude1", "side": "end"}, "name": "Centres", "draw": [{"type": "point", "at": [-40, 30], "as": "a"}, {"type": "point", "at": [40, 30]}, {"type": "horizontal_distance", "of": ["a", "origin"], "value": 40}]}
{"op": "feature", "feature": "Centres"}
{"op": "hole", "sketch": "Centres", "standard": {"size": "M5"}, "kind": "counterbore"}
{"op": "feature", "feature": "Hole1"}
{"op": "faces"}
"#,
    );
    assert_eq!(ran.code, OK, "{}", ran.out);
    // The dimensioned point stayed on the side it was drawn; the sketch says which
    // points are still free.
    let points = ran.replies[3]["entities"].as_array().unwrap();
    assert_eq!(points[0]["at"], json!([-40.0, 30.0]));
    assert_eq!(points[0]["free"], true, "its height is not fixed");
    assert_eq!(points[1]["free"], true);
    assert_eq!(ran.replies[3]["definition"], "under_defined");
    // The sizes the skill quotes for an M5 hole.
    let hole = &ran.replies[5]["fields"];
    assert_eq!(hole["diameter"], 5.5);
    assert_eq!(hole["counterbore_diameter"], 10.0);
    assert_eq!(hole["counterbore_depth"], 5.4);
    for face in ran.replies[6]["faces"].as_array().unwrap() {
        let what = face["what"].as_str().unwrap();
        assert!(!what.contains("copy of"), "{what}");
    }
    assert!(ran.replies[6]["faces"].as_array().unwrap().iter().any(|f| {
        f["what"] == "the side face of Hole1 (one of several)"
            && f["made_by"] == json!([{"feature": "Hole1", "side": "side"}])
    }));

    // A new part is called after its file from the start.
    let dir = Folder::new("named");
    let part = dir.path("bracket.peet");
    let ran = peet(&["status", "--new", "-f", &part, "-q"]);
    assert_eq!(ran.replies[0]["name"], "bracket.peet");
}
