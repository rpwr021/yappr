use crate::audio;
use crate::config::Config;
use crate::logger::log_line;
use crate::mascot::icon_for_state;
use std::cell::RefCell;
use std::ffi::c_void;
use std::process::Command;
use std::sync::atomic::Ordering;
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    TrayIcon, TrayIconBuilder,
};

pub const IDLE: u8 = 0;
pub const RECORDING_DICTATE: u8 = 1;
pub const RECORDING_CHAT: u8 = 2;
pub const TRANSCRIBING: u8 = 3;
pub const ANSWERING: u8 = 4;
pub const SPEAKING: u8 = 5;
pub const ERROR: u8 = 6;
pub const NOTICE: u8 = 7;
pub const PROVISIONING_MODEL: u8 = 8;
pub const PROVISIONING_ENGINE: u8 = 9;
pub const STARTING: u8 = 10;
/// The backend was up and then stopped answering. Distinct from ERROR because it
/// is not tied to one request: nothing will work until Yappr is relaunched.
pub const BACKEND_DOWN: u8 = 11;
/// First-run setup never finished (download, engine install, or server start).
/// Distinct from ERROR so "the 4 GB download failed" doesn't look like "one paste
/// didn't land"; partial downloads resume, so relaunching is worth saying.
pub const SETUP_FAILED: u8 = 12;

/// Radio-style menu groups. Named constants rather than bare strings because the
/// group has to match between the builder here and the click handler in `runtime`;
/// a typo in either used to mean the checkmark silently stopped moving.
pub mod group {
    pub const MIC: &str = "mic";
    pub const MODEL: &str = "model";
    pub const LANGUAGE: &str = "lang";
    pub const SPEECH_BACKEND: &str = "speech_backend";
    pub const SAY_VOICE: &str = "speech_voice";
    pub const KOKORO_SID: &str = "kokoro_sid";
}

thread_local! {
    static SELECTABLE_MENU_ITEMS: RefCell<Vec<SelectableMenuItem>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
struct SelectableMenuItem {
    group: &'static str,
    id: String,
    item: CheckMenuItem,
}

pub struct StatusItem {
    tray: TrayIcon,
    status: MenuItem,
    frame: usize,
    last_state: u8,
}

pub fn create_status_item(cfg: &Config) -> Result<StatusItem, Box<dyn std::error::Error>> {
    clear_selectable_menu_items();
    let menu = Menu::new();
    let status = MenuItem::with_id("status", "Status: Ready", false, None);
    // Non-clickable reminders of the (fixed) push-to-talk hotkeys.
    let dictate_hint = MenuItem::with_id("hint_dictate", "Dictate: hold Right Option", false, None);
    let chat_hint_text = if cfg.mode.is_poor() {
        "Chat: unavailable in Dictation Only mode"
    } else {
        "Chat: hold ⌘ + Right Option"
    };
    let chat_hint = MenuItem::with_id("hint_chat", chat_hint_text, false, None);
    let microphone = microphone_menu(cfg)?;
    let model = model_menu(cfg)?;
    let language = language_menu(cfg)?;
    let speech = speech_menu(cfg)?;
    let copy = MenuItem::with_id("copy_transcript", "Copy Last Transcript", true, None);
    let logs = MenuItem::with_id("logs", log_label(cfg), false, None);
    let version = MenuItem::with_id(
        "version",
        format!("Yappr {}", crate::version()),
        false,
        None,
    );
    let quit = MenuItem::with_id("quit", "Quit", true, None);
    let separator = PredefinedMenuItem::separator();
    menu.append_items(&[
        &status,
        &dictate_hint,
        &chat_hint,
        &PredefinedMenuItem::separator(),
        // The mode NSSwitch toggle is inserted here (index 4) after the
        // tray is built, via mode_switch::install on the native NSMenu.
        &microphone,
        &model,
        &language,
        &speech,
        &copy,
        &logs,
        &separator,
        &version,
        &quit,
    ])?;

    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        if let Some(runtime) = crate::runtime::runtime() {
            runtime.handle_menu(event.id.0.as_str());
        }
    }));

    // Grab the native NSMenu before muda's Menu is moved into the tray; the tray
    // keeps the Menu alive, so the pointer stays valid for the menu's lifetime.
    #[cfg(target_os = "macos")]
    let ns_menu = {
        use tray_icon::menu::ContextMenu;
        menu.ns_menu()
    };
    let tray = TrayIconBuilder::new()
        .with_icon(icon_for_state(IDLE, 0)?)
        .with_title(" ")
        .with_tooltip("Yappr: hold Right Option to dictate")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .with_menu_on_right_click(true)
        .build()?;
    #[cfg(target_os = "macos")]
    crate::mode_switch::install(ns_menu, !cfg.mode.is_poor());
    log_line("menu bar status item created");
    Ok(StatusItem {
        tray,
        status,
        frame: 0,
        last_state: IDLE,
    })
}

pub fn install_animation_timer(item: &'static mut StatusItem) {
    unsafe {
        let context = TimerContext {
            version: 0,
            info: item as *mut StatusItem as *mut c_void,
            retain: None,
            release: None,
            copy_description: None,
        };
        let timer = CFRunLoopTimerCreate(
            std::ptr::null(),
            CFAbsoluteTimeGetCurrent(),
            0.35,
            0,
            0,
            animation_tick,
            &context as *const TimerContext as *const c_void,
        );
        CFRunLoopAddTimer(CFRunLoopGetCurrent(), timer, kCFRunLoopCommonModes);
    }
}

extern "C" fn animation_tick(_timer: *mut c_void, info: *mut c_void) {
    let Some(runtime) = crate::runtime::runtime() else {
        return;
    };
    let item = unsafe { &mut *(info as *mut StatusItem) };
    item.frame = item.frame.wrapping_add(1);
    let state = runtime.status.load(Ordering::SeqCst);
    // Redraw on a state change, or every tick for states that pulse, so the
    // animation advances. Static states only redraw when the state changes.
    let should_draw = state != item.last_state || crate::mascot::is_animated(state);
    if should_draw {
        if let Ok(icon) = icon_for_state(state, item.frame) {
            let _ = item.tray.set_icon(Some(icon));
        }
    }
    item.last_state = state;
    item.status.set_text(status_text(state));
}

/// Status line text, with live download progress appended while fetching the model.
fn status_text(state: u8) -> String {
    let base = status_label(state);
    if state == PROVISIONING_MODEL {
        if let Some(pct) = crate::server::download_percent() {
            return format!("{base} {pct}%");
        }
    }
    base.to_string()
}

fn microphone_menu(cfg: &Config) -> Result<Submenu, Box<dyn std::error::Error>> {
    let menu = Submenu::with_id("microphone", "Microphone", true);
    menu.append(&selectable_item(
        group::MIC,
        "mic:",
        "System Default",
        cfg.audio.device.is_none(),
        true,
    ))?;
    for name in audio::input_devices() {
        let checked = cfg.audio.device.as_deref() == Some(name.as_str());
        menu.append(&selectable_item(
            group::MIC,
            format!("mic:{name}"),
            &name,
            checked,
            true,
        ))?;
    }
    Ok(menu)
}

fn model_menu(cfg: &Config) -> Result<Submenu, Box<dyn std::error::Error>> {
    let menu = Submenu::with_id(
        "model",
        if cfg.mode.is_poor() {
            "Chat Model (Dictate + Chat mode only)"
        } else {
            "Chat Model"
        },
        !cfg.mode.is_poor(),
    );
    if cfg.model.choices.is_empty() {
        let item = MenuItem::with_id("model_none", "No configured models", false, None);
        menu.append(&item)?;
        return Ok(menu);
    }
    for choice in &cfg.model.choices {
        menu.append(&selectable_item(
            group::MODEL,
            format!("model:{}", choice.id),
            &choice.label,
            choice.id == cfg.model.active,
            true,
        ))?;
    }
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id(
        "model_restart_note",
        "Restart to Apply",
        false,
        None,
    ))?;
    Ok(menu)
}

fn language_menu(cfg: &Config) -> Result<Submenu, Box<dyn std::error::Error>> {
    let menu = Submenu::with_id("language", "Output Language", true);
    for language in &cfg.language.options {
        menu.append(&selectable_item(
            group::LANGUAGE,
            format!("lang:{language}"),
            language,
            language == &cfg.language.target,
            true,
        ))?;
    }
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id(
        "language_restart_note",
        "Restart to Apply",
        false,
        None,
    ))?;
    Ok(menu)
}

fn speech_menu(cfg: &Config) -> Result<Submenu, Box<dyn std::error::Error>> {
    let menu = Submenu::with_id("speech", "Speech Output", true);
    let backend = Submenu::with_id("speech_backend", "Backend", true);
    for (id, label) in visible_speech_backends() {
        backend.append(&selectable_item(
            group::SPEECH_BACKEND,
            format!("speech_backend:{id}"),
            label,
            cfg.speech.backend == id,
            true,
        ))?;
    }
    menu.append(&backend)?;

    let say_voice = Submenu::with_id("say_voice", "macOS Voice", true);
    say_voice.append(&selectable_item(
        group::SAY_VOICE,
        "speech_voice:",
        "System Default",
        cfg.speech.voice.is_none(),
        true,
    ))?;
    for voice in say_voices(cfg.speech.voice.as_deref()) {
        let selected = cfg.speech.voice.as_deref() == Some(voice.as_str());
        say_voice.append(&selectable_item(
            group::SAY_VOICE,
            format!("speech_voice:{voice}"),
            say_voice_label(&voice),
            selected,
            true,
        ))?;
    }
    menu.append(&say_voice)?;

    let kokoro = Submenu::with_id("kokoro_voice", "Kokoro Speaker", true);
    for voice in kokoro_voices() {
        kokoro.append(&selectable_item(
            group::KOKORO_SID,
            format!("kokoro_sid:{}", voice.sid),
            voice.label(),
            cfg.speech.kokoro.sid == voice.sid,
            true,
        ))?;
    }
    menu.append(&kokoro)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id(
        "speech_apply_note",
        "Applies to Next Response",
        false,
        None,
    ))?;

    Ok(menu)
}

fn visible_speech_backends() -> [(&'static str, &'static str); 2] {
    [("say", "macOS Voice"), ("kokoro", "Kokoro")]
}

/// Move the tick within a radio-style group to `selected_id`. Uses the real
/// macOS checkmark gutter, so labels no longer shift when selection changes.
pub fn select_menu_item(group: &'static str, selected_id: &str) {
    SELECTABLE_MENU_ITEMS.with(|items| {
        for item in items.borrow().iter().filter(|item| item.group == group) {
            item.item.set_checked(item.id == selected_id);
        }
    });
}

/// Capability-first mode names used throughout the menu. "GPU Poor/Rich" was
/// technically descriptive but made it unclear whether chat was available.
pub(crate) fn mode_name(tier: &str) -> &'static str {
    if tier == "poor" {
        "Dictation Only"
    } else {
        "Dictate + Chat"
    }
}

pub(crate) fn mode_memory(tier: &str) -> &'static str {
    if tier == "poor" {
        "~0.65 GB memory"
    } else {
        "~4 GB memory"
    }
}

pub(crate) fn mode_switch_text(active_tier: &str, selected_tier: &str) -> (String, String) {
    if selected_tier == active_tier {
        (
            format!("Mode: {}", mode_name(active_tier)),
            format!("Running now · {}", mode_memory(active_tier)),
        )
    } else {
        (
            format!("Next launch: {}", mode_name(selected_tier)),
            format!("Restart required · currently {}", mode_name(active_tier)),
        )
    }
}

fn clear_selectable_menu_items() {
    SELECTABLE_MENU_ITEMS.with(|items| items.borrow_mut().clear());
}

/// Build one radio-style option and register it so `select_menu_item` can move
/// the tick later. Every toggle group goes through here, so no group can be left
/// unregistered and silently stop updating its checkmark.
fn selectable_item(
    group: &'static str,
    id: impl Into<String>,
    label: impl AsRef<str>,
    selected: bool,
    enabled: bool,
) -> CheckMenuItem {
    let id = id.into();
    let item = CheckMenuItem::with_id(id.clone(), label, enabled, selected, None);
    SELECTABLE_MENU_ITEMS.with(|items| {
        items.borrow_mut().push(SelectableMenuItem {
            group,
            id,
            item: item.clone(),
        });
    });
    item
}

fn log_label(cfg: &Config) -> String {
    if cfg.logging.enabled {
        format!("Logs: {}", cfg.logging.path)
    } else {
        "Logs: Disabled".to_string()
    }
}

fn say_voices(current: Option<&str>) -> Vec<String> {
    let Ok(output) = Command::new("/usr/bin/say").arg("-v").arg("?").output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut voices = text
        .lines()
        .filter_map(say_voice_name)
        .filter(|voice| is_preferred_say_voice(voice))
        .collect::<Vec<_>>();
    voices.sort();
    voices.dedup();
    if let Some(current) = current {
        if !current.is_empty() && !voices.iter().any(|voice| voice == current) {
            voices.push(current.to_string());
        }
    }
    voices
}

fn kokoro_voices() -> &'static [KokoroVoice] {
    &KOKORO_VOICES
}

const KOKORO_VOICES: [KokoroVoice; 53] = [
    KokoroVoice::new(0, "Alloy", "American Female"),
    KokoroVoice::new(1, "Aoede", "American Female"),
    KokoroVoice::new(2, "Bella", "American Female"),
    KokoroVoice::new(3, "Heart", "American Female"),
    KokoroVoice::new(4, "Jessica", "American Female"),
    KokoroVoice::new(5, "Kore", "American Female"),
    KokoroVoice::new(6, "Nicole", "American Female"),
    KokoroVoice::new(7, "Nova", "American Female"),
    KokoroVoice::new(8, "River", "American Female"),
    KokoroVoice::new(9, "Sarah", "American Female"),
    KokoroVoice::new(10, "Sky", "American Female"),
    KokoroVoice::new(11, "Adam", "American Male"),
    KokoroVoice::new(12, "Echo", "American Male"),
    KokoroVoice::new(13, "Eric", "American Male"),
    KokoroVoice::new(14, "Fenrir", "American Male"),
    KokoroVoice::new(15, "Liam", "American Male"),
    KokoroVoice::new(16, "Michael", "American Male"),
    KokoroVoice::new(17, "Onyx", "American Male"),
    KokoroVoice::new(18, "Puck", "American Male"),
    KokoroVoice::new(19, "Santa", "American Male"),
    KokoroVoice::new(20, "Alice", "British Female"),
    KokoroVoice::new(21, "Emma", "British Female"),
    KokoroVoice::new(22, "Isabella", "British Female"),
    KokoroVoice::new(23, "Lily", "British Female"),
    KokoroVoice::new(24, "Daniel", "British Male"),
    KokoroVoice::new(25, "Fable", "British Male"),
    KokoroVoice::new(26, "George", "British Male"),
    KokoroVoice::new(27, "Lewis", "British Male"),
    KokoroVoice::new(28, "Dora", "Spanish Female"),
    KokoroVoice::new(29, "Alex", "Spanish Male"),
    KokoroVoice::new(30, "Siwis", "French Female"),
    KokoroVoice::new(31, "Alpha", "Hindi Female"),
    KokoroVoice::new(32, "Beta", "Hindi Female"),
    KokoroVoice::new(33, "Omega", "Hindi Male"),
    KokoroVoice::new(34, "Psi", "Hindi Male"),
    KokoroVoice::new(35, "Sara", "Italian Female"),
    KokoroVoice::new(36, "Nicola", "Italian Male"),
    KokoroVoice::new(37, "Alpha", "Japanese Female"),
    KokoroVoice::new(38, "Gongitsune", "Japanese Female"),
    KokoroVoice::new(39, "Nezumi", "Japanese Female"),
    KokoroVoice::new(40, "Tebukuro", "Japanese Female"),
    KokoroVoice::new(41, "Kumo", "Japanese Male"),
    KokoroVoice::new(42, "Dora", "Brazilian Portuguese Female"),
    KokoroVoice::new(43, "Alex", "Brazilian Portuguese Male"),
    KokoroVoice::new(44, "Santa", "Brazilian Portuguese Male"),
    KokoroVoice::new(45, "Xiaobei", "Chinese Female"),
    KokoroVoice::new(46, "Xiaoni", "Chinese Female"),
    KokoroVoice::new(47, "Xiaoxiao", "Chinese Female"),
    KokoroVoice::new(48, "Xiaoyi", "Chinese Female"),
    KokoroVoice::new(49, "Yunjian", "Chinese Male"),
    KokoroVoice::new(50, "Yunxi", "Chinese Male"),
    KokoroVoice::new(51, "Yunxia", "Chinese Male"),
    KokoroVoice::new(52, "Yunyang", "Chinese Male"),
];

struct KokoroVoice {
    sid: i32,
    name: &'static str,
    description: &'static str,
}

impl KokoroVoice {
    const fn new(sid: i32, name: &'static str, description: &'static str) -> Self {
        Self {
            sid,
            name,
            description,
        }
    }

    fn label(&self) -> String {
        format!("{} - {} ({})", self.name, self.description, self.sid)
    }
}

fn say_voice_name(line: &str) -> Option<String> {
    let left = line.split('#').next()?.trim_end();
    let (name, locale) = left.rsplit_once(char::is_whitespace)?;
    if locale.len() == 5 && locale.as_bytes().get(2) == Some(&b'_') {
        Some(name.trim().to_string())
    } else {
        None
    }
}

fn is_preferred_say_voice(voice: &str) -> bool {
    voice.contains("(Premium)")
        || matches!(
            voice,
            "Eddy (English (US))"
                | "Flo (English (US))"
                | "Reed (English (US))"
                | "Rocko (English (US))"
                | "Sandy (English (US))"
                | "Shelley (English (US))"
        )
}

fn say_voice_label(voice: &str) -> String {
    voice
        .replace(" (Premium)", " - Premium")
        .replace(" (English (US))", " - English US")
}

pub(crate) fn status_label(state: u8) -> &'static str {
    match state {
        RECORDING_DICTATE => "Status: Listening for dictation",
        RECORDING_CHAT => "Status: Listening for chat",
        TRANSCRIBING => "Status: Transcribing",
        ANSWERING => "Status: Answering",
        SPEAKING => "Status: Speaking",
        NOTICE => "Status: Needs Input/Access/Mic",
        ERROR => "Status: Error; see log",
        BACKEND_DOWN => "Status: Backend stopped; quit and reopen Yappr",
        SETUP_FAILED => "Status: Setup failed; reopen Yappr to resume",
        PROVISIONING_MODEL => "Status: Downloading model…",
        PROVISIONING_ENGINE => "Status: Installing engine…",
        STARTING => "Status: Starting…",
        _ => "Status: Ready",
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddTimer(rl: *mut c_void, timer: *mut c_void, mode: *const c_void);
    fn CFRunLoopTimerCreate(
        allocator: *const c_void,
        fire_date: f64,
        interval: f64,
        flags: u64,
        order: isize,
        callout: extern "C" fn(*mut c_void, *mut c_void),
        context: *const c_void,
    ) -> *mut c_void;
}

#[repr(C)]
struct TimerContext {
    version: isize,
    info: *mut c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    copy_description: Option<extern "C" fn(*const c_void) -> *const c_void>,
}

#[cfg(test)]
mod tests {
    use super::{
        group, is_preferred_say_voice, kokoro_voices, log_label, mode_switch_text, say_voice_label,
        say_voice_name, visible_speech_backends,
    };

    /// Menu item ids are `"<prefix>:<value>"` and the group name is the prefix, so
    /// `select_menu_item(group, id)` only matches when the two agree. Registering
    /// mic/model/language under the wrong prefix is exactly how their checkmarks
    /// silently stopped moving before.
    #[test]
    fn group_names_match_their_menu_id_prefix() {
        for group in [
            group::MIC,
            group::MODEL,
            group::LANGUAGE,
            group::SPEECH_BACKEND,
            group::SAY_VOICE,
            group::KOKORO_SID,
        ] {
            assert!(
                !group.contains(':'),
                "group {group} must be the bare prefix, without the colon"
            );
        }
        // The ids built in the menu constructors, spelled out here so a rename on
        // one side without the other fails the build's test run rather than at runtime.
        assert_eq!(group::MIC, "mic");
        assert_eq!(group::MODEL, "model");
        assert_eq!(group::LANGUAGE, "lang");
        assert_eq!(group::SPEECH_BACKEND, "speech_backend");
        assert_eq!(group::SAY_VOICE, "speech_voice");
        assert_eq!(group::KOKORO_SID, "kokoro_sid");
    }

    #[test]
    fn mode_switch_distinguishes_running_and_next_launch_modes() {
        assert_eq!(
            mode_switch_text("rich", "rich"),
            (
                "Mode: Dictate + Chat".to_string(),
                "Running now · ~4 GB memory".to_string()
            )
        );
        assert_eq!(
            mode_switch_text("rich", "poor"),
            (
                "Next launch: Dictation Only".to_string(),
                "Restart required · currently Dictate + Chat".to_string()
            )
        );
    }
    use crate::config::{
        AsrConfig, AudioConfig, ChatConfig, Config, KokoroConfig, LanguageConfig, LoggingConfig,
        ModeConfig, ModelConfig, SearchConfig, ServerConfig, SpeechConfig, SupertonicConfig,
        VadConfig,
    };

    #[test]
    fn parses_say_voice_names_with_spaces() {
        assert_eq!(
            say_voice_name("Ava (Premium)       en_US    # Hello").as_deref(),
            Some("Ava (Premium)")
        );
        assert_eq!(
            say_voice_name("Eddy (English (US)) en_US    # Hello").as_deref(),
            Some("Eddy (English (US))")
        );
    }

    #[test]
    fn keeps_only_premium_or_neural_say_voices() {
        assert!(is_preferred_say_voice("Ava (Premium)"));
        assert!(is_preferred_say_voice("Eddy (English (US))"));
        assert!(!is_preferred_say_voice("Cellos"));
        assert!(!is_preferred_say_voice("Eddy (German (Germany))"));
    }

    #[test]
    fn formats_macos_voice_labels_for_menu() {
        assert_eq!(
            say_voice_label("Sandy (English (US))"),
            "Sandy - English US"
        );
        assert_eq!(say_voice_label("Ava (Premium)"), "Ava - Premium");
    }

    #[test]
    fn exposes_kokoro_voice_descriptions() {
        let voices = kokoro_voices();

        assert_eq!(voices.len(), 53);
        assert_eq!(voices[0].label(), "Alloy - American Female (0)");
        assert_eq!(voices[52].label(), "Yunyang - Chinese Male (52)");
    }

    #[test]
    fn only_exposes_supported_primary_speech_backends() {
        assert_eq!(
            visible_speech_backends(),
            [("say", "macOS Voice"), ("kokoro", "Kokoro")]
        );
    }

    #[test]
    fn log_menu_uses_effective_config() {
        let mut cfg = test_config();
        assert_eq!(log_label(&cfg), "Logs: /tmp/yappr.log");

        cfg.logging.enabled = false;
        assert_eq!(log_label(&cfg), "Logs: Disabled");
    }

    fn test_config() -> Config {
        Config {
            mode: ModeConfig {
                tier: "rich".to_string(),
            },
            asr: AsrConfig {
                repo: String::new(),
                release: String::new(),
                archive: String::new(),
                model_dir: String::new(),
            },
            server: ServerConfig {
                endpoint: String::new(),
                port: 0,
                manage: false,
                binary: String::new(),
                timeout_secs: 0,
            },
            model: ModelConfig {
                repo: String::new(),
                weights: String::new(),
                mmproj: String::new(),
                ctx_size: String::new(),
                ngl: String::new(),
                active: String::new(),
                choices: Vec::new(),
            },
            audio: AudioConfig {
                device: None,
                samplerate: 16000,
                max_seconds: 0.0,
                tail_seconds: 0.0,
            },
            vad: VadConfig {
                enabled: true,
                threshold: 0.5,
                min_speech_duration_ms: 250,
                min_silence_duration_ms: 100,
                speech_pad_ms: 30,
            },
            language: LanguageConfig {
                source: String::new(),
                target: String::new(),
                options: Vec::new(),
            },
            chat: ChatConfig { context_seconds: 0 },
            speech: SpeechConfig {
                backend: String::new(),
                kokoro: KokoroConfig {
                    model_dir: String::new(),
                    sid: 0,
                    speed: 1.0,
                    lang: String::new(),
                    threads: 0,
                },
                supertonic: SupertonicConfig {
                    model_dir: String::new(),
                    sid: 0,
                    speed: 1.0,
                    lang: String::new(),
                    steps: 0,
                    threads: 0,
                },
                voice: None,
                rate: 0,
            },
            logging: LoggingConfig {
                enabled: true,
                debug: false,
                path: "/tmp/yappr.log".to_string(),
            },
            search: SearchConfig {
                enabled: false,
                endpoint: String::new(),
                max_results: 0,
                timeout_secs: 0,
            },
        }
    }
}
