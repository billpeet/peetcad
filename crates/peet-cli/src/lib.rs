//! `peet`: PeetCAD's command line.
//!
//! It applies operations (`peet_ops`: the ones the application uses) to a part, without
//! a window: build a part from a script, change a parameter, ask what a part contains,
//! export it. One JSON reply per operation goes to standard output; what it did to files,
//! and what went wrong outside an operation, goes to standard error.
//!
//! ```text
//! peet run build.jsonl --new --file bracket.peet
//! peet set_parameter name=thickness value=2mm --file bracket.peet
//! peet features --file bracket.peet
//! peet export path=flat.dxf --file bracket.peet
//! ```
//!
//! The exit code is 0 if every operation was applied, 1 if one was not (or, with
//! `--strict`, if the part ends with features that can't be built), and 2 if the command
//! line or a file was the problem.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use peet_document::{Document, Session};
use peet_ops::{Headless, Op, Reply, Source, Undo, apply_in, apply_json_in, apply_session_json};
use serde_json::{Map, Value, json};

/// Every operation was applied.
pub const OK: i32 = 0;
/// An operation could not be applied (or, with `--strict`, a feature can't be built).
pub const FAILED: i32 = 1;
/// The command line or a file was the problem: nothing was applied.
pub const USAGE: i32 = 2;

const HELP: &str = "\
peet: PeetCAD's command line. It applies operations to a part, without a window.

USAGE
  peet run [SCRIPT...] [options]        apply the operations of scripts (- or none: standard input)
  peet op JSON... [options]             apply operations given as JSON
  peet OPERATION [field=value...]       apply one operation, by name
  peet ops [OPERATION]                  list the operations, or one with its fields
  peet skills [NAME]                    how to use peet, for an agent: start with 'peet skills core'
  peet new PART.peet                    make an empty part file
  peet dump PART.peet [OUT.ron]         write a part as readable text
  peet pack IN.ron PART.peet            turn the text back into a part

OPTIONS
  -f, --file PART.peet   the part to work on (without it: a new, empty part, in memory)
  -o, --out PART.peet    save the result here, not over --file
      --new              start from an empty part even if --file exists (it is overwritten)
      --no-save          don't save the part, even if it changed
      --no-caches        save without the geometry caches (smaller; the application rebuilds on open)
      --keep-going       carry on after an operation that can't be applied
      --strict           fail if the part ends with features that can't be built
      --materials CSV    use these material and gauge tables, not the built-in ones
      --pretty           indent the replies
  -q, --quiet            say nothing on standard error
  -h, --help             this text
      --version

A script is a JSON list of operations, or one operation per line (lines starting with
# or // are skipped). `peet ops` lists the operations; each is an object with an \"op\"
and its fields:

  {\"op\": \"sketch\", \"on\": \"top\", \"draw\": [{\"type\": \"rectangle\", \"from\": [0, 0], \"to\": [80, 50]}]}
  {\"op\": \"extrude\", \"sketch\": \"Sketch1\", \"depth\": 8}

A field given as field=value is JSON if it reads as JSON (8, true, [0,0,1], {\"at\":[0,0,8]}),
and text otherwise (2mm, Sketch1, flat.dxf).

One reply per operation is written to standard output, as a line of JSON. The part is
saved if it changed and every operation was applied.

EXIT CODE
  0  every operation was applied
  1  an operation was not applied (nothing is saved), or --strict found features that can't be built
  2  the command line or a file was the problem
";

/// The skills: instructions for an agent on using `peet`, kept in this repository and
/// built into the binary, so they always describe the version being run. `core` comes
/// first and points to the others.
pub const SKILLS: [(&str, &str); 5] = [
    ("core", include_str!("../skills/core.md")),
    ("sketching", include_str!("../skills/sketching.md")),
    ("selectors", include_str!("../skills/selectors.md")),
    ("solids", include_str!("../skills/solids.md")),
    ("sheet-metal", include_str!("../skills/sheet-metal.md")),
];

/// A skill's `description`, from the front matter at its top: what it is for.
fn skill_description(text: &str) -> &str {
    text.lines()
        .skip(1)
        .take_while(|line| line.trim() != "---")
        .find_map(|line| line.strip_prefix("description:"))
        .map_or("", str::trim)
}

/// What to do, from the command line.
#[derive(Debug, Default, PartialEq)]
struct Options {
    file: Option<PathBuf>,
    out: Option<PathBuf>,
    new: bool,
    no_save: bool,
    no_caches: bool,
    keep_going: bool,
    strict: bool,
    materials: Option<PathBuf>,
    pretty: bool,
    quiet: bool,
}

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Version,
    /// Scripts to run (`-`: standard input).
    Run(Vec<String>),
    /// Operations as JSON text.
    Ops(Vec<String>),
    /// One operation by name, with `field=value` arguments.
    Named(String, Vec<String>),
    /// List the operations, or describe one.
    List(Option<String>),
    /// List the skills, or print one.
    Skills(Option<String>),
    New(PathBuf),
    Dump(PathBuf, Option<PathBuf>),
    Pack(PathBuf, PathBuf),
}

fn parse(args: &[String]) -> Result<(Command, Options), String> {
    let mut options = Options::default();
    let mut words: Vec<String> = Vec::new();
    let mut help = false;
    let mut version = false;
    let mut literal = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value."))
        };
        if literal || arg == "-" || !arg.starts_with('-') {
            words.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => literal = true,
            "-f" | "--file" => options.file = Some(PathBuf::from(value("--file")?)),
            "-o" | "--out" => options.out = Some(PathBuf::from(value("--out")?)),
            "--materials" => options.materials = Some(PathBuf::from(value("--materials")?)),
            "--new" => options.new = true,
            "--no-save" => options.no_save = true,
            "--no-caches" => options.no_caches = true,
            "--keep-going" => options.keep_going = true,
            "--strict" => options.strict = true,
            "--pretty" => options.pretty = true,
            "-q" | "--quiet" => options.quiet = true,
            "-h" | "--help" => help = true,
            "--version" => version = true,
            other => {
                return Err(format!(
                    "'{other}' is not an option. (An operation's field is written field=value; -- ends the options.)"
                ));
            }
        }
    }
    if version {
        return Ok((Command::Version, options));
    }
    let mut words = words.into_iter();
    let Some(command) = words.next().filter(|_| !help) else {
        return Ok((Command::Help, options));
    };
    let rest: Vec<String> = words.collect();
    let one = |what: &str| -> Result<PathBuf, String> {
        rest.first()
            .map(PathBuf::from)
            .ok_or_else(|| format!("'peet {command}' needs {what}."))
    };
    let command = match command.as_str() {
        "help" => Command::Help,
        "run" => Command::Run(rest),
        "op" => {
            if rest.is_empty() {
                return Err("'peet op' needs an operation as JSON.".to_owned());
            }
            Command::Ops(rest)
        }
        "ops" => Command::List(rest.first().cloned()),
        "skills" => Command::Skills(rest.first().cloned()),
        "new" => Command::New(one("the part to make")?),
        "dump" => Command::Dump(one("the part to dump")?, rest.get(1).map(PathBuf::from)),
        "pack" => match &rest[..] {
            [input, output] => Command::Pack(PathBuf::from(input), PathBuf::from(output)),
            _ => {
                return Err(
                    "'peet pack' needs the text and the part to make: IN.ron PART.peet.".to_owned(),
                );
            }
        },
        _ => Command::Named(command, rest),
    };
    Ok((command, options))
}

/// `field=value` arguments as the fields of an operation: a value that reads as JSON is
/// JSON, and anything else is text.
fn named_operation(name: &str, fields: &[String]) -> Result<Value, String> {
    let mut map = Map::new();
    map.insert("op".to_owned(), json!(name));
    for field in fields {
        let Some((key, value)) = field.split_once('=') else {
            return Err(format!(
                "'{field}' is not a field: write field=value (as in depth=8, or path=flat.dxf)."
            ));
        };
        if key.is_empty() || key == "op" {
            return Err(format!("'{field}' is not a field: it has no name."));
        }
        let value = serde_json::from_str(value).unwrap_or_else(|_| json!(value));
        if map.insert(key.to_owned(), value).is_some() {
            return Err(format!("'{key}' is given twice."));
        }
    }
    Ok(Value::Object(map))
}

fn read_script(name: &str, stdin: &mut dyn Read) -> Result<Vec<Value>, String> {
    let text = if name == "-" {
        let mut text = String::new();
        stdin
            .read_to_string(&mut text)
            .map_err(|e| format!("Couldn't read standard input: {e}"))?;
        text
    } else {
        std::fs::read_to_string(name).map_err(|e| format!("Couldn't read {name}: {e}"))?
    };
    peet_ops::parse_script(&text).map_err(|e| {
        if name == "-" {
            format!("Standard input: {e}")
        } else {
            format!("{name}: {e}")
        }
    })
}

fn error_of(reply: &Reply) -> String {
    reply.json["error"]
        .as_str()
        .unwrap_or("It failed.")
        .to_owned()
}

/// Everything that is said to the user that is not a reply.
struct Notes<'a> {
    err: &'a mut dyn Write,
    quiet: bool,
}

impl Notes<'_> {
    fn say(&mut self, text: &str) {
        if !self.quiet {
            let _ = writeln!(self.err, "{text}");
        }
    }

    /// Something that stops the run: said even when quiet.
    fn problem(&mut self, text: &str) {
        let _ = writeln!(self.err, "peet: {text}");
    }
}

fn convert(command: &Command) -> Result<Option<String>, String> {
    let read =
        |p: &Path| std::fs::read(p).map_err(|e| format!("Couldn't read {}: {e}", p.display()));
    let write = |p: &Path, bytes: &[u8]| {
        std::fs::write(p, bytes).map_err(|e| format!("Couldn't write {}: {e}", p.display()))
    };
    match command {
        Command::Dump(input, output) => {
            let text = peet_io::document::to_text(&read(input)?).map_err(|e| e.message)?;
            match output {
                Some(out) => write(out, text.as_bytes()).map(|()| None),
                None => Ok(Some(text)),
            }
        }
        Command::Pack(input, output) => {
            let text = String::from_utf8(read(input)?)
                .map_err(|_| format!("{} is not UTF-8 text.", input.display()))?;
            let bytes = peet_io::document::from_text(&text).map_err(|e| e.message)?;
            write(output, &bytes).map(|()| None)
        }
        _ => Ok(None),
    }
}

/// Runs the command line `args` (without the program's name). Replies go to `out`, notes
/// and problems to `err`. Returns the exit code.
pub fn run(args: &[String], stdin: &mut dyn Read, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (command, options) = match parse(args) {
        Ok(parsed) => parsed,
        Err(e) => {
            let _ = writeln!(err, "peet: {e}\nTry 'peet --help'.");
            return USAGE;
        }
    };
    let mut notes = Notes {
        err,
        quiet: options.quiet,
    };
    let print = |out: &mut dyn Write, value: &Value, pretty: bool| {
        let text = if pretty {
            serde_json::to_string_pretty(value)
        } else {
            serde_json::to_string(value)
        };
        let _ = writeln!(out, "{}", text.unwrap_or_default());
    };

    // ---- Commands that are not operations on a part ----
    match &command {
        Command::Help => {
            let _ = write!(out, "{HELP}");
            return OK;
        }
        Command::Version => {
            let _ = writeln!(out, "peet {}", env!("CARGO_PKG_VERSION"));
            return OK;
        }
        Command::Dump(..) | Command::Pack(..) => {
            return match convert(&command) {
                Ok(text) => {
                    if let Some(text) = text {
                        let _ = writeln!(out, "{text}");
                    }
                    OK
                }
                Err(e) => {
                    notes.problem(&e);
                    USAGE
                }
            };
        }
        Command::Skills(None) => {
            let mut text =
                "Skills: how to use peet, for an agent. Print one with 'peet skills NAME'.\n\n"
                    .to_owned();
            for (name, skill) in SKILLS {
                text.push_str(&format!("{name}\n    {}\n", skill_description(skill)));
            }
            text.push_str("\nStart with 'peet skills core'.\n");
            let _ = write!(out, "{text}");
            return OK;
        }
        Command::Skills(Some(name)) => {
            return match SKILLS.iter().find(|(n, _)| n == name) {
                Some((_, skill)) => {
                    let _ = write!(out, "{skill}");
                    OK
                }
                None => {
                    let names: Vec<&str> = SKILLS.iter().map(|(n, _)| *n).collect();
                    notes.problem(&format!(
                        "There is no skill '{name}'. The skills are: {}.",
                        names.join(", ")
                    ));
                    USAGE
                }
            };
        }
        Command::List(name) => {
            let mut doc = Document::default();
            let help = apply_json_in(
                &mut Headless::default(),
                &mut doc,
                &json!({"op": "help"}),
                Undo::Step,
            )
            .json;
            return match name {
                None => {
                    print(out, &help, true);
                    OK
                }
                Some(name) => {
                    // An operation, with what goes in a draw list for the two that take
                    // one; or one of the other sections of the reference.
                    let mut entry = Map::new();
                    if let Some(op) = help["operations"].get(name) {
                        entry.insert(name.clone(), op.clone());
                        if matches!(name.as_str(), "draw" | "sketch") {
                            entry.insert("draw items".to_owned(), help["draw"].clone());
                        }
                    } else if matches!(name.as_str(), "selectors" | "values") {
                        entry.insert(name.clone(), help[name.as_str()].clone());
                    }
                    if entry.is_empty() {
                        notes.problem(&format!(
                            "There is no operation '{name}'. 'peet ops' lists them."
                        ));
                        USAGE
                    } else {
                        print(out, &Value::Object(entry), true);
                        OK
                    }
                }
            };
        }
        _ => {}
    }

    // ---- The operations to apply ----
    let mut options = options;
    let ops: Vec<Value> = match &command {
        Command::Run(scripts) => {
            let scripts: Vec<String> = if scripts.is_empty() {
                vec!["-".to_owned()]
            } else {
                scripts.clone()
            };
            let mut ops = Vec::new();
            for script in &scripts {
                match read_script(script, stdin) {
                    Ok(more) => ops.extend(more),
                    Err(e) => {
                        notes.problem(&e);
                        return USAGE;
                    }
                }
            }
            ops
        }
        Command::Ops(texts) => {
            let mut ops = Vec::new();
            for text in texts {
                match serde_json::from_str::<Value>(text) {
                    Ok(Value::Array(more)) => ops.extend(more),
                    Ok(op) => ops.push(op),
                    Err(e) => {
                        notes.problem(&format!("This operation is not valid JSON ({e}): {text}"));
                        return USAGE;
                    }
                }
            }
            ops
        }
        Command::Named(name, fields) => match named_operation(name, fields) {
            Ok(op) => vec![op],
            Err(e) => {
                notes.problem(&e);
                return USAGE;
            }
        },
        Command::New(path) => {
            if path.exists() {
                notes.problem(&format!(
                    "{} already exists. (To rebuild a part from a script, use 'peet run SCRIPT --new --file PART'.)",
                    path.display()
                ));
                return USAGE;
            }
            options.file = Some(path.clone());
            options.new = true;
            Vec::new()
        }
        _ => Vec::new(),
    };

    // ---- The part ----
    // A script can open more documents beside it; the part of --file is the one a run
    // saves by itself.
    let mut host = Headless::default();
    let mut doc = Session::default();
    let part = doc.current_id();
    if let Some(csv) = &options.materials {
        let reply = apply_in(
            &mut host,
            &mut doc,
            &Op::ImportMaterials {
                file: Source::path(csv.clone()),
            },
            Undo::Step,
        );
        if !reply.ok {
            notes.problem(&error_of(&reply));
            return USAGE;
        }
    }
    if let Some(file) = options.file.as_ref().filter(|_| !options.new) {
        if !file.exists() {
            notes.problem(&format!(
                "There is no {}. (To start a new part there, add --new.)",
                file.display()
            ));
            return USAGE;
        }
        let reply = apply_in(
            &mut host,
            &mut doc,
            &Op::Open {
                file: Source::path(file.clone()),
                discard: true,
                keep: false,
            },
            Undo::Step,
        );
        if !reply.ok {
            notes.problem(&error_of(&reply));
            return USAGE;
        }
        if let Some(warnings) = reply.json["warnings"].as_array() {
            for w in warnings.iter().filter_map(Value::as_str) {
                notes.say(&format!("{}: {w}", file.display()));
            }
        }
    }

    // ---- Apply ----
    let mut failed = false;
    let mut wrote = false;
    for op in &ops {
        let reply = apply_session_json(&mut host, &mut doc, op, Undo::Step);
        print(out, &reply.json, options.pretty);
        if reply.ok {
            wrote |= matches!(reply.json["op"].as_str(), Some("save" | "export"));
        } else {
            failed = true;
            if !options.keep_going {
                break;
            }
        }
    }
    if failed {
        notes.say(if options.keep_going {
            "An operation was not applied: the part is not saved."
        } else {
            "An operation was not applied: the run stopped there, and the part is not saved."
        });
        return FAILED;
    }
    // Documents the script opened beside the part are its own to save.
    for (id, other) in doc.documents() {
        if id != part && other.is_modified() {
            notes.say(&format!(
                "{} was changed and not saved: a document opened with \"keep\" is saved by a 'save' operation sent to it.",
                other.title()
            ));
        }
    }
    // The part itself, whichever document is current by now. A script that closed it
    // has nothing left to save.
    let Some(doc) = doc.get_mut(part) else {
        return OK;
    };
    if options.strict {
        let status = apply_json_in(&mut host, doc, &json!({"op": "status"}), Undo::Step);
        let failures: Vec<String> = status.json["failures"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|f| {
                        format!(
                            "{}: {}",
                            f["name"].as_str().unwrap_or("a feature"),
                            f["message"].as_str().unwrap_or("it can't be built")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !failures.is_empty() {
            notes.problem(&format!(
                "--strict: {} can't be built, so the part is not saved. {}",
                if failures.len() == 1 {
                    "a feature".to_owned()
                } else {
                    format!("{} features", failures.len())
                },
                failures.join(" ")
            ));
            return FAILED;
        }
    }

    // ---- Save ----
    let target = options
        .out
        .clone()
        .or_else(|| doc.file.as_ref().and_then(|f| f.path.clone()))
        .or_else(|| options.file.clone());
    let changed = doc.is_modified() || options.new || options.out.is_some();
    match target {
        Some(path) if changed && !options.no_save => {
            let reply = apply_in(
                &mut host,
                doc,
                &Op::Save {
                    path: Some(path.clone()),
                    caches: !options.no_caches,
                },
                Undo::Step,
            );
            if !reply.ok {
                notes.problem(&error_of(&reply));
                return USAGE;
            }
            notes.say(&format!("Saved {}", path.display()));
        }
        None if doc.is_modified() && !options.no_save && !wrote => {
            notes.say(
                "The part was changed in memory and not saved: give --file or --out to keep it.",
            );
        }
        _ => {}
    }
    OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn options_go_anywhere_and_fields_are_not_options() {
        let (command, options) = parse(&words(&[
            "-f",
            "a.peet",
            "set_parameter",
            "name=t",
            "--strict",
            "value=-5",
            "-q",
        ]))
        .unwrap();
        assert_eq!(
            command,
            Command::Named("set_parameter".to_owned(), words(&["name=t", "value=-5"]))
        );
        assert_eq!(options.file, Some(PathBuf::from("a.peet")));
        assert!(options.strict && options.quiet && !options.new);

        assert_eq!(parse(&[]).unwrap().0, Command::Help);
        assert_eq!(parse(&words(&["run", "--help"])).unwrap().0, Command::Help);
        assert_eq!(parse(&words(&["--version"])).unwrap().0, Command::Version);
        assert_eq!(
            parse(&words(&["run", "a.jsonl", "-"])).unwrap().0,
            Command::Run(words(&["a.jsonl", "-"]))
        );
        // After --, a word starting with a dash is not an option.
        assert_eq!(
            parse(&words(&["op", "--", "-x"])).unwrap().0,
            Command::Ops(words(&["-x"]))
        );
        assert!(
            parse(&words(&["run", "--fast"]))
                .unwrap_err()
                .contains("--fast")
        );
        assert!(
            parse(&words(&["run", "--file"]))
                .unwrap_err()
                .contains("needs a value")
        );
        assert!(parse(&words(&["pack", "a.ron"])).is_err());
    }

    #[test]
    fn fields_are_json_where_they_read_as_json() {
        let op = named_operation(
            "edge_flange",
            &words(&[
                "length=25",
                "radius=2mm",
                "flip=true",
                "name=Front",
                r#"edge={"between":[[0,0,1.5],[200,0,1.5]]}"#,
                r#"note="8""#,
            ]),
        )
        .unwrap();
        assert_eq!(
            op,
            json!({"op": "edge_flange", "length": 25, "radius": "2mm", "flip": true,
                   "name": "Front", "edge": {"between": [[0, 0, 1.5], [200, 0, 1.5]]},
                   "note": "8"})
        );
        assert!(
            named_operation("x", &words(&["depth"]))
                .unwrap_err()
                .contains("field=value")
        );
        assert!(
            named_operation("x", &words(&["a=1", "a=2"]))
                .unwrap_err()
                .contains("twice")
        );
        assert!(named_operation("x", &words(&["op=y"])).is_err());
    }
}
