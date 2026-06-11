use crate::audio::{CapturedAudio, Recording};
use crate::chat::{ChatClient, ChatMode};
use crate::config::{Config, SpeechConfig};
use crate::inject;
use crate::instance::InstanceLock;
use crate::logger::{debug_line, log_line};
use crate::perms;
use crate::server::{self, ManagedServer};
use crate::speech;
use crate::ui;
use crate::vad;
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
    ready: AtomicBool,
    pub menu_config: Config,
    audio_device: Mutex<Option<String>>,
    speech: Mutex<SpeechConfig>,
    last_transcript: Mutex<Option<String>>,
    instance_lock: Mutex<Option<InstanceLock>>,
    managed_server: Mutex<Option<ManagedServer>>,
    last_announce: Mutex<std::time::Instant>,
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
    pub fn new(cfg: Config, client: ChatClient) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let menu_config = cfg.clone();
        let audio_device = Mutex::new(cfg.audio.device.clone());
        let speech = Mutex::new(cfg.speech.clone());
        let client = Arc::new(client);
        std::thread::spawn(move || audio_worker(cfg, client, rx));
        Arc::new(Self {
            tx,
            busy: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            status: AtomicU8::new(ui::STARTING),
            ready: AtomicBool::new(false),
            menu_config,
            audio_device,
            speech,
            last_transcript: Mutex::new(None),
            instance_lock: Mutex::new(None),
            managed_server: Mutex::new(None),
            // Start stale so the first "still fetching" announcement isn't debounced.
            last_announce: Mutex::new(
                std::time::Instant::now() - std::time::Duration::from_secs(60),
            ),
        })
    }

    /// Download the model and engine, then start llama-server, off the main
    /// thread so the menu bar is responsive during the (potentially multi-GB,
    /// multi-minute) first-run download. Status transitions drive the tray icon.
    pub fn provision(self: &Arc<Self>) {
        let runtime = Arc::clone(self);
        std::thread::spawn(move || {
            let cfg = &runtime.menu_config;
            if !cfg.server.manage {
                runtime.mark_ready();
                return;
            }
            runtime
                .status
                .store(ui::PROVISIONING_MODEL, Ordering::SeqCst);
            let paths = match server::ensure_model(cfg) {
                Ok(paths) => paths,
                Err(err) => return runtime.fail_provision(format!("model download failed: {err}")),
            };
            let (Some(weights), Some(mmproj)) = (paths.weights, paths.mmproj) else {
                return runtime.fail_provision("model files missing after download".into());
            };
            runtime
                .status
                .store(ui::PROVISIONING_ENGINE, Ordering::SeqCst);
            if let Err(err) = server::ensure_engine() {
                return runtime.fail_provision(format!("engine install failed: {err}"));
            }
            runtime.status.store(ui::STARTING, Ordering::SeqCst);
            match server::start(cfg, &weights, &mmproj) {
                Ok(server) => {
                    if let Ok(mut slot) = runtime.managed_server.lock() {
                        *slot = Some(server);
                    }
                    runtime.mark_ready();
                }
                Err(err) => runtime.fail_provision(format!("llama-server failed to start: {err}")),
            }
        });
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Speak a short status message, rate-limited so repeated hotkey presses
    /// during the download don't stack overlapping speech.
    fn announce(&self, message: &str) {
        if let Ok(mut last) = self.last_announce.lock() {
            if last.elapsed() < std::time::Duration::from_secs(6) {
                return;
            }
            *last = std::time::Instant::now();
        }
        let speech = self.speech.lock().ok().map(|s| s.clone());
        let text = message.to_string();
        std::thread::spawn(move || {
            if let Some(cfg) = speech {
                let _ = speech::speak(&text, &cfg);
            }
        });
    }

    fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
        // Don't clobber a permission notice raised by the hotkey layer.
        if self.status.load(Ordering::SeqCst) != ui::NOTICE {
            self.status.store(ui::IDLE, Ordering::SeqCst);
        }
        log_line("backend ready");
    }

    fn fail_provision(&self, message: String) {
        self.status.store(ui::ERROR, Ordering::SeqCst);
        log_line(message);
    }

    /// Tear down the managed llama-server before exiting. `process::exit` skips
    /// destructors, so without this the backend (and its loaded model) would
    /// survive in memory after quit. Dropping the ManagedServer kills the child.
    pub fn shutdown(&self) {
        if let Ok(mut slot) = self.managed_server.lock() {
            if let Some(server) = slot.take() {
                drop(server);
                log_line("managed llama-server stopped");
            }
        }
    }

    pub fn hold_instance_lock(&self, lock: InstanceLock) {
        if let Ok(mut slot) = self.instance_lock.lock() {
            *slot = Some(lock);
        }
    }

    pub fn hotkey_down(&self, chat: bool) {
        if !self.ready.load(Ordering::SeqCst) {
            // Let the user know it's working, not broken — especially during the
            // long first-run model download.
            let downloading = self.status.load(Ordering::SeqCst) == ui::PROVISIONING_MODEL;
            let msg = if downloading {
                match server::download_percent() {
                    Some(p) => format!("I'm still fetching files, {p} percent done."),
                    None => "I'm still fetching files, one moment.".to_string(),
                }
            } else {
                "I'm still starting up, one moment.".to_string()
            };
            log_line(format!("ignoring hotkey: backend not ready ({msg})"));
            self.announce(&msg);
            return;
        }
        if self.recording.load(Ordering::SeqCst) {
            log_line("recording: hotkey press already active");
            return;
        }
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        if self.busy.swap(false, Ordering::SeqCst) {
            log_line("busy: interrupted by new hotkey press");
            speech::stop();
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
                self.shutdown();
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
            id if id.starts_with("speech_backend:") => {
                let backend = id.trim_start_matches("speech_backend:");
                let already_selected = self
                    .speech
                    .lock()
                    .map(|speech| speech.backend == backend)
                    .unwrap_or(false);
                if already_selected {
                    return;
                }
                match Config::set_user_value("speech", "backend", backend) {
                    Ok(()) => {
                        self.update_speech(|speech| speech.backend = backend.to_string());
                        ui::select_menu_item("speech_backend", id);
                        log_line(format!("speech backend selected: {backend}"));
                    }
                    Err(err) => log_line(format!("speech backend save failed: {err}")),
                }
            }
            id if id == "speech_voice:" || id.starts_with("speech_voice:") => {
                let voice = id.trim_start_matches("speech_voice:");
                let saved = Config::set_user_value("speech", "backend", "say")
                    .and_then(|()| Config::set_user_value("speech", "voice", voice));
                match saved {
                    Ok(()) => {
                        self.update_speech(|speech| {
                            speech.backend = "say".to_string();
                            speech.voice = (!voice.is_empty()).then_some(voice.to_string());
                        });
                        ui::select_menu_item("speech_backend", "speech_backend:say");
                        ui::select_menu_item("speech_voice", id);
                        log_line(format!(
                            "macOS speech voice selected: {}; backend=say",
                            if voice.is_empty() {
                                "System Default"
                            } else {
                                voice
                            }
                        ));
                    }
                    Err(err) => log_line(format!("speech voice save failed: {err}")),
                }
            }
            id if id.starts_with("supertonic_sid:") => {
                let sid = id.trim_start_matches("supertonic_sid:");
                let saved = Config::set_user_value("speech", "backend", "supertonic")
                    .and_then(|()| Config::set_user_value("speech", "supertonic_sid", sid));
                match saved {
                    Ok(()) => {
                        if let Ok(parsed) = sid.parse() {
                            self.update_speech(|speech| {
                                speech.backend = "supertonic".to_string();
                                speech.supertonic.sid = parsed;
                            });
                            ui::select_menu_item("speech_backend", "speech_backend:supertonic");
                            ui::select_menu_item("supertonic_sid", id);
                        }
                        log_line(format!(
                            "supertonic voice selected: {sid}; backend=supertonic"
                        ));
                    }
                    Err(err) => log_line(format!("supertonic voice save failed: {err}")),
                }
            }
            id if id.starts_with("kokoro_sid:") => {
                let sid = id.trim_start_matches("kokoro_sid:");
                let Ok(parsed) = sid.parse() else {
                    log_line(format!("kokoro speaker ignored: invalid sid {sid}"));
                    return;
                };
                let already_selected = self
                    .speech
                    .lock()
                    .map(|speech| speech.backend == "kokoro" && speech.kokoro.sid == parsed)
                    .unwrap_or(false);
                if already_selected {
                    return;
                }
                let saved = Config::set_user_value("speech", "backend", "kokoro")
                    .and_then(|()| Config::set_user_value("speech", "kokoro_sid", sid));
                match saved {
                    Ok(()) => {
                        self.update_speech(|speech| {
                            speech.backend = "kokoro".to_string();
                            speech.kokoro.sid = parsed;
                        });
                        ui::select_menu_item("speech_backend", "speech_backend:kokoro");
                        ui::select_menu_item("kokoro_sid", id);
                        log_line(format!("kokoro speaker selected: {sid}; backend=kokoro"));
                    }
                    Err(err) => log_line(format!("kokoro speaker save failed: {err}")),
                }
            }
            _ => {}
        }
    }

    fn update_speech(&self, update: impl FnOnce(&mut SpeechConfig)) {
        if let Ok(mut speech) = self.speech.lock() {
            update(&mut speech);
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

fn audio_worker(cfg: Config, client: Arc<ChatClient>, rx: Receiver<HotkeyCommand>) {
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
        log_line(format!(
            "no audio captured; {}",
            perms::report().log_summary()
        ));
        return;
    }
    if cfg.vad.enabled {
        match vad::detect(&captured.pcm, captured.sample_rate, &cfg.vad) {
            Ok(decision) if decision.has_speech => log_line(format!(
                "vad: speech detected segments={} speech_seconds={:.2}",
                decision.segments, decision.speech_seconds
            )),
            Ok(_) => {
                set_status(ui::IDLE);
                log_line(format!(
                    "vad: no speech detected; skipping ASR (threshold={:.2})",
                    cfg.vad.threshold
                ));
                return;
            }
            Err(err) => log_line(format!("vad failed: {err}; continuing without VAD")),
        }
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
    debug_line(format!("heard: {text}"));
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
                debug_line(format!("answer: {answer}"));
                set_status(ui::SPEAKING);
                let speech_cfg = current_speech().unwrap_or_else(|| cfg.speech.clone());
                if let Err(err) = speech::speak(&answer, &speech_cfg) {
                    if is_current(epoch) {
                        set_status(ui::ERROR);
                        log_line(format!("speech failed: {err}"));
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

fn current_speech() -> Option<SpeechConfig> {
    RUNTIME
        .get()
        .and_then(|runtime| runtime.speech.lock().ok().map(|speech| speech.clone()))
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
