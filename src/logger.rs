use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::expand_tilde;

static CONFIG: OnceLock<RwLock<LogConfig>> = OnceLock::new();

struct LogConfig {
    enabled: bool,
    path: PathBuf,
}

pub fn init(enabled: bool, path: &str) {
    let lock = CONFIG.get_or_init(|| RwLock::new(default_config()));
    if let Ok(mut cfg) = lock.write() {
        cfg.enabled = enabled;
        cfg.path = expand_tilde(path);
    }
}

pub fn log_line(message: impl AsRef<str>) {
    let line = message.as_ref();
    eprintln!("{line}");

    let lock = CONFIG.get_or_init(|| RwLock::new(default_config()));
    let Ok(cfg) = lock.read() else {
        return;
    };
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
        path: expand_tilde("~/.yappr/yappr.log"),
    }
}
