use crate::logger::log_line;
use crate::runtime::{self, Runtime};
use crate::ui;
use std::ffi::c_void;
use std::sync::Arc;

pub fn run(runtime: Arc<Runtime>) -> Result<(), Box<dyn std::error::Error>> {
    init_appkit()?;
    let _ = runtime::install(runtime);
    let runtime = runtime::runtime().ok_or("runtime unavailable")?;
    let status_item = Box::leak(Box::new(ui::create_status_item(&runtime.menu_config)?));
    ui::install_animation_timer(status_item);
    unsafe {
        let mask = 1_u64 << K_CG_EVENT_FLAGS_CHANGED;
        let tap = CGEventTapCreate(
            K_CG_SESSION_EVENT_TAP,
            K_CG_HEAD_INSERT_EVENT_TAP,
            K_CG_EVENT_TAP_OPTION_DEFAULT,
            mask,
            event_callback,
            std::ptr::null_mut(),
        );
        if tap.is_null() {
            return Err("failed to create CGEventTap; grant Input Monitoring to Yappr".into());
        }
        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
        if source.is_null() {
            return Err("failed to create run-loop source for event tap".into());
        }
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CGEventTapEnable(tap, true);
        log_line("Yappr ready: hold Right Option to dictate; hold Cmd+Right Option to chat");
    }
    run_appkit();
    Ok(())
}

#[cfg(target_os = "macos")]
fn init_appkit() -> Result<(), Box<dyn std::error::Error>> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    let mtm = MainThreadMarker::new().ok_or("app mode must run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_appkit() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.finishLaunching();
        app.run();
    }
}

#[cfg(not(target_os = "macos"))]
fn init_appkit() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn run_appkit() {}

extern "C" fn event_callback(
    _proxy: *mut c_void,
    event_type: u32,
    event: *mut c_void,
    _user_info: *mut c_void,
) -> *mut c_void {
    if event_type == K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT
        || event_type == K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT
    {
        return event;
    }
    if event_type == K_CG_EVENT_FLAGS_CHANGED {
        let keycode = unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
        if keycode == RIGHT_OPTION_KEYCODE {
            let flags = unsafe { CGEventGetFlags(event) };
            let option_down = flags & K_CG_EVENT_FLAG_MASK_ALTERNATE != 0;
            let command_down = flags & K_CG_EVENT_FLAG_MASK_COMMAND != 0;
            log_line(format!(
                "hotkey event: right_option={} command={}",
                if option_down { "down" } else { "up" },
                command_down
            ));
            if let Some(runtime) = runtime::runtime() {
                if option_down {
                    runtime.hotkey_down(command_down);
                } else {
                    runtime.hotkey_up();
                }
            }
        }
    }
    event
}

const RIGHT_OPTION_KEYCODE: i64 = 61;
const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFFFFFE;
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFFFFFF;
const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const K_CG_EVENT_FLAG_MASK_ALTERNATE: u64 = 1 << 19;
const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 1 << 20;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    fn CGEventGetFlags(event: *mut c_void) -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: *mut c_void,
        order: isize,
    ) -> *mut c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, source: *mut c_void, mode: *const c_void);
}
