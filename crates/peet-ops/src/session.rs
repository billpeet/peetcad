//! Operations on a [`Session`]: several documents open together.
//!
//! [`apply_session`] is [`crate::apply_in`] for a session. An operation goes to the
//! current document, or to the one it names (`"document": "Bracket"` in JSON) without
//! making that one current. On top of that it carries out what only a session can:
//! listing the open documents, switching, closing, and opening a document beside the
//! others (`"keep": true` on `new`, `open` and `open_sample`).
//!
//! It is also where an assembly meets the other documents: a component is inserted from
//! an open document (`"part"`), and a component's part is opened as a document of its
//! own (`open_component`), which `save` stores back in the assembly.

use std::sync::Arc;

use peet_document::{DocId, Document, Embedded, Session};
use serde_json::{Map, Value, json};

use crate::args::{Args, integer};
use crate::assembly::{ComponentChange, InsertSource};
use crate::host::Host;
use crate::op::{self, Op};
use crate::{Reply, Undo, apply_in, apply_with, failed, query};

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
            // The parts of an assembly that are open for editing close with it.
            let parts: Vec<DocId> = session
                .documents()
                .filter(|(_, d)| d.embedded.is_some_and(|e| e.assembly == id))
                .map(|(i, _)| i)
                .collect();
            for check in parts.iter().chain([&id]) {
                if let Some(doc) = session.get(*check)
                    && doc.is_modified()
                    && !discard
                {
                    return Err(format!(
                        "{} has unsaved changes: save it first, or give 'close' \"discard\": true.",
                        doc.title()
                    ));
                }
            }
            for part in parts {
                session.close(part);
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
            assembly,
        } => Some(Op::New {
            keep: false,
            discard: *discard,
            assembly: *assembly,
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
    // What an assembly does with the other documents.
    let from_document = |source: &InsertSource| match source {
        InsertSource::Document(part) => Some(open_model(session, part, id)),
        _ => None,
    };
    let resolved = match op {
        Op::Insert {
            from,
            name,
            placing,
            fixed,
        } => from_document(from).map(|model| {
            model.map(|m| Op::Insert {
                from: InsertSource::Model(m),
                name: name.clone(),
                placing: placing.clone(),
                fixed: *fixed,
            })
        }),
        Op::Component {
            component,
            change: ComponentChange::Replace(from),
        } => from_document(from).map(|model| {
            model.map(|m| Op::Component {
                component: component.clone(),
                change: ComponentChange::Replace(InsertSource::Model(m)),
            })
        }),
        _ => None,
    };
    let op = match &resolved {
        Some(Ok(op)) => op,
        Some(Err(e)) => return failed(word, e.clone()),
        None => op,
    };
    match op {
        Op::OpenComponent { component } => {
            return open_component(session, id, component).unwrap_or_else(|e| failed(word, e));
        }
        Op::Save { path, .. } => {
            if let Some(embedded) = session.get(id).and_then(|d| d.embedded) {
                return store(host, session, id, embedded, path.is_some(), undo)
                    .unwrap_or_else(|e| failed(word, e));
            }
        }
        _ => {}
    }
    match session.get_mut(id) {
        Some(doc) => apply_in(host, doc, op, undo),
        None => failed(word, "The document is no longer open.".to_owned()),
    }
}

/// The model of an open document, to make a component of in the document `into`.
fn open_model(
    session: &Session,
    part: &DocSel,
    into: DocId,
) -> Result<Arc<peet_model::Model>, String> {
    let id = part.resolve(session)?;
    let doc = session
        .get(id)
        .ok_or_else(|| "The document is no longer open.".to_owned())?;
    if id == into {
        return Err(format!("{} can't be a component of itself.", doc.title()));
    }
    let mut model = doc.model.clone();
    // A part is known by its file's name, as in its title.
    if matches!(model.name.as_str(), "Part1" | "Assembly1") {
        let title = doc.title();
        title
            .strip_suffix(".peet")
            .unwrap_or(&title)
            .clone_into(&mut model.name);
    }
    Ok(Arc::new(model))
}

/// Opens a component's part as a document, or goes to it if it is open already.
fn open_component(
    session: &mut Session,
    assembly: DocId,
    component: &crate::assembly::CompSel,
) -> Result<Reply, String> {
    let was = session.current_id();
    let doc = session
        .get(assembly)
        .ok_or_else(|| "The document is no longer open.".to_owned())?;
    let id = component.resolve(doc)?;
    let definition = doc
        .model
        .assembly()
        .and_then(|a| a.definition_of(id))
        .ok_or_else(|| "The component's part is missing.".to_owned())?;
    let embedded = Embedded {
        assembly,
        definition: definition.id,
    };
    let open = session
        .documents()
        .find(|(_, d)| d.embedded == Some(embedded))
        .map(|(i, _)| i);
    match open {
        Some(open) => {
            session.switch(open);
        }
        None => {
            let mut part = Document::from_model((*definition.model).clone(), None);
            part.embedded = Some(embedded);
            // Rebuilding writes solved sketches back: that is not a change of the user's.
            part.mark_saved(None);
            session.open(part);
        }
    }
    let mut more = Map::new();
    more.insert("part".to_owned(), json!(session.title()));
    more.insert(
        "note".to_owned(),
        json!("'save' on this document stores the part back in its assembly, for every instance."),
    );
    let mut reply = done(session, "open_component", was, more);
    reply.replaced = true;
    Ok(reply)
}

/// Stores a part that was opened from an assembly back in it: what `save` does there.
fn store(
    host: &mut dyn Host,
    session: &mut Session,
    part: DocId,
    embedded: Embedded,
    to_file: bool,
    undo: Undo,
) -> Result<Reply, String> {
    let doc = session
        .get(part)
        .ok_or_else(|| "The document is no longer open.".to_owned())?;
    let name = doc.title();
    if to_file {
        return Err(format!(
            "{name} is a part of an assembly, which is where it is saved: leave 'path' out to store it there, then save the assembly."
        ));
    }
    let model = Arc::new(doc.model.clone());
    let assembly = session.get_mut(embedded.assembly).ok_or_else(|| {
        format!("The assembly {name} came from is no longer open, so it can't be stored there.")
    })?;
    let owner = assembly.title();
    let stored = apply_with(
        host,
        assembly,
        &Op::SetPart {
            part: embedded.definition,
            model,
        },
        undo,
        None,
    );
    if !stored.ok {
        return Err(stored.json["error"]
            .as_str()
            .unwrap_or("It could not be stored.")
            .to_owned());
    }
    if let Some(doc) = session.get_mut(part) {
        doc.mark_saved(None);
    }
    let mut out = Map::new();
    out.insert("ok".to_owned(), json!(true));
    out.insert("op".to_owned(), json!("save"));
    out.insert("stored".to_owned(), json!(name));
    out.insert("in".to_owned(), json!(owner));
    if let Some(failures) = stored.json.get("failures") {
        out.insert("failures".to_owned(), failures.clone());
    }
    Ok(Reply {
        ok: true,
        changed: stored.changed,
        created: Vec::new(),
        replaced: false,
        json: Value::Object(out),
    })
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
