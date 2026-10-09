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
//! If the part is open in a running PeetCAD, the operations are applied there instead
//! (`peet_live`): the part changes on screen, each change is an undo step of the
//! application, and nothing is saved unless the script saves.
//!
//! The exit code is 0 if every operation was applied, 1 if one was not (or, with
//! `--strict`, if the part ends with features that can't be built), and 2 if the command
//! line or a file was the problem.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use peet_document::Document;
use peet_live::{Client, Session};
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
  peet sessions                         list the running PeetCADs and the part each has open
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
      --live             apply to a running PeetCAD, and fail if there is none to apply to
      --headless         work on the file, even if it is open in a running PeetCAD
      --pid N            apply to the running PeetCAD with this process id
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

LIVE
  If the part given with --file is open in a running PeetCAD, the operations are applied
  there: it changes on screen, each change is an undo step, and it is saved only by a
  save operation. --live without --file applies to the one PeetCAD that is running.
  --new, --out and --materials work on files only: add --headless.

EXIT CODE
  0  every operation was applied
  1  an operation was not applied (nothing is saved), or --strict found features that can't be built
  2  the command line or a file was the problem
";

/// The skills: instructions for an agent on using `peet`, kept in this repository and
/// built into the binary, so they always describe the version being run. `core` comes
/// first and points to the others.
pub const SKILLS: [(&str, &str); 8] = [
    ("core", include_str!("../skills/core.md")),
    ("sketching", include_str!("../skills/sketching.md")),
    ("selectors", include_str!("../skills/selectors.md")),
    ("solids", include_str!("../skills/solids.md")),
    ("sheet-metal", include_str!("../skills/sheet-metal.md")),
    ("assemblies", include_str!("../skills/assemblies.md")),
    (
        "configurations",
        include_str!("../skills/configurations.md"),
    ),
    ("live", include_str!("../skills/live.md")),
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
    live: bool,
    headless: bool,
    pid: Option<u32>,
    /// Where the running sessions are listed, if not the usual folder.
    sessions: Option<PathBuf>,
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
    /// List the running sessions.
    Sessions,
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
            "--sessions" => options.sessions = Some(PathBuf::from(value("--sessions")?)),
            "--pid" => {
                let text = value("--pid")?;
                options.pid = Some(
                    text.parse()
                        .map_err(|_| format!("--pid needs a process id, not '{text}'."))?,
                );
            }
            "--live" => options.live = true,
            "--headless" => options.headless = true,
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
        "sessions" => Command::Sessions,
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

fn print(out: &mut dyn Write, value: &Value, pretty: bool) {
    let text = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    };
    let _ = writeln!(out, "{}", text.unwrap_or_default());
}

/// The features a `status` reply says can't be built, each with why.
fn failures_of(status: &Value) -> Vec<String> {
    status["failures"]
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
        .unwrap_or_default()
}

fn strict_problem(failures: &[String], then: &str) -> String {
    format!(
        "--strict: {} can't be built{then}. {}",
        if failures.len() == 1 {
            "a feature".to_owned()
        } else {
            format!("{} features", failures.len())
        },
        failures.join(" ")
    )
}

fn describe(session: &Session) -> String {
    let open = session.file.as_ref().map_or_else(
        || format!("{} (not saved to a file)", session.name),
        |f| f.display().to_string(),
    );
    format!("process {} has {open} open", session.pid)
}

fn running(sessions: &[Session]) -> String {
    if sessions.is_empty() {
        "No PeetCAD is running.".to_owned()
    } else {
        let list: Vec<String> = sessions.iter().map(describe).collect();
        format!("Running: {}.", list.join("; "))
    }
}

/// The running PeetCAD to apply the operations to, if they go to one: the one that has
/// `--file` open, or the one asked for with `--pid` or `--live`.
fn attach(options: &Options) -> Result<Option<Client>, String> {
    let asked = options.live || options.pid.is_some();
    if options.headless {
        return if asked {
            Err("--headless works on the file, and --live and --pid on a running PeetCAD: give one or the other.".to_owned())
        } else {
            Ok(None)
        };
    }
    if !asked && options.file.is_none() {
        return Ok(None);
    }
    let Some(dir) = options.sessions.clone().or_else(peet_live::sessions_dir) else {
        return if asked {
            Err("Couldn't find the folder the running PeetCADs are listed in.".to_owned())
        } else {
            Ok(None)
        };
    };
    let sessions = peet_live::sessions(&dir);
    let chosen = match (options.pid, &options.file) {
        (Some(pid), file) => {
            let session = sessions.iter().find(|s| s.pid == pid).ok_or_else(|| {
                format!(
                    "No PeetCAD with process id {pid} is running. {}",
                    running(&sessions)
                )
            })?;
            if let Some(file) = file.as_ref().filter(|f| !session.has_open(f)) {
                return Err(format!(
                    "{} is not what that PeetCAD has open: {}.",
                    file.display(),
                    describe(session)
                ));
            }
            session
        }
        (None, Some(file)) => match sessions.iter().find(|s| s.has_open(file)) {
            Some(session) => session,
            None if options.live => {
                return Err(format!(
                    "{} is not open in a running PeetCAD. {}",
                    file.display(),
                    running(&sessions)
                ));
            }
            None => return Ok(None),
        },
        (None, None) => match &sessions[..] {
            [session] => session,
            [] => return Err("No PeetCAD is running.".to_owned()),
            _ => {
                return Err(format!(
                    "Several PeetCADs are running: say which with --file or --pid. {}",
                    running(&sessions)
                ));
            }
        },
    };
    for (given, option) in [
        (options.new, "--new"),
        (options.out.is_some(), "--out"),
        (options.materials.is_some(), "--materials"),
    ] {
        if given {
            return Err(format!(
                "{option} works on files, and this part is open in PeetCAD ({}). Use operations instead (save, with a path, saves it somewhere else), or add --headless to work on the file anyway: PeetCAD will not see that change.",
                describe(chosen)
            ));
        }
    }
    Client::connect(chosen)
        .map(Some)
        .map_err(|e| format!("Couldn't reach PeetCAD ({}): {e}", describe(chosen)))
}

/// Applies the operations to a running PeetCAD. Nothing is saved: that is up to the
/// script, as it is up to the user in the application.
fn run_live(
    mut client: Client,
    ops: &[Value],
    options: &Options,
    out: &mut dyn Write,
    notes: &mut Notes,
) -> i32 {
    notes.say(&format!(
        "Applying to PeetCAD, where {}. It is saved only by a save operation.",
        describe(&client.session)
    ));
    let mut send = |op: &Value, notes: &mut Notes| match client.send(op) {
        Ok(reply) => Some(reply),
        Err(e) => {
            notes.problem(&format!("PeetCAD stopped answering: {e}"));
            None
        }
    };
    let mut failed = false;
    for op in ops {
        // A file is where this run would find it, not where PeetCAD was started.
        let mut op = op.clone();
        if let Some(path) = op.get("path").and_then(Value::as_str)
            && let Ok(full) = std::path::absolute(path)
        {
            op["path"] = json!(full.to_string_lossy());
        }
        let Some(reply) = send(&op, notes) else {
            return FAILED;
        };
        print(out, &reply, options.pretty);
        if reply["ok"] != true {
            failed = true;
            if !options.keep_going {
                break;
            }
        }
    }
    if failed {
        notes.say(if options.keep_going {
            "An operation was not applied. The others were applied in PeetCAD: undo there takes them back."
        } else {
            "An operation was not applied: the run stopped there. The ones before it were applied in PeetCAD: undo there takes them back."
        });
        return FAILED;
    }
    if options.strict {
        let Some(status) = send(&json!({"op": "status"}), notes) else {
            return FAILED;
        };
        let failures = failures_of(&status);
        if !failures.is_empty() {
            notes.problem(&strict_problem(&failures, ""));
            return FAILED;
        }
    }
    OK
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
        Command::Sessions => {
            let Some(dir) = options.sessions.clone().or_else(peet_live::sessions_dir) else {
                notes.problem("Couldn't find the folder the running PeetCADs are listed in.");
                return USAGE;
            };
            let sessions: Vec<Value> = peet_live::sessions(&dir)
                .iter()
                .map(|s| {
                    json!({
                        "pid": s.pid,
                        "file": s.file.as_ref().map(|f| f.to_string_lossy().into_owned()),
                        "name": s.name,
                        "modified": s.modified,
                        "version": s.version,
                    })
                })
                .collect();
            print(
                out,
                &json!({"ok": true, "sessions": sessions}),
                options.pretty,
            );
            return OK;
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

    // ---- A part that is open in a running PeetCAD is changed there ----
    if !matches!(command, Command::New(_)) {
        match attach(&options) {
            Ok(Some(client)) => return run_live(client, &ops, &options, out, &mut notes),
            Ok(None) => {}
            Err(e) => {
                notes.problem(&e);
                return USAGE;
            }
        }
    }

    // ---- The part ----
    // A script can open more documents beside it; the part of --file is the one a run
    // saves by itself.
    let mut host = Headless::default();
    let mut doc = peet_document::Session::default();
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
    if let Some(file) = options.file.as_ref().filter(|_| options.new) {
        // A new part is called after the file it will be saved to.
        doc.file = Some(peet_document::FileLocation {
            name: file.file_name().map_or_else(
                || file.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            ),
            path: Some(file.clone()),
        });
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
                "{} was changed and not saved: a document a script opens beside the part is saved by a 'save' operation sent to it.",
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
        let failures = failures_of(&status.json);
        if !failures.is_empty() {
            notes.problem(&strict_problem(&failures, ", so the part is not saved"));
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
