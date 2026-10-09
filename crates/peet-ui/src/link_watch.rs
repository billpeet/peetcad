//! Watching the files of linked parts for changes made outside the application.
//!
//! The system tells us when something happens in the folders the files are in (the
//! `notify` crate: `ReadDirectoryChangesW` on Windows, inotify on Linux, FSEvents on
//! macOS). Folders are watched, not files: programs often save by writing a new file and
//! renaming it over the old one, which a watch on the file itself would lose, and a file
//! that is missing can be seen arriving. A file that was touched is reported once
//! nothing more has happened to it for [`SETTLE`], so it is not read half written.
//!
//! If the system's watcher can't be started (or can't watch a folder), the files are
//! looked at once a second instead, comparing their time and size.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use peet_platform::Instant;

/// How long a file must have been left alone before it is read.
pub const SETTLE: Duration = Duration::from_millis(300);

/// How often the files are looked at when the system can't tell us about them.
const LOOK_EVERY: Duration = Duration::from_millis(1000);

/// What a file looked like: when it was last written, and how long it is.
type Seen = Option<(std::time::SystemTime, u64)>;

/// The system's watcher, with the folders it watches and what it has reported.
#[cfg(not(target_arch = "wasm32"))]
struct System {
    watcher: notify::RecommendedWatcher,
    events: std::sync::mpsc::Receiver<PathBuf>,
    folders: Vec<PathBuf>,
}

#[derive(Default)]
pub struct LinkWatch {
    /// The files watched: where the links of the open assemblies point.
    files: Vec<PathBuf>,
    /// What `files` was worked out for (see [`LinkWatch::follow`]).
    signature: Option<u64>,
    /// Files that were touched, and when last: reported once they are left alone.
    settling: HashMap<PathBuf, Instant>,
    #[cfg(not(target_arch = "wasm32"))]
    system: Option<System>,
    /// The system's watcher failed: the files are looked at instead.
    looking: bool,
    looked: Option<Instant>,
    seen: HashMap<PathBuf, Seen>,
}

/// Whether two paths name the same file, as far as their text tells.
fn same(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
    } else {
        a == b
    }
}

impl LinkWatch {
    /// Watches `files()`, if `signature` (which identifies the state they were worked out
    /// from) is another than last time. `ctx` is woken when the system reports something.
    pub fn follow(
        &mut self,
        signature: u64,
        files: impl FnOnce() -> Vec<PathBuf>,
        ctx: &egui::Context,
    ) {
        if self.signature == Some(signature) {
            return;
        }
        self.signature = Some(signature);
        self.files = files();
        let files = &self.files;
        self.settling.retain(|f, _| files.contains(f));
        self.seen.retain(|f, _| files.contains(f));
        self.watch_folders(ctx);
    }

    /// Makes the system watch the folders of the files, and no others.
    #[cfg(not(target_arch = "wasm32"))]
    fn watch_folders(&mut self, ctx: &egui::Context) {
        use notify::Watcher;
        let mut wanted: Vec<PathBuf> = self
            .files
            .iter()
            .filter_map(|f| f.parent())
            .filter(|folder| folder.is_dir())
            .map(Path::to_owned)
            .collect();
        wanted.sort();
        wanted.dedup();
        if self.looking || (wanted.is_empty() && self.system.is_none()) {
            return;
        }
        if self.system.is_none() {
            let (send, events) = std::sync::mpsc::channel();
            let wake = ctx.clone();
            let started =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    let Ok(event) = event else {
                        return;
                    };
                    // Reading a file is not a change to it.
                    if matches!(event.kind, notify::EventKind::Access(_)) {
                        return;
                    }
                    for path in event.paths {
                        let _ = send.send(path);
                    }
                    wake.request_repaint();
                });
            match started {
                Ok(watcher) => {
                    self.system = Some(System {
                        watcher,
                        events,
                        folders: Vec::new(),
                    });
                }
                Err(e) => {
                    log::warn!("Linked parts can't be watched ({e}): they are looked at instead.");
                    self.looking = true;
                    return;
                }
            }
        }
        let Some(system) = &mut self.system else {
            return;
        };
        for gone in system.folders.iter().filter(|f| !wanted.contains(f)) {
            let _ = system.watcher.unwatch(gone);
        }
        let mut failed = false;
        for new in wanted.iter().filter(|f| !system.folders.contains(f)) {
            if let Err(e) = system
                .watcher
                .watch(new, notify::RecursiveMode::NonRecursive)
            {
                log::warn!(
                    "{} can't be watched ({e}): linked parts are looked at instead.",
                    new.display()
                );
                failed = true;
            }
        }
        system.folders = wanted;
        if failed {
            self.system = None;
            self.looking = true;
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn watch_folders(&mut self, _ctx: &egui::Context) {}

    /// Notes that something happened to `path` at `at`: if it is one of the files
    /// watched, it will be reported once it has been left alone.
    pub fn note(&mut self, path: &Path, at: Instant) {
        if let Some(file) = self.files.iter().find(|f| same(f, path)) {
            self.settling.insert(file.clone(), at);
        }
    }

    /// The files that were changed and have been left alone since: each is given once.
    pub fn take_changed(&mut self, now: Instant) -> Vec<PathBuf> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let touched: Vec<PathBuf> = self
                .system
                .as_ref()
                .map(|s| s.events.try_iter().collect())
                .unwrap_or_default();
            for path in touched {
                self.note(&path, now);
            }
        }
        if self.looking
            && self
                .looked
                .is_none_or(|at| now.duration_since(at) >= LOOK_EVERY)
        {
            self.looked = Some(now);
            for file in self.files.clone() {
                let is: Seen = std::fs::metadata(&file)
                    .ok()
                    .and_then(|m| Some((m.modified().ok()?, m.len())));
                // (A file seen for the first time has not changed.)
                if self
                    .seen
                    .insert(file.clone(), is)
                    .is_some_and(|was| was != is)
                {
                    self.note(&file, now);
                }
            }
        }
        let settled: Vec<PathBuf> = self
            .settling
            .iter()
            .filter(|(_, at)| now.duration_since(**at) >= SETTLE)
            .map(|(file, _)| file.clone())
            .collect();
        for file in &settled {
            self.settling.remove(file);
        }
        settled
    }

    /// When to come back, if nothing else happens: to report a file that is settling, or
    /// to look at the files again.
    pub fn wake_in(&self) -> Option<Duration> {
        if !self.settling.is_empty() {
            Some(SETTLE)
        } else if self.looking && !self.files.is_empty() {
            Some(LOOK_EVERY)
        } else {
            None
        }
    }

    /// Whether the system is telling us about changes (and we are not looking for them).
    #[cfg(test)]
    pub fn is_notified(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.system.is_some() && !self.looking
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// Looks at the files instead of being told about them, as when the system's watcher
    /// can't be started.
    #[cfg(test)]
    pub fn look_instead(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.system = None;
        }
        self.looking = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_touched_file_is_reported_once_it_is_left_alone() {
        let ctx = egui::Context::default();
        let mut watch = LinkWatch::default();
        let (a, b) = (
            PathBuf::from("/nowhere/a.peet"),
            PathBuf::from("/nowhere/b.peet"),
        );
        watch.follow(1, || vec![a.clone(), b.clone()], &ctx);
        let start = Instant::now();
        assert!(watch.take_changed(start).is_empty());
        assert_eq!(watch.wake_in(), None);

        // Written in three goes: reported once, after the last.
        let step = SETTLE / 2;
        watch.note(&a, start);
        watch.note(&a, start + step);
        assert_eq!(watch.wake_in(), Some(SETTLE));
        assert!(watch.take_changed(start + step * 2).is_empty());
        watch.note(&a, start + step * 2);
        watch.note(Path::new("/nowhere/other.txt"), start + step * 2);
        assert!(watch.take_changed(start + step * 3).is_empty());
        assert_eq!(
            watch.take_changed(start + step * 4),
            std::slice::from_ref(&a)
        );
        assert!(watch.take_changed(start + step * 9).is_empty());

        // The same state is not worked out again; another one is, and what is no longer
        // watched is forgotten.
        watch.note(&b, start);
        watch.follow(1, || panic!("already known"), &ctx);
        watch.follow(2, || vec![a.clone()], &ctx);
        assert!(watch.take_changed(start + SETTLE * 5).is_empty());
    }

    #[test]
    fn files_are_looked_at_when_the_system_cant_watch() {
        let dir = std::env::temp_dir().join(format!("peet-look-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("part.peet");
        std::fs::write(&file, b"one").unwrap();
        let ctx = egui::Context::default();
        let mut watch = LinkWatch::default();
        watch.look_instead();
        watch.follow(1, || vec![file.clone()], &ctx);
        assert_eq!(watch.wake_in(), Some(LOOK_EVERY));
        let start = Instant::now();
        assert!(
            watch.take_changed(start).is_empty(),
            "seen for the first time"
        );
        std::fs::write(&file, b"longer").unwrap();
        // Not looked at again before its time; then seen, and reported once settled.
        assert!(watch.take_changed(start + LOOK_EVERY / 2).is_empty());
        assert!(watch.take_changed(start + LOOK_EVERY).is_empty());
        assert_eq!(
            watch.take_changed(start + LOOK_EVERY + SETTLE),
            std::slice::from_ref(&file)
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
