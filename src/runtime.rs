use crate::audio::{CapturedAudio, Recording};
use crate::chat::{ChatClient, ChatMode};
use crate::config::Config;
use crate::inject;
use crate::instance::InstanceLock;
use crate::logger::log_line;
use crate::server::ManagedServer;
use crate::ui;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();

pub struct Runtime {
    tx: Sender<HotkeyCommand>,
    busy: AtomicBool,
    recording: AtomicBool,
    epoch: AtomicU64,
    pub status: AtomicU8,
    pub menu_config: Config,
    audio_device: Mutex<Option<String>>,
    last_transcript: Mutex<Option<String>>,
    instance_lock: Mutex<Option<InstanceLock>>,
}

struct ActiveRecording {
    recording: Recording,
    chat: bool,
    epoch: u64,
}

enum HotkeyCommand {
    Start { chat: bool, epoch: u64 },
    Stop,
}

impl Runtime {
    pub fn new(cfg: Config, client: ChatClient, server: Option<ManagedServer>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let menu_config = cfg.clone();
        let audio_device = Mutex::new(cfg.audio.device.clone());
        let client = Arc::new(client);
        std::thread::spawn(move || audio_worker(cfg, client, server, rx));
        Arc::new(Self {
            tx,
            busy: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            status: AtomicU8::new(ui::IDLE),
            menu_config,
            audio_device,
            last_transcript: Mutex::new(None),
            instance_lock: Mutex::new(None),
        })
    }

    pub fn hold_instance_lock(&self, lock: InstanceLock) {
        if let Ok(mut slot) = self.instance_lock.lock() {
            *slot = Some(lock);
        }
    }

    pub fn hotkey_down(&self, chat: bool) {
        if self.recording.load(Ordering::SeqCst) {
            log_line("recording: hotkey press already active");
            return;
        }
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        if self.busy.swap(false, Ordering::SeqCst) {
            log_line("busy: interrupted by new hotkey press");
            inject::stop_speech();
        }
        if let Err(err) = self.tx.send(HotkeyCommand::Start { chat, epoch }) {
            log_line(format!("hotkey command failed: {err}"));
        }
    }

    pub fn hotkey_up(self: &Arc<Self>) {
        if !self.recording.load(Ordering::SeqCst) {
            return;
        }
        self.busy.store(true, Ordering::SeqCst);
        if let Err(err) = self.tx.send(HotkeyCommand::Stop) {
            self.busy.store(false, Ordering::SeqCst);
            log_line(format!("hotkey command failed: {err}"));
        }
    }

    pub fn handle_menu(&self, id: &str) {
        match id {
            "quit" => {
                log_line("quit requested from menu");
                std::process::exit(0);
            }
            "copy_transcript" => match self.last_transcript.lock().ok().and_then(|v| v.clone()) {
                Some(text) if !text.trim().is_empty() => match inject::copy_text(&text) {
                    Ok(()) => log_line("last transcript copied"),
                    Err(err) => log_line(format!("copy transcript failed: {err}")),
                },
                _ => log_line("copy transcript ignored: no transcript yet"),
            },
            id if id == "mic:" || id.starts_with("mic:") => {
                let name = id.strip_prefix("mic:").unwrap_or_default();
                let value = (!name.is_empty()).then_some(name.to_string());
                if let Ok(mut device) = self.audio_device.lock() {
                    *device = value.clone();
                }
                let persisted = value.as_deref().unwrap_or("");
                match Config::set_user_value("audio", "device", persisted) {
                    Ok(()) => log_line(format!(
                        "audio device selected: {}",
                        value.as_deref().unwrap_or("System Default")
                    )),
                    Err(err) => log_line(format!("audio device save failed: {err}")),
                }
            }
            id if id.starts_with("model:") => {
                let model = id.trim_start_matches("model:");
                match Config::set_user_value("models", "active", model) {
                    Ok(()) => log_line(format!("model selected: {model}; restart Yappr to apply")),
                    Err(err) => log_line(format!("model save failed: {err}")),
                }
            }
            id if id.starts_with("lang:") => {
                let language = id.trim_start_matches("lang:");
                match Config::set_user_value("language", "target", language) {
                    Ok(()) => log_line(format!(
                        "output language selected: {language}; restart Yappr to apply"
                    )),
                    Err(err) => log_line(format!("language save failed: {err}")),
                }
            }
            _ => log_line(format!("menu event ignored: {id}")),
        }
    }
}

pub fn install(runtime: Arc<Runtime>) -> Result<(), &'static str> {
    RUNTIME
        .set(runtime)
        .map_err(|_| "runtime already installed")
}

pub fn runtime() -> Option<&'static Arc<Runtime>> {
    RUNTIME.get()
}

fn audio_worker(
    cfg: Config,
    client: Arc<ChatClient>,
    _server: Option<ManagedServer>,
    rx: Receiver<HotkeyCommand>,
) {
    let mut active: Option<ActiveRecording> = None;
    while let Ok(command) = rx.recv() {
        match command {
            HotkeyCommand::Start { chat, epoch } => {
                if active.is_some() {
                    continue;
                }
                let device = RUNTIME
                    .get()
                    .and_then(|runtime| runtime.audio_device.lock().ok().and_then(|v| v.clone()));
                match Recording::start(
                    device.as_deref(),
                    cfg.audio.samplerate,
                    cfg.audio.max_seconds,
                ) {
                    Ok(recording) => {
                        if let Some(runtime) = RUNTIME.get() {
                            runtime.recording.store(true, Ordering::SeqCst);
                        }
                        set_status(if chat {
                            ui::RECORDING_CHAT
                        } else {
                            ui::RECORDING_DICTATE
                        });
                        log_line(format!(
                            "hotkey down -> {}",
                            if chat { "chat" } else { "dictate" }
                        ));
                        active = Some(ActiveRecording {
                            recording,
                            chat,
                            epoch,
                        });
                    }
                    Err(err) => {
                        set_status(ui::ERROR);
                        log_line(format!("recording failed to start: {err}"));
                    }
                }
            }
            HotkeyCommand::Stop => {
                if let Some(runtime) = RUNTIME.get() {
                    runtime.recording.store(false, Ordering::SeqCst);
                }
                let Some(recording) = active.take() else {
                    clear_busy();
                    continue;
                };
                let chat = recording.chat;
                let epoch = recording.epoch;
                let captured = match stop_recording(&cfg, recording) {
                    Some(captured) => captured,
                    None => {
                        clear_busy();
                        continue;
                    }
                };
                let cfg = cfg.clone();
                let client = client.clone();
                std::thread::spawn(move || {
                    process_recording(&cfg, &client, chat, epoch, captured);
                    clear_busy();
                });
            }
        }
    }
}

fn clear_busy() {
    if let Some(runtime) = RUNTIME.get() {
        runtime.busy.store(false, Ordering::SeqCst);
    }
}

fn stop_recording(cfg: &Config, active: ActiveRecording) -> Option<CapturedAudio> {
    let captured = match active.recording.stop(cfg.audio.tail_seconds) {
        Ok(captured) => captured,
        Err(err) => {
            set_status(ui::ERROR);
            log_line(format!("recording stop failed: {err}"));
            return None;
        }
    };
    log_line(format!(
        "captured {:.2}s audio, {} bytes wav, peak={:.4}, samples={}, nonzero_samples={}",
        captured.seconds,
        captured.wav.len(),
        captured.peak,
        captured.samples,
        captured.nonzero_samples
    ));
    if !is_current(active.epoch) {
        log_line("recording discarded: interrupted");
        return None;
    }
    Some(captured)
}

fn process_recording(
    cfg: &Config,
    client: &ChatClient,
    chat: bool,
    epoch: u64,
    captured: CapturedAudio,
) {
    if captured.peak < 0.001 {
        set_status(ui::ERROR);
        log_line("No audio - grant Microphone access to Yappr");
        return;
    }
    set_status(ui::TRANSCRIBING);
    let text = match client.transcribe_wav(&captured.wav) {
        Ok(text) => text,
        Err(err) => {
            if !is_current(epoch) {
                log_line("transcription discarded: interrupted");
                return;
            }
            set_status(ui::ERROR);
            log_line(format!("transcription failed: {err}"));
            return;
        }
    };
    if !is_current(epoch) {
        log_line("transcript discarded: interrupted");
        return;
    }
    log_line(format!("heard: {text}"));
    if let Some(runtime) = RUNTIME.get() {
        if let Ok(mut transcript) = runtime.last_transcript.lock() {
            *transcript = Some(text.clone());
        }
    }
    if chat {
        set_status(ui::ANSWERING);
        match client.answer(&text, ChatMode::Spoken) {
            Ok(answer) => {
                if !is_current(epoch) {
                    log_line("answer discarded: interrupted");
                    return;
                }
                log_line(format!("answer: {answer}"));
                set_status(ui::SPEAKING);
                if let Err(err) = inject::say(&answer, cfg.chat.voice.as_deref(), cfg.chat.rate) {
                    if is_current(epoch) {
                        set_status(ui::ERROR);
                        log_line(format!("say failed: {err}"));
                    }
                }
            }
            Err(err) => {
                if !is_current(epoch) {
                    log_line("chat error discarded: interrupted");
                    return;
                }
                set_status(ui::ERROR);
                log_line(format!("chat failed: {err}"));
            }
        }
    } else if !is_current(epoch) {
        log_line("paste discarded: interrupted");
        return;
    } else if let Err(err) = inject::paste_text(&text) {
        set_status(ui::ERROR);
        log_line(format!("paste failed: {err}"));
    }
    if is_current(epoch) {
        set_status(ui::IDLE);
    }
}

fn is_current(epoch: u64) -> bool {
    RUNTIME
        .get()
        .is_none_or(|runtime| runtime.epoch.load(Ordering::SeqCst) == epoch)
}

fn set_status(status: u8) {
    if let Some(runtime) = RUNTIME.get() {
        runtime.status.store(status, Ordering::SeqCst);
    }
}
