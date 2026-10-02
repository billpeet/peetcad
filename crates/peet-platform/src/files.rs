//! Opening and saving files the user asked for (documents and exports).
//!
//! Natively this shows the system dialogs and reads or writes the file. In the browser there
//! is no file system: opening goes through the browser's file picker, and saving offers the
//! bytes as a download with the suggested name.

use std::path::{Path, PathBuf};

use crate::Pending;

/// Outcome of [`save_file`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Written to this location (a path natively, the download name on the web).
    Saved(String),
    /// The user closed the dialog.
    Cancelled,
}

/// A file the user picked in [`open_file`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedFile {
    /// File name for display ("bracket.peet").
    pub name: String,
    /// Where it lives, if the platform has paths (`None` on the web).
    pub path: Option<PathBuf>,
    pub bytes: Vec<u8>,
}

/// Where a document was saved: display text plus the path if there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedFile {
    /// File name for display ("bracket.peet").
    pub name: String,
    /// Where it lives, if the platform has paths (`None` on the web, where it was downloaded).
    pub path: Option<PathBuf>,
}

/// Offers `bytes` to the user as a file called `suggested_name`. `filter` is a
/// description and the extensions to offer, like `("STL mesh", &["stl"])`.
pub fn save_file(
    suggested_name: &str,
    filter: (&str, &[&str]),
    bytes: &[u8],
) -> Result<SaveOutcome, String> {
    imp::save_file(suggested_name, filter, bytes)
}

/// Shows the open dialog and reads the chosen file. `Ok(None)` means the user cancelled.
/// `filter` is a description and the extensions to offer, like `("PeetCAD part", &["peet"])`.
///
/// Natively the dialog is modal and the result is ready when this returns.
///
/// On the web the result arrives later. Browsers only show a file picker shortly after a
/// click or key press, so call this in response to one. Browsers that don't report a
/// dismissed picker (before Chrome 113, Firefox 91 and Safari 16.4), or that refuse to show
/// it, never tell the page, and then the `Pending` never resolves: don't block the UI on it.
/// Calling `open_file` again always works; it resolves a picker that is still waiting as
/// cancelled and shows a new one.
pub fn open_file(filter: (&str, &[&str])) -> Pending<Result<Option<OpenedFile>, String>> {
    imp::open_file(filter)
}

/// "Save As": asks where to save (the system dialog natively; a download on the web) and
/// writes `bytes` there. `Ok(None)` means the user cancelled.
///
/// On the web the browser decides where the download goes and doesn't say whether it
/// succeeded, so `Ok(Some(..))` only means the download was started.
pub fn save_file_as(
    suggested_name: &str,
    filter: (&str, &[&str]),
    bytes: &[u8],
) -> Result<Option<SavedFile>, String> {
    imp::save_file_as(suggested_name, filter, bytes)
}

/// "Save" to a known path (native only; on the web this returns an error because there are
/// no paths, use [`save_file_as`] there).
///
/// The write is atomic: the bytes go to a temporary file next to the target, which is then
/// renamed over it, so a crash mid-save never destroys the previous file. The folder must
/// already exist.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    imp::write_file(path, bytes)
}

/// Writes `bytes` to `path` through a temporary file in the same folder and a rename, so
/// `path` always holds either the old or the new contents. Cleans the temporary file up if
/// anything fails.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Makes the temporary name unique within the process; the process id makes it unique
    // across processes saving into the same folder.
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the path has no file name",
        )
    })?;
    let mut temp_name = std::ffi::OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = path.with_file_name(temp_name);

    let result = (|| {
        let mut file = std::fs::File::create_new(&temp)?;
        file.write_all(bytes)?;
        // Flush to the disk before the rename, or a power cut could leave an empty file
        // under the final name.
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::path::{Path, PathBuf};

    use super::{OpenedFile, SaveOutcome, SavedFile, write_atomic};
    use crate::Pending;

    fn display_name(path: &Path) -> String {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    }

    fn ask_save_path(
        suggested_name: &str,
        (description, extensions): (&str, &[&str]),
    ) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_file_name(suggested_name)
            .add_filter(description, extensions)
            .save_file()
    }

    pub fn save_file(
        suggested_name: &str,
        filter: (&str, &[&str]),
        bytes: &[u8],
    ) -> Result<SaveOutcome, String> {
        let Some(path) = ask_save_path(suggested_name, filter) else {
            return Ok(SaveOutcome::Cancelled);
        };
        std::fs::write(&path, bytes)
            .map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
        Ok(SaveOutcome::Saved(path.display().to_string()))
    }

    pub fn open_file(
        (description, extensions): (&str, &[&str]),
    ) -> Pending<Result<Option<OpenedFile>, String>> {
        let picked = rfd::FileDialog::new()
            .add_filter(description, extensions)
            .pick_file();
        Pending::ready(picked.map(read_file).transpose())
    }

    pub fn read_file(path: PathBuf) -> Result<OpenedFile, String> {
        let bytes = std::fs::read(&path).map_err(|e| {
            format!(
                "Couldn't open {}: {e}. Check that the file still exists and that you're allowed to read it.",
                path.display()
            )
        })?;
        Ok(OpenedFile {
            name: display_name(&path),
            path: Some(path),
            bytes,
        })
    }

    pub fn save_file_as(
        suggested_name: &str,
        filter: (&str, &[&str]),
        bytes: &[u8],
    ) -> Result<Option<SavedFile>, String> {
        let Some(path) = ask_save_path(suggested_name, filter) else {
            return Ok(None);
        };
        write_file(&path, bytes)?;
        Ok(Some(SavedFile {
            name: display_name(&path),
            path: Some(path),
        }))
    }

    pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
        write_atomic(path, bytes).map_err(|e| {
            format!(
                "Couldn't save {}: {e}. Check that the folder exists and that you're allowed to write there, or use Save As to pick another place.",
                path.display()
            )
        })
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::path::Path;

    use super::{OpenedFile, SaveOutcome, SavedFile};
    use crate::Pending;
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen_futures::JsFuture;

    /// Id of the hidden file input, so a picker that never reported back can be found again.
    const PICKER_ID: &str = "peetcad-open-file";

    pub fn save_file(
        suggested_name: &str,
        _filter: (&str, &[&str]),
        bytes: &[u8],
    ) -> Result<SaveOutcome, String> {
        let fail = |what: &str| format!("Couldn't start the download ({what}).");
        let window = web_sys::window().ok_or_else(|| fail("no window"))?;
        let document = window.document().ok_or_else(|| fail("no document"))?;
        let array = js_sys::Uint8Array::from(bytes);
        let parts = js_sys::Array::of1(&array);
        let options = web_sys::BlobPropertyBag::new();
        options.set_type("application/octet-stream");
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
            .map_err(|_| fail("blob"))?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| fail("url"))?;
        let anchor: web_sys::HtmlAnchorElement = document
            .create_element("a")
            .map_err(|_| fail("element"))?
            .dyn_into()
            .map_err(|_| fail("anchor"))?;
        anchor.set_href(&url);
        anchor.set_download(suggested_name);
        anchor.click();
        let _ = web_sys::Url::revoke_object_url(&url);
        Ok(SaveOutcome::Saved(suggested_name.to_owned()))
    }

    pub fn save_file_as(
        suggested_name: &str,
        filter: (&str, &[&str]),
        bytes: &[u8],
    ) -> Result<Option<SavedFile>, String> {
        save_file(suggested_name, filter, bytes)?;
        Ok(Some(SavedFile {
            name: suggested_name.to_owned(),
            path: None,
        }))
    }

    pub fn write_file(_path: &Path, _bytes: &[u8]) -> Result<(), String> {
        Err(
            "The browser version of PeetCAD can't save to a file on your computer directly. Use Save As to download the file instead."
                .to_owned(),
        )
    }

    pub fn open_file(
        (_description, extensions): (&str, &[&str]),
    ) -> Pending<Result<Option<OpenedFile>, String>> {
        let accept = extensions
            .iter()
            .map(|extension| format!(".{extension}"))
            .collect::<Vec<_>>()
            .join(",");
        let (pending, resolver) = Pending::deferred();
        // The picker has to be shown right here, while the browser still counts the user's
        // click as recent. Only the waiting and reading happen later.
        match show_picker(&accept) {
            Ok((input, picked)) => wasm_bindgen_futures::spawn_local(async move {
                resolver.resolve(read_picked(input, picked).await);
            }),
            Err(message) => resolver.resolve(Err(message)),
        }
        pending
    }

    /// Adds a hidden file input to the page and opens its picker. The promise settles when
    /// the user picks a file or dismisses the picker.
    fn show_picker(accept: &str) -> Result<(web_sys::HtmlInputElement, js_sys::Promise), String> {
        let fail =
            |what: &str| format!("Couldn't show the open dialog ({what}). Try reloading the page.");
        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| fail("no document"))?;
        let body = document.body().ok_or_else(|| fail("no page body"))?;

        // A picker that never reported back (see `open_file`) is still on the page. Tell it
        // that it was cancelled, which resolves its `Pending`, and replace it.
        if let Some(stale) = document.get_element_by_id(PICKER_ID) {
            if let Ok(cancel) = web_sys::Event::new("cancel") {
                let _ = stale.dispatch_event(&cancel);
            }
            stale.remove();
        }

        let input: web_sys::HtmlInputElement = document
            .create_element("input")
            .map_err(|_| fail("element"))?
            .dyn_into()
            .map_err(|_| fail("input"))?;
        input.set_type("file");
        input.set_id(PICKER_ID);
        input.set_accept(accept);
        input.set_hidden(true);
        body.append_child(&input).map_err(|_| fail("page"))?;

        let mut listening = true;
        let picked = js_sys::Promise::new(&mut |resolve, _reject| {
            listening = input
                .add_event_listener_with_callback("change", &resolve)
                .is_ok()
                && input
                    .add_event_listener_with_callback("cancel", &resolve)
                    .is_ok();
        });
        if !listening {
            input.remove();
            return Err(fail("events"));
        }
        input.click();
        Ok((input, picked))
    }

    async fn read_picked(
        input: web_sys::HtmlInputElement,
        picked: js_sys::Promise,
    ) -> Result<Option<OpenedFile>, String> {
        // Resolves on "change" (a file was picked) and on "cancel"; it never rejects.
        let _ = JsFuture::from(picked).await;
        let file = input.files().and_then(|files| files.get(0));
        input.remove();
        let Some(file) = file else {
            return Ok(None);
        };
        let name = file.name();
        let buffer = JsFuture::from(file.array_buffer()).await.map_err(|e| {
            format!(
                "Couldn't read {name} ({}). Try choosing the file again.",
                crate::js_error_text(&e)
            )
        })?;
        Ok(Some(OpenedFile {
            name,
            path: None,
            bytes: js_sys::Uint8Array::new(&buffer).to_vec(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn write_file_creates_and_overwrites() {
        let dir = TestDir::new("write-file");
        let path = dir.path().join("bracket.peet");
        write_file(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        write_file(&path, b"second, longer").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second, longer");
        write_file(&path, b"").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"");
    }

    #[test]
    fn write_file_leaves_no_temp_file() {
        let dir = TestDir::new("no-temp");
        let path = dir.path().join("bracket.peet");
        write_file(&path, b"one").unwrap();
        write_file(&path, b"two").unwrap();
        assert_eq!(names(dir.path()), ["bracket.peet"]);
    }

    #[test]
    fn write_file_to_missing_directory_errors_cleanly() {
        let dir = TestDir::new("missing-dir");
        let path = dir.path().join("not-there").join("bracket.peet");
        let message = write_file(&path, b"data").unwrap_err();
        assert!(message.contains("bracket.peet"), "{message}");
        assert!(message.contains("Save As"), "{message}");
        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn failed_write_keeps_the_target_and_cleans_up() {
        let dir = TestDir::new("failed-write");
        // The rename can't replace a folder, so this fails after the temp file was written.
        let target = dir.path().join("occupied");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep.txt"), b"keep").unwrap();
        assert!(write_file(&target, b"data").is_err());
        assert_eq!(names(dir.path()), ["occupied"]);
        assert_eq!(std::fs::read(target.join("keep.txt")).unwrap(), b"keep");
    }

    #[test]
    fn write_file_without_a_file_name_errors() {
        assert!(write_file(Path::new(""), b"data").is_err());
    }

    #[test]
    fn concurrent_writes_to_one_file_leave_one_complete_version() {
        let dir = TestDir::new("concurrent");
        let path = dir.path().join("bracket.peet");
        let contents: Vec<Vec<u8>> = (0..8u8).map(|i| vec![i; 4096]).collect();
        std::thread::scope(|scope| {
            for bytes in &contents {
                let path = &path;
                // Replacing a file another thread is replacing can be refused on Windows;
                // what matters is that nothing is torn and nothing is left behind.
                scope.spawn(move || {
                    let _ = write_file(path, bytes);
                });
            }
        });
        let written = std::fs::read(&path).unwrap();
        assert!(contents.contains(&written));
        assert_eq!(names(dir.path()), ["bracket.peet"]);
    }

    #[test]
    fn read_file_reports_name_path_and_bytes() {
        let dir = TestDir::new("read-file");
        let path = dir.path().join("bracket.peet");
        std::fs::write(&path, b"part").unwrap();
        let opened = imp::read_file(path.clone()).unwrap();
        assert_eq!(opened.name, "bracket.peet");
        assert_eq!(opened.path.as_deref(), Some(path.as_path()));
        assert_eq!(opened.bytes, b"part");
    }

    #[test]
    fn read_file_that_is_missing_errors_cleanly() {
        let dir = TestDir::new("read-missing");
        let message = imp::read_file(dir.path().join("gone.peet")).unwrap_err();
        assert!(message.contains("gone.peet"), "{message}");
    }
}
