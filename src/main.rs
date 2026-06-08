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
