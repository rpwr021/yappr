use crate::logger::log_line;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub struct InstanceLock {
    path: PathBuf,
}

impl InstanceLock {
    pub fn acquire() -> Result<Self, Box<dyn std::error::Error>> {
        let path = dirs::home_dir()
            .ok_or("home directory not found")?
            .join(".yappr/app.pid");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(pid) = raw.trim().parse::<i32>() {
                if pid == std::process::id() as i32 {
                    // Our own pid; nothing to take over.
                } else if !process_alive(pid) {
                    log_line(format!("clearing stale instance lock: pid={pid} not running"));
                } else if !is_yappr(pid) {
                    // The pid file survives reboots and pids get recycled, so a
                    // live pid is not necessarily the previous Yappr. Signalling
                    // it blindly could SIGTERM an unrelated process.
                    log_line(format!(
                        "ignoring instance lock: pid={pid} is alive but is not Yappr ({})",
                        process_path(pid).unwrap_or_else(|| "unknown".to_string())
                    ));
                } else {
                    log_line(format!("terminating previous instance: pid={pid}"));
                    terminate(pid);
                    wait_for_exit(pid);
                }
            }
        }
        let pid = std::process::id();
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        writeln!(file, "{pid}")?;
        log_line(format!("active instance lock acquired: pid={pid}"));
        Ok(Self { path })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        log_line("active instance lock released");
    }
}

fn process_alive(pid: i32) -> bool {
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

/// Executable path of a running process, via `ps`. None if it cannot be read.
fn process_path(pid: i32) -> Option<String> {
    let output = std::process::Command::new("/bin/ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("comm=")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// Whether `pid` is another Yappr instance rather than an unrelated process that
/// happens to have inherited a recycled pid.
fn is_yappr(pid: i32) -> bool {
    process_path(pid).is_some_and(|path| is_yappr_path(&path))
}

/// Does this executable path belong to Yappr?
///
/// Matches the bundle executable and the bare binary (`cargo run`, tests), while
/// rejecting unrelated processes. Kept separate so it is testable without
/// spawning anything.
fn is_yappr_path(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    file == "Yappr" || file == "yappr"
}

fn terminate(pid: i32) {
    unsafe {
        let _ = libc::kill(pid, libc::SIGTERM);
    }
}

fn wait_for_exit(pid: i32) {
    for _ in 0..20 {
        if !process_alive(pid) {
            log_line(format!("previous instance exited: pid={pid}"));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    log_line(format!(
        "previous instance still alive after SIGTERM: pid={pid}; sending SIGKILL"
    ));
    unsafe {
        let _ = libc::kill(pid, libc::SIGKILL);
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
}

#[cfg(test)]
mod tests {
    use super::{is_yappr, is_yappr_path, process_alive, process_path};

    #[test]
    fn recognises_yappr_executables_only() {
        assert!(is_yappr_path("/Applications/Yappr.app/Contents/MacOS/Yappr"));
        assert!(is_yappr_path("/Users/me/proj/target/debug/yappr"));
        // The pid file survives reboots and pids are recycled, so these must not
        // be mistaken for a previous instance and signalled.
        assert!(!is_yappr_path("/sbin/launchd"));
        assert!(!is_yappr_path("/usr/bin/ssh"));
        assert!(!is_yappr_path("/Applications/Safari.app/Contents/MacOS/Safari"));
        // Substring matches must not count.
        assert!(!is_yappr_path("/tmp/yappr-helper"));
        assert!(!is_yappr_path("/tmp/not-yappr"));
    }

    #[test]
    fn pid_1_is_alive_but_is_not_yappr() {
        // launchd always exists and is never Yappr, so this pins the guard that
        // stops a recycled pid being SIGTERMed.
        assert!(process_alive(1), "pid 1 should always be alive");
        assert_eq!(process_path(1).as_deref(), Some("/sbin/launchd"));
        assert!(!is_yappr(1), "must never treat launchd as a Yappr instance");
    }

    #[test]
    fn this_test_binary_is_not_mistaken_for_the_app() {
        // Running as the test harness, our own executable is not named Yappr.
        let me = std::process::id() as i32;
        assert!(process_alive(me));
        assert!(process_path(me).is_some(), "should read our own path");
    }
}
