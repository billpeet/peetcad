//! Operations on a [`Session`]: several documents open together.
//!
//! [`apply_session`] is [`crate::apply_in`] for a session. An operation goes to the
//! current document, or to the one it names (`"document": "Bracket"` in JSON) without
//! making that one current. On top of that it carries out what only a session can:
//! listing the open documents, switching, closing, and opening a document beside the
//! others (`"keep": true` on `new`, `open` and `open_sample`).

use peet_document::{DocId, Document, Session};
use serde_json::{Map, Value, json};

use crate::args::{Args, integer};
use crate::host::Host;
use crate::op::{self, Op};
use crate::{Reply, Undo, apply_in, failed, query};

/// An open document, by name (`"Bracket"`, as `documents` lists it) or id.
#[derive(Clone, Debug, PartialEq)]
pub enum DocSel {
    Id(DocId),
    Name(String),
}

impl From<DocId> for DocSel {
    fn from(id: DocId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for DocSel {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

/// "1 (Bracket), 2 (Cover)": the open documents, for messages.
fn listed(session: &Session) -> String {
    session
        .documents()
        .map(|(id, d)| format!("{} ({})", id.0, d.title()))
        .collect::<Vec<_>>()
        .join(", ")
}

impl DocSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(DocId(integer(v)?))),
            Value::String(name) => Ok(Self::Name(name.clone())),
            other => Err(format!(
                "expected an open document's name or id, not {other}"
            )),
        }
    }

    /// The document this means in `session`, or why there is none (or more than one).
    pub fn resolve(&self, session: &Session) -> Result<DocId, String> {
        match self {
            Self::Id(id) => session.get(*id).map(|_| *id).ok_or_else(|| {
                format!(
                    "There is no open document with the id {}. The open documents are: {}.",
                    id.0,
                    listed(session)
                )
            }),
            Self::Name(name) => {
                let mut found = session.documents().filter(|(_, d)| d.title() == *name);
                match (found.next(), found.next()) {
                    (Some((id, _)), None) => Ok(id),
                    (None, _) => Err(format!(
                        "There is no open document called '{name}'. The open documents are: {}.",
                        listed(session)
                    )),
                    (Some(_), Some(_)) => Err(format!(
                        "More than one open document is called '{name}': use an id. The open documents are: {}.",
                        listed(session)
                    )),
                }
            }
        }
    }
}

/// Something only a session of documents can do.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionCommand {
    /// The open documents: id, name, file, whether modified, which is current.
    Documents,
    /// Make a document current: operations that name no document go to it.
    Switch { document: DocSel },
    /// Close a document (the current one if none is named). Refused if it has unsaved
    /// changes, unless told to discard them.
    Close {
        document: Option<DocSel>,
        discard: bool,
    },
}

impl SessionCommand {
    /// The operation's name, as in JSON.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Documents => "documents",
            Self::Switch { .. } => "switch",
            Self::Close { .. } => "close",
        }
    }

    /// Reads the session command called `op`, if `op` is one.
    pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Self, String>> {
        Some(match op {
            "documents" => Ok(Self::Documents),
            "switch" => a
                .required("document", DocSel::parse)
                .map(|document| Self::Switch { document }),
            "close" => (|| {
                Ok(Self::Close {
                    document: a.parsed("document", DocSel::parse)?,
                    discard: a.flag("discard", false)?,
                })
            })(),
            _ => return None,
        })
    }
}

/// The session commands, with their fields, for `help`.
pub(crate) const SESSION_OPS: &[(&str, &str, &str)] = &[
    (
        "documents",
        "",
        "The open documents: id, name, file, whether modified, and which is current.",
    ),
    (
        "switch",
        "document (a name or an id)",
        "Make an open document current: operations that name no document go to it.",
    ),
    (
        "close",
        "document (default the current one), discard (default false)",
        "Close an open document. Refused if it has unsaved changes, unless discard is true.",
    ),
];

/// Why an operation that needs a session can't be applied to a document on its own.
pub(crate) fn needs_session(word: &str) -> String {
    format!(
        "'{word}' works on the documents that are open together, and here there is only one document: it needs the application or the command line."
    )
}

fn documents(session: &Session) -> Value {
    let current = session.current_id();
    let list: Vec<Value> = session
        .documents()
        .map(|(id, doc)| {
            let mut m = Map::new();
            m.insert("id".to_owned(), json!(id.0));
            m.insert("name".to_owned(), json!(doc.title()));
            if let Some(path) = doc.file.as_ref().and_then(|f| f.path.as_ref()) {
                m.insert("file".to_owned(), json!(path.to_string_lossy()));
            }
            m.insert("modified".to_owned(), json!(doc.is_modified()));
            m.insert("current".to_owned(), json!(id == current));
            Value::Object(m)
        })
        .collect();
    json!(list)
}

/// The reply to a session command that was carried out: the documents as they are now,
/// and the current one's state if it is another than before.
fn done(session: &Session, word: &str, was_current: DocId, more: Map<String, Value>) -> Reply {
    let replaced = session.current_id() != was_current;
    let mut out = Map::new();
    out.insert("ok".to_owned(), json!(true));
    out.insert("op".to_owned(), json!(word));
    out.extend(more);
    out.insert("documents".to_owned(), documents(session));
    if replaced {
        out.insert("document".to_owned(), json!(session.current_id().0));
        if let Value::Object(status) = query::status(session) {
            out.extend(status);
        }
    }
    Reply {
        ok: true,
        changed: false,
        created: Vec::new(),
        replaced,
        json: Value::Object(out),
    }
}

fn command(session: &mut Session, command: &SessionCommand) -> Result<Reply, String> {
    let was = session.current_id();
    let mut more = Map::new();
    match command {
        SessionCommand::Documents => {}
        SessionCommand::Switch { document } => {
            let id = document.resolve(session)?;
            session.switch(id);
        }
        SessionCommand::Close { document, discard } => {
            let id = match document {
                Some(d) => d.resolve(session)?,
                None => was,
            };
            if let Some(doc) = session.get(id)
                && doc.is_modified()
                && !discard
            {
                return Err(format!(
                    "{} has unsaved changes: save it first, or give 'close' \"discard\": true.",
                    doc.title()
                ));
            }
            if let Some(doc) = session.close(id) {
                more.insert("closed".to_owned(), json!(doc.title()));
            }
        }
    }
    Ok(done(session, command.word(), was, more))
}

/// Applies one operation to a session, in `host`: to the document `target` names, or to
/// the current one. See the module docs for what a session adds.
pub fn apply_session(
    host: &mut dyn Host,
    session: &mut Session,
    target: Option<&DocSel>,
    op: &Op,
    undo: Undo,
) -> Reply {
    let word = op.word();
    let own_document = |what: &str| {
        failed(
            word,
            format!("'{word}' {what}, so it can't be sent to a document with 'document'."),
        )
    };
    // Open beside the others: in a new document, which is closed again if it fails.
    let beside = match op {
        Op::New {
            keep: true,
            discard,
        } => Some(Op::New {
            keep: false,
            discard: *discard,
        }),
        Op::Open {
            keep: true,
            file,
            discard,
        } => Some(Op::Open {
            keep: false,
            file: file.clone(),
            discard: *discard,
        }),
        Op::OpenSample {
            keep: true,
            sample,
            discard,
        } => Some(Op::OpenSample {
            keep: false,
            sample: *sample,
            discard: *discard,
        }),
        _ => None,
    };
    if let Some(op) = beside {
        if target.is_some() {
            return own_document("opens a document of its own");
        }
        let was = session.current_id();
        let fresh = session.open(Document::default());
        let mut reply = apply_in(host, session, &op, undo);
        if reply.ok {
            reply.json["document"] = json!(fresh.0);
        } else {
            session.close(fresh);
            session.switch(was);
        }
        return reply;
    }
    if let Op::Session(c) = op {
        if target.is_some() {
            return own_document("is about the open documents");
        }
        return command(session, c).unwrap_or_else(|e| failed(word, e));
    }
    let id = match target {
        Some(sel) => match sel.resolve(session) {
            Ok(id) => id,
            Err(e) => return failed(word, e),
        },
        None => session.current_id(),
    };
    match session.get_mut(id) {
        Some(doc) => apply_in(host, doc, op, undo),
        None => failed(word, "The document is no longer open.".to_owned()),
    }
}

/// Applies one JSON operation to a session, in `host`. A `document` field (a name or an
/// id) sends it to that document instead of the current one.
pub fn apply_session_json(
    host: &mut dyn Host,
    session: &mut Session,
    op: &Value,
    undo: Undo,
) -> Reply {
    // `switch` and `close` have a `document` field of their own.
    let own = matches!(op["op"].as_str(), Some("switch" | "close"));
    let mut op = op.clone();
    let target = match op.as_object_mut().filter(|_| !own) {
        Some(map) => map.remove("document").filter(|v| !v.is_null()),
        None => None,
    };
    let name = op["op"].as_str().unwrap_or_default().to_owned();
    let target = match target.as_ref().map(DocSel::parse).transpose() {
        Ok(t) => t,
        Err(e) => return failed(&name, format!("document: {e}")),
    };
    let id = match &target {
        Some(sel) => match sel.resolve(session) {
            Ok(id) => id,
            Err(e) => return failed(&name, e),
        },
        None => session.current_id(),
    };
    // Read against the document it is for: an `edit` needs to know the feature's kind.
    let Some(doc) = session.get_mut(id) else {
        return failed(&name, "The document is no longer open.".to_owned());
    };
    doc.finish_loading();
    match op::from_json(&op, doc) {
        (_, Ok(op)) => apply_session(host, session, target.as_ref(), &op, undo),
        (name, Err(e)) => failed(&name, e),
    }
}
