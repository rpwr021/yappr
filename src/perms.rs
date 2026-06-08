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

pub struct PermissionReport {
    pub input_monitoring: String,
    pub accessibility: String,
    pub microphone: String,
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
        if !self.microphone.starts_with("available ") {
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
                "permissions missing: {}; input_monitoring={}, accessibility={}, microphone={}",
                missing.join(", "),
                self.input_monitoring,
                self.accessibility,
                self.microphone
            )
        }
    }
}

pub fn report() -> PermissionReport {
    PermissionReport {
        input_monitoring: input_monitoring_status().to_string(),
        accessibility: accessibility_status().to_string(),
        microphone: microphone_status(),
    }
}

pub use macos::{accessibility_status, input_monitoring_status, microphone_status};

#[cfg(test)]
mod tests {
    use super::PermissionReport;

    #[test]
    fn reports_missing_permission_names() {
        let report = PermissionReport {
            input_monitoring: "denied".to_string(),
            accessibility: "not granted".to_string(),
            microphone: "available (MacBook Pro Microphone; F32, 1 ch, 96000 Hz)".to_string(),
        };

        assert_eq!(report.missing(), ["Input Monitoring", "Accessibility"]);
        assert!(report.log_summary().contains("permissions missing"));
    }

    #[test]
    fn reports_all_permissions_ok() {
        let report = PermissionReport {
            input_monitoring: "granted".to_string(),
            accessibility: "granted".to_string(),
            microphone: "available (MacBook Pro Microphone; F32, 1 ch, 96000 Hz)".to_string(),
        };

        assert!(report.missing().is_empty());
        assert_eq!(
            report.log_summary(),
            "permissions ok: Input Monitoring, Accessibility, Microphone"
        );
    }
}
