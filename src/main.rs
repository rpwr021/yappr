mod app;
mod audio;
mod chat;
mod config;
mod hotkey;
mod inject;
mod instance;
mod logger;
mod mascot;
mod perms;
mod runtime;
mod search;
mod server;
mod speech;
mod ui;
mod vad;

use std::path::PathBuf;

fn main() {
    if let Err(err) = app::run(std::env::args().skip(1).collect()) {
        logger::log_line(format!("fatal: {err}"));
        eprintln!("yappr: {err}");
        std::process::exit(1);
    }
}

pub(crate) fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(path))
    } else if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// The app version shown in the UI. Reads `CFBundleShortVersionString` from the
/// bundle's Info.plist (patched at build time), since the Cargo version is a
/// fixed placeholder. Falls back to the compile-time crate version for non-bundle
/// (e.g. `cargo run`) execution.
pub(crate) fn version() -> String {
    bundle_plist_version().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn bundle_plist_version() -> Option<String> {
    // Executable is at Yappr.app/Contents/MacOS/Yappr; Info.plist is two up.
    let exe = std::env::current_exe().ok()?;
    let plist = exe.parent()?.parent()?.join("Info.plist");
    let text = std::fs::read_to_string(plist).ok()?;
    let key = "<key>CFBundleShortVersionString</key>";
    let after = &text[text.find(key)? + key.len()..];
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    Some(after[start..end].trim().to_string())
}
