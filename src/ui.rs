use crate::audio;
use crate::config::Config;
use crate::logger::log_line;
use crate::mascot::icon_for_state;
use std::cell::RefCell;
use std::ffi::c_void;
use std::process::Command;
use std::sync::atomic::Ordering;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
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

thread_local! {
    static SELECTABLE_MENU_ITEMS: RefCell<Vec<SelectableMenuItem>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
struct SelectableMenuItem {
    group: &'static str,
    id: String,
    label: String,
    item: MenuItem,
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
    let chat_hint = MenuItem::with_id("hint_chat", "Chat: hold ⌘ + Right Option", false, None);
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

    let tray = TrayIconBuilder::new()
        .with_icon(icon_for_state(IDLE, 0)?)
        .with_title(" ")
        .with_tooltip("Yappr: hold Right Option to dictate")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .with_menu_on_right_click(true)
        .build()?;
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

fn microphone_menu(cfg: &Config) -> Result<Submenu, Box<dyn std::error::Error>> {
    let menu = Submenu::with_id("microphone", "Microphone", true);
    let default = MenuItem::with_id(
        "mic:",
        selected_label("System Default", cfg.audio.device.is_none()),
        true,
        None,
    );
    menu.append(&default)?;
    for name in audio::input_devices() {
        let checked = cfg.audio.device.as_deref() == Some(name.as_str());
        let item = MenuItem::with_id(
            format!("mic:{name}"),
            selected_label(&name, checked),
            true,
            None,
        );
        menu.append(&item)?;
    }
    Ok(menu)
}

fn model_menu(cfg: &Config) -> Result<Submenu, Box<dyn std::error::Error>> {
    let menu = Submenu::with_id("model", "Model", true);
    if cfg.model.choices.is_empty() {
        let item = MenuItem::with_id("model_none", "No configured models", false, None);
        menu.append(&item)?;
        return Ok(menu);
    }
    for choice in &cfg.model.choices {
        let item = MenuItem::with_id(
            format!("model:{}", choice.id),
            selected_label(&choice.label, choice.id == cfg.model.active),
            true,
            None,
        );
        menu.append(&item)?;
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
        let item = MenuItem::with_id(
            format!("lang:{language}"),
            selected_label(language, language == &cfg.language.target),
            true,
            None,
        );
        menu.append(&item)?;
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
    for (id, label) in [
        ("supertonic", "Supertonic 3"),
        ("kokoro", "Kokoro"),
        ("say", "macOS Say"),
    ] {
        let item_id = format!("speech_backend:{id}");
        let item = MenuItem::with_id(
            &item_id,
            selected_label(label, cfg.speech.backend == id),
            true,
            None,
        );
        remember_selectable("speech_backend", item_id, label, &item);
        backend.append(&item)?;
    }
    menu.append(&backend)?;

    let say_voice = Submenu::with_id("say_voice", "macOS Voice", true);
    let system_voice = MenuItem::with_id(
        "speech_voice:",
        selected_label("System Default", cfg.speech.voice.is_none()),
        true,
        None,
    );
    remember_selectable(
        "speech_voice",
        "speech_voice:",
        "System Default",
        &system_voice,
    );
    say_voice.append(&system_voice)?;
    for voice in say_voices(cfg.speech.voice.as_deref()) {
        let item_id = format!("speech_voice:{voice}");
        let label = say_voice_label(&voice);
        let item = MenuItem::with_id(
            &item_id,
            selected_label(&label, cfg.speech.voice.as_deref() == Some(voice.as_str())),
            true,
            None,
        );
        remember_selectable("speech_voice", item_id, label, &item);
        say_voice.append(&item)?;
    }
    menu.append(&say_voice)?;

    let supertonic = Submenu::with_id("supertonic_voice", "Supertonic Voice", true);
    let supertonic_default = MenuItem::with_id(
        "supertonic_sid:0",
        selected_label("Default", cfg.speech.supertonic.sid == 0),
        true,
        None,
    );
    remember_selectable(
        "supertonic_sid",
        "supertonic_sid:0",
        "Default",
        &supertonic_default,
    );
    supertonic.append(&supertonic_default)?;
    menu.append(&supertonic)?;

    let kokoro = Submenu::with_id("kokoro_voice", "Kokoro Speaker", true);
    for voice in kokoro_voices() {
        let item_id = format!("kokoro_sid:{}", voice.sid);
        let label = voice.label();
        let item = MenuItem::with_id(
            &item_id,
            selected_label(&label, cfg.speech.kokoro.sid == voice.sid),
            true,
            None,
        );
        remember_selectable("kokoro_sid", item_id, label, &item);
        kokoro.append(&item)?;
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

pub fn select_menu_item(group: &'static str, selected_id: &str) {
    SELECTABLE_MENU_ITEMS.with(|items| {
        for item in items.borrow().iter().filter(|item| item.group == group) {
            item.item
                .set_text(selected_label(&item.label, item.id == selected_id));
        }
    });
}

fn clear_selectable_menu_items() {
    SELECTABLE_MENU_ITEMS.with(|items| items.borrow_mut().clear());
}

fn remember_selectable(
    group: &'static str,
    id: impl Into<String>,
    label: impl Into<String>,
    item: &MenuItem,
) {
    SELECTABLE_MENU_ITEMS.with(|items| {
        items.borrow_mut().push(SelectableMenuItem {
            group,
            id: id.into(),
            label: label.into(),
            item: item.clone(),
        });
    });
}

fn selected_label(label: &str, selected: bool) -> String {
    if selected {
        format!("✓ {label}")
    } else {
        label.to_string()
    }
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

fn status_text(state: u8) -> &'static str {
    match state {
        RECORDING_DICTATE => "Status: Listening for dictation",
        RECORDING_CHAT => "Status: Listening for chat",
        TRANSCRIBING => "Status: Transcribing",
        ANSWERING => "Status: Answering",
        SPEAKING => "Status: Speaking",
        NOTICE => "Status: Needs Input/Access/Mic",
        ERROR => "Status: Error; see log",
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
        is_preferred_say_voice, kokoro_voices, log_label, say_voice_label, say_voice_name,
    };
    use crate::config::{
        AudioConfig, ChatConfig, Config, KokoroConfig, LanguageConfig, LoggingConfig, ModelConfig,
        SearchConfig, ServerConfig, SpeechConfig, SupertonicConfig, VadConfig,
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
    fn log_menu_uses_effective_config() {
        let mut cfg = test_config();
        assert_eq!(log_label(&cfg), "Logs: /tmp/yappr.log");

        cfg.logging.enabled = false;
        assert_eq!(log_label(&cfg), "Logs: Disabled");
    }

    fn test_config() -> Config {
        Config {
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
