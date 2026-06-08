use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::expand_tilde;

static CONFIG: OnceLock<RwLock<LogConfig>> = OnceLock::new();

struct LogConfig {
    enabled: bool,
    debug: bool,
    path: PathBuf,
}

pub fn init(enabled: bool, debug: bool, path: &str) {
    let lock = CONFIG.get_or_init(|| RwLock::new(default_config()));
    if let Ok(mut cfg) = lock.write() {
        cfg.enabled = enabled;
        cfg.debug = debug;
        cfg.path = expand_tilde(path);
    }
}

pub fn log_line(message: impl AsRef<str>) {
    write_line(message.as_ref(), true);
}

pub fn debug_line(message: impl AsRef<str>) {
    write_line(message.as_ref(), false);
}

fn write_line(line: &str, always: bool) {
    let lock = CONFIG.get_or_init(|| RwLock::new(default_config()));
    let Ok(cfg) = lock.read() else {
        return;
    };
    if !always && !cfg.debug {
        return;
    }
    eprintln!("{line}");
    if !cfg.enabled {
        return;
    }
    if let Some(parent) = cfg.path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&cfg.path) {
        let _ = writeln!(file, "{} {line}", chrono::Local::now().format("%F %T"));
    }
}

fn default_config() -> LogConfig {
    LogConfig {
        enabled: true,
        debug: false,
        path: expand_tilde("~/.yappr/yappr.log"),
    }
}
