use crate::audio;
use crate::config::Config;
use crate::logger::log_line;
use crate::mascot::icon_for_state;
use std::ffi::c_void;
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

pub struct StatusItem {
    tray: TrayIcon,
    status: MenuItem,
    frame: usize,
    last_state: u8,
}

pub fn create_status_item(cfg: &Config) -> Result<StatusItem, Box<dyn std::error::Error>> {
    let menu = Menu::new();
    let status = MenuItem::with_id("status", "Status: Ready", false, None);
    let microphone = microphone_menu(cfg)?;
    let model = model_menu(cfg)?;
    let language = language_menu(cfg)?;
    let copy = MenuItem::with_id("copy_transcript", "Copy Last Transcript", true, None);
    let logs = MenuItem::with_id("logs", "Logs: ~/.yappr/yappr.log", false, None);
    let quit = MenuItem::with_id("quit", "Quit", true, None);
    let separator = PredefinedMenuItem::separator();
    menu.append_items(&[
        &status,
        &microphone,
        &model,
        &language,
        &copy,
        &logs,
        &separator,
        &quit,
    ])?;

    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        log_line(format!("menu event: {}", event.id.0));
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
            &context,
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
    let should_draw = state != IDLE || state != item.last_state;
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
    let default = CheckMenuItem::with_id(
        "mic:",
        "System Default",
        true,
        cfg.audio.device.is_none(),
        None,
    );
    menu.append(&default)?;
    for name in audio::input_devices() {
        let checked = cfg.audio.device.as_deref() == Some(name.as_str());
        let item = CheckMenuItem::with_id(format!("mic:{name}"), name, true, checked, None);
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
        let item = CheckMenuItem::with_id(
            format!("model:{}", choice.id),
            &choice.label,
            true,
            choice.id == cfg.model.active,
            None,
        );
        menu.append(&item)?;
    }
    Ok(menu)
}

fn language_menu(cfg: &Config) -> Result<Submenu, Box<dyn std::error::Error>> {
    let menu = Submenu::with_id("language", "Output Language", true);
    for language in &cfg.language.options {
        let item = CheckMenuItem::with_id(
            format!("lang:{language}"),
            language,
            true,
            language == &cfg.language.target,
            None,
        );
        menu.append(&item)?;
    }
    Ok(menu)
}

fn status_text(state: u8) -> &'static str {
    match state {
        RECORDING_DICTATE => "Status: Listening for dictation",
        RECORDING_CHAT => "Status: Listening for chat",
        TRANSCRIBING => "Status: Transcribing",
        ANSWERING => "Status: Answering",
        SPEAKING => "Status: Speaking",
        ERROR => "Status: Error; see log",
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
        context: *const TimerContext,
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
