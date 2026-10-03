//! Opening, saving, autosave and crash recovery.
//!
//! **Autosave.** While the part has unsaved changes, it is written every
//! [`AUTOSAVE_INTERVAL`] to the platform's blob store (a file in the user's data folder
//! natively, IndexedDB in the browser), and also when the app shuts down. Saving the part,
//! or undoing back to the saved state, removes the autosave. So an autosave found at
//! startup means PeetCAD closed (or crashed, or the tab was closed) with unsaved work,
//! and the user is offered to recover it.

use peet_platform::{Duration, Instant, OpenedFile, Pending, storage};

use crate::document::{Document, FileLocation};

pub const AUTOSAVE_KEY: &str = "unsaved-part";
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(20);
pub const FILTER: (&str, &[&str]) = ("PeetCAD part", &["peet"]);

/// What to do once the user has decided about unsaved changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterDiscard {
    New,
    Open,
    Sample,
    SampleEnclosure,
    SampleChassis,
    Quit,
}

pub struct FileState {
    /// An open dialog in progress (always finishes later on the web).
    pub opening: Option<Pending<Result<Option<OpenedFile>, String>>>,
    /// Looking for an autosave at startup.
    recovery: Option<Pending<Result<Option<Vec<u8>>, String>>>,
    /// An autosave found at startup, offered to the user.
    pub recovered: Option<Vec<u8>>,
    /// Waiting for the user to save or discard unsaved changes.
    pub confirm: Option<AfterDiscard>,
    last_autosave: Instant,
    /// Hash of the model last autosaved, so an unchanged part isn't written again.
    autosaved: Option<u64>,
    /// Background writes, polled so their errors are logged.
    writes: Vec<Pending<Result<(), String>>>,
}

impl Default for FileState {
    fn default() -> Self {
        Self {
            opening: None,
            recovery: Some(storage::get(AUTOSAVE_KEY)),
            recovered: None,
            confirm: None,
            last_autosave: Instant::now(),
            autosaved: None,
            writes: Vec::new(),
        }
    }
}

impl FileState {
    /// Whether something is still running and the UI must keep polling.
    pub fn busy(&self) -> bool {
        self.opening.is_some() || self.recovery.is_some() || !self.writes.is_empty()
    }

    /// Polls background work: a found autosave is moved to `recovered`.
    pub fn poll(&mut self) {
        if let Some(p) = &self.recovery
            && let Some(result) = p.take()
        {
            self.recovery = None;
            match result {
                Ok(Some(bytes)) if !bytes.is_empty() => self.recovered = Some(bytes),
                Ok(_) => {}
                Err(e) => log::warn!("Couldn't look for unsaved work: {e}"),
            }
        }
        self.writes.retain(|w| match w.take() {
            None => !w.is_done(),
            Some(Ok(())) => false,
            Some(Err(e)) => {
                log::warn!("Autosave failed: {e}");
                false
            }
        });
    }

    /// Writes or removes the autosave as the document requires. `force` writes now
    /// (shutdown) instead of waiting for the interval.
    pub fn autosave(&mut self, doc: &Document, force: bool) {
        // Never overwrite work that is still being offered for recovery.
        if self.recovered.is_some() || self.recovery.is_some() {
            return;
        }
        if !doc.is_modified() {
            if self.autosaved.take().is_some() {
                self.writes.push(storage::remove(AUTOSAVE_KEY));
            }
            return;
        }
        if !force && self.last_autosave.elapsed() < AUTOSAVE_INTERVAL {
            return;
        }
        self.last_autosave = Instant::now();
        let hash = peet_model::hash::of(&doc.model);
        if self.autosaved == Some(hash) {
            return;
        }
        match doc.save_bytes(false) {
            Ok(bytes) => {
                self.writes.push(storage::put(AUTOSAVE_KEY, bytes));
                self.autosaved = Some(hash);
            }
            Err(e) => log::warn!("Autosave failed: {e}"),
        }
    }

    /// Forgets the autosave (after saving, or when recovery was declined).
    pub fn discard_autosave(&mut self) {
        self.autosaved = None;
        self.recovered = None;
        self.writes.push(storage::remove(AUTOSAVE_KEY));
    }
}

/// Opens a part from file bytes.
pub fn document_from_bytes(
    bytes: &[u8],
    file: Option<FileLocation>,
) -> Result<(Document, Vec<String>), String> {
    let opened = peet_io::document::open(bytes).map_err(|e| e.message)?;
    let warnings = opened.warnings.clone();
    Ok((Document::from_opened(opened, file), warnings))
}

/// A file name for the part, with the extension.
pub fn file_name(doc: &Document) -> String {
    let title = doc.title();
    if title.ends_with(".peet") {
        title
    } else {
        format!("{title}.peet")
    }
}
