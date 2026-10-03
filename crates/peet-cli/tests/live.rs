//! The command line and a running session: a part that is open in PeetCAD is changed
//! there, not in its file. The session here is a stand-in for the application: it applies
//! what arrives to a document of its own, as `PeetApp` does between frames.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use peet_cli::{FAILED, OK, USAGE};
use peet_document::Document;
use peet_live::Server;
use peet_ops::{Headless, Undo, apply_json_in};
use serde_json::{Value, json};

/// A stand-in for a running PeetCAD with `file` open.
struct Session {
    dir: PathBuf,
    stop: Arc<AtomicBool>,
    /// The operations it was sent.
    received: Arc<Mutex<Vec<Value>>>,
    thread: Option<JoinHandle<Document>>,
}

impl Session {
    fn start(name: &str, file: Option<&Path>) -> Self {
        let dir = std::env::temp_dir().join(format!("peet-cli-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let server = Server::start(&dir.join("sessions"), "test", || {}).unwrap();
        server.set_document(file, "Part1", false);
        let stop = Arc::new(AtomicBool::new(false));
        let received = Arc::new(Mutex::new(Vec::new()));
        let (stopping, log) = (stop.clone(), received.clone());
        let thread = std::thread::spawn(move || {
            let mut doc = Document::default();
            let mut host = Headless::default();
            while !stopping.load(Ordering::Relaxed) {
                while let Some(request) = server.next() {
                    log.lock().unwrap().push(request.op().clone());
                    // Nothing is written by the stand-in: files are the real thing's job.
                    let reply = if matches!(request.op()["op"].as_str(), Some("save" | "export")) {
                        json!({"ok": true, "op": request.op()["op"]})
                    } else {
                        apply_json_in(&mut host, &mut doc, request.op(), Undo::Step).json
                    };
                    request.reply(reply);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            doc
        });
        Self {
            dir,
            stop,
            received,
            thread: Some(thread),
        }
    }

    fn sessions(&self) -> String {
        self.dir.join("sessions").to_string_lossy().into_owned()
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).to_string_lossy().into_owned()
    }

    fn received(&self) -> Vec<Value> {
        self.received.lock().unwrap().clone()
    }

    /// Closes the session and gives the part it had open.
    fn close(mut self) -> Document {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Ran {
    code: i32,
    replies: Vec<Value>,
    err: String,
}

fn peet(args: &[&str]) -> Ran {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = peet_cli::run(&args, &mut "".as_bytes(), &mut out, &mut err);
    Ran {
        code,
        replies: String::from_utf8(out)
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
        err: String::from_utf8(err).unwrap(),
    }
}

const SKETCH: &str = r#"{"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}"#;
const EXTRUDE: &str = r#"{"op": "extrude", "sketch": "Sketch1", "depth": 5}"#;

#[test]
fn a_part_open_in_a_session_is_changed_there() {
    let dir = std::env::temp_dir().join(format!("peet-cli-live-file-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let part = dir.join("part.peet");
    let part_text = part.to_string_lossy().into_owned();
    let made = peet(&["new", &part_text]);
    assert_eq!(made.code, OK, "{}", made.err);
    let on_disk = std::fs::read(&part).unwrap();

    let session = Session::start("open", Some(&part));
    let sessions = session.sessions();

    // The list.
    let listed = peet(&["sessions", "--sessions", &sessions]);
    assert_eq!(listed.code, OK);
    let list = listed.replies[0]["sessions"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["pid"], std::process::id());
    assert_eq!(list[0]["file"], part_text);

    // Operations on the file go to the session that has it open.
    let ran = peet(&[
        "op",
        SKETCH,
        EXTRUDE,
        "--file",
        &part_text,
        "--sessions",
        &sessions,
    ]);
    assert_eq!(ran.code, OK, "{}", ran.err);
    assert_eq!(ran.replies[1]["created"][0]["name"], "Extrude1");
    assert!(ran.err.contains("Applying to PeetCAD"), "{}", ran.err);
    assert!(!ran.err.contains("Saved"), "{}", ran.err);
    assert_eq!(
        std::fs::read(&part).unwrap(),
        on_disk,
        "the file is not touched"
    );

    // Queries see what the session has, which the file does not.
    let bodies = peet(&["bodies", "-f", &part_text, "--sessions", &sessions]);
    assert_eq!(
        bodies.replies[0]["bodies"][0]["size"],
        json!([60.0, 40.0, 5.0])
    );
    let headless = peet(&[
        "bodies",
        "-f",
        &part_text,
        "--sessions",
        &sessions,
        "--headless",
    ]);
    assert_eq!(headless.replies[0]["bodies"], json!([]));

    // A path is where this run would find it.
    let exported = peet(&[
        "export",
        "path=out.step",
        "-f",
        &part_text,
        "--sessions",
        &sessions,
    ]);
    assert_eq!(exported.code, OK, "{}", exported.err);
    let sent = session.received();
    let path = Path::new(sent.last().unwrap()["path"].as_str().unwrap());
    assert!(path.is_absolute(), "{}", path.display());
    assert!(path.ends_with("out.step"));

    // An operation that can't be applied stops the run; what was applied stays.
    let ran = peet(&[
        "op",
        r#"{"op": "set_parameter", "name": "t", "value": 2}"#,
        r#"{"op": "extrude", "sketch": "Nothing", "depth": 5}"#,
        r#"{"op": "set_parameter", "name": "u", "value": 3}"#,
        "-f",
        &part_text,
        "--sessions",
        &sessions,
    ]);
    assert_eq!(ran.code, FAILED);
    assert_eq!(ran.replies.len(), 2);
    assert!(ran.err.contains("undo there"), "{}", ran.err);

    // What only makes sense for a file is refused, saying what to do.
    for option in [vec!["--new"], vec!["--out", "other.peet"]] {
        let mut args = vec!["status", "-f", &part_text, "--sessions", &sessions];
        args.extend(option.iter());
        let ran = peet(&args);
        assert_eq!(ran.code, USAGE);
        assert!(ran.err.contains("--headless"), "{}", ran.err);
        assert!(ran.replies.is_empty());
    }
    let ran = peet(&[
        "status",
        "-f",
        &part_text,
        "--sessions",
        &sessions,
        "--live",
        "--headless",
    ]);
    assert_eq!(ran.code, USAGE);

    // --strict asks the session.
    let ran = peet(&[
        "op",
        r#"{"op": "edit", "feature": "Extrude1", "depth": "nothing * 2"}"#,
        "--strict",
        "--keep-going",
        "-f",
        &part_text,
        "--sessions",
        &sessions,
    ]);
    assert_ne!(ran.code, OK, "{}", ran.err);

    let doc = session.close();
    assert_eq!(doc.bodies.len(), 1);
    assert!(doc.model.parameters.entries.iter().any(|p| p.name == "t"));
    assert!(!doc.model.parameters.entries.iter().any(|p| p.name == "u"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_needs_a_session_and_a_file_that_is_not_open_is_a_file() {
    let session = Session::start("unsaved", None);
    let sessions = session.sessions();
    let other = session.path("other.peet");

    // A part that no session has open is worked on as a file.
    let ran = peet(&["op", SKETCH, "--new", "-f", &other, "--sessions", &sessions]);
    assert_eq!(ran.code, OK, "{}", ran.err);
    assert!(ran.err.contains("Saved"), "{}", ran.err);
    assert!(session.received().is_empty());

    // --live says it must be open somewhere.
    let ran = peet(&["status", "--live", "-f", &other, "--sessions", &sessions]);
    assert_eq!(ran.code, USAGE);
    assert!(
        ran.err.contains("not open in a running PeetCAD"),
        "{}",
        ran.err
    );
    assert!(
        ran.err.contains("Part1"),
        "it says what is running: {}",
        ran.err
    );

    // --live without a file: the one session, whatever it has open.
    let ran = peet(&["op", SKETCH, "--live", "--sessions", &sessions]);
    assert_eq!(ran.code, OK, "{}", ran.err);
    assert_eq!(session.received().len(), 1);
    let pid = std::process::id().to_string();
    let ran = peet(&["features", "--pid", &pid, "--sessions", &sessions]);
    assert_eq!(ran.code, OK, "{}", ran.err);
    assert_eq!(ran.replies[0]["features"][0]["name"], "Sketch1");
    let ran = peet(&["features", "--pid", "1", "--sessions", &sessions]);
    assert_eq!(ran.code, USAGE);

    // Without --live or --file nothing is attached to.
    let ran = peet(&["features", "--sessions", &sessions]);
    assert_eq!(ran.replies[0]["features"], json!([]));

    // Nothing running.
    drop(session);
    let empty = std::env::temp_dir().join(format!("peet-cli-live-none-{}", std::process::id()));
    let ran = peet(&["status", "--live", "--sessions", &empty.to_string_lossy()]);
    assert_eq!(ran.code, USAGE);
    assert!(ran.err.contains("No PeetCAD is running"), "{}", ran.err);
}
