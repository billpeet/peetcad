//! Linked parts: components whose part is a file of its own, not a copy kept in the
//! assembly.
//!
//! A linked part is read from its file when the assembly is opened and when it is asked
//! for (`update_links`), and when the part's document is saved in the same session. The
//! assembly keeps the part as it was last read, so it still opens, complete, when the
//! file can't be found; the link then says so.
//!
//! This needs files on disk, so it is for the desktop: in the browser a part is always
//! the assembly's own.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use peet_document::Document;
use peet_model::{DefId, Link, Model};
use serde_json::{Map, Value, json};

use crate::args::integer;

/// A part of the assembly, by name (`"Bracket"`, as `components` lists the parts) or id.
#[derive(Clone, Debug, PartialEq)]
pub enum PartSel {
    Id(DefId),
    Name(String),
}

impl From<DefId> for PartSel {
    fn from(id: DefId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for PartSel {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

impl PartSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(DefId(integer(v)?))),
            Value::String(name) => Ok(Self::Name(name.clone())),
            other => Err(format!("expected a part's name or id, not {other}")),
        }
    }

    /// The part this means in `doc`, or why there is none (or more than one).
    pub fn resolve(&self, doc: &Document) -> Result<DefId, String> {
        let assembly = doc
            .model
            .assembly()
            .ok_or_else(|| format!("{} is a part: it has no parts of its own.", doc.title()))?;
        let listed = || {
            let parts: Vec<String> = assembly
                .definitions()
                .map(|d| format!("{} ({})", d.id.0, d.name()))
                .collect();
            if parts.is_empty() {
                "The assembly has no parts yet: add one with insert.".to_owned()
            } else {
                format!("The parts are: {}.", parts.join(", "))
            }
        };
        match self {
            Self::Id(id) => assembly
                .definition(*id)
                .map(|d| d.id)
                .ok_or_else(|| format!("There is no part with the id {}. {}", id.0, listed())),
            Self::Name(name) => {
                let mut found = assembly.definitions().filter(|d| d.name() == name);
                match (found.next(), found.next()) {
                    (Some(d), None) => Ok(d.id),
                    (None, _) => Err(format!("There is no part called '{name}'. {}", listed())),
                    (Some(_), Some(_)) => Err(format!(
                        "More than one part is called '{name}': use an id. {}",
                        listed()
                    )),
                }
            }
        }
    }
}

/// The operations of linked parts, with their fields, for `help`.
pub(crate) const LINK_OPS: &[(&str, &str, &str)] = &[
    (
        "update_links",
        "",
        "Read the assembly's linked parts from their files again, so that it shows them as they are now.",
    ),
    (
        "link",
        "part (a name or an id, as 'components' lists the parts), path (a .peet file), absolute (default false: the link is kept relative to the assembly's folder)",
        "Link a part of the assembly to a file: to the file if there is one (the assembly then shows what is in it), else the part is written there first.",
    ),
    (
        "unlink",
        "part",
        "Make a linked part the assembly's own again: it keeps the part as it is, and no longer follows the file.",
    ),
];

/// Why linked parts can't be used here, if they can't.
pub(crate) fn unavailable() -> Result<(), String> {
    if cfg!(target_arch = "wasm32") {
        Err("Linked parts are read from files on disk, which the browser doesn't have: insert the part without 'link', and the assembly keeps its own copy.".to_owned())
    } else {
        Ok(())
    }
}

/// A path as a link holds it while its assembly is open: in full.
pub(crate) fn full(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_owned())
        .to_string_lossy()
        .into_owned()
}

/// The folder a document's file is in: where its linked parts are also looked for.
pub(crate) fn folder(doc: &Document) -> Option<PathBuf> {
    doc.file
        .as_ref()?
        .path
        .as_ref()?
        .parent()
        .map(Path::to_owned)
}

/// The file a link means now: where it says, or (if the files were moved together) a
/// file of the same name next to the assembly's.
pub(crate) fn find(link: &Link, beside: Option<&Path>) -> Option<PathBuf> {
    let stored = PathBuf::from(&link.path);
    if stored.is_file() {
        return Some(stored);
    }
    // A relative link whose assembly was read from nowhere is still relative.
    let beside = beside?;
    let relative = beside.join(&stored);
    if !stored.is_absolute() && relative.is_file() {
        return Some(relative);
    }
    let moved = beside.join(stored.file_name()?);
    moved.is_file().then_some(moved)
}

/// A link as its assembly's file has it: relative to the assembly's folder if it is a
/// relative link and there is a way from one to the other.
pub(crate) fn written(link: &Link, beside: Option<&Path>) -> String {
    beside
        .filter(|_| link.relative)
        .and_then(|folder| peet_model::relative_to(Path::new(&link.path), folder))
        .unwrap_or_else(|| link.path.clone())
}

/// The files the linked parts of an assembly are read from: what to watch for changes.
/// Those of the assemblies in it are among them, and so is where a file that can't be
/// found should be (it may be put back).
pub fn linked_files(doc: &Document) -> Vec<PathBuf> {
    fn collect(model: &Model, beside: Option<&Path>, depth: usize, out: &mut Vec<PathBuf>) {
        let Some(assembly) = model.assembly().filter(|_| depth < DEPTH) else {
            return;
        };
        for d in assembly.definitions() {
            let file = d.link.as_ref().and_then(|l| {
                find(l, beside).or_else(|| Some(PathBuf::from(&l.path)).filter(|p| p.is_absolute()))
            });
            // A linked assembly's own links are relative to where it is.
            let inside = file.as_ref().and_then(|f| f.parent()).or(beside);
            collect(&d.model, inside, depth + 1, out);
            if let Some(file) = file.filter(|f| !out.contains(f)) {
                out.push(file);
            }
        }
    }
    let mut out = Vec::new();
    collect(&doc.model, folder(doc).as_deref(), 0, &mut out);
    out
}

/// Whether two paths are the same file.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Most assemblies a linked assembly's own links are followed through.
const DEPTH: usize = 16;

/// The model in a part's file, with the linked parts of an assembly in it read too.
pub(crate) fn read(path: &Path, depth: usize) -> Result<Model, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let opened = peet_io::document::open(&bytes)
        .map_err(|e| format!("Couldn't open {}: {}", path.display(), e.message))?;
    let mut model = opened.model;
    // An assembly's own links are relative to where it is.
    model.from_file(path.parent());
    // A part is known by its file's name, as in the application's title.
    if let Some(stem) = path.file_stem().map(|s| s.to_string_lossy())
        && matches!(model.name.as_str(), "Part1" | "Assembly1")
    {
        model.name = stem.into_owned();
    }
    if depth < DEPTH {
        refresh(&mut model, path.parent(), depth + 1);
    }
    Ok(model)
}

/// How a link is: what `components` and `update_links` say about it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum State {
    /// The assembly has the part as its file has it.
    Current,
    /// The file has changed since the assembly read it.
    Changed,
    /// The file can't be found, or can't be read: the assembly has the part as it last
    /// read it.
    Missing(String),
}

/// The state of a link, with the part as its file has it now if it can be read.
pub(crate) fn state(model: &Model, link: &Link, beside: Option<&Path>) -> (State, Option<Model>) {
    let Some(path) = find(link, beside) else {
        return (
            State::Missing(format!(
                "{} can't be found, so the part is shown as it was when it was last read. Put the file back, or make the part the assembly's own with 'unlink'.",
                link.path
            )),
            None,
        );
    };
    match read(&path, 0) {
        Ok(now) if now == *model => (State::Current, None),
        Ok(now) => (State::Changed, Some(now)),
        Err(e) => (State::Missing(e), None),
    }
}

/// Reads every linked part of an assembly from its file again. Returns the parts that
/// changed and what couldn't be read, by name.
pub(crate) fn refresh(
    model: &mut Model,
    beside: Option<&Path>,
    depth: usize,
) -> (Vec<String>, Vec<String>) {
    let (mut updated, mut problems) = (Vec::new(), Vec::new());
    let Some(assembly) = model.assembly() else {
        return (updated, problems);
    };
    let mut read_again = Vec::new();
    for d in assembly.definitions() {
        let Some(link) = &d.link else {
            continue;
        };
        match find(link, beside).map(|p| read(&p, depth)) {
            Some(Ok(now)) if now != *d.model => {
                updated.push(now.name.clone());
                read_again.push((d.id, now));
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => problems.push(e),
            None => problems.push(format!(
                "{} can't be found: {} is shown as it was when it was last read.",
                link.path,
                d.name()
            )),
        }
    }
    if let Some(assembly) = model.assembly_mut() {
        for (id, now) in read_again {
            assembly.set_model(id, Arc::new(now));
        }
    }
    (updated, problems)
}

/// What `update_links` replies.
pub(crate) fn report(updated: &[String], problems: &[String]) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("updated".to_owned(), json!(updated));
    if !problems.is_empty() {
        out.insert("warnings".to_owned(), json!(problems));
    }
    out
}

/// A part's link as `components` gives it.
pub(crate) fn link_out(m: &mut Map<String, Value>, model: &Model, link: &Link, doc: &Document) {
    // As the assembly's file has it, and the file it means.
    let beside = folder(doc);
    m.insert("link".to_owned(), json!(written(link, beside.as_deref())));
    m.insert(
        "link_file".to_owned(),
        json!(
            find(link, beside.as_deref())
                .map_or_else(|| link.path.clone(), |p| p.to_string_lossy().into_owned())
        ),
    );
    let (word, message) = match state(model, link, folder(doc).as_deref()).0 {
        State::Current => ("current", None),
        State::Changed => (
            "changed",
            Some(
                "The file has changed since it was read: 'update_links' reads it again.".to_owned(),
            ),
        ),
        State::Missing(why) => ("missing", Some(why)),
    };
    m.insert("link_status".to_owned(), json!(word));
    if let Some(message) = message {
        m.insert("link_message".to_owned(), json!(message));
    }
}
