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
                if pid != std::process::id() as i32 && process_alive(pid) {
                    log_line(format!("terminating previous instance: pid={pid}"));
                    terminate(pid);
                    wait_for_exit(pid);
                }
                log_line(format!("removing previous instance lock: pid={pid}"));
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
