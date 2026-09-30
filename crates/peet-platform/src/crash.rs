//! Crash reporting.
//!
//! On native builds a panic hook writes a crash report (message, location, backtrace and
//! build info) to `<data_dir>/crashes/`, so users can attach it to a bug report. On the web,
//! eframe's own panic handler catches the panic and the page shows the message instead
//! (see `index.html`), and the report goes to the browser console.

/// Information identifying the build, included in every crash report.
#[derive(Clone, Debug)]
pub struct BuildInfo {
    pub app_name: &'static str,
    pub version: &'static str,
}

/// Installs the crash reporting panic hook. Call once, early in `main`.
///
/// The previous hook still runs afterwards, so panics are also printed to stderr as usual.
pub fn install_panic_hook(build: BuildInfo) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = format_report(&build, info);
        log::error!("{report}");
        #[cfg(not(target_arch = "wasm32"))]
        match write_report(&report) {
            Some(path) => eprintln!("Crash report written to {}", path.display()),
            None => eprintln!("Could not write a crash report file."),
        }
        previous(info);
    }));
}

fn format_report(build: &BuildInfo, info: &std::panic::PanicHookInfo<'_>) -> String {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_owned());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_owned());
    let backtrace = std::backtrace::Backtrace::force_capture();
    format!(
        "{} {} crashed.\n\nPlatform: {}\nMessage: {message}\nLocation: {location}\n\nBacktrace:\n{backtrace}\n",
        build.app_name,
        build.version,
        crate::target_description(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn write_report(report: &str) -> Option<std::path::PathBuf> {
    let dir = crate::data_dir()?.join("crashes");
    std::fs::create_dir_all(&dir).ok()?;
    let secs = crate::SystemTime::now()
        .duration_since(crate::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{secs}.txt"));
    std::fs::write(&path, report).ok()?;
    Some(path)
}
