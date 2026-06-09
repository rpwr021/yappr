use crate::config::{KokoroConfig, SpeechConfig, SupertonicConfig};
use crate::expand_tilde;
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig, OfflineTtsSupertonicModelConfig,
};
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static KOKORO: OnceLock<Mutex<KokoroEngine>> = OnceLock::new();
static SUPERTONIC: OnceLock<Mutex<SupertonicEngine>> = OnceLock::new();

pub fn speak(text: &str, cfg: &SpeechConfig) -> Result<(), Box<dyn std::error::Error>> {
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    let result = match cfg.backend.as_str() {
        "say" => return say(text, cfg.voice.as_deref(), cfg.rate),
        "kokoro" => speak_kokoro(text, &cfg.kokoro),
        "supertonic" => speak_supertonic(text, &cfg.supertonic),
        other => Err(format!("unknown speech backend: {other}").into()),
    };
    // Fall back to the always-available macOS `say` if a model backend fails
    // (e.g. its model files were never downloaded), so the user still hears the
    // answer instead of silence.
    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            crate::logger::log_line(format!(
                "speech backend '{}' failed ({err}); falling back to say",
                cfg.backend
            ));
            say(text, cfg.voice.as_deref(), cfg.rate)
        }
    }
}

pub fn stop() {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
    let _ = Command::new("/usr/bin/killall").arg("say").status();
    let _ = Command::new("/usr/bin/killall").arg("afplay").status();
}

fn say(text: &str, voice: Option<&str>, rate: i32) -> Result<(), Box<dyn std::error::Error>> {
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

fn speak_kokoro(text: &str, cfg: &KokoroConfig) -> Result<(), Box<dyn std::error::Error>> {
    let engine = KOKORO.get_or_init(|| Mutex::new(KokoroEngine::new()));
    let mut engine = engine.lock().map_err(|_| "kokoro lock poisoned")?;
    engine.speak(text, cfg)
}

fn speak_supertonic(text: &str, cfg: &SupertonicConfig) -> Result<(), Box<dyn std::error::Error>> {
    let engine = SUPERTONIC.get_or_init(|| Mutex::new(SupertonicEngine::new()));
    let mut engine = engine.lock().map_err(|_| "supertonic lock poisoned")?;
    engine.speak(text, cfg)
}

struct KokoroEngine {
    model_dir: String,
    tts: Option<OfflineTts>,
}

impl KokoroEngine {
    fn new() -> Self {
        Self {
            model_dir: String::new(),
            tts: None,
        }
    }

    fn speak(&mut self, text: &str, cfg: &KokoroConfig) -> Result<(), Box<dyn std::error::Error>> {
        self.ensure_loaded(cfg)?;
        let tts = self.tts.as_mut().ok_or("kokoro failed to initialize")?;
        let gen_config = GenerationConfig {
            sid: cfg.sid,
            speed: cfg.speed,
            ..Default::default()
        };
        let audio = tts.generate_with_config(text, &gen_config, Some(keep_generating));
        let Some(audio) = audio else {
            return Err("kokoro generation failed".into());
        };
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            return Ok(());
        }
        let path = speech_path();
        let filename = path.display().to_string();
        if !audio.save(&filename) {
            return Err("failed to save kokoro audio".into());
        }
        play_wav(&path)
    }

    fn ensure_loaded(&mut self, cfg: &KokoroConfig) -> Result<(), Box<dyn std::error::Error>> {
        if self.tts.is_some() && self.model_dir == cfg.model_dir {
            return Ok(());
        }
        self.tts =
            Some(OfflineTts::create(&kokoro_tts_config(cfg)).ok_or("kokoro failed to initialize")?);
        self.model_dir = cfg.model_dir.clone();
        Ok(())
    }
}

struct SupertonicEngine {
    model_dir: String,
    tts: Option<OfflineTts>,
}

impl SupertonicEngine {
    fn new() -> Self {
        Self {
            model_dir: String::new(),
            tts: None,
        }
    }

    fn speak(
        &mut self,
        text: &str,
        cfg: &SupertonicConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.ensure_loaded(cfg)?;
        let tts = self.tts.as_mut().ok_or("supertonic failed to initialize")?;
        let gen_config = GenerationConfig {
            sid: cfg.sid,
            num_steps: cfg.steps,
            speed: cfg.speed,
            extra: Some(json_extra("lang", &cfg.lang)),
            ..Default::default()
        };
        let audio = tts.generate_with_config(text, &gen_config, Some(keep_generating));
        let Some(audio) = audio else {
            return Err("supertonic generation failed".into());
        };
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            return Ok(());
        }
        let path = speech_path();
        let filename = path.display().to_string();
        if !audio.save(&filename) {
            return Err("failed to save supertonic audio".into());
        }
        play_wav(&path)
    }

    fn ensure_loaded(&mut self, cfg: &SupertonicConfig) -> Result<(), Box<dyn std::error::Error>> {
        if self.tts.is_some() && self.model_dir == cfg.model_dir {
            return Ok(());
        }
        self.tts = Some(
            OfflineTts::create(&supertonic_tts_config(cfg))
                .ok_or("supertonic failed to initialize")?,
        );
        self.model_dir = cfg.model_dir.clone();
        Ok(())
    }
}

fn json_extra(key: &str, value: &str) -> HashMap<String, serde_json::Value> {
    HashMap::from([(
        key.to_string(),
        serde_json::Value::String(value.to_string()),
    )])
}

fn keep_generating(_: &[f32], _: f32) -> bool {
    !STOP_REQUESTED.load(Ordering::SeqCst)
}

fn kokoro_tts_config(cfg: &KokoroConfig) -> OfflineTtsConfig {
    let dir = expand_tilde(&cfg.model_dir);
    let path = |name: &str| dir.join(name).display().to_string();
    OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            kokoro: OfflineTtsKokoroModelConfig {
                model: Some(path("model.onnx")),
                voices: Some(path("voices.bin")),
                tokens: Some(path("tokens.txt")),
                data_dir: Some(path("espeak-ng-data")),
                dict_dir: Some(path("dict")),
                lexicon: Some(kokoro_lexicon(&dir)),
                lang: Some(cfg.lang.clone()),
                length_scale: 1.0,
            },
            num_threads: cfg.threads,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn supertonic_tts_config(cfg: &SupertonicConfig) -> OfflineTtsConfig {
    let dir = expand_tilde(&cfg.model_dir);
    let path = |name: &str| dir.join(name).display().to_string();
    OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            supertonic: OfflineTtsSupertonicModelConfig {
                duration_predictor: Some(path("duration_predictor.int8.onnx")),
                text_encoder: Some(path("text_encoder.int8.onnx")),
                vector_estimator: Some(path("vector_estimator.int8.onnx")),
                vocoder: Some(path("vocoder.int8.onnx")),
                tts_json: Some(path("tts.json")),
                unicode_indexer: Some(path("unicode_indexer.bin")),
                voice_style: Some(path("voice.bin")),
            },
            num_threads: cfg.threads,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn kokoro_lexicon(dir: &std::path::Path) -> String {
    ["lexicon-us-en.txt", "lexicon-zh.txt"]
        .into_iter()
        .map(|name| dir.join(name))
        .filter(|path| path.exists())
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn speech_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("yappr-speech-{}.wav", std::process::id()))
}

fn play_wav(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("/usr/bin/afplay").arg(path).status()?;
    let _ = std::fs::remove_file(path);
    if !status.success() {
        return Err("audio playback failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::kokoro_lexicon;

    #[test]
    fn missing_kokoro_lexicons_are_omitted() {
        let dir = std::path::Path::new("/path/that/does/not/exist");
        assert_eq!(kokoro_lexicon(dir), "");
    }
}
