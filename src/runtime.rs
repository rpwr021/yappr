use crate::asr;
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

/// How long the error icon stays up before the tray falls back to idle.
/// Long enough to notice, short enough that the app never looks wedged.
const ERROR_LINGER: std::time::Duration = std::time::Duration::from_secs(4);

/// How often to probe the backend once it has come up, and how long to wait before
/// the confirming second probe. Slow enough not to matter, fast enough that the
/// tray is honest well before the user reaches for the hotkey.
const HEALTH_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
const HEALTH_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

pub struct Runtime {
    tx: Sender<HotkeyCommand>,
    busy: AtomicBool,
    recording: AtomicBool,
    epoch: AtomicU64,
    pub status: AtomicU8,
    /// Bumped on every status change so a pending error-clear can tell whether
    /// the status it wants to reset is still the one it set.
    status_gen: AtomicU64,
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
            status_gen: AtomicU64::new(0),
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
            // GPU-poor tier: fetch the on-device ASR model and we're done — no
            // llama-server, no multi-GB Gemma download, no engine install.
            if cfg.mode.is_poor() {
                runtime.store_status(ui::PROVISIONING_MODEL);
                match server::ensure_asr_model(cfg) {
                    Ok(_) => runtime.mark_ready(),
                    Err(err) => runtime.fail_provision(format!("ASR model download failed: {err}")),
                }
                return;
            }
            runtime.store_status(ui::PROVISIONING_MODEL);
            let paths = match server::ensure_model(cfg) {
                Ok(paths) => paths,
                Err(err) => return runtime.fail_provision(format!("model download failed: {err}")),
            };
            let (Some(weights), Some(mmproj)) = (paths.weights, paths.mmproj) else {
                return runtime.fail_provision("model files missing after download".into());
            };
            runtime.store_status(ui::PROVISIONING_ENGINE);
            if let Err(err) = server::ensure_engine() {
                return runtime.fail_provision(format!("engine install failed: {err}"));
            }
            runtime.store_status(ui::STARTING);
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

    /// Whether there is a transcript worth copying, used to enable the menu item.
    pub fn has_transcript(&self) -> bool {
        self.last_transcript
            .lock()
            .map(|t| t.as_deref().is_some_and(|text| !text.trim().is_empty()))
            .unwrap_or(false)
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

    /// Single write path for the tray status. Every change bumps `status_gen` so
    /// a pending error-clear can detect that it has been superseded. Returns the
    /// generation this write produced.
    fn store_status(&self, status: u8) -> u64 {
        self.status.store(status, Ordering::SeqCst);
        self.status_gen.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn mark_ready(self: &Arc<Self>) {
        self.ready.store(true, Ordering::SeqCst);
        // Don't clobber a permission notice raised by the hotkey layer.
        if self.status.load(Ordering::SeqCst) != ui::NOTICE {
            self.store_status(ui::IDLE);
        }
        log_line("backend ready");
        self.watch_backend();
    }

    /// Counterpart to `mark_ready`: the backend answered once and has now stopped.
    /// Without this, `ready` was write-once, so a dead llama-server left the tray
    /// showing "Ready" while every dictation burned the full request timeout and
    /// produced nothing. We report and stop accepting work; we do not respawn.
    fn mark_backend_down(&self, reason: &str) {
        if !self.ready.swap(false, Ordering::SeqCst) {
            return;
        }
        self.store_status(ui::BACKEND_DOWN);
        log_line(format!(
            "backend stopped answering ({reason}); quit and reopen Yappr to restart it"
        ));
    }

    /// Poll the backend so its death is noticed while idle, not on the next hotkey
    /// press. Only meaningful for a managed server: with `manage = false` the
    /// endpoint is someone else's to run, and in poor mode there is no server.
    fn watch_backend(self: &Arc<Self>) {
        let cfg = &self.menu_config;
        if !cfg.server.manage || cfg.mode.is_poor() {
            return;
        }
        let runtime = Arc::clone(self);
        let port = cfg.server.port;
        std::thread::spawn(move || loop {
            std::thread::sleep(HEALTH_POLL_INTERVAL);
            if !runtime.ready.load(Ordering::SeqCst) {
                return;
            }
            // One failed probe can be a slow model holding the socket, so require
            // two consecutive misses before declaring the backend dead.
            if server::healthy(port) {
                continue;
            }
            std::thread::sleep(HEALTH_RETRY_DELAY);
            if !server::healthy(port) {
                runtime.mark_backend_down("health probe failed twice");
                return;
            }
        });
    }

    /// Provisioning failures are terminal: there is no working backend to fall
    /// back to, so this stays on screen (unlike transient per-request errors,
    /// which `set_status` clears after ERROR_LINGER). Uses its own status rather
    /// than ERROR so the tray can say setup failed and a relaunch resumes it,
    /// instead of the same "see log" as a one-off paste failure.
    fn fail_provision(&self, message: String) {
        self.store_status(ui::SETUP_FAILED);
        log_line(format!("{message}; reopen Yappr to retry (downloads resume)"));
    }

    /// Tear down the managed llama-server before exiting. `process::exit` skips
    /// destructors, so without this the backend (and its loaded model) would
    /// survive in memory after quit. Dropping the ManagedServer kills the child.
    pub fn shutdown(&self) {
        // Recover from poisoning rather than skipping the kill: a panic elsewhere
        // must not silently leak a multi-GB backend. The slot holds an Option, so
        // there is no torn state to worry about.
        let mut slot = match self.managed_server.lock() {
            Ok(slot) => slot,
            Err(poisoned) => {
                log_line("managed server lock poisoned; stopping backend anyway");
                poisoned.into_inner()
            }
        };
        if let Some(server) = slot.take() {
            // An adopted server (started outside Yappr) has no child to kill, so
            // saying "stopped" would assert something that did not happen.
            let owned = server.owns_process();
            drop(server);
            log_line(if owned {
                "managed llama-server stopped"
            } else {
                "left externally-started llama-server running"
            });
        }
    }

    pub fn hold_instance_lock(&self, lock: InstanceLock) {
        if let Ok(mut slot) = self.instance_lock.lock() {
            *slot = Some(lock);
        }
    }

    pub fn hotkey_down(&self, chat: bool) {
        // Dictation Only mode has no chat model loaded.
        if chat && self.menu_config.mode.is_poor() {
            log_line("ignoring chat hotkey: Dictation Only mode has no chat model");
            // Flash the tray as well as speaking: the announcement is the only
            // feedback otherwise, so with the volume down or output routed
            // elsewhere the keypress appeared to do nothing at all.
            set_status(ui::ERROR);
            self.announce("Chat is unavailable in Dictation Only mode.");
            return;
        }
        if !self.ready.load(Ordering::SeqCst) {
            // Let the user know it's working, not broken — especially during the
            // long first-run model download. A stopped backend is the exception:
            // that one is broken, and waiting will not fix it.
            let status = self.status.load(Ordering::SeqCst);
            let msg = match status {
                ui::BACKEND_DOWN => "The backend stopped. Quit and reopen Yappr.".to_string(),
                ui::SETUP_FAILED => "Setup didn't finish. Reopen Yappr to retry.".to_string(),
                ui::PROVISIONING_MODEL => match server::download_percent() {
                    Some(p) => format!("I'm still fetching files, {p} percent done."),
                    None => "I'm still fetching files, one moment.".to_string(),
                },
                _ => "I'm still starting up, one moment.".to_string(),
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
            "restart" => {
                log_line("restart requested from menu");
                self.shutdown();
                // `open` the .app bundle, not the inner executable: relaunching the
                // binary directly gives up the bundle identity that TCC grants are
                // tied to, so hotkeys would silently stop working.
                if let Some(bundle) = app_bundle_path() {
                    // Detached, so it survives our exit. The new instance takes over
                    // the pid lock, which is why we must exit rather than linger.
                    match std::process::Command::new("/usr/bin/open")
                        .arg("-n")
                        .arg(&bundle)
                        .spawn()
                    {
                        Ok(_) => {
                            log_line(format!("relaunching {}", bundle.display()));
                            std::process::exit(0);
                        }
                        Err(err) => log_line(format!("restart failed: {err}; quit manually")),
                    }
                } else {
                    log_line("restart unavailable: not running from an .app bundle");
                }
            }
            "about" => match std::process::Command::new("/usr/bin/open")
                .arg(ui::WEBSITE)
                .spawn()
            {
                Ok(_) => log_line(format!("opened {}", ui::WEBSITE)),
                Err(err) => log_line(format!("open website failed: {err}")),
            },
            "logs" => match open_log_path() {
                Ok(path) => log_line(format!("opened log: {path}")),
                Err(err) => log_line(format!("open log failed: {err}")),
            },
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
                // A CheckMenuItem toggles itself on click; this enforces radio
                // behaviour by clearing the other devices in the group.
                ui::select_menu_item(ui::group::MIC, id);
                match Config::set_user_value("audio", "device", persisted) {
                    Ok(()) => log_line(format!(
                        "audio device selected: {}",
                        value.as_deref().unwrap_or("System Default")
                    )),
                    Err(err) => self.report_save_failure("microphone", err),
                }
            }
            id if id.starts_with("mode:") => {
                // Fired by the NSSwitch toggle (mode:rich / mode:poor). The switch
                // updates its own position; we just persist. Takes effect on next
                // launch since provisioning differs per tier.
                let tier = id.trim_start_matches("mode:");
                match Config::set_user_value("mode", "tier", tier) {
                    Ok(()) => log_line(format!("mode selected: {tier}; restart Yappr to apply")),
                    Err(err) => self.report_save_failure("mode", err),
                }
            }
            id if id.starts_with("model:") => {
                let model = id.trim_start_matches("model:");
                ui::select_menu_item(ui::group::MODEL, id);
                match Config::set_user_value("models", "active", model) {
                    Ok(()) => log_line(format!("model selected: {model}; restart Yappr to apply")),
                    Err(err) => self.report_save_failure("chat model", err),
                }
            }
            id if id.starts_with("lang:") => {
                let language = id.trim_start_matches("lang:");
                ui::select_menu_item(ui::group::LANGUAGE, id);
                match Config::set_user_value("language", "target", language) {
                    Ok(()) => log_line(format!(
                        "output language selected: {language}; restart Yappr to apply"
                    )),
                    Err(err) => self.report_save_failure("output language", err),
                }
            }
            // No speech_backend arm: the Backend submenu is gone, since choosing a
            // voice already implies its engine. Selecting a voice below sets both.

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
                        ui::select_menu_item(ui::group::SAY_VOICE, id);
                        log_line(format!(
                            "macOS speech voice selected: {}; backend=say",
                            if voice.is_empty() {
                                "System Default"
                            } else {
                                voice
                            }
                        ));
                    }
                    Err(err) => {
                        self.report_save_failure("voice", err);
                        self.resync_speech_menu();
                    }
                }
            }
            // No supertonic_sid arm: the menu stopped exposing supertonic, so this
            // id can never be emitted. `speech.rs` still honours `backend =
            // supertonic` from config.ini for anyone who set it by hand.
            id if id.starts_with("kokoro_sid:") => {
                let sid = id.trim_start_matches("kokoro_sid:");
                let Ok(parsed) = sid.parse() else {
                    log_line(format!("kokoro speaker ignored: invalid sid {sid}"));
                    self.resync_speech_menu();
                    return;
                };
                let already_selected = self
                    .speech
                    .lock()
                    .map(|speech| speech.backend == "kokoro" && speech.kokoro.sid == parsed)
                    .unwrap_or(false);
                if already_selected {
                    // muda unchecks on re-click before dispatching; put it back.
                    ui::select_menu_item(ui::group::KOKORO_SID, id);
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
                        ui::select_menu_item(ui::group::KOKORO_SID, id);
                        log_line(format!("kokoro speaker selected: {sid}; backend=kokoro"));
                    }
                    Err(err) => {
                        self.report_save_failure("Kokoro speaker", err);
                        self.resync_speech_menu();
                    }
                }
            }
            _ => {}
        }
    }

    /// A setting could not be written to config.ini.
    ///
    /// These used to only log, while the in-memory value and the checkmark had
    /// already been updated: the menu showed the new selection, the file kept the
    /// old one, and it silently reverted on the next launch. Flash the tray and
    /// say so out loud, since the user is looking at the menu when it happens.
    fn report_save_failure(&self, what: &str, err: impl std::fmt::Display) {
        log_line(format!("{what} save failed: {err}"));
        set_status(ui::ERROR);
        self.announce(&format!("Could not save the {what} setting."));
    }

    /// Put the speech menu's checkmarks back in sync with the live config.
    ///
    /// muda toggles a CheckMenuItem before dispatching its event, so a click that
    /// we then reject (bad value, failed save) has already moved the tick. Without
    /// this the menu would claim a selection that was never stored.
    fn resync_speech_menu(&self) {
        let Ok(speech) = self.speech.lock() else {
            return;
        };
        ui::select_menu_item(
            ui::group::SAY_VOICE,
            &format!("speech_voice:{}", speech.voice.as_deref().unwrap_or("")),
        );
        ui::select_menu_item(
            ui::group::KOKORO_SID,
            &format!("kokoro_sid:{}", speech.kokoro.sid),
        );
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

/// Path of the enclosing `.app` bundle, or None when running the bare binary
/// (`cargo run`, tests). The executable lives at `Yappr.app/Contents/MacOS/Yappr`,
/// so the bundle is three levels up.
fn app_bundle_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.parent()?.parent()?.parent()?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

/// Reveal the log in Finder. The Logs row displayed a path but was built disabled
/// with no handler, while the status line told the user to "see log".
fn open_log_path() -> Result<String, Box<dyn std::error::Error>> {
    let runtime = RUNTIME.get().ok_or("runtime unavailable")?;
    let cfg = &runtime.menu_config.logging;
    if !cfg.enabled {
        return Err("logging is disabled in config.ini".into());
    }
    let path = crate::expand_tilde(&cfg.path);
    if !path.exists() {
        return Err(format!("no log yet at {}", path.display()).into());
    }
    // -R reveals it in Finder rather than opening it in a text editor, which is
    // friendlier for a file that can be thousands of lines.
    std::process::Command::new("/usr/bin/open")
        .arg("-R")
        .arg(&path)
        .spawn()?;
    Ok(path.display().to_string())
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
    let transcription = if cfg.mode.is_poor() {
        asr::transcribe(
            &captured.pcm,
            captured.sample_rate,
            &cfg.asr,
            &cfg.language.source,
        )
    } else {
        client.transcribe_wav(&captured.wav)
    };
    let text = match transcription {
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
    // An empty transcript is a failure, not a success. ASR returns Ok("") for
    // undecodable audio, and nothing checked: dictation then fired Cmd+V with an
    // empty clipboard and ended at Ready, so a broken transcription looked like a
    // working one that had nothing to say.
    if text.trim().is_empty() {
        set_status(ui::ERROR);
        log_line(format!(
            "transcription produced no text; peak={:.4} speech may not have been captured",
            captured.peak
        ));
        return;
    }
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
        let gen = runtime.store_status(status);
        // A transient failure must not leave the tray stuck on the error icon.
        // Fall back to idle after a beat, unless something else has since moved
        // the status on (a new recording, a provisioning step, another error).
        if status == ui::ERROR {
            let runtime = Arc::clone(runtime);
            std::thread::spawn(move || {
                std::thread::sleep(ERROR_LINGER);
                if should_recover(
                    gen,
                    runtime.status_gen.load(Ordering::SeqCst),
                    runtime.ready.load(Ordering::SeqCst),
                ) {
                    runtime.store_status(ui::IDLE);
                    log_line("recovered to idle after error");
                }
            });
        }
    }
}

/// Whether a lingering error should fall back to idle. Recover only if nothing
/// else has changed the status since (`gen == current_gen`) and the backend is
/// actually up — a failed provision has no working state to return to, so its
/// error stays on screen.
fn should_recover(gen: u64, current_gen: u64, ready: bool) -> bool {
    gen == current_gen && ready
}

#[cfg(test)]
mod tests {
    use super::should_recover;
    use crate::ui;

    #[test]
    fn recovers_to_idle_after_a_transient_error() {
        assert!(should_recover(7, 7, true));
    }

    #[test]
    fn keeps_error_visible_when_backend_never_came_up() {
        assert!(!should_recover(7, 7, false));
    }

    #[test]
    fn skips_recovery_when_status_moved_on() {
        // A new recording (or another error) bumped the generation while the
        // clear was pending; clobbering it back to idle would hide live state.
        assert!(!should_recover(7, 8, true));
    }

    #[test]
    fn keeps_error_visible_after_the_backend_stops() {
        // mark_backend_down clears `ready`, so a request that failed against a
        // dead backend must not self-clear to idle and claim everything is fine.
        assert!(!should_recover(7, 7, false));
    }

    #[test]
    fn terminal_states_are_distinct_from_a_transient_error() {
        // These used to all be ui::ERROR with one "see log" label, so a failed 4GB
        // download was indistinguishable from a single paste that didn't land.
        let states = [ui::ERROR, ui::BACKEND_DOWN, ui::SETUP_FAILED];
        for (i, a) in states.iter().enumerate() {
            for b in &states[i + 1..] {
                assert_ne!(a, b, "each failure state needs its own value");
            }
        }
        // Only ERROR self-clears; the other two persist until the user acts, which
        // is enforced by `set_status` scheduling the clear for ERROR alone.
        assert_ne!(ui::SETUP_FAILED, ui::ERROR);
        assert_ne!(ui::BACKEND_DOWN, ui::ERROR);
    }

    #[test]
    fn every_failure_state_has_its_own_message() {
        let labels = [
            ui::status_label(ui::ERROR),
            ui::status_label(ui::BACKEND_DOWN),
            ui::status_label(ui::SETUP_FAILED),
        ];
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                assert_ne!(a, b, "failure states must not share a label");
            }
        }
        // The two terminal states tell the user what to do; ERROR cannot, since it
        // covers everything from a failed paste to a VAD hiccup.
        assert!(ui::status_label(ui::BACKEND_DOWN).contains("reopen"));
        assert!(ui::status_label(ui::SETUP_FAILED).contains("reopen"));
    }
}
