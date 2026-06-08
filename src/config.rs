use crate::expand_tilde;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub server: ServerConfig,
    pub model: ModelConfig,
    pub audio: AudioConfig,
    pub language: LanguageConfig,
    pub chat: ChatConfig,
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
pub struct ChatConfig {
    pub voice: Option<String>,
    pub rate: i32,
    pub context_seconds: i64,
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
        if let Ok(contents) = fs::read_to_string(&user_path) {
            ini.merge(&contents);
        }

        let active_model = ini.get("models", "active", "e4b-qat");
        let model_section = format!("model:{active_model}");

        Ok(Self {
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
                repo: ini.model_value(&model_section, "repo", "google/gemma-4-E4B-it-qat-q4_0-gguf"),
                weights: ini.model_value(&model_section, "weights", "gemma-4-E4B_q4_0-it.gguf"),
                mmproj: ini.model_value(&model_section, "mmproj", "gemma-4-E4B-it-mmproj.gguf"),
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
                voice: non_empty(ini.get("chat", "voice", "")),
                rate: ini.get("chat", "rate", "190").parse().unwrap_or(190),
                context_seconds: ini
                    .get("chat", "context_seconds", "60")
                    .parse()
                    .unwrap_or(60),
            },
            search: SearchConfig {
                enabled: ini.get("search", "enabled", "true").parse().unwrap_or(true),
                endpoint: ini.get("search", "endpoint", "http://127.0.0.1:8888/search"),
                max_results: ini.get("search", "max_results", "5").parse().unwrap_or(5),
                timeout_secs: ini.get("search", "timeout", "15").parse().unwrap_or(15),
            },
        })
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
