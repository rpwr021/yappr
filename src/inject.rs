use std::io::Write;
use std::process::{Command, Stdio};

pub fn paste_text(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    copy_text(text)?;
    let status = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(r#"tell application "System Events" to keystroke "v" using command down"#)
        .status()?;
    if !status.success() {
        return Err("paste keystroke failed; grant Accessibility to Yappr".into());
    }
    Ok(())
}

pub fn copy_text(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut pbcopy = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()?;
    pbcopy
        .stdin
        .as_mut()
        .ok_or("pbcopy stdin unavailable")?
        .write_all(text.as_bytes())?;
    let status = pbcopy.wait()?;
    if !status.success() {
        return Err("pbcopy failed".into());
    }
    Ok(())
}

pub fn say(text: &str, voice: Option<&str>, rate: i32) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Command::new("/usr/bin/say");
    command.arg("-r").arg(rate.to_string());
    if let Some(voice) = voice {
        command.arg("-v").arg(voice);
    }
    command.arg(text);
    let status = command.status()?;
    if !status.success() {
        return Err("say failed".into());
    }
    Ok(())
}

pub fn stop_speech() {
    let _ = Command::new("/usr/bin/killall").arg("say").status();
}
