use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Recording {
    stream: Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    input_sample_rate: u32,
    target_sample_rate: u32,
}

pub struct CapturedAudio {
    pub wav: Vec<u8>,
    pub peak: f32,
    pub seconds: f32,
    pub samples: usize,
    pub nonzero_samples: usize,
}

impl Recording {
    pub fn start(
        device_name: Option<&str>,
        sample_rate: u32,
        _max_seconds: f32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = match device_name {
            Some(name) => host
                .input_devices()?
                .find(|device| device.name().map(|n| n == name).unwrap_or(false))
                .ok_or_else(|| format!("input device not found: {name}"))?,
            None => host
                .default_input_device()
                .ok_or("no default input device available")?,
        };
        let supported = device.default_input_config()?;
        let config: StreamConfig = supported.clone().into();
        let input_sample_rate = config.sample_rate.0;
        let channels = config.channels as usize;
        crate::logger::log_line(format!(
            "audio input: device='{}' format={:?} channels={} rate={} target_rate={}",
            device.name().unwrap_or_else(|_| "unknown".to_string()),
            supported.sample_format(),
            channels,
            input_sample_rate,
            sample_rate
        ));

        let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
        let sink = Arc::clone(&samples);
        let err_fn = |err| crate::logger::log_line(format!("audio input error: {err}"));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _| push_input(data.iter().copied(), channels, &sink),
                err_fn,
                None,
            )?,
            SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    push_input(
                        data.iter().map(|v| *v as f32 / i16::MAX as f32),
                        channels,
                        &sink,
                    )
                },
                err_fn,
                None,
            )?,
            SampleFormat::U16 => device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    push_input(
                        data.iter()
                            .map(|v| (*v as f32 / u16::MAX as f32) * 2.0 - 1.0),
                        channels,
                        &sink,
                    )
                },
                err_fn,
                None,
            )?,
            other => return Err(format!("unsupported input sample format: {other:?}").into()),
        };
        stream.play()?;
        crate::logger::log_line("audio input stream started");
        Ok(Self {
            stream,
            samples,
            input_sample_rate,
            target_sample_rate: sample_rate,
        })
    }

    pub fn stop(self, tail_seconds: f32) -> Result<CapturedAudio, Box<dyn std::error::Error>> {
        std::thread::sleep(Duration::from_secs_f32(tail_seconds.max(0.0)));
        drop(self.stream);
        let samples = self
            .samples
            .lock()
            .map_err(|_| "audio lock poisoned")?
            .clone();
        crate::logger::log_line(format!(
            "audio input stream stopped: raw_samples={} raw_peak={:.4}",
            samples.len(),
            peak(&samples)
        ));
        let samples = resample_linear(&samples, self.input_sample_rate, self.target_sample_rate);
        encode_wav(&samples, self.target_sample_rate)
    }
}

pub fn record_for(
    seconds: f32,
    sample_rate: u32,
) -> Result<CapturedAudio, Box<dyn std::error::Error>> {
    let recording = Recording::start(None, sample_rate, seconds)?;
    std::thread::sleep(Duration::from_secs_f32(seconds.max(0.0)));
    recording.stop(0.0)
}

pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    let mut names = devices
        .filter_map(|device| device.name().ok())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

pub fn input_device_status() -> String {
    let host = cpal::default_host();
    match host.default_input_device() {
        Some(device) => {
            let name = device.name().unwrap_or_else(|_| "unknown".to_string());
            match device.default_input_config() {
                Ok(config) => format!(
                    "available ({name}; {:?}, {} ch, {} Hz)",
                    config.sample_format(),
                    config.channels(),
                    config.sample_rate().0
                ),
                Err(err) => format!("available ({name}; config error: {err})"),
            }
        }
        None => "not available".to_string(),
    }
}

fn push_input<I>(iter: I, channels: usize, sink: &Arc<Mutex<Vec<f32>>>)
where
    I: Iterator<Item = f32>,
{
    if let Ok(mut samples) = sink.lock() {
        if channels <= 1 {
            samples.extend(iter);
        } else {
            let mut frame = Vec::with_capacity(channels);
            for sample in iter {
                frame.push(sample);
                if frame.len() == channels {
                    samples.push(frame.iter().sum::<f32>() / channels as f32);
                    frame.clear();
                }
            }
        }
    }
}

fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max)
}

fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if samples.is_empty() || from_rate == to_rate {
        return samples.to_vec();
    }
    let out_len = (samples.len() as u64 * to_rate as u64 / from_rate as u64) as usize;
    let ratio = from_rate as f32 / to_rate as f32;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f32 * ratio;
        let idx = pos.floor() as usize;
        let frac = pos - idx as f32;
        let a = samples.get(idx).copied().unwrap_or(0.0);
        let b = samples.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

fn encode_wav(
    samples: &[f32],
    sample_rate: u32,
) -> Result<CapturedAudio, Box<dyn std::error::Error>> {
    let peak = peak(samples);
    let nonzero_samples = samples
        .iter()
        .filter(|sample| sample.abs() > 0.00001)
        .count();
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        for sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            writer.write_sample((clamped * i16::MAX as f32) as i16)?;
        }
        writer.finalize()?;
    }
    Ok(CapturedAudio {
        wav: cursor.into_inner(),
        peak,
        seconds: samples.len() as f32 / sample_rate as f32,
        samples: samples.len(),
        nonzero_samples,
    })
}
