use crate::config::Config;
use crate::expand_tilde;
use crate::logger::log_line;
use reqwest::blocking::Client;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct ModelPaths {
    pub weights: Option<PathBuf>,
    pub mmproj: Option<PathBuf>,
}

pub struct ManagedServer {
    child: Option<Child>,
}

impl Drop for ManagedServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn resolve_binary(configured: &str) -> Option<PathBuf> {
    if configured != "auto" && !configured.trim().is_empty() {
        let path = expand_tilde(configured);
        return path.exists().then_some(path);
    }
    let bundled = expand_tilde("~/.yappr/bin/llama-server");
    if bundled.exists() {
        return Some(bundled);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("llama-server"))
            .find(|candidate| candidate.exists())
    })
}

pub fn model_paths(cfg: &Config) -> ModelPaths {
    let base = model_snapshot_dir(cfg)
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| model_snapshot_dir(cfg));
    let mut weights = None;
    let mut mmproj = None;
    if let Ok(snapshots) = std::fs::read_dir(base) {
        for entry in snapshots.flatten() {
            let root = entry.path();
            let w = root.join(&cfg.model.weights);
            let m = root.join(&cfg.model.mmproj);
            if w.exists() {
                weights = Some(w);
            }
            if m.exists() {
                mmproj = Some(m);
            }
        }
    }
    ModelPaths { weights, mmproj }
}

pub fn ensure_model(cfg: &Config) -> Result<ModelPaths, Box<dyn std::error::Error>> {
    let paths = model_paths(cfg);
    if paths.weights.is_some() && paths.mmproj.is_some() {
        return Ok(paths);
    }

    let root = model_snapshot_dir(cfg);
    fs::create_dir_all(&root)?;
    download_model_file(cfg, &cfg.model.weights, &root.join(&cfg.model.weights))?;
    download_model_file(cfg, &cfg.model.mmproj, &root.join(&cfg.model.mmproj))?;
    Ok(model_paths(cfg))
}

pub fn ensure_engine() -> Result<(), Box<dyn std::error::Error>> {
    if resolve_binary("auto").is_some() {
        return Ok(());
    }
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engine/install.sh");
    if !script.exists() {
        return Err("engine installer not found".into());
    }
    let status = Command::new("bash").arg(script).status()?;
    if !status.success() {
        return Err("engine installer failed".into());
    }
    Ok(())
}

pub fn start(
    cfg: &Config,
    weights: &PathBuf,
    mmproj: &PathBuf,
) -> Result<ManagedServer, Box<dyn std::error::Error>> {
    if healthy(cfg.server.port) {
        return Ok(ManagedServer { child: None });
    }
    let binary = resolve_binary(&cfg.server.binary).ok_or("llama-server binary not found")?;
    let stdout = server_log_file(&cfg.logging)?;
    let stderr = stdout.try_clone()?;
    let child = Command::new(binary)
        .arg("-m")
        .arg(weights)
        .arg("--mmproj")
        .arg(mmproj)
        .arg("-fa")
        .arg("on")
        .arg("--jinja")
        .arg("-c")
        .arg(&cfg.model.ctx_size)
        .arg("-ngl")
        .arg(&cfg.model.ngl)
        .arg("--port")
        .arg(cfg.server.port.to_string())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(120) {
        if healthy(cfg.server.port) {
            return Ok(ManagedServer { child: Some(child) });
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("llama-server did not become healthy within 120s".into())
}

fn server_log_file(
    cfg: &crate::config::LoggingConfig,
) -> Result<fs::File, Box<dyn std::error::Error>> {
    if !cfg.enabled {
        return Ok(fs::OpenOptions::new().write(true).open("/dev/null")?);
    }
    Ok(fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/yappr-llama-server.log")?)
}

fn healthy(port: u16) -> bool {
    Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .and_then(|client| client.get(format!("http://127.0.0.1:{port}/health")).send())
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

fn model_snapshot_dir(cfg: &Config) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache/huggingface/hub")
        .join(format!("models--{}", cfg.model.repo.replace('/', "--")))
        .join("snapshots")
        .join("yappr")
}

fn download_model_file(
    cfg: &Config,
    filename: &str,
    dest: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    if dest.exists() {
        return Ok(());
    }
    let url = format!(
        "https://huggingface.co/{}/resolve/main/{filename}",
        cfg.model.repo
    );
    log_line(format!("downloading model file: {url}"));
    let mut response = Client::builder()
        .timeout(Duration::from_secs(1800))
        .build()?
        .get(url)
        .send()?
        .error_for_status()?;
    let tmp = dest.with_extension("part");
    let mut file = fs::File::create(&tmp)?;
    io::copy(&mut response, &mut file)?;
    fs::rename(tmp, dest)?;
    Ok(())
}
