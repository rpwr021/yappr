use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::expand_tilde;

static CONFIG: OnceLock<RwLock<LogConfig>> = OnceLock::new();

/// Rotate once the log passes this size, keeping one previous generation.
///
/// The log had no cap at all: on this machine it reached 3000 lines spanning
/// three months and kept growing. With `debug = true` every transcript and answer
/// is written, so it also became an unbounded verbatim record of everything the
/// user dictated. Two files of this size is a few hundred KB, which is plenty of
/// history for diagnosing a problem and bounded.
const MAX_LOG_BYTES: u64 = 1_048_576;

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
        // Check after writing so the size reflects this line. `metadata` on the
        // open handle avoids a second path lookup.
        if should_rotate(file.metadata().map(|m| m.len()).unwrap_or(0)) {
            drop(file);
            rotate(&cfg.path);
        }
    }
}

/// Move the current log aside to `<name>.1`, replacing any previous generation.
///
/// One generation only: the point is to bound growth, and a rename is atomic
/// enough that a concurrent writer either lands in the old file (and is rotated
/// away) or creates a fresh one on its next append.
fn rotate(path: &std::path::Path) {
    let mut previous = path.as_os_str().to_owned();
    previous.push(".1");
    let _ = std::fs::rename(path, std::path::PathBuf::from(previous));
}

/// Whether a log of `len` bytes should be rotated.
fn should_rotate(len: u64) -> bool {
    len >= MAX_LOG_BYTES
}

fn default_config() -> LogConfig {
    LogConfig {
        enabled: true,
        debug: false,
        path: expand_tilde("~/.yappr/yappr.log"),
    }
}

#[cfg(test)]
mod tests {
    use super::{rotate, should_rotate, MAX_LOG_BYTES};

    #[test]
    fn rotates_only_once_the_cap_is_reached() {
        assert!(!should_rotate(0));
        assert!(!should_rotate(MAX_LOG_BYTES - 1));
        assert!(should_rotate(MAX_LOG_BYTES));
        assert!(should_rotate(MAX_LOG_BYTES * 3));
    }

    #[test]
    fn writing_past_the_cap_rotates_through_the_real_write_path() {
        // Drives write_line rather than rotate() directly, so this covers the
        // metadata check and the drop-before-rename ordering too.
        let dir = std::env::temp_dir().join(format!("yappr-caplog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let log = dir.join("capped.log");
        super::init(true, false, log.to_str().expect("utf8 path"));

        // Seed just under the cap so the next line crosses it.
        std::fs::write(&log, vec![b'x'; (MAX_LOG_BYTES - 10) as usize]).expect("seed");
        super::log_line("this line crosses the cap");

        let previous = dir.join("capped.log.1");
        assert!(previous.exists(), "oversized log should be rotated to .1");
        // The rotated file holds the seed plus the crossing line; the live log is
        // recreated empty by the next write.
        let rotated = std::fs::metadata(&previous).expect("stat .1").len();
        assert!(rotated >= MAX_LOG_BYTES, "rotated at {rotated} bytes");

        super::log_line("after rotation");
        let fresh = std::fs::read_to_string(&log).expect("fresh log");
        assert!(fresh.contains("after rotation"));
        assert!(
            fresh.len() < 200,
            "fresh log should start over, got {} bytes",
            fresh.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_moves_the_log_aside_and_replaces_the_previous_generation() {
        let dir = std::env::temp_dir().join(format!("yappr-rotate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let log = dir.join("yappr.log");
        let previous = dir.join("yappr.log.1");

        std::fs::write(&previous, b"older generation").expect("seed .1");
        std::fs::write(&log, b"current generation").expect("seed log");
        rotate(&log);

        // The live log is moved aside, and the older .1 is replaced rather than
        // accumulating generations.
        assert!(!log.exists(), "log should have been renamed away");
        assert_eq!(
            std::fs::read_to_string(&previous).expect("read .1"),
            "current generation"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
