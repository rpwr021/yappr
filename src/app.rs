use crate::asr;
use crate::audio;
use crate::chat::{ChatClient, ChatMode};
use crate::config::Config;
use crate::hotkey;
use crate::inject;
use crate::instance::InstanceLock;
use crate::logger;
use crate::perms;
use crate::runtime::Runtime;
use crate::server::{self, ManagedServer};
use crate::speech;
use std::fs;
use std::path::PathBuf;

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::load()?;
    logger::init(cfg.logging.enabled, cfg.logging.debug, &cfg.logging.path);
    install_panic_hook();

    if args.iter().any(|arg| arg == "--check") {
        let checks = print_checks(&cfg);
        // Exit non-zero on a failed probe so this is usable from a script or a
        // health check, rather than 67 lines the caller has to read by eye.
        if !checks.failures.is_empty() {
            std::process::exit(1);
        }
        return Ok(());
    }

    if args.iter().any(|arg| arg == "--record-test") {
        let seconds = string_arg(&args, "--seconds")
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(3.0);
        let captured =
            audio::record_for(cfg.audio.device.as_deref(), seconds, cfg.audio.samplerate)?;
        let path = PathBuf::from("/tmp/yappr-record-test.wav");
        fs::write(&path, &captured.wav)?;
        println!(
            "recorded {:.2}s, peak={:.4}, samples={}, nonzero_samples={}, wav={}",
            captured.seconds,
            captured.peak,
            captured.samples,
            captured.nonzero_samples,
            path.display()
        );
        return Ok(());
    }

    if args.iter().any(|arg| arg == "--serve") {
        let _server = start_backend(&cfg)?;
        println!("server ready at {}", cfg.server.endpoint);
        park_until_ctrl_c();
        return Ok(());
    }

    if let Some(text) = string_arg(&args, "--speak") {
        speech::speak(text, &cfg.speech)?;
        return Ok(());
    }

    if let Some(question) = string_arg(&args, "--ask") {
        let _server = start_backend(&cfg)?;
        let answer = ChatClient::new(cfg.clone())?.answer(question, ChatMode::Spoken)?;
        println!("answer: {answer}");
        return Ok(());
    }

    if let Some(path) = arg_value(&args, "--wav") {
        let text = if cfg.mode.is_poor() {
            // GPU-poor: in-process ASR, no llama-server.
            server::ensure_asr_model(&cfg)?;
            let captured = audio::decode_wav(&fs::read(path)?)?;
            asr::transcribe(
                &captured.pcm,
                captured.sample_rate,
                &cfg.asr,
                &cfg.language.source,
            )?
        } else {
            let _server = start_backend(&cfg)?;
            ChatClient::new(cfg.clone())?.transcribe_wav(&fs::read(path)?)?
        };
        println!("{text}");
        if args.iter().any(|arg| arg == "--paste") {
            inject::paste_text(&text)?;
        }
        return Ok(());
    }

    if let Some(path) = arg_value(&args, "--ask-wav") {
        let _server = start_backend(&cfg)?;
        let wav = fs::read(path)?;
        let client = ChatClient::new(cfg.clone())?;
        let question = client.transcribe_wav(&wav)?;
        println!("heard: {question}");
        let answer = client.answer(&question, ChatMode::Spoken)?;
        println!("answer: {answer}");
        speech::speak(&answer, &cfg.speech)?;
        return Ok(());
    }

    if args.is_empty() || args.iter().any(|arg| arg == "--app") {
        let instance_lock = InstanceLock::acquire()?;
        // Trigger the macOS microphone prompt up front if access hasn't been
        // decided yet. Without an explicit request the OS silently streams zero
        // samples instead of prompting. Resolves asynchronously; capture
        // re-checks authorization.
        perms::request_microphone_access();
        let client = ChatClient::new(cfg.clone())?;
        let runtime = Runtime::new(cfg, client);
        runtime.hold_instance_lock(instance_lock);
        // A takeover SIGTERMs the previous instance, and the default disposition
        // exits without unwinding, so its managed llama-server was never killed
        // and leaked a multi-GB process the new instance then adopted unmanaged.
        install_term_handler();
        // Backend (model download, engine install, llama-server) is provisioned
        // in the background from hotkey::run so the menu bar appears immediately.
        return hotkey::run(runtime);
    }

    print_usage();
    Ok(())
}

fn start_backend(cfg: &Config) -> Result<Option<ManagedServer>, Box<dyn std::error::Error>> {
    if !cfg.server.manage {
        return Ok(None);
    }
    let paths = server::ensure_model(cfg)?;
    let weights = paths
        .weights
        .ok_or("model weights not found in Hugging Face cache; download the configured model")?;
    let mmproj = paths
        .mmproj
        .ok_or("audio mmproj not found in Hugging Face cache; download the configured model")?;
    server::ensure_engine()?;
    server::start(cfg, &weights, &mmproj).map(Some)
}

/// Outcome of the probes `--check` runs, so the command can exit non-zero and be
/// used from a script or a health check instead of needing its output eyeballed.
#[derive(Default)]
struct Checks {
    failures: Vec<String>,
    warnings: Vec<String>,
}

impl Checks {
    /// Print `label: value` and record a failure when the probe did not pass.
    fn probe(&mut self, label: &str, ok: bool, value: impl std::fmt::Display) {
        println!("{label}: {value}");
        if !ok {
            self.failures.push(label.to_string());
        }
    }

    fn warn(&mut self, label: &str, ok: bool, value: impl std::fmt::Display) {
        println!("{label}: {value}");
        if !ok {
            self.warnings.push(label.to_string());
        }
    }
}

fn print_checks(cfg: &Config) -> Checks {
    let mut checks = Checks::default();
    println!("yappr version: {}", crate::version());
    println!("config: {}", Config::user_config_path().display());
    println!("mode tier: {}", cfg.mode.tier);
    if cfg.mode.is_poor() {
        println!("asr archive: {}", cfg.asr.archive);
        println!("asr model_dir: {}", cfg.asr.model_dir);
        // Same marker ensure_asr_model uses to decide it has already extracted.
        let asr_ready = crate::expand_tilde(&cfg.asr.model_dir)
            .join("encoder.int8.onnx")
            .exists();
        checks.probe(
            "asr model present",
            asr_ready,
            if asr_ready {
                "yes"
            } else {
                "NO (encoder.int8.onnx missing; will download on next launch)"
            },
        );
    }
    print_audio_checks(cfg, &mut checks);
    println!("server endpoint: {}", cfg.server.endpoint);
    println!("server port: {}", cfg.server.port);
    println!("server manage: {}", cfg.server.manage);
    println!("server binary: {}", cfg.server.binary);
    println!("server timeout: {}s", cfg.server.timeout_secs);
    println!("model active: {}", cfg.model.active);
    println!("model repo: {}", cfg.model.repo);
    println!("model weights file: {}", cfg.model.weights);
    println!("model mmproj file: {}", cfg.model.mmproj);
    println!("model ctx_size: {}", cfg.model.ctx_size);
    println!("model ngl: {}", cfg.model.ngl);
    println!("vad enabled: {}", cfg.vad.enabled);
    println!("vad threshold: {}", cfg.vad.threshold);
    println!(
        "vad min_speech_duration_ms: {}",
        cfg.vad.min_speech_duration_ms
    );
    println!(
        "vad min_silence_duration_ms: {}",
        cfg.vad.min_silence_duration_ms
    );
    println!("vad speech_pad_ms: {}", cfg.vad.speech_pad_ms);
    println!("chat context_seconds: {}", cfg.chat.context_seconds);
    println!("speech backend: {}", cfg.speech.backend);
    println!(
        "speech voice: {}",
        cfg.speech.voice.as_deref().unwrap_or("system default")
    );
    println!("speech rate: {}", cfg.speech.rate);
    println!("supertonic model_dir: {}", cfg.speech.supertonic.model_dir);
    println!("supertonic sid: {}", cfg.speech.supertonic.sid);
    println!("supertonic speed: {}", cfg.speech.supertonic.speed);
    println!("supertonic lang: {}", cfg.speech.supertonic.lang);
    println!("supertonic steps: {}", cfg.speech.supertonic.steps);
    println!("supertonic threads: {}", cfg.speech.supertonic.threads);
    println!("kokoro model_dir: {}", cfg.speech.kokoro.model_dir);
    println!("kokoro sid: {}", cfg.speech.kokoro.sid);
    println!("kokoro speed: {}", cfg.speech.kokoro.speed);
    println!("kokoro lang: {}", cfg.speech.kokoro.lang);
    println!("kokoro threads: {}", cfg.speech.kokoro.threads);
    print_logging_checks(cfg, &mut checks);
    println!("search enabled: {}", cfg.search.enabled);
    println!("search endpoint: {}", cfg.search.endpoint);
    println!("search max_results: {}", cfg.search.max_results);
    println!("search timeout: {}s", cfg.search.timeout_secs);
    if cfg.search.enabled {
        // Reachability, not just configuration. A dead SearXNG silently falls
        // back to DDG, so this is a warning rather than a failure.
        let reachable = crate::search::available(&cfg.search);
        checks.warn(
            "search reachable",
            reachable,
            if reachable {
                "yes"
            } else {
                "no (will fall back to DuckDuckGo)"
            },
        );
    }
    let permissions = perms::report();
    println!("permissions: {}", permissions.log_summary());
    println!("input monitoring: {}", permissions.input_monitoring);
    println!("accessibility: {}", permissions.accessibility);
    println!("microphone: {}", permissions.microphone);
    println!("microphone device: {}", permissions.microphone_device);
    let missing = permissions.missing();
    checks.probe(
        "permissions granted",
        missing.is_empty(),
        if missing.is_empty() {
            "yes".to_string()
        } else {
            format!("NO (missing: {})", missing.join(", "))
        },
    );
    print_backend_checks(cfg, &mut checks);
    print_summary(&checks);
    checks
}

/// The audio section was absent from `--check` entirely, which is why an unplugged
/// configured microphone could fail every recording for months while this command
/// reported everything fine.
fn print_audio_checks(cfg: &Config, checks: &mut Checks) {
    println!("audio samplerate: {}", cfg.audio.samplerate);
    println!("audio max_seconds: {}", cfg.audio.max_seconds);
    println!("audio tail_seconds: {}", cfg.audio.tail_seconds);
    let devices = audio::input_devices();
    println!(
        "audio devices: {}",
        if devices.is_empty() {
            "none found".to_string()
        } else {
            devices.join(", ")
        }
    );
    match cfg.audio.device.as_deref() {
        None => println!("audio device: system default"),
        Some(name) => {
            let present = devices.iter().any(|d| d == name);
            checks.warn(
                "audio device",
                present,
                if present {
                    format!("{name} (present)")
                } else {
                    format!("{name} NOT CONNECTED; recording falls back to system default")
                },
            );
        }
    }
}

fn print_logging_checks(cfg: &Config, checks: &mut Checks) {
    println!("logging enabled: {}", cfg.logging.enabled);
    println!("logging debug: {}", cfg.logging.debug);
    println!("logging path: {}", cfg.logging.path);
    println!("llama-server log: /tmp/yappr-llama-server.log");
    if !cfg.logging.enabled {
        return;
    }
    // Printing the path proved nothing: an unwritable path makes every log write
    // a silent no-op, with no signal anywhere.
    let path = crate::expand_tilde(&cfg.logging.path);
    let writable = path
        .parent()
        .map(|parent| {
            std::fs::create_dir_all(parent).is_ok()
                && std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .is_ok()
        })
        .unwrap_or(false);
    checks.probe(
        "log writable",
        writable,
        if writable { "yes" } else { "NO" },
    );
    if let Ok(meta) = std::fs::metadata(&path) {
        println!("log size: {} KB", meta.len() / 1024);
    }
}

fn print_backend_checks(cfg: &Config, checks: &mut Checks) {
    let binary = server::resolve_binary(&cfg.server.binary);
    checks.probe(
        "llama-server binary",
        binary.is_some(),
        match &binary {
            Some(path) => path.display().to_string(),
            None => "NOT FOUND (engine install needed)".to_string(),
        },
    );
    let paths = server::model_paths(cfg);
    // Only the rich tier loads these; in poor mode their absence is expected.
    let want_model = !cfg.mode.is_poor();
    for (label, path) in [("weights", &paths.weights), ("mmproj", &paths.mmproj)] {
        match path {
            Some(path) => println!("{label}: {}", path.display()),
            None if want_model => checks.probe(label, false, "MISSING (will download)"),
            None => println!("{label}: not needed in Dictation Only mode"),
        }
    }
    if !want_model {
        return;
    }
    // The probe that was missing: is anything actually answering on the port?
    let up = server::healthy(cfg.server.port);
    checks.warn(
        "backend health",
        up,
        if up {
            "answering".to_string()
        } else {
            format!("not answering on port {} (not running?)", cfg.server.port)
        },
    );
    if let (true, Some(weights)) = (up, paths.weights.as_ref()) {
        // A server on our port serving a different model is a hard failure: start
        // would refuse rather than adopt it.
        let ours = server::serves_model(cfg.server.port, weights);
        checks.probe(
            "backend model",
            ours,
            if ours {
                "matches configured weights"
            } else {
                "DIFFERENT model on this port; stop it before launching Yappr"
            },
        );
    }
}

fn print_summary(checks: &Checks) {
    if checks.failures.is_empty() && checks.warnings.is_empty() {
        println!("\nsummary: ok");
        return;
    }
    if !checks.warnings.is_empty() {
        println!("\nsummary: {} warning(s)", checks.warnings.len());
        for warning in &checks.warnings {
            println!("  warn: {warning}");
        }
    }
    if !checks.failures.is_empty() {
        println!("summary: {} failure(s)", checks.failures.len());
        for failure in &checks.failures {
            println!("  fail: {failure}");
        }
    }
}

/// Route panics into the log.
///
/// The default hook writes to stderr, which for a Finder-launched `.app` goes
/// nowhere a user can see. A panic in the audio worker or a provisioning thread
/// therefore killed only that thread and left the app running with a Ready menu
/// bar, silently unable to do the thing that panicked.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        // Panic payloads are usually &str or String; anything else is opaque.
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_string());
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed").to_string();
        crate::logger::log_line(format!(
            "PANIC in thread '{name}' at {location}: {message}"
        ));
        default(info);
    }));
}

/// Shut the backend down on SIGTERM instead of dying with it still running.
///
/// The handler itself only sets a flag: it runs on an interrupted thread where
/// almost nothing is safe to call, so logging or taking the runtime's mutexes
/// there could deadlock. A watcher thread notices the flag and does the work.
fn install_term_handler() {
    static TERMINATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    extern "C" fn on_term(_signal: i32) {
        TERMINATED.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    unsafe {
        libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t);
    }

    std::thread::spawn(|| loop {
        if TERMINATED.load(std::sync::atomic::Ordering::SeqCst) {
            crate::logger::log_line("SIGTERM received; stopping backend before exit");
            if let Some(runtime) = crate::runtime::runtime() {
                runtime.shutdown();
            }
            std::process::exit(0);
        }
        // Fast enough to beat the 2s SIGTERM-to-SIGKILL window in instance.rs.
        std::thread::sleep(std::time::Duration::from_millis(100));
    });
}

fn arg_value(args: &[String], key: &str) -> Option<PathBuf> {
    args.windows(2)
        .find(|pair| pair[0] == key)
        .map(|pair| PathBuf::from(&pair[1]))
}

fn string_arg<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == key)
        .map(|pair| pair[1].as_str())
}

fn print_usage() {
    eprintln!(
        "Yappr Rust shell\n\n  yappr [--app]\n  yappr --check\n  yappr --record-test [--seconds 3]\n  yappr --serve\n  yappr --speak text\n  yappr --ask question\n  yappr --wav audio.wav [--paste]\n  yappr --ask-wav audio.wav\n\nDefault app mode: hold Right Option to dictate; hold Cmd+Right Option to chat."
    );
}

fn park_until_ctrl_c() {
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(3600));
    }
}

#[cfg(test)]
mod tests {
    use super::Checks;

    #[test]
    fn clean_run_has_no_failures_or_warnings() {
        let checks = Checks::default();
        assert!(checks.failures.is_empty());
        assert!(checks.warnings.is_empty());
    }

    #[test]
    fn only_failures_drive_the_exit_code() {
        // A warning is for a degraded-but-working state (unplugged mic falls back
        // to the default, dead SearXNG falls back to DDG), so it must not turn
        // --check red. Only a failure does.
        let mut checks = Checks::default();
        checks.warn("audio device", false, "not connected");
        assert!(checks.failures.is_empty(), "a warning is not a failure");
        assert_eq!(checks.warnings, vec!["audio device"]);

        checks.probe("permissions granted", false, "no");
        assert_eq!(checks.failures, vec!["permissions granted"]);
    }

    #[test]
    fn passing_probes_record_nothing() {
        let mut checks = Checks::default();
        checks.probe("log writable", true, "yes");
        checks.warn("backend health", true, "answering");
        assert!(checks.failures.is_empty());
        assert!(checks.warnings.is_empty());
    }
}
