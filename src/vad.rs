use crate::config::VadConfig;
use crate::expand_tilde;
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static DETECTOR: OnceLock<Mutex<VoiceActivityDetector>> = OnceLock::new();
const MODEL_BYTES: &[u8] = include_bytes!("../resources/assets/models/silero_vad.int8.onnx");

pub struct Decision {
    pub has_speech: bool,
    pub segments: usize,
    pub speech_seconds: f64,
}

pub fn detect(
    samples: &[f32],
    sample_rate: u32,
    cfg: &VadConfig,
) -> Result<Decision, Box<dyn std::error::Error>> {
    if samples.is_empty() {
        return Ok(Decision {
            has_speech: false,
            segments: 0,
            speech_seconds: 0.0,
        });
    }

    if sample_rate != 16_000 {
        return Err(format!("sherpa silero VAD requires 16000 Hz audio, got {sample_rate}").into());
    }

    let detector = detector(cfg)?.lock().map_err(|_| "vad lock poisoned")?;
    detector.reset();
    detector.clear();
    detector.accept_waveform(samples);
    detector.flush();

    let mut segments = 0;
    let mut speech_samples = 0;
    while let Some(segment) = detector.front() {
        segments += 1;
        speech_samples += segment.n().max(0) as usize;
        detector.pop();
    }

    Ok(Decision {
        has_speech: segments > 0,
        segments,
        speech_seconds: speech_samples as f64 / sample_rate as f64,
    })
}

fn detector(
    cfg: &VadConfig,
) -> Result<&'static Mutex<VoiceActivityDetector>, Box<dyn std::error::Error>> {
    if DETECTOR.get().is_none() {
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(model_path()?.display().to_string()),
                threshold: cfg.threshold,
                min_silence_duration: cfg.min_silence_duration_ms as f32 / 1000.0,
                min_speech_duration: cfg.min_speech_duration_ms as f32 / 1000.0,
                window_size: 512,
                max_speech_duration: 30.0,
            },
            sample_rate: 16_000,
            num_threads: 1,
            provider: Some("cpu".to_string()),
            debug: false,
            ..Default::default()
        };
        let detector =
            VoiceActivityDetector::create(&config, 30.0).ok_or("failed to create VAD detector")?;
        let _ = DETECTOR.set(Mutex::new(detector));
    }
    DETECTOR
        .get()
        .ok_or_else(|| "vad detector not initialized".into())
}

fn model_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = if cfg!(test) {
        std::env::temp_dir().join("yappr-silero-vad/silero_vad.int8.onnx")
    } else {
        expand_tilde("~/.yappr/models/silero-vad/silero_vad.int8.onnx")
    };
    let needs_write = fs::metadata(&path)
        .map(|metadata| metadata.len() != MODEL_BYTES.len() as u64)
        .unwrap_or(true);
    if needs_write {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, MODEL_BYTES)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::detect;
    use crate::config::VadConfig;

    #[test]
    fn empty_audio_has_no_speech_without_loading_model() {
        let decision = detect(
            &[],
            16_000,
            &VadConfig {
                enabled: true,
                threshold: 0.5,
                min_speech_duration_ms: 250,
                min_silence_duration_ms: 100,
                speech_pad_ms: 30,
            },
        )
        .unwrap();

        assert!(!decision.has_speech);
        assert_eq!(decision.segments, 0);
        assert_eq!(decision.speech_seconds, 0.0);
    }

    #[test]
    fn silence_is_not_speech() {
        let samples = vec![0.0; 16_000];
        let decision = detect(
            &samples,
            16_000,
            &VadConfig {
                enabled: true,
                threshold: 0.5,
                min_speech_duration_ms: 250,
                min_silence_duration_ms: 100,
                speech_pad_ms: 30,
            },
        )
        .unwrap();

        assert!(!decision.has_speech);
        assert_eq!(decision.segments, 0);
    }
}
