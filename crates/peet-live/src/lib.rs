//! How `peet` reaches a running PeetCAD.
//!
//! A running application is a **session**. It listens on a local socket (a named pipe on
//! Windows, a Unix domain socket elsewhere) and lists itself in the sessions folder: one
//! small file per session, saying which process it is, where it listens and which part it
//! has open. `peet` reads the folder, finds the session that has the part open, and sends
//! its operations there instead of working on the file.
//!
//! What goes over the socket is lines of JSON. The client says hello and the session
//! answers with what it has open; after that each line is an operation as `peet_ops`
//! reads it, and the answer is that operation's reply.
//!
//! The session side is a [`Server`]: its threads only carry lines. The operations are
//! handed to the application ([`Server::next`]), which applies them on its own thread
//! between frames and answers ([`Request::reply`]).
//!
//! Native only: in the browser this crate is empty.

#![cfg(not(target_arch = "wasm32"))]

use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name, Stream, ToNsName as _};
use serde_json::{Value, json};

/// How long an operation waits for the application to take it before the session
/// answers that it is busy (a file dialog is open in it).
pub const BUSY_AFTER: Duration = Duration::from_secs(20);

/// The folder the sessions are listed in: `sessions` in PeetCAD's data folder (which
/// `PEETCAD_DATA_DIR` moves, as it does for the application).
pub fn sessions_dir() -> Option<PathBuf> {
    // The same folder as `peet_platform::data_dir`, which the command line doesn't
    // depend on (it brings the file dialogs with it).
    let data = match std::env::var_os("PEETCAD_DATA_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let base = if cfg!(windows) {
                std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            } else if cfg!(target_os = "macos") {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join("Library/Application Support"))
            } else {
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
                    })
            };
            base?.join("PeetCAD")
        }
    };
    Some(data.join("sessions"))
}

/// A running PeetCAD, as it lists itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The process.
    pub pid: u32,
    /// The name of the socket it listens on.
    pub socket: String,
    /// The file of the part it has open, if the part has one.
    pub file: Option<PathBuf>,
    /// The name of the part it has open.
    pub name: String,
    /// Whether the part has changes that are not saved.
    pub modified: bool,
    /// The application's version.
    pub version: String,
}

impl Session {
    /// The session as JSON: what its file in the sessions folder holds, and what it
    /// answers to a hello.
    pub fn to_json(&self) -> Value {
        json!({
            "pid": self.pid,
            "socket": self.socket,
            "file": self.file.as_ref().map(|f| f.to_string_lossy().into_owned()),
            "name": self.name,
            "modified": self.modified,
            "version": self.version,
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            pid: u32::try_from(value["pid"].as_u64()?).ok()?,
            socket: value["socket"].as_str()?.to_owned(),
            file: value["file"].as_str().map(PathBuf::from),
            name: value["name"].as_str().unwrap_or_default().to_owned(),
            modified: value["modified"].as_bool().unwrap_or(false),
            version: value["version"].as_str().unwrap_or_default().to_owned(),
        })
    }

    /// Whether this session has `file` open.
    pub fn has_open(&self, file: &Path) -> bool {
        self.file.as_ref().is_some_and(|open| same_file(open, file))
    }
}

/// Whether two paths are the same file: compared as the file system resolves them, so a
/// relative path and a full one match.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let resolve = |p: &Path| {
        let full = std::fs::canonicalize(p)
            .or_else(|_| std::path::absolute(p))
            .unwrap_or_else(|_| p.to_owned());
        let text = full.to_string_lossy().into_owned();
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text
        }
    };
    resolve(a) == resolve(b)
}

fn socket_name(socket: &str) -> io::Result<Name<'_>> {
    socket.to_ns_name::<GenericNamespaced>()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn write_line(stream: &mut impl Write, value: &Value) -> io::Result<()> {
    let mut line = serde_json::to_string(value).map_err(io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()
}

/// The next line as JSON, or `None` at the end of the stream.
fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ---- The session's side ----

/// Where a request is: it is answered either by the application or, if the application
/// doesn't take it in time, by the connection (never both).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Waiting,
    Taken,
    GivenUp,
}

/// An operation a client sent, for the application to apply and answer.
pub struct Request {
    op: Value,
    answer: Sender<Value>,
}

impl Request {
    /// The operation, as JSON.
    pub fn op(&self) -> &Value {
        &self.op
    }

    /// Answers the client.
    pub fn reply(self, reply: Value) {
        // The client may have gone: then nobody is waiting.
        let _ = self.answer.send(reply);
    }
}

struct Queued {
    request: Request,
    stage: Arc<Mutex<Stage>>,
}

/// A session that is listening. Dropping it takes the session off the list.
pub struct Server {
    queue: Receiver<Queued>,
    session: Arc<Mutex<Session>>,
    entry: PathBuf,
}

impl Server {
    /// Starts listening as this process, and lists the session in `dir`. `wake` is called
    /// (from another thread) when an operation arrives, to get the application to look.
    pub fn start(
        dir: &Path,
        version: &str,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let pid = std::process::id();
        // One per process, except in tests, which start several.
        static STARTED: AtomicU32 = AtomicU32::new(0);
        let socket = match STARTED.fetch_add(1, Ordering::Relaxed) {
            0 => format!("peetcad-{pid}"),
            n => format!("peetcad-{pid}-{n}"),
        };
        let entry = dir.join(format!("{socket}.json"));
        let socket = format!("{socket}.sock");
        let listener = ListenerOptions::new()
            .name(socket_name(&socket)?)
            .create_sync()?;
        let session = Arc::new(Mutex::new(Session {
            pid,
            socket,
            file: None,
            name: String::new(),
            modified: false,
            version: version.to_owned(),
        }));
        std::fs::create_dir_all(dir)?;
        let server = Self {
            queue: {
                let (send, queue) = channel::<Queued>();
                let session = session.clone();
                let wake = Arc::new(wake);
                std::thread::Builder::new()
                    .name("peet-live".to_owned())
                    .spawn(move || {
                        for stream in listener.incoming().filter_map(Result::ok) {
                            let (send, session, wake) =
                                (send.clone(), session.clone(), wake.clone());
                            let serve = move || {
                                if let Err(e) = connection(stream, &send, &session, &*wake) {
                                    log::debug!("A peet connection ended: {e}");
                                }
                            };
                            if let Err(e) = std::thread::Builder::new()
                                .name("peet-live-connection".to_owned())
                                .spawn(serve)
                            {
                                log::warn!("Couldn't serve a peet connection: {e}");
                            }
                        }
                    })?;
                queue
            },
            session,
            entry,
        };
        server.write_entry()?;
        Ok(server)
    }

    /// This session, as it is listed.
    pub fn session(&self) -> Session {
        lock(&self.session).clone()
    }

    /// Says which part is open now. Cheap when nothing changed, so call it every frame.
    pub fn set_document(&self, file: Option<&Path>, name: &str, modified: bool) {
        {
            let mut session = lock(&self.session);
            if session.file.as_deref() == file
                && session.name == name
                && session.modified == modified
            {
                return;
            }
            session.file = file.map(Path::to_owned);
            name.clone_into(&mut session.name);
            session.modified = modified;
        }
        if let Err(e) = self.write_entry() {
            log::warn!("Couldn't update {}: {e}", self.entry.display());
        }
    }

    fn write_entry(&self) -> io::Result<()> {
        let text =
            serde_json::to_string_pretty(&self.session().to_json()).map_err(io::Error::other)?;
        // Written whole or not at all: a reader never sees half of it.
        let temporary = self.entry.with_extension("tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &self.entry)
    }

    /// The next operation waiting to be applied, if there is one.
    pub fn next(&self) -> Option<Request> {
        loop {
            let queued = self.queue.try_recv().ok()?;
            let mut stage = lock(&queued.stage);
            if *stage == Stage::Waiting {
                *stage = Stage::Taken;
                return Some(queued.request);
            }
            // The client was told the application is busy: this one is not applied.
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.entry);
    }
}

fn connection(
    stream: Stream,
    queue: &Sender<Queued>,
    session: &Mutex<Session>,
    wake: &dyn Fn(),
) -> io::Result<()> {
    let mut stream = BufReader::new(stream);
    // The hello: answered with what is open now, which may have changed since the list
    // was read.
    if read_line(&mut stream)?.is_none() {
        return Ok(());
    }
    let hello = lock(session).to_json();
    write_line(stream.get_mut(), &hello)?;

    while let Some(op) = read_line(&mut stream)? {
        let name = op["op"].as_str().unwrap_or_default().to_owned();
        let refused = |error: &str| json!({"ok": false, "op": name, "error": error});
        let (answer, answered) = channel();
        let stage = Arc::new(Mutex::new(Stage::Waiting));
        let queued = Queued {
            request: Request { op, answer },
            stage: stage.clone(),
        };
        let gone = "PeetCAD is closing.";
        if queue.send(queued).is_err() {
            write_line(stream.get_mut(), &refused(gone))?;
            return Ok(());
        }
        wake();
        let reply = match answered.recv_timeout(BUSY_AFTER) {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Disconnected) => refused(gone),
            Err(RecvTimeoutError::Timeout) => {
                let taken = {
                    let mut stage = lock(&stage);
                    if *stage == Stage::Waiting {
                        *stage = Stage::GivenUp;
                    }
                    *stage == Stage::Taken
                };
                if taken {
                    // Being applied (a long rebuild): wait for it.
                    answered.recv().unwrap_or_else(|_| refused(gone))
                } else {
                    refused(
                        "PeetCAD is not answering, so the operation was not applied: a dialog may be open in it. Close the dialog and try again.",
                    )
                }
            }
        };
        write_line(stream.get_mut(), &reply)?;
    }
    Ok(())
}

// ---- The command line's side ----

/// The sessions listed in `dir` that answer, in order of process id. A session that is
/// listed and no longer there (PeetCAD crashed) is taken off the list.
pub fn sessions(dir: &Path) -> Vec<Session> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Session> = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let listed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| Session::from_json(&value));
        let Some(listed) = listed else {
            continue;
        };
        match Client::connect(&listed) {
            Ok(client) => found.push(client.session),
            Err(e) => {
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
    found.sort_by_key(|s| s.pid);
    found
}

/// A connection to a session.
pub struct Client {
    stream: BufReader<Stream>,
    /// The session, as it answered: what it has open now.
    pub session: Session,
}

impl Client {
    /// Connects to a listed session.
    pub fn connect(listed: &Session) -> io::Result<Self> {
        let stream = Stream::connect(socket_name(&listed.socket)?)?;
        let mut stream = BufReader::new(stream);
        write_line(
            stream.get_mut(),
            &json!({"hello": "peet", "version": env!("CARGO_PKG_VERSION")}),
        )?;
        let session = read_line(&mut stream)?
            .as_ref()
            .and_then(Session::from_json)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "it didn't say what it is")
            })?;
        Ok(Self { stream, session })
    }

    /// Sends an operation and waits for its reply.
    pub fn send(&mut self, op: &Value) -> io::Result<Value> {
        write_line(self.stream.get_mut(), op)?;
        read_line(&mut self.stream)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "PeetCAD closed the connection",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("peet-live-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_session_is_listed_answers_and_is_delisted() {
        let dir = folder("listed");
        let woken = Arc::new(Mutex::new(0));
        let count = woken.clone();
        let server = Server::start(&dir, "1.2.3", move || *lock(&count) += 1).unwrap();
        server.set_document(Some(Path::new("some/part.peet")), "part.peet", true);

        // The application: applies what arrives, until told to stop.
        let app = std::thread::spawn(move || {
            loop {
                while let Some(request) = server.next() {
                    if request.op()["op"] == "stop" {
                        request.reply(json!({"ok": true}));
                        return;
                    }
                    let reply = json!({"ok": true, "echo": request.op().clone()});
                    request.reply(reply);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });

        let listed = sessions(&dir);
        assert_eq!(listed.len(), 1);
        let session = &listed[0];
        assert_eq!(session.pid, std::process::id());
        assert_eq!(session.name, "part.peet");
        assert_eq!(session.version, "1.2.3");
        assert!(session.modified);
        assert!(session.has_open(Path::new("some/../some/part.peet")));
        assert!(!session.has_open(Path::new("other.peet")));

        let mut client = Client::connect(session).unwrap();
        let reply = client.send(&json!({"op": "status", "n": 1})).unwrap();
        assert_eq!(reply["echo"]["n"], 1);
        // Two clients at once.
        let mut second = Client::connect(session).unwrap();
        assert_eq!(
            second.send(&json!({"op": "x", "n": 2})).unwrap()["echo"]["n"],
            2
        );
        assert_eq!(
            client.send(&json!({"op": "x", "n": 3})).unwrap()["echo"]["n"],
            3
        );
        assert!(*lock(&woken) >= 3);

        client.send(&json!({"op": "stop"})).unwrap();
        app.join().unwrap();
        // The server was dropped with the application.
        assert!(sessions(&dir).is_empty());
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_that_is_gone_is_taken_off_the_list() {
        let dir = folder("gone");
        std::fs::create_dir_all(&dir).unwrap();
        let stale = Session {
            pid: 1,
            socket: "peetcad-nobody-listens-here.sock".to_owned(),
            file: None,
            name: "Part1".to_owned(),
            modified: false,
            version: "0".to_owned(),
        };
        let entry = dir.join("1.json");
        std::fs::write(&entry, stale.to_json().to_string()).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a session").unwrap();
        assert!(sessions(&dir).is_empty());
        assert!(!entry.exists());
        assert!(dir.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
