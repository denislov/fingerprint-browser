//! Reports the fingerprint switches a resolved core cannot honour.
//!
//! A typed profile is generated, not preserved: when the core cannot be shown
//! to support a switch, the serializer omits it. Omitting silently would let a
//! profile claim a fingerprint the engine never applies, so every omission is
//! reported here and surfaces in the runtime snapshot as `last_warning`.

use domain::{BrowserCore, BrowserProfile, CoreCapabilities, FingerprintGeneration};

/// Why a switch is not passed to the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Omission {
    /// The switch set was only verified from the pivot major onwards.
    UnverifiedGeneration { major: u32 },
    /// The capability table does not carry this switch for any generation.
    UnsupportedSwitch { major: u32 },
    /// `--fingerprint-brand` does not accept this brand on this core.
    UnsupportedBrand { brand: String, major: u32 },
}

/// One switch (or switch group) that will not reach the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Switch, or comma-joined group, that is omitted.
    pub switch: String,
    /// What in the profile asked for it.
    pub requested_by: String,
    /// Core the finding belongs to, as named in the catalogue.
    pub core: String,
    pub omission: Omission,
}

impl Finding {
    fn message(&self) -> String {
        let core = &self.core;
        match &self.omission {
            Omission::UnverifiedGeneration { major } => format!(
                "{core} (major {major}) is older than the verified fingerprint generation \
                 (major {}) and omits {}; requested by {}",
                FingerprintGeneration::PIVOT_MAJOR,
                self.switch,
                self.requested_by
            ),
            Omission::UnsupportedSwitch { major } => format!(
                "{core} (major {major}) does not support {}; requested by {}",
                self.switch, self.requested_by
            ),
            Omission::UnsupportedBrand { brand, major } => format!(
                "{core} (major {major}) does not declare brand {brand}; {} is omitted and the \
                 engine default applies",
                self.switch
            ),
        }
    }
}

/// Everything that will be missing from the launch, in a stable order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompatibilityReport {
    core: String,
    findings: Vec<Finding>,
}

impl CompatibilityReport {
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Single-line summary for [`crate::RuntimeEvent::Warning`].
    pub fn message(&self) -> Option<String> {
        if self.findings.is_empty() {
            return None;
        }
        Some(
            self.findings
                .iter()
                .map(Finding::message)
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    fn push(
        &mut self,
        switch: impl Into<String>,
        requested_by: impl Into<String>,
        omission: Omission,
    ) {
        self.findings.push(Finding {
            switch: switch.into(),
            requested_by: requested_by.into(),
            core: self.core.clone(),
            omission,
        });
    }
}

/// Checks every switch this profile would emit against the core's capabilities.
///
/// The order is fixed so the resulting warning is stable for the same inputs.
pub fn check(
    core: &BrowserCore,
    profile: &BrowserProfile,
    capabilities: &CoreCapabilities,
) -> CompatibilityReport {
    let mut report = CompatibilityReport {
        core: core.name.clone(),
        findings: Vec::new(),
    };
    let fingerprint = &profile.fingerprint;

    if !capabilities.supports_brand(fingerprint.brand) {
        report.push(
            "--fingerprint-brand",
            format!("profile brand {}", fingerprint.brand),
            Omission::UnsupportedBrand {
                brand: fingerprint.brand.to_string(),
                major: capabilities.major,
            },
        );
    }

    // Driven by the capability, not by the generation: the noise switch is
    // carried below the pivot too, and reading the generation here would report
    // an omission that does not happen.
    if !capabilities.supports_canvas_noise {
        report.push(
            "--fingerprinting-canvas-image-data-noise,--fingerprinting-client-rects-noise",
            "profile canvas and client-rects spoofing",
            if capabilities.is_verified() {
                Omission::UnsupportedSwitch {
                    major: capabilities.major,
                }
            } else {
                Omission::UnverifiedGeneration {
                    major: capabilities.major,
                }
            },
        );
    }

    if !capabilities.supports_brand_version && fingerprint.brand_version.is_some() {
        report.push(
            "--fingerprint-brand-version",
            "profile brand version",
            Omission::UnsupportedSwitch {
                major: capabilities.major,
            },
        );
    }

    if !capabilities.supports_platform_version && fingerprint.platform_version.is_some() {
        report.push(
            "--fingerprint-platform-version",
            "profile platform version",
            Omission::UnsupportedSwitch {
                major: capabilities.major,
            },
        );
    }

    if !capabilities.supports_disable_spoofing && !fingerprint.disabled_spoofing.is_empty() {
        report.push(
            "--disable-spoofing",
            format!(
                "profile exclusions ({})",
                fingerprint
                    .disabled_spoofing
                    .iter()
                    .map(|feature| feature.as_flag_name())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            // The exclusions are a verified-generation feature: a major below
            // the pivot accepts the switch and applies the noise anyway, so
            // "older than the verified generation" is the accurate reason.
            if capabilities.is_verified() {
                Omission::UnsupportedSwitch {
                    major: capabilities.major,
                }
            } else {
                Omission::UnverifiedGeneration {
                    major: capabilities.major,
                }
            },
        );
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        BrowserBrand, CoreId, FingerprintProfile, Platform, ProfileId, SpoofingFeature,
        StartTarget, WebRtcPolicy, WindowProfile,
    };
    use std::path::PathBuf;

    fn core(major: u32) -> BrowserCore {
        BrowserCore {
            id: CoreId::new(),
            name: format!("chromium {major}"),
            executable: PathBuf::from("chrome"),
            version: format!("{major}.0.0.0"),
            major,
        }
    }

    fn profile() -> BrowserProfile {
        BrowserProfile {
            id: ProfileId::new(),
            name: "Primary".to_string(),
            core_id: CoreId::new(),
            user_data_dir: PathBuf::from("data/profiles/primary"),
            fingerprint: FingerprintProfile {
                seed: 4242,
                brand: BrowserBrand::Chrome,
                brand_version: None,
                platform: Platform::Windows,
                platform_version: None,
                language: "en-US".to_string(),
                accept_language: "en-US,en;q=0.9".to_string(),
                timezone: "America/New_York".to_string(),
                hardware_concurrency: Some(8),
                webrtc_policy: WebRtcPolicy::DisableNonProxiedUdp,
                disabled_spoofing: Vec::new(),
            },
            proxy_id: None,
            window: WindowProfile::new(1920, 1080),
            start_target: StartTarget::Blank,
        }
    }

    #[test]
    fn the_verified_generation_reports_nothing() {
        let report = check(&core(148), &profile(), &CoreCapabilities::for_major(148));

        assert!(report.is_empty(), "{:?}", report.findings());
        assert_eq!(report.message(), None);
    }

    #[test]
    fn a_legacy_core_reports_the_exclusions_it_ignores() {
        // Measured on 142: the engine accepts `--disable-spoofing=canvas` and
        // applies the noise anyway, so the switch is omitted and reported.
        let mut profile = profile();
        profile.fingerprint.disabled_spoofing = vec![SpoofingFeature::Canvas];
        let report = check(&core(142), &profile, &CoreCapabilities::for_major(142));

        assert!(!report.is_empty());
        let message = report.message().expect("warning");
        assert!(message.contains("--disable-spoofing"), "{message}");
        assert!(message.contains("major 144"), "{message}");
        assert!(
            !message.contains("canvas-image-data-noise"),
            "the noise switch is carried below the pivot: {message}"
        );
    }

    /// The noise switch is not keyed on the generation: a legacy core gets it
    /// and must not be told it is missing.
    #[test]
    fn a_legacy_core_is_not_told_the_noise_switch_is_missing() {
        let report = check(&core(142), &profile(), &CoreCapabilities::for_major(142));
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn an_unsupported_brand_is_reported_with_what_asked_for_it() {
        let mut capabilities = CoreCapabilities::for_major(148);
        capabilities.supported_brands = vec![BrowserBrand::Chrome];
        let mut profile = profile();
        profile.fingerprint.brand = BrowserBrand::Opera;

        let report = check(&core(148), &profile, &capabilities);

        assert_eq!(report.findings().len(), 1);
        assert_eq!(report.findings()[0].switch, "--fingerprint-brand");
        assert!(report.findings()[0].requested_by.contains("Opera"));
        assert!(report.message().expect("warning").contains("brand Opera"));
    }

    #[test]
    fn unsupported_switches_are_reported_only_when_the_profile_uses_them() {
        let mut capabilities = CoreCapabilities::for_major(148);
        capabilities.supports_brand_version = false;
        capabilities.supports_platform_version = false;
        capabilities.supports_disable_spoofing = false;

        let plain = check(&core(148), &profile(), &capabilities);
        assert!(plain.is_empty(), "nothing requested, nothing to report");

        let mut profile = profile();
        profile.fingerprint.brand_version = Some("144.0.7559.132".to_string());
        profile.fingerprint.platform_version = Some("10.0.0".to_string());
        profile.fingerprint.disabled_spoofing = vec![SpoofingFeature::Canvas];

        let report = check(&core(148), &profile, &capabilities);
        let switches: Vec<&str> = report
            .findings()
            .iter()
            .map(|finding| finding.switch.as_str())
            .collect();

        assert_eq!(
            switches,
            [
                "--fingerprint-brand-version",
                "--fingerprint-platform-version",
                "--disable-spoofing"
            ],
            "the noise group is no longer reported for a legacy major"
        );
        let message = report.message().expect("warning");
        assert!(message.contains("canvas"), "{message}");
    }

    #[test]
    fn findings_keep_a_stable_order() {
        let mut profile = profile();
        profile.fingerprint.brand_version = Some("144.0.7559.132".to_string());
        profile.fingerprint.platform_version = Some("10.0.0".to_string());
        profile.fingerprint.disabled_spoofing = vec![SpoofingFeature::Font];

        let mut capabilities = CoreCapabilities::for_major(128);
        capabilities.supports_brand_version = false;
        capabilities.supports_platform_version = false;
        capabilities.supports_canvas_noise = false;
        capabilities.supports_disable_spoofing = false;

        let report = check(&core(128), &profile, &capabilities);
        let switches: Vec<&str> = report
            .findings()
            .iter()
            .map(|finding| finding.switch.as_str())
            .collect();

        assert_eq!(
            switches,
            [
                "--fingerprinting-canvas-image-data-noise,--fingerprinting-client-rects-noise",
                "--fingerprint-brand-version",
                "--fingerprint-platform-version",
                "--disable-spoofing"
            ]
        );
    }
}
