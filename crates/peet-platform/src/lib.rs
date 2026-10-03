//! Platform abstraction for PeetCAD.
//!
//! Core crates must not call OS APIs directly, because the same code runs natively on
//! Windows and in the browser (`wasm32-unknown-unknown`). Anything that differs between
//! the two lives here, behind a small API that is identical on both.
//!
//! That covers the clock, file dialogs ([`open_file`], [`save_file_as`], [`write_file`]),
//! the autosave store ([`storage`]), crash reports ([`crash`]) and updates of an installed
//! copy ([`update`]). Operations the browser
//! can only do asynchronously return a [`Pending`]. Background workers will be added here
//! when a later phase needs them.

pub mod crash;
mod files;
mod pending;
pub mod storage;
pub mod update;

pub use files::{
    OpenedFile, SaveOutcome, SavedFile, open_file, save_file, save_file_as, write_file,
};
pub use pending::{Pending, Resolver};

/// Monotonic clock that works on native and in the browser (`std::time::Instant` panics on wasm32).
pub use web_time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Which kind of platform the app is running on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformKind {
    Native,
    Web,
}

/// The platform this build targets.
pub const fn kind() -> PlatformKind {
    if cfg!(target_arch = "wasm32") {
        PlatformKind::Web
    } else {
        PlatformKind::Native
    }
}

pub const fn is_web() -> bool {
    matches!(kind(), PlatformKind::Web)
}

/// A short human readable description of the build target, for About dialogs and bug reports.
pub fn target_description() -> String {
    if is_web() {
        "Web (WebAssembly)".to_owned()
    } else {
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

/// Directory for per-user application data (settings, crash reports, autosaves).
///
/// Returns `None` on the web, where data lives in browser storage instead of files.
///
/// Setting the environment variable `PEETCAD_DATA_DIR` to a folder makes that folder the data
/// directory (for portable installs and for testing).
pub fn data_dir() -> Option<std::path::PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(dir) = std::env::var_os("PEETCAD_DATA_DIR").filter(|dir| !dir.is_empty()) {
            return Some(std::path::PathBuf::from(dir));
        }
        let base = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|h| std::path::PathBuf::from(h).join(".local/share"))
                })
        };
        base.map(|b| b.join("PeetCAD"))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// Time elapsed since `start`, in milliseconds, as a float (convenient for display).
/// The current time as `2026-01-31T14:05:09` (UTC), for file headers.
pub fn timestamp_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    iso_from_unix(secs)
}

/// A count of seconds since 1970 as a calendar date and time (UTC).
fn iso_from_unix(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Civil date from a day count (Howard Hinnant's algorithm), in the era starting
    // 0000-03-01.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

pub fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// Text of an error thrown by a browser API, for messages shown to the user.
#[cfg(target_arch = "wasm32")]
pub(crate) fn js_error_text(error: &wasm_bindgen::JsValue) -> String {
    use wasm_bindgen::JsCast as _;

    if let Some(error) = error.dyn_ref::<js_sys::Error>() {
        return String::from(error.message());
    }
    error
        .as_string()
        .unwrap_or_else(|| "unknown error".to_owned())
}

/// Scratch folders for tests. Tests run in parallel, so each gets a folder of its own, under
/// the system temp folder rather than the user's profile.
#[cfg(test)]
pub(crate) mod test_dir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// An empty folder that is deleted again when this is dropped.
    pub struct TestDir(PathBuf);

    impl TestDir {
        pub fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "peetcad-test-{}-{}-{label}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("create test folder");
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_platform() {
        assert_eq!(kind(), PlatformKind::Native);
        assert!(!target_description().is_empty());
    }

    #[test]
    fn clock_is_monotonic() {
        let a = Instant::now();
        let b = Instant::now();
        assert!(b >= a);
        assert!(elapsed_ms(a) >= 0.0);
    }
}
