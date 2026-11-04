//! When Tap itself goes down, what happened goes to `crash.log` in the store folder
//! (Search's `Links.swift` keeps the same file). Release builds abort on a panic
//! (`panic = "abort"`, docs/PLAN.md §10): this hook runs first and writes the report.

use std::{fmt::Write as _, io::Write as _, panic::PanicHookInfo, path::Path};

use crate::store;

/// Installs the hook. The path is worked out now, before anything can have gone wrong.
pub fn install() {
    let log = store::file("crash.log");
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        append(&log, &report(info, &backtrace));
        previous(info);
    }));
}

fn report(info: &PanicHookInfo<'_>, backtrace: &str) -> String {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".to_owned());
    let place = info
        .location()
        .map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
    entry(&message, &place, backtrace)
}

fn entry(message: &str, place: &str, backtrace: &str) -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut text = String::new();
    let _ = writeln!(
        text,
        "--- Tap {} panicked{place} (unix time {seconds})",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(text, "{message}");
    let _ = writeln!(text, "{backtrace}");
    text
}

fn append(log: &Path, text: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let _ = file.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_names_the_version_the_place_and_the_message_and_appends() {
        let log = std::env::temp_dir().join(format!("tap-crash-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&log);
        append(&log, &entry("first", " at a.rs:1", "trace one"));
        append(&log, &entry("second", "", "trace two"));
        let text = std::fs::read_to_string(&log).expect("written");
        assert!(text.contains(&format!(
            "--- Tap {} panicked at a.rs:1",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(text.find("first").expect("first") < text.find("second").expect("second"));
        assert!(text.contains("trace two"));
        let _ = std::fs::remove_file(&log);
    }
}
