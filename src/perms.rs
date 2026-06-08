#[cfg(target_os = "macos")]
mod macos {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOHIDCheckAccess(request_type: u32) -> i32;
    }

    const K_IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;

    pub fn input_monitoring_status() -> &'static str {
        match unsafe { IOHIDCheckAccess(K_IOHID_REQUEST_TYPE_LISTEN_EVENT) } {
            0 => "granted",
            1 => "denied",
            2 => "unknown",
            _ => "unknown",
        }
    }

    pub fn accessibility_status() -> &'static str {
        if unsafe { AXIsProcessTrusted() } {
            "granted"
        } else {
            "not granted"
        }
    }

    pub fn microphone_status() -> String {
        crate::audio::input_device_status()
    }
}

#[cfg(not(target_os = "macos"))]
mod macos {
    pub fn input_monitoring_status() -> &'static str {
        "unsupported"
    }
    pub fn accessibility_status() -> &'static str {
        "unsupported"
    }
    pub fn microphone_status() -> String {
        "unsupported".to_string()
    }
}

pub use macos::{accessibility_status, input_monitoring_status, microphone_status};
