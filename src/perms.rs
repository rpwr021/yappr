#[cfg(target_os = "macos")]
mod macos {
    use objc2::runtime::Bool;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOHIDCheckAccess(request_type: u32) -> i32;
    }

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        // AVMediaType is a typedef for NSString *; this is the audio constant.
        static AVMediaTypeAudio: *const NSString;
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

    /// Real microphone TCC authorization via AVCaptureDevice, NOT device
    /// enumeration. Enumeration succeeds even when access is denied (macOS then
    /// feeds silent buffers), so this is the only reliable signal.
    /// AVAuthorizationStatus: 0=notDetermined 1=restricted 2=denied 3=authorized.
    pub fn microphone_authorization() -> &'static str {
        let status: isize = unsafe {
            let cls = class!(AVCaptureDevice);
            msg_send![cls, authorizationStatusForMediaType: AVMediaTypeAudio]
        };
        match status {
            0 => "not_determined",
            1 => "restricted",
            2 => "denied",
            3 => "authorized",
            _ => "unknown",
        }
    }

    /// Ask macOS for microphone access. If status is not-determined this shows
    /// the system prompt; otherwise it's a no-op. Returns immediately; the grant
    /// resolves asynchronously, so callers should re-check `microphone_authorization`.
    pub fn request_microphone_access() {
        if microphone_authorization() != "not_determined" {
            return;
        }
        let handler = block2::RcBlock::new(|_granted: Bool| {});
        unsafe {
            let cls = class!(AVCaptureDevice);
            let _: () = msg_send![
                cls,
                requestAccessForMediaType: AVMediaTypeAudio,
                completionHandler: &*handler,
            ];
        }
    }

    /// Device-level description (name/format), for diagnostics only. Does not
    /// reflect TCC authorization.
    pub fn microphone_device(_unused: ()) -> String {
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
    pub fn microphone_authorization() -> &'static str {
        "unsupported"
    }
    pub fn request_microphone_access() {}
    pub fn microphone_device(_unused: ()) -> String {
        "unsupported".to_string()
    }
}

pub struct PermissionReport {
    pub input_monitoring: String,
    pub accessibility: String,
    /// Real TCC authorization: authorized/denied/not_determined/restricted.
    pub microphone: String,
    /// Device name + format, for the log only.
    pub microphone_device: String,
}

impl PermissionReport {
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.input_monitoring != "granted" {
            missing.push("Input Monitoring");
        }
        if self.accessibility != "granted" {
            missing.push("Accessibility");
        }
        if self.microphone != "authorized" {
            missing.push("Microphone");
        }
        missing
    }

    pub fn log_summary(&self) -> String {
        let missing = self.missing();
        if missing.is_empty() {
            "permissions ok: Input Monitoring, Accessibility, Microphone".to_string()
        } else {
            format!(
                "permissions missing: {}; input_monitoring={}, accessibility={}, microphone={} ({})",
                missing.join(", "),
                self.input_monitoring,
                self.accessibility,
                self.microphone,
                self.microphone_device
            )
        }
    }
}

pub fn report() -> PermissionReport {
    PermissionReport {
        microphone_device: macos::microphone_device(()),
        ..grants()
    }
}

/// The three TCC grants without the device description. Cheap: three status
/// syscalls, no CoreAudio. `report()` additionally queries the input device,
/// which can take up to a timeout, so anything on the UI thread wants this.
pub fn grants() -> PermissionReport {
    PermissionReport {
        input_monitoring: input_monitoring_status().to_string(),
        accessibility: accessibility_status().to_string(),
        microphone: microphone_authorization().to_string(),
        microphone_device: String::new(),
    }
}

pub use macos::{
    accessibility_status, input_monitoring_status, microphone_authorization,
    request_microphone_access,
};

#[cfg(test)]
mod tests {
    use super::PermissionReport;

    fn report(mic: &str) -> PermissionReport {
        PermissionReport {
            input_monitoring: "granted".to_string(),
            accessibility: "granted".to_string(),
            microphone: mic.to_string(),
            microphone_device: "available (Mic; F32, 1 ch, 48000 Hz)".to_string(),
        }
    }

    #[test]
    fn only_the_actually_missing_permission_is_named() {
        // The real case from a user's machine: mic authorized, the other two not.
        // The tray used to say "Needs Input/Access/Mic" regardless, sending them
        // to re-grant a Microphone permission that was already fine.
        let report = PermissionReport {
            input_monitoring: "denied".to_string(),
            accessibility: "not granted".to_string(),
            microphone: "authorized".to_string(),
            microphone_device: String::new(),
        };
        assert_eq!(report.missing(), vec!["Input Monitoring", "Accessibility"]);
        assert!(!report.missing().contains(&"Microphone"));
    }

    #[test]
    fn denied_microphone_is_reported_missing() {
        // The bug: device enumeration said "available" while TCC was denied.
        // Now a non-authorized status must surface as missing.
        assert_eq!(report("denied").missing(), ["Microphone"]);
        assert_eq!(report("not_determined").missing(), ["Microphone"]);
        assert!(report("denied").log_summary().contains("permissions missing"));
    }

    #[test]
    fn authorized_microphone_is_ok() {
        assert!(report("authorized").missing().is_empty());
        assert_eq!(
            report("authorized").log_summary(),
            "permissions ok: Input Monitoring, Accessibility, Microphone"
        );
    }

    #[test]
    fn missing_event_permissions_listed() {
        let mut r = report("authorized");
        r.input_monitoring = "denied".to_string();
        r.accessibility = "not granted".to_string();
        assert_eq!(r.missing(), ["Input Monitoring", "Accessibility"]);
    }
}
