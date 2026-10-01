//! Platform abstraction for PeetCAD.
//!
//! Core crates must not call OS APIs directly, because the same code runs natively on
//! Windows and in the browser (`wasm32-unknown-unknown`). Anything that differs between
//! the two lives here, behind a small API that is identical on both.
//!
//! File dialogs, IndexedDB autosave and background workers will be added here as later
//! phases need them.

pub mod crash;
mod files;

pub use files::{SaveOutcome, save_file};

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
pub fn data_dir() -> Option<std::path::PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
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
pub fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
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
