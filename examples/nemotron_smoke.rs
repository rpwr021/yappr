//! Smoke test for the "GPU poor" STT path: NVIDIA Nemotron streaming (int8)
//! via the in-process sherpa-onnx OnlineRecognizer, no llama-server involved.
//!
//! Run:
//!   cargo run --example nemotron_smoke -- <model_dir> [wav ...]
//!
//! With no wav args it transcribes the model's bundled test_wavs/*.wav.

use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, Wave};
use std::path::{Path, PathBuf};

fn main() {
    // Optional leading "--language <code>" sets a per-stream language (e.g. en, ja, hi).
    let mut all: Vec<String> = std::env::args().skip(1).collect();
    let mut language: Option<String> = None;
    if let Some(i) = all.iter().position(|a| a == "--language") {
        language = all.get(i + 1).cloned();
        all.drain(i..=i + 1);
    }
    let mut args = all.into_iter();
    let model_dir = args.next().unwrap_or_else(|| {
        eprintln!("usage: nemotron_smoke [--language <code>] <model_dir> [wav ...]");
        std::process::exit(2);
    });
    let model_dir = PathBuf::from(model_dir);

    let wavs: Vec<PathBuf> = {
        let explicit: Vec<PathBuf> = args.map(PathBuf::from).collect();
        if explicit.is_empty() {
            default_wavs(&model_dir)
        } else {
            explicit
        }
    };
    if wavs.is_empty() {
        eprintln!("no wav files given and none found under {model_dir:?}/test_wavs");
        std::process::exit(2);
    }

    let path = |name: &str| model_dir.join(name).display().to_string();
    let mut config = OnlineRecognizerConfig::default();
    config.model_config.transducer.encoder = Some(path("encoder.int8.onnx"));
    config.model_config.transducer.decoder = Some(path("decoder.int8.onnx"));
    config.model_config.transducer.joiner = Some(path("joiner.int8.onnx"));
    config.model_config.tokens = Some(path("tokens.txt"));
    config.model_config.num_threads = 2;
    config.decoding_method = Some("greedy_search".into());

    let start = std::time::Instant::now();
    let recognizer = OnlineRecognizer::create(&config).unwrap_or_else(|| {
        eprintln!("failed to create recognizer; check model paths under {model_dir:?}");
        std::process::exit(1);
    });
    eprintln!("recognizer loaded in {:?}", start.elapsed());

    for wav in &wavs {
        match Wave::read(&wav.display().to_string()) {
            Some(wave) => {
                let t = std::time::Instant::now();
                let stream = recognizer.create_stream();
                if let Some(lang) = &language {
                    stream.set_option("language", lang);
                }
                stream.accept_waveform(wave.sample_rate(), wave.samples());
                stream.input_finished();
                while recognizer.is_ready(&stream) {
                    recognizer.decode(&stream);
                }
                let text = recognizer
                    .get_result(&stream)
                    .map(|r| r.text)
                    .unwrap_or_default();
                let audio_secs = wave.samples().len() as f32 / wave.sample_rate() as f32;
                let elapsed = t.elapsed().as_secs_f32();
                println!("[{}] ({audio_secs:.1}s audio, {elapsed:.2}s decode, RTF {:.3})\n  {text}",
                    wav.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                    elapsed / audio_secs.max(0.001));
            }
            None => eprintln!("could not read wav: {wav:?}"),
        }
    }
}

fn default_wavs(model_dir: &Path) -> Vec<PathBuf> {
    let dir = model_dir.join("test_wavs");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut wavs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    wavs.sort();
    wavs
}
