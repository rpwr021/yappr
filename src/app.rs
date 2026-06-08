use crate::audio;
use crate::chat::{ChatClient, ChatMode};
use crate::config::Config;
use crate::hotkey;
use crate::inject;
use crate::instance::InstanceLock;
use crate::perms;
use crate::runtime::Runtime;
use crate::server::{self, ManagedServer};
use std::fs;
use std::path::PathBuf;

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::load()?;

    if args.iter().any(|arg| arg == "--check") {
        print_checks(&cfg);
        return Ok(());
    }

    if args.iter().any(|arg| arg == "--record-test") {
        let seconds = string_arg(&args, "--seconds")
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(3.0);
        let captured = audio::record_for(seconds, cfg.audio.samplerate)?;
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

    if let Some(path) = arg_value(&args, "--wav") {
        let _server = start_backend(&cfg)?;
        let wav = fs::read(path)?;
        let client = ChatClient::new(cfg.clone())?;
        let text = client.transcribe_wav(&wav)?;
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
        inject::say(&answer, cfg.chat.voice.as_deref(), cfg.chat.rate)?;
        return Ok(());
    }

    if args.is_empty() || args.iter().any(|arg| arg == "--app") {
        let instance_lock = InstanceLock::acquire()?;
        let server = start_backend(&cfg)?;
        let client = ChatClient::new(cfg.clone())?;
        let runtime = Runtime::new(cfg, client, server);
        runtime.hold_instance_lock(instance_lock);
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

fn print_checks(cfg: &Config) {
    println!("config: {}", Config::user_config_path().display());
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
    println!(
        "chat voice: {}",
        cfg.chat.voice.as_deref().unwrap_or("system default")
    );
    println!("chat rate: {}", cfg.chat.rate);
    println!("chat context_seconds: {}", cfg.chat.context_seconds);
    println!("search enabled: {}", cfg.search.enabled);
    println!("search endpoint: {}", cfg.search.endpoint);
    println!("search max_results: {}", cfg.search.max_results);
    println!("search timeout: {}s", cfg.search.timeout_secs);
    println!("input monitoring: {}", perms::input_monitoring_status());
    println!("accessibility: {}", perms::accessibility_status());
    println!("microphone: {}", perms::microphone_status());
    println!(
        "llama-server: {:?}",
        server::resolve_binary(&cfg.server.binary)
    );
    let paths = server::model_paths(cfg);
    println!("weights: {:?}", paths.weights);
    println!("mmproj: {:?}", paths.mmproj);
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
        "Yappr Rust shell\n\n  yappr [--app]\n  yappr --check\n  yappr --record-test [--seconds 3]\n  yappr --serve\n  yappr --wav audio.wav [--paste]\n  yappr --ask-wav audio.wav\n\nDefault app mode: hold Right Option to dictate; hold Cmd+Right Option to chat."
    );
}

fn park_until_ctrl_c() {
    loop {
        std::thread::park_timeout(std::time::Duration::from_secs(3600));
    }
}
