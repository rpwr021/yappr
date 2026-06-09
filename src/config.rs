use crate::expand_tilde;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub server: ServerConfig,
    pub model: ModelConfig,
    pub audio: AudioConfig,
    pub vad: VadConfig,
    pub language: LanguageConfig,
    pub chat: ChatConfig,
    pub speech: SpeechConfig,
    pub logging: LoggingConfig,
    pub search: SearchConfig,
}

#[derive(Clone, Debug)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub endpoint: String,
    pub port: u16,
    pub manage: bool,
    pub binary: String,
    pub timeout_secs: u64,
}

#[derive(Clone, Debug)]
pub struct ModelConfig {
    pub repo: String,
    pub weights: String,
    pub mmproj: String,
    pub ctx_size: String,
    pub ngl: String,
    pub active: String,
    pub choices: Vec<ModelChoice>,
}

#[derive(Clone, Debug)]
pub struct LanguageConfig {
    pub source: String,
    pub target: String,
    pub options: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AudioConfig {
    pub device: Option<String>,
    pub samplerate: u32,
    pub max_seconds: f32,
    pub tail_seconds: f32,
}

#[derive(Clone, Debug)]
pub struct VadConfig {
    pub enabled: bool,
    pub threshold: f32,
    pub min_speech_duration_ms: u32,
    pub min_silence_duration_ms: u32,
    pub speech_pad_ms: u32,
}

#[derive(Clone, Debug)]
pub struct ChatConfig {
    pub context_seconds: i64,
}

#[derive(Clone, Debug)]
pub struct SpeechConfig {
    pub backend: String,
    pub kokoro: KokoroConfig,
    pub supertonic: SupertonicConfig,
    pub voice: Option<String>,
    pub rate: i32,
}

#[derive(Clone, Debug)]
pub struct KokoroConfig {
    pub model_dir: String,
    pub sid: i32,
    pub speed: f32,
    pub lang: String,
    pub threads: i32,
}

#[derive(Clone, Debug)]
pub struct SupertonicConfig {
    pub model_dir: String,
    pub sid: i32,
    pub speed: f32,
    pub lang: String,
    pub steps: i32,
    pub threads: i32,
}

#[derive(Clone, Debug)]
pub struct LoggingConfig {
    pub enabled: bool,
    pub debug: bool,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct SearchConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub max_results: usize,
    pub timeout_secs: u64,
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let mut ini = Ini::default();
        ini.merge(include_str!("../resources/config.default.ini"));
        let user_path = Self::user_config_path();
        if !user_path.exists() {
            if let Some(parent) = user_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&user_path, include_str!("../resources/config.default.ini"))?;
        }
        let user_contents = fs::read_to_string(&user_path).ok();
        if let Some(contents) = &user_contents {
            ini.merge(contents);
        }

        let cfg = Self::from_ini(&mut ini);
        if user_contents.is_some() {
            let updated = ini.to_string();
            if user_contents.as_deref() != Some(updated.as_str()) {
                fs::write(&user_path, updated)?;
            }
        }
        Ok(cfg)
    }

    fn from_ini(ini: &mut Ini) -> Self {
        ini.migrate_old_model_defaults();
        ini.migrate_old_speech_defaults();
        ini.remove_obsolete_keys();
        let active_model = ini.get("models", "active", "e2b-qat");
        let model_section = format!("model:{active_model}");

        Self {
            server: ServerConfig {
                endpoint: ini.get(
                    "server",
                    "endpoint",
                    "http://127.0.0.1:8089/v1/chat/completions",
                ),
                port: ini.get("server", "port", "8089").parse().unwrap_or(8089),
                manage: ini.get("server", "manage", "true").parse().unwrap_or(true),
                binary: ini.get("server", "binary", "auto"),
                timeout_secs: ini.get("server", "timeout", "60").parse().unwrap_or(60),
            },
            model: ModelConfig {
                repo: ini.model_value(&model_section, "repo", "google/gemma-4-E2B-it-qat-q4_0-gguf"),
                weights: ini.model_value(&model_section, "weights", "gemma-4-E2B_q4_0-it.gguf"),
                mmproj: ini.model_value(&model_section, "mmproj", "gemma-4-E2B-it-mmproj.gguf"),
                ctx_size: ini.model_value(&model_section, "ctx_size", "8192"),
                ngl: ini.model_value(&model_section, "ngl", "99"),
                active: active_model,
                choices: ini.model_choices(),
            },
            audio: AudioConfig {
                device: non_empty(ini.get("audio", "device", "")),
                samplerate: ini
                    .get("audio", "samplerate", "16000")
                    .parse()
                    .unwrap_or(16000),
                max_seconds: ini
                    .get("audio", "max_seconds", "28")
                    .parse()
                    .unwrap_or(28.0),
                tail_seconds: ini
                    .get("audio", "tail_seconds", "0.4")
                    .parse()
                    .unwrap_or(0.4),
            },
            vad: VadConfig {
                enabled: ini.get("vad", "enabled", "true").parse().unwrap_or(true),
                threshold: ini.get("vad", "threshold", "0.5").parse().unwrap_or(0.5),
                min_speech_duration_ms: ini
                    .get("vad", "min_speech_duration_ms", "250")
                    .parse()
                    .unwrap_or(250),
                min_silence_duration_ms: ini
                    .get("vad", "min_silence_duration_ms", "100")
                    .parse()
                    .unwrap_or(100),
                speech_pad_ms: ini
                    .get("vad", "speech_pad_ms", "30")
                    .parse()
                    .unwrap_or(30),
            },
            language: LanguageConfig {
                source: ini.get("language", "source", "auto"),
                target: ini.get("language", "target", "auto"),
                options: ini
                    .get(
                        "language",
                        "options",
                        "auto,English,Spanish,French,German,Hindi,Japanese,Chinese,Portuguese,Italian",
                    )
                    .split(',')
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .collect(),
            },
            chat: ChatConfig {
                context_seconds: ini
                    .get("chat", "context_seconds", "60")
                    .parse()
                    .unwrap_or(60),
            },
            speech: ini.speech_config(),
            logging: LoggingConfig {
                enabled: ini.get("logging", "enabled", "true").parse().unwrap_or(true),
                debug: ini.get("logging", "debug", "false").parse().unwrap_or(false),
                path: ini.get("logging", "path", "~/.yappr/yappr.log"),
            },
            search: SearchConfig {
                enabled: ini.get("search", "enabled", "true").parse().unwrap_or(true),
                endpoint: ini.get("search", "endpoint", "http://127.0.0.1:8888/search"),
                max_results: ini.get("search", "max_results", "5").parse().unwrap_or(5),
                timeout_secs: ini.get("search", "timeout", "15").parse().unwrap_or(15),
            },
        }
    }

    pub fn user_config_path() -> PathBuf {
        expand_tilde("~/.yappr/config.ini")
    }

    pub fn set_user_value(
        section: &str,
        key: &str,
        value: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = Self::user_config_path();
        let mut ini = Ini::default();
        if path.exists() {
            ini.merge(&fs::read_to_string(&path)?);
        } else {
            ini.merge(include_str!("../resources/config.default.ini"));
        }
        ini.0
            .insert((section.to_string(), key.to_string()), value.to_string());
        fs::write(path, ini.to_string())?;
        Ok(())
    }
}

#[derive(Default)]
struct Ini(HashMap<(String, String), String>);

impl Ini {
    fn merge(&mut self, input: &str) {
        let mut section = String::new();
        for raw in input.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_string();
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                self.0.insert(
                    (section.clone(), key.trim().to_string()),
                    value.trim().to_string(),
                );
            }
        }
    }

    fn get(&self, section: &str, key: &str, default: &str) -> String {
        self.0
            .get(&(section.to_string(), key.to_string()))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    fn model_choices(&self) -> Vec<ModelChoice> {
        let mut choices = self
            .0
            .keys()
            .filter_map(|(section, _)| section.strip_prefix("model:").map(str::to_string))
            .collect::<Vec<_>>();
        choices.sort();
        choices.dedup();
        choices
            .into_iter()
            .map(|id| {
                let label = self.get(&format!("model:{id}"), "label", &id);
                ModelChoice { id, label }
            })
            .collect()
    }

    fn model_value(&self, section: &str, key: &str, default: &str) -> String {
        let fallback = self.get("model", key, default);
        self.get(section, key, &fallback)
    }

    fn speech_config(&self) -> SpeechConfig {
        let speech_rate = self.get("speech", "rate", "190");
        SpeechConfig {
            backend: self.get("speech", "backend", "say"),
            kokoro: KokoroConfig {
                model_dir: self.get(
                    "speech",
                    "kokoro_model_dir",
                    "~/.yappr/models/kokoro-multi-lang-v1_0",
                ),
                sid: self.get("speech", "kokoro_sid", "3").parse().unwrap_or(3),
                speed: self
                    .get("speech", "kokoro_speed", "1.0")
                    .parse()
                    .unwrap_or(1.0),
                lang: self.get("speech", "kokoro_lang", "en"),
                threads: self
                    .get("speech", "kokoro_threads", "2")
                    .parse()
                    .unwrap_or(2),
            },
            supertonic: SupertonicConfig {
                model_dir: self.get(
                    "speech",
                    "supertonic_model_dir",
                    "~/.yappr/models/sherpa-onnx-supertonic-3-tts-int8-2026-05-11",
                ),
                sid: self
                    .get("speech", "supertonic_sid", "0")
                    .parse()
                    .unwrap_or(0),
                speed: self
                    .get("speech", "supertonic_speed", "1.0")
                    .parse()
                    .unwrap_or(1.0),
                lang: self.get("speech", "supertonic_lang", "en"),
                steps: self
                    .get("speech", "supertonic_steps", "8")
                    .parse()
                    .unwrap_or(8),
                threads: self
                    .get("speech", "supertonic_threads", "2")
                    .parse()
                    .unwrap_or(2),
            },
            voice: non_empty(self.get("speech", "voice", "")),
            rate: speech_rate.parse().unwrap_or(190),
        }
    }

    fn migrate_old_model_defaults(&mut self) {
        self.0.retain(|(section, _), _| section != "model:e4b-q4km");
        if self.get("models", "active", "") == "e4b-q4km" {
            self.0.insert(
                ("models".to_string(), "active".to_string()),
                "e2b-qat".to_string(),
            );
        }

        let old_repo = "google/gemma-4-E4B-it-qat-q4_0-gguf";
        let old_weights = "gemma-4-E4B_q4_0-it.gguf";
        if self.get("model", "repo", "") == old_repo
            && self.get("model", "weights", "") == old_weights
            && self.get("models", "active", "") == "e4b-qat"
        {
            self.0.insert(
                ("models".to_string(), "active".to_string()),
                "e2b-qat".to_string(),
            );
            self.0.insert(
                ("model".to_string(), "repo".to_string()),
                "google/gemma-4-E2B-it-qat-q4_0-gguf".to_string(),
            );
            self.0.insert(
                ("model".to_string(), "weights".to_string()),
                "gemma-4-E2B_q4_0-it.gguf".to_string(),
            );
            self.0.insert(
                ("model".to_string(), "mmproj".to_string()),
                "gemma-4-E2B-it-mmproj.gguf".to_string(),
            );
        }
    }

    fn migrate_old_speech_defaults(&mut self) {
        let chat_voice = non_empty(self.get("chat", "voice", ""));
        let speech_voice = non_empty(self.get("speech", "voice", ""));
        if speech_voice.is_none() {
            if let Some(voice) = chat_voice {
                self.0
                    .insert(("speech".to_string(), "voice".to_string()), voice);
            }
        }

        let chat_rate = self.get("chat", "rate", "");
        if !chat_rate.is_empty() && self.get("speech", "rate", "190") == "190" {
            self.0
                .insert(("speech".to_string(), "rate".to_string()), chat_rate);
        }

        self.0.remove(&("chat".to_string(), "voice".to_string()));
        self.0.remove(&("chat".to_string(), "rate".to_string()));
    }

    fn remove_obsolete_keys(&mut self) {
        self.0.retain(|(section, key), _| {
            !(section == "hotkey"
                || section == "output"
                || section.starts_with("model:") && key == "name"
                || section == "audio" && matches!(key.as_str(), "beep_start" | "beep_stop"))
        });
    }
}

impl std::fmt::Display for Ini {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut sections = self
            .0
            .keys()
            .map(|(section, _)| section.clone())
            .collect::<Vec<_>>();
        sections.sort();
        sections.dedup();
        for section in sections {
            if !section.is_empty() {
                writeln!(f, "[{section}]")?;
            }
            let mut keys = self
                .0
                .iter()
                .filter(|((s, _), _)| *s == section)
                .map(|((_, key), value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>();
            keys.sort_by(|a, b| a.0.cmp(&b.0));
            for (key, value) in keys {
                writeln!(f, "{key} = {value}")?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Ini};

    fn config_from(input: &str) -> Config {
        let mut ini = Ini::default();
        ini.merge(include_str!("../resources/config.default.ini"));
        ini.merge(input);
        Config::from_ini(&mut ini)
    }

    #[test]
    fn default_speech_backend_is_say() {
        // Shipped default must be the always-available macOS backend so a fresh
        // install speaks without any model download. See speech::speak fallback.
        let cfg = config_from("");
        assert_eq!(cfg.speech.backend, "say");
    }

    #[test]
    fn migrates_old_e4b_default_to_e2b() {
        let mut ini = Ini::default();
        ini.merge(
            r#"
            [model]
            repo = google/gemma-4-E4B-it-qat-q4_0-gguf
            weights = gemma-4-E4B_q4_0-it.gguf
            mmproj = gemma-4-E4B-it-mmproj.gguf

            [models]
            active = e4b-qat

            [model:e4b-q4km]
            label = Gemma 4 E4B Q4_K_M
            "#,
        );

        ini.migrate_old_model_defaults();

        assert_eq!(
            ini.get("model", "repo", ""),
            "google/gemma-4-E2B-it-qat-q4_0-gguf"
        );
        assert_eq!(ini.get("model", "weights", ""), "gemma-4-E2B_q4_0-it.gguf");
        assert_eq!(ini.get("model", "mmproj", ""), "gemma-4-E2B-it-mmproj.gguf");
        assert_eq!(ini.get("models", "active", ""), "e2b-qat");
        assert!(!ini
            .model_choices()
            .iter()
            .any(|choice| choice.id == "e4b-q4km"));
    }

    #[test]
    fn migrates_old_chat_voice_to_speech() {
        let mut ini = Ini::default();
        ini.merge(include_str!("../resources/config.default.ini"));
        ini.merge(
            r#"
            [chat]
            voice = Samantha
            rate = 220
            "#,
        );

        ini.migrate_old_speech_defaults();

        assert_eq!(ini.get("speech", "voice", ""), "Samantha");
        assert_eq!(ini.get("speech", "rate", ""), "220");
        assert_eq!(ini.get("chat", "voice", "missing"), "missing");
        assert_eq!(ini.get("chat", "rate", "missing"), "missing");
    }

    #[test]
    fn removes_obsolete_default_keys() {
        let mut ini = Ini::default();
        ini.merge(
            r#"
            [hotkey]
            key = right_option

            [audio]
            beep_start = none
            beep_stop = none

            [model:e2b-qat]
            name = gemma-4-E2B_q4_0-it

            [output]
            inject = paste
            "#,
        );

        ini.remove_obsolete_keys();

        assert_eq!(ini.get("hotkey", "key", "missing"), "missing");
        assert_eq!(ini.get("audio", "beep_start", "missing"), "missing");
        assert_eq!(ini.get("audio", "beep_stop", "missing"), "missing");
        assert_eq!(ini.get("model:e2b-qat", "name", "missing"), "missing");
        assert_eq!(ini.get("output", "inject", "missing"), "missing");
    }

    #[test]
    fn leaves_custom_model_repo_alone() {
        let mut ini = Ini::default();
        ini.merge(
            r#"
            [model]
            repo = custom/audio-model
            weights = custom.gguf

            [models]
            active = custom

            [model:custom]
            label = Custom
            "#,
        );

        ini.migrate_old_model_defaults();

        assert_eq!(ini.get("model", "repo", ""), "custom/audio-model");
        assert_eq!(ini.get("models", "active", ""), "custom");
        assert!(ini
            .model_choices()
            .iter()
            .any(|choice| choice.id == "custom"));
    }

    #[test]
    fn parses_active_model_and_llama_params() {
        let cfg = config_from(
            r#"
            [models]
            active = local-audio

            [model:local-audio]
            label = Local Audio
            repo = local/repo
            weights = local.gguf
            mmproj = local-mmproj.gguf
            ctx_size = 4096
            ngl = 42
            "#,
        );

        assert_eq!(cfg.model.active, "local-audio");
        assert_eq!(cfg.model.repo, "local/repo");
        assert_eq!(cfg.model.weights, "local.gguf");
        assert_eq!(cfg.model.mmproj, "local-mmproj.gguf");
        assert_eq!(cfg.model.ctx_size, "4096");
        assert_eq!(cfg.model.ngl, "42");
        assert!(cfg
            .model
            .choices
            .iter()
            .any(|choice| choice.id == "local-audio" && choice.label == "Local Audio"));
    }

    #[test]
    fn parses_server_overrides() {
        let cfg = config_from(
            r#"
            [server]
            endpoint = http://127.0.0.1:9999/v1/chat/completions
            port = 9999
            manage = false
            binary = /tmp/llama-server
            timeout = 17
            "#,
        );

        assert_eq!(
            cfg.server.endpoint,
            "http://127.0.0.1:9999/v1/chat/completions"
        );
        assert_eq!(cfg.server.port, 9999);
        assert!(!cfg.server.manage);
        assert_eq!(cfg.server.binary, "/tmp/llama-server");
        assert_eq!(cfg.server.timeout_secs, 17);
    }

    #[test]
    fn parses_legacy_chat_voice_and_context() {
        let cfg = config_from(
            r#"
            [chat]
            voice = Samantha
            rate = 220
            context_seconds = 90
            "#,
        );

        assert_eq!(cfg.speech.voice.as_deref(), Some("Samantha"));
        assert_eq!(cfg.speech.rate, 220);
        assert_eq!(cfg.chat.context_seconds, 90);
    }

    #[test]
    fn blank_speech_voice_uses_system_default() {
        let cfg = config_from(
            r#"
            [speech]
            voice =
            "#,
        );

        assert!(cfg.speech.voice.is_none());
    }

    #[test]
    fn parses_kokoro_speech_config() {
        let cfg = config_from(
            r#"
            [speech]
            backend = kokoro
            kokoro_model_dir = /tmp/kokoro
            kokoro_sid = 7
            kokoro_speed = 1.2
            kokoro_lang = en-us
            kokoro_threads = 4
            "#,
        );

        assert_eq!(cfg.speech.backend, "kokoro");
        assert_eq!(cfg.speech.kokoro.model_dir, "/tmp/kokoro");
        assert_eq!(cfg.speech.kokoro.sid, 7);
        assert_eq!(cfg.speech.kokoro.speed, 1.2);
        assert_eq!(cfg.speech.kokoro.lang, "en-us");
        assert_eq!(cfg.speech.kokoro.threads, 4);
    }

    #[test]
    fn parses_supertonic_speech_config() {
        let cfg = config_from(
            r#"
            [speech]
            backend = supertonic
            supertonic_model_dir = /tmp/supertonic
            supertonic_sid = 2
            supertonic_speed = 0.9
            supertonic_lang = de
            supertonic_steps = 6
            supertonic_threads = 3
            "#,
        );

        assert_eq!(cfg.speech.backend, "supertonic");
        assert_eq!(cfg.speech.supertonic.model_dir, "/tmp/supertonic");
        assert_eq!(cfg.speech.supertonic.sid, 2);
        assert_eq!(cfg.speech.supertonic.speed, 0.9);
        assert_eq!(cfg.speech.supertonic.lang, "de");
        assert_eq!(cfg.speech.supertonic.steps, 6);
        assert_eq!(cfg.speech.supertonic.threads, 3);
    }

    #[test]
    fn parses_vad_config() {
        let cfg = config_from(
            r#"
            [vad]
            enabled = false
            threshold = 0.62
            min_speech_duration_ms = 180
            min_silence_duration_ms = 220
            speech_pad_ms = 45
            "#,
        );

        assert!(!cfg.vad.enabled);
        assert_eq!(cfg.vad.threshold, 0.62);
        assert_eq!(cfg.vad.min_speech_duration_ms, 180);
        assert_eq!(cfg.vad.min_silence_duration_ms, 220);
        assert_eq!(cfg.vad.speech_pad_ms, 45);
    }

    #[test]
    fn parses_search_tool_config() {
        let cfg = config_from(
            r#"
            [search]
            enabled = false
            endpoint = http://10.0.0.2:8888/search
            max_results = 3
            timeout = 4
            "#,
        );

        assert!(!cfg.search.enabled);
        assert_eq!(cfg.search.endpoint, "http://10.0.0.2:8888/search");
        assert_eq!(cfg.search.max_results, 3);
        assert_eq!(cfg.search.timeout_secs, 4);
    }

    #[test]
    fn parses_logging_config() {
        let cfg = config_from(
            r#"
            [logging]
            enabled = false
            debug = true
            path = /tmp/yappr-test.log
            "#,
        );

        assert!(!cfg.logging.enabled);
        assert!(cfg.logging.debug);
        assert_eq!(cfg.logging.path, "/tmp/yappr-test.log");
    }

    #[test]
    fn user_config_overrides_defaults_without_rebuild() {
        let cfg = config_from(
            r#"
            [models]
            active = e4b-qat

            [model:e4b-qat]
            ctx_size = 16384
            ngl = 12

            [server]
            endpoint = http://127.0.0.1:9090/v1/chat/completions
            port = 9090

            [speech]
            voice = Alex

            [search]
            endpoint = http://localhost:7777/search
            "#,
        );

        assert_eq!(cfg.model.active, "e4b-qat");
        assert_eq!(cfg.model.ctx_size, "16384");
        assert_eq!(cfg.model.ngl, "12");
        assert_eq!(
            cfg.server.endpoint,
            "http://127.0.0.1:9090/v1/chat/completions"
        );
        assert_eq!(cfg.server.port, 9090);
        assert_eq!(cfg.speech.voice.as_deref(), Some("Alex"));
        assert_eq!(cfg.search.endpoint, "http://localhost:7777/search");
    }

    #[test]
    fn parses_audio_and_language_options() {
        let cfg = config_from(
            r#"
            [audio]
            device = Studio Mic
            samplerate = 24000
            max_seconds = 12
            tail_seconds = 0.8

            [language]
            source = English
            target = Spanish
            options = auto, English, Spanish
            "#,
        );

        assert_eq!(cfg.audio.device.as_deref(), Some("Studio Mic"));
        assert_eq!(cfg.audio.samplerate, 24000);
        assert_eq!(cfg.audio.max_seconds, 12.0);
        assert_eq!(cfg.audio.tail_seconds, 0.8);
        assert_eq!(cfg.language.source, "English");
        assert_eq!(cfg.language.target, "Spanish");
        assert_eq!(cfg.language.options, ["auto", "English", "Spanish"]);
    }
}
