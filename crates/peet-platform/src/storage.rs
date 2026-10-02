//! Persistent key → bytes store for autosave and crash recovery.
//!
//! Natively each key is a file under `data_dir()/autosave/`, written atomically. On the web
//! the values live in IndexedDB: database "peetcad", object store "autosave".
//!
//! Every function returns a [`Pending`]: resolved straight away natively, later on the web.
//! Operations take effect in the order they were started (on the web that ordering is
//! IndexedDB's: transactions on one object store run in the order they were created).

use crate::Pending;

/// Stores `bytes` under `key`, replacing what was there.
pub fn put(key: &str, bytes: Vec<u8>) -> Pending<Result<(), String>> {
    imp::put(key, bytes)
}

/// Reads the bytes stored under `key`. `Ok(None)` if there are none.
pub fn get(key: &str) -> Pending<Result<Option<Vec<u8>>, String>> {
    imp::get(key)
}

/// Deletes `key`. Deleting a key that doesn't exist is not an error.
pub fn remove(key: &str) -> Pending<Result<(), String>> {
    imp::remove(key)
}

/// All keys, sorted.
pub fn keys() -> Pending<Result<Vec<String>, String>> {
    imp::keys()
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};

    use crate::Pending;

    const PREFIX: &str = "k-";
    const SUFFIX: &str = ".bin";
    /// Longest encoded key. File names are limited to 255 bytes on the common file systems,
    /// and the prefix, suffix and temporary file decoration need room too.
    const MAX_ENCODED_KEY: usize = 200;

    pub fn put(key: &str, bytes: Vec<u8>) -> Pending<Result<(), String>> {
        Pending::ready(storage_dir().and_then(|dir| put_in(&dir, key, &bytes)))
    }

    pub fn get(key: &str) -> Pending<Result<Option<Vec<u8>>, String>> {
        Pending::ready(storage_dir().and_then(|dir| get_in(&dir, key)))
    }

    pub fn remove(key: &str) -> Pending<Result<(), String>> {
        Pending::ready(storage_dir().and_then(|dir| remove_in(&dir, key)))
    }

    pub fn keys() -> Pending<Result<Vec<String>, String>> {
        Pending::ready(storage_dir().and_then(|dir| keys_in(&dir)))
    }

    fn storage_dir() -> Result<PathBuf, String> {
        crate::data_dir()
            .map(|dir| dir.join("autosave"))
            .ok_or_else(|| {
                "Couldn't find a folder for autosave data. Set the PEETCAD_DATA_DIR environment variable to a folder PeetCAD may write to."
                    .to_owned()
            })
    }

    fn fail(action: &str, dir: &Path, error: &std::io::Error) -> String {
        format!(
            "Couldn't {action} in {}: {error}. Check that the folder is writable and the disk isn't full.",
            dir.display()
        )
    }

    /// The file name for a key. Lowercase ASCII letters, digits, `-` and `_` are kept and
    /// every other byte becomes `%XX`, so any key gives a name that is valid everywhere,
    /// distinct keys stay distinct on case-insensitive file systems, and the key can be
    /// recovered from the name. The prefix keeps the name clear of the reserved Windows
    /// device names (`con`, `nul`, ...).
    pub(super) fn file_name_for(key: &str) -> Result<String, String> {
        use std::fmt::Write as _;

        let mut name = String::from(PREFIX);
        for byte in key.bytes() {
            match byte {
                b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => name.push(char::from(byte)),
                _ => {
                    let _ = write!(name, "%{byte:02X}");
                }
            }
        }
        if name.len() - PREFIX.len() > MAX_ENCODED_KEY {
            return Err(format!(
                "Couldn't use the autosave name \"{key}\": it is too long. Give the document a shorter name."
            ));
        }
        name.push_str(SUFFIX);
        Ok(name)
    }

    /// The key a file name stands for, or `None` if the file isn't one of ours.
    fn key_for(file_name: &str) -> Option<String> {
        let encoded = file_name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
        let mut bytes = Vec::with_capacity(encoded.len());
        let mut rest = encoded.as_bytes();
        while let Some((&first, tail)) = rest.split_first() {
            if first == b'%' {
                let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                rest = &tail[2..];
            } else {
                bytes.push(first);
                rest = tail;
            }
        }
        let key = String::from_utf8(bytes).ok()?;
        // Only names this module would have written itself count.
        (file_name_for(&key).ok()? == file_name).then_some(key)
    }

    pub(super) fn put_in(dir: &Path, key: &str, bytes: &[u8]) -> Result<(), String> {
        let path = dir.join(file_name_for(key)?);
        std::fs::create_dir_all(dir).map_err(|e| fail("create the autosave folder", dir, &e))?;
        crate::files::write_atomic(&path, bytes).map_err(|e| fail("write the autosave", dir, &e))
    }

    pub(super) fn get_in(dir: &Path, key: &str) -> Result<Option<Vec<u8>>, String> {
        match std::fs::read(dir.join(file_name_for(key)?)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(fail("read the autosave", dir, &e)),
        }
    }

    pub(super) fn remove_in(dir: &Path, key: &str) -> Result<(), String> {
        match std::fs::remove_file(dir.join(file_name_for(key)?)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(fail("delete the autosave", dir, &e)),
        }
    }

    pub(super) fn keys_in(dir: &Path) -> Result<Vec<String>, String> {
        let list_failed = |e: std::io::Error| fail("list the autosaves", dir, &e);
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(list_failed(e)),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(list_failed)?;
            // Anything else in the folder (a temporary file left by a crash mid-write, a
            // file or folder the user put there) is not a key.
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            if let Some(key) = entry.file_name().to_str().and_then(key_for) {
                keys.push(key);
            }
        }
        keys.sort();
        Ok(keys)
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use std::future::Future;

    use crate::{Pending, js_error_text};
    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::JsValue;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{IdbDatabase, IdbObjectStore, IdbRequest, IdbTransaction, IdbTransactionMode};

    const DATABASE: &str = "peetcad";
    const STORE: &str = "autosave";
    /// Raising this needs an upgrade step in `open_database`, and a handler for the open
    /// request's `blocked` event (another tab still holding the old version).
    const VERSION: u32 = 1;

    pub fn put(key: &str, bytes: Vec<u8>) -> Pending<Result<(), String>> {
        let key = JsValue::from_str(key);
        run(async move {
            let (database, transaction, store) = begin(IdbTransactionMode::Readwrite).await?;
            let value = js_sys::Uint8Array::from(bytes.as_slice());
            drop(bytes);
            let outcome = match store.put_with_key(&value, &key) {
                Ok(_) => committed(&transaction).await,
                Err(e) => Err(js_error_text(&e)),
            };
            database.close();
            outcome.map_err(|detail| {
                format!(
                    "Couldn't write the autosave to browser storage ({detail}). Free up space for this site, and use Save As to keep your work."
                )
            })
        })
    }

    pub fn get(key: &str) -> Pending<Result<Option<Vec<u8>>, String>> {
        let key = JsValue::from_str(key);
        run(async move {
            let (database, _transaction, store) = begin(IdbTransactionMode::Readonly).await?;
            let outcome = match store.get(&key) {
                Ok(request) => finished(&request).await,
                Err(e) => Err(js_error_text(&e)),
            };
            database.close();
            let value = outcome.map_err(|detail| {
                format!("Couldn't read the autosave from browser storage ({detail}). Try reloading the page.")
            })?;
            if value.is_undefined() || value.is_null() {
                return Ok(None);
            }
            if !(value.is_instance_of::<js_sys::Uint8Array>()
                || value.is_instance_of::<js_sys::ArrayBuffer>())
            {
                return Err(
                    "Couldn't read the autosave from browser storage (it isn't in a format PeetCAD wrote). Clear this site's data to reset it."
                        .to_owned(),
                );
            }
            Ok(Some(js_sys::Uint8Array::new(&value).to_vec()))
        })
    }

    pub fn remove(key: &str) -> Pending<Result<(), String>> {
        let key = JsValue::from_str(key);
        run(async move {
            let (database, transaction, store) = begin(IdbTransactionMode::Readwrite).await?;
            let outcome = match store.delete(&key) {
                Ok(_) => committed(&transaction).await,
                Err(e) => Err(js_error_text(&e)),
            };
            database.close();
            outcome.map_err(|detail| {
                format!("Couldn't delete the autosave from browser storage ({detail}). Try reloading the page.")
            })
        })
    }

    pub fn keys() -> Pending<Result<Vec<String>, String>> {
        run(async move {
            let (database, _transaction, store) = begin(IdbTransactionMode::Readonly).await?;
            let outcome = match store.get_all_keys() {
                Ok(request) => finished(&request).await,
                Err(e) => Err(js_error_text(&e)),
            };
            database.close();
            let value = outcome.map_err(|detail| {
                format!("Couldn't list the autosaves in browser storage ({detail}). Try reloading the page.")
            })?;
            let mut keys: Vec<String> = js_sys::Array::from(&value)
                .iter()
                .filter_map(|key| key.as_string())
                .collect();
            keys.sort();
            Ok(keys)
        })
    }

    /// Runs `operation` on the browser's event loop and delivers its result to a `Pending`.
    fn run<T: 'static>(
        operation: impl Future<Output = Result<T, String>> + 'static,
    ) -> Pending<Result<T, String>> {
        let (pending, resolver) = Pending::deferred();
        wasm_bindgen_futures::spawn_local(async move {
            resolver.resolve(operation.await);
        });
        pending
    }

    fn refused(detail: &str) -> String {
        format!(
            "The browser refused storage (private browsing?), so autosave and crash recovery aren't available. Use Save As to keep your work. Details: {detail}."
        )
    }

    /// Opens the database and starts a transaction on the store. The caller must issue its
    /// request before awaiting anything else, or the browser commits the empty transaction.
    async fn begin(
        mode: IdbTransactionMode,
    ) -> Result<(IdbDatabase, IdbTransaction, IdbObjectStore), String> {
        let database = open_database().await?;
        let started = database
            .transaction_with_str_and_mode(STORE, mode)
            .and_then(|transaction| {
                let store = transaction.object_store(STORE)?;
                Ok((transaction, store))
            });
        match started {
            Ok((transaction, store)) => Ok((database, transaction, store)),
            Err(e) => {
                database.close();
                Err(refused(&js_error_text(&e)))
            }
        }
    }

    async fn open_database() -> Result<IdbDatabase, String> {
        let factory = web_sys::window()
            .ok_or_else(|| refused("no window"))?
            .indexed_db()
            .map_err(|e| refused(&js_error_text(&e)))?
            .ok_or_else(|| refused("IndexedDB is not available"))?;
        let request = factory
            .open_with_u32(DATABASE, VERSION)
            .map_err(|e| refused(&js_error_text(&e)))?;

        // Runs when the database is new: create the object store.
        let upgrade = Closure::<dyn FnMut(web_sys::Event)>::new({
            let request = request.clone();
            move |_event: web_sys::Event| {
                let Ok(database) = request.result().and_then(|v| v.dyn_into::<IdbDatabase>())
                else {
                    return;
                };
                if !database.object_store_names().contains(STORE) {
                    // A failure here aborts the upgrade and the open request reports it.
                    let _ = database.create_object_store(STORE);
                }
            }
        });
        request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        let opened = finished(&request).await;
        request.set_onupgradeneeded(None);
        drop(upgrade);

        opened
            .and_then(|value| {
                value
                    .dyn_into::<IdbDatabase>()
                    .map_err(|_| "the database didn't open".to_owned())
            })
            .map_err(|detail| refused(&detail))
    }

    /// Waits for a request and returns its result, or a description of why it failed.
    async fn finished(request: &IdbRequest) -> Result<JsValue, String> {
        // The promise's own resolve and reject functions serve as the event handlers.
        let settled = js_sys::Promise::new(&mut |resolve, reject| {
            request.set_onsuccess(Some(&resolve));
            request.set_onerror(Some(&reject));
        });
        let outcome = JsFuture::from(settled).await;
        request.set_onsuccess(None);
        request.set_onerror(None);
        match outcome {
            Ok(_) => request.result().map_err(|e| js_error_text(&e)),
            Err(_) => Err(match request.error() {
                Ok(Some(error)) => error.message(),
                Ok(None) => "unknown error".to_owned(),
                Err(e) => js_error_text(&e),
            }),
        }
    }

    /// Waits until the transaction's changes are stored. A write only counts once its
    /// transaction completes: running out of quota, for one, shows up as an abort.
    async fn committed(transaction: &IdbTransaction) -> Result<(), String> {
        let settled = js_sys::Promise::new(&mut |resolve, reject| {
            transaction.set_oncomplete(Some(&resolve));
            transaction.set_onabort(Some(&reject));
            transaction.set_onerror(Some(&reject));
        });
        let outcome = JsFuture::from(settled).await;
        transaction.set_oncomplete(None);
        transaction.set_onabort(None);
        transaction.set_onerror(None);
        outcome.map(|_| ()).map_err(|_| {
            transaction
                .error()
                .map(|error| error.message())
                .unwrap_or_else(|| "the browser cancelled the write".to_owned())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::imp::{file_name_for, get_in, keys_in, put_in, remove_in};
    use crate::test_dir::TestDir;

    #[test]
    fn round_trip() {
        let dir = TestDir::new("storage-round-trip");
        let bytes: Vec<u8> = (0..=255).collect();
        put_in(dir.path(), "document-1", &bytes).unwrap();
        assert_eq!(get_in(dir.path(), "document-1").unwrap(), Some(bytes));
    }

    #[test]
    fn empty_value_is_not_missing() {
        let dir = TestDir::new("storage-empty");
        put_in(dir.path(), "empty", &[]).unwrap();
        assert_eq!(get_in(dir.path(), "empty").unwrap(), Some(Vec::new()));
    }

    #[test]
    fn missing_key_and_missing_folder_read_as_nothing() {
        let dir = TestDir::new("storage-missing");
        let never_created = dir.path().join("autosave");
        assert_eq!(get_in(&never_created, "nothing").unwrap(), None);
        assert!(keys_in(&never_created).unwrap().is_empty());
        remove_in(&never_created, "nothing").unwrap();
        assert!(!never_created.exists());
    }

    #[test]
    fn put_creates_the_folder() {
        let dir = TestDir::new("storage-create");
        let nested = dir.path().join("PeetCAD").join("autosave");
        put_in(&nested, "a", b"1").unwrap();
        assert_eq!(get_in(&nested, "a").unwrap().as_deref(), Some(&b"1"[..]));
    }

    #[test]
    fn put_overwrites() {
        let dir = TestDir::new("storage-overwrite");
        put_in(dir.path(), "doc", b"a long first version").unwrap();
        put_in(dir.path(), "doc", b"short").unwrap();
        assert_eq!(
            get_in(dir.path(), "doc").unwrap().as_deref(),
            Some(&b"short"[..])
        );
        assert_eq!(keys_in(dir.path()).unwrap(), ["doc"]);
    }

    #[test]
    fn remove_deletes_only_that_key() {
        let dir = TestDir::new("storage-remove");
        put_in(dir.path(), "a", b"1").unwrap();
        put_in(dir.path(), "b", b"2").unwrap();
        remove_in(dir.path(), "a").unwrap();
        assert_eq!(get_in(dir.path(), "a").unwrap(), None);
        assert_eq!(get_in(dir.path(), "b").unwrap().as_deref(), Some(&b"2"[..]));
        assert_eq!(keys_in(dir.path()).unwrap(), ["b"]);
        // Removing it again is fine.
        remove_in(dir.path(), "a").unwrap();
    }

    #[test]
    fn keys_are_sorted() {
        let dir = TestDir::new("storage-sorted");
        for key in ["pear", "apple", "Zebra", "fig", "apple-2", "10", "9"] {
            put_in(dir.path(), key, key.as_bytes()).unwrap();
        }
        let mut expected = vec!["pear", "apple", "Zebra", "fig", "apple-2", "10", "9"];
        expected.sort_unstable();
        assert_eq!(keys_in(dir.path()).unwrap(), expected);
    }

    #[test]
    fn weird_keys_round_trip() {
        let dir = TestDir::new("storage-weird");
        let keys = [
            "",
            " ",
            ".",
            "..",
            "../../escape",
            "C:\\Windows\\system32",
            "/etc/passwd",
            "con",
            "NUL",
            "aux.txt",
            "trailing.",
            "trailing ",
            "a/b\\c:d*e?f\"g<h>i|j",
            "100%",
            "%41",
            "tab\tnew\nline\0nul",
            "Grüße, 世界 🚀",
            "k-nested.bin",
        ];
        for (index, key) in keys.iter().enumerate() {
            put_in(dir.path(), key, &[index as u8]).unwrap();
        }
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(
                get_in(dir.path(), key).unwrap(),
                Some(vec![index as u8]),
                "{key:?}"
            );
        }
        let mut expected: Vec<&str> = keys.to_vec();
        expected.sort_unstable();
        assert_eq!(keys_in(dir.path()).unwrap(), expected);

        // Everything landed directly in the folder, as plain files with tame names.
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            let name = entry.file_name().into_string().unwrap();
            assert!(
                name.bytes().all(|b| b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || b.is_ascii_uppercase()
                    || matches!(b, b'-' | b'_' | b'%' | b'.')),
                "{name}"
            );
        }
        for key in keys {
            remove_in(dir.path(), key).unwrap();
        }
        assert!(keys_in(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn keys_differing_only_in_case_are_distinct() {
        let dir = TestDir::new("storage-case");
        put_in(dir.path(), "part", b"lower").unwrap();
        put_in(dir.path(), "Part", b"mixed").unwrap();
        put_in(dir.path(), "PART", b"upper").unwrap();
        assert_eq!(
            get_in(dir.path(), "part").unwrap().as_deref(),
            Some(&b"lower"[..])
        );
        assert_eq!(
            get_in(dir.path(), "Part").unwrap().as_deref(),
            Some(&b"mixed"[..])
        );
        assert_eq!(
            get_in(dir.path(), "PART").unwrap().as_deref(),
            Some(&b"upper"[..])
        );
        assert_eq!(keys_in(dir.path()).unwrap(), ["PART", "Part", "part"]);
    }

    #[test]
    fn too_long_key_errors_cleanly() {
        let dir = TestDir::new("storage-long");
        let key = "x".repeat(500);
        assert!(put_in(dir.path(), &key, b"1").is_err());
        assert!(get_in(dir.path(), &key).is_err());
        assert!(remove_in(dir.path(), &key).is_err());
        assert!(keys_in(dir.path()).unwrap().is_empty());
        // The longest accepted key still fits in a file name, temporary decoration included.
        let longest = "x".repeat(200);
        put_in(dir.path(), &longest, b"1").unwrap();
        assert_eq!(keys_in(dir.path()).unwrap(), [longest]);
    }

    #[test]
    fn put_leaves_no_temp_file() {
        let dir = TestDir::new("storage-no-temp");
        put_in(dir.path(), "doc", b"one").unwrap();
        put_in(dir.path(), "doc", b"two").unwrap();
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [file_name_for("doc").unwrap()]);
    }

    #[test]
    fn foreign_files_are_not_keys() {
        let dir = TestDir::new("storage-foreign");
        put_in(dir.path(), "doc", b"1").unwrap();
        for name in [
            "notes.txt",
            ".k-doc.bin.1234-0.tmp",
            "k-%zz.bin",
            "k-%4.bin",
            "k-%61.bin",
            "k-UPPER.bin",
            "k-%FF.bin",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("k-folder.bin")).unwrap();
        assert_eq!(keys_in(dir.path()).unwrap(), ["doc"]);
    }

    #[test]
    fn storage_folder_that_is_a_file_errors_cleanly() {
        let dir = TestDir::new("storage-blocked");
        let blocked = dir.path().join("autosave");
        std::fs::write(&blocked, b"not a folder").unwrap();
        let message = put_in(&blocked, "doc", b"1").unwrap_err();
        assert!(message.contains("autosave"), "{message}");
    }
}
