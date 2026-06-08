use std::fs::OpenOptions;
use std::io::Write;

pub fn log_line(message: impl AsRef<str>) {
    let line = message.as_ref();
    eprintln!("{line}");
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let dir = home.join(".yappr");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("yappr.log"))
    {
        let _ = writeln!(file, "{} {line}", chrono::Local::now().format("%F %T"));
    }
}
