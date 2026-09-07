//! In-process streaming ASR for GPU-poor mode.
//!
//! Loads an NVIDIA Nemotron streaming transducer (encoder/decoder/joiner int8
//! ONNX) via sherpa-onnx and transcribes captured audio without a llama-server.
//! The recognizer is built once and reused; the spoken language is selected
//! per-stream, so a single recognizer serves every language the model supports.

use crate::config::AsrConfig;
use crate::expand_tilde;
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig};
use std::sync::{Mutex, OnceLock};

static RECOGNIZER: OnceLock<Mutex<OnlineRecognizer>> = OnceLock::new();

/// Transcribe mono PCM samples. `source_language` is a menu language name
/// (e.g. "English", "Hindi") or "auto"; anything else falls back to auto.
pub fn transcribe(
    pcm: &[f32],
    sample_rate: u32,
    cfg: &AsrConfig,
    source_language: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    if pcm.is_empty() {
        return Ok(String::new());
    }
    let recognizer = recognizer(cfg)?.lock().map_err(|_| "asr lock poisoned")?;
    let stream = recognizer.create_stream();
    if let Some(code) = language_code(source_language) {
        stream.set_option("language", code);
    }
    stream.accept_waveform(sample_rate as i32, pcm);
    stream.input_finished();
    while recognizer.is_ready(&stream) {
        recognizer.decode(&stream);
    }
    Ok(recognizer
        .get_result(&stream)
        .map(|result| result.text)
        .unwrap_or_default()
        .trim()
        .to_string())
}

fn recognizer(
    cfg: &AsrConfig,
) -> Result<&'static Mutex<OnlineRecognizer>, Box<dyn std::error::Error>> {
    if RECOGNIZER.get().is_none() {
        let dir = expand_tilde(&cfg.model_dir);
        let path = |name: &str| dir.join(name).display().to_string();
        let mut config = OnlineRecognizerConfig::default();
        config.model_config.transducer.encoder = Some(path("encoder.int8.onnx"));
        config.model_config.transducer.decoder = Some(path("decoder.int8.onnx"));
        config.model_config.transducer.joiner = Some(path("joiner.int8.onnx"));
        config.model_config.tokens = Some(path("tokens.txt"));
        config.model_config.num_threads = 2;
        config.decoding_method = Some("greedy_search".to_string());
        let recognizer = OnlineRecognizer::create(&config)
            .ok_or("failed to create ASR recognizer; check model files")?;
        let _ = RECOGNIZER.set(Mutex::new(recognizer));
    }
    RECOGNIZER
        .get()
        .ok_or_else(|| "asr recognizer not initialized".into())
}

/// Map a menu language name to the short code sherpa-onnx expects (en, ja, hi,
/// …). Returns `None` for "auto" or unknown names, leaving the model's built-in
/// language detection in charge.
fn language_code(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "english" => Some("en"),
        "spanish" => Some("es"),
        "french" => Some("fr"),
        "german" => Some("de"),
        "hindi" => Some("hi"),
        "japanese" => Some("ja"),
        "chinese" => Some("zh"),
        "portuguese" => Some("pt"),
        "italian" => Some("it"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::language_code;

    #[test]
    fn maps_known_languages_to_short_codes() {
        assert_eq!(language_code("English"), Some("en"));
        assert_eq!(language_code("Hindi"), Some("hi"));
        assert_eq!(language_code("japanese"), Some("ja"));
    }

    #[test]
    fn auto_and_unknown_have_no_code() {
        assert_eq!(language_code("auto"), None);
        assert_eq!(language_code("Klingon"), None);
    }
}
