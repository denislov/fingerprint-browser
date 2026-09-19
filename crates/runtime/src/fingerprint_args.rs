use domain::{CoreCapabilities, FingerprintProfile, WebRtcPolicy};
use std::ffi::OsString;

/// Serializes the fingerprint portion of a launch command line into
/// fingerprint-chromium switches.
///
/// The switch names and their emitted order follow the layout the upstream
/// browser expects: seed, brand and brand version, platform and platform
/// version, locale, hardware, then the WebRTC and spoofing switches.
///
/// Every field the profile carries is either emitted here or deliberately
/// dropped by a documented rule. Fields that the engine cannot honour for the
/// resolved core version are reported by the capability layer, not silently
/// removed at serialization time.
pub fn serialize_fingerprint_args(
    fingerprint: &FingerprintProfile,
    capabilities: &CoreCapabilities,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();

    args.push(format!("--fingerprint={}", fingerprint.seed).into());
    // The brand and its version are one claim about the engine: a core that does
    // not declare the brand gets neither switch, and the compatibility layer
    // reports the omission.
    let brand_supported = capabilities.supports_brand(fingerprint.brand);
    if brand_supported {
        args.push(format!("--fingerprint-brand={}", fingerprint.brand.as_arg_value()).into());
        if capabilities.supports_brand_version
            && let Some(brand_version) = non_empty(&fingerprint.brand_version)
        {
            args.push(format!("--fingerprint-brand-version={brand_version}").into());
        }
    }

    args.push(
        format!(
            "--fingerprint-platform={}",
            fingerprint.platform.as_arg_value()
        )
        .into(),
    );
    if capabilities.supports_platform_version
        && let Some(platform_version) = non_empty(&fingerprint.platform_version)
    {
        args.push(format!("--fingerprint-platform-version={platform_version}").into());
    }

    args.push(format!("--lang={}", fingerprint.language).into());
    args.push(format!("--accept-lang={}", fingerprint.accept_language).into());
    args.push(format!("--timezone={}", fingerprint.timezone).into());

    if let Some(concurrency) = fingerprint.hardware_concurrency {
        args.push(format!("--fingerprint-hardware-concurrency={concurrency}").into());
    }

    if fingerprint.webrtc_policy == WebRtcPolicy::DisableNonProxiedUdp {
        args.push("--disable-non-proxied-udp".into());
    }

    if capabilities.supports_canvas_noise {
        args.push("--fingerprinting-canvas-image-data-noise".into());
        args.push("--fingerprinting-client-rects-noise".into());
    }

    if capabilities.supports_disable_spoofing && !fingerprint.disabled_spoofing.is_empty() {
        let features: Vec<&str> = fingerprint
            .disabled_spoofing
            .iter()
            .map(|feature| feature.as_flag_name())
            .collect();
        args.push(format!("--disable-spoofing={}", features.join(",")).into());
    }

    args
}

/// Returns the trimmed value, treating a blank field as absent so an empty
/// switch value is never emitted.
fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{BrowserBrand, Platform, SpoofingFeature};

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }

    fn profile() -> FingerprintProfile {
        FingerprintProfile {
            seed: 4242,
            brand: BrowserBrand::Chrome,
            brand_version: Some("144.0.7559.132".to_string()),
            platform: Platform::Windows,
            platform_version: Some("10.0.0".to_string()),
            language: "en-US".to_string(),
            accept_language: "en-US,en;q=0.9".to_string(),
            timezone: "America/New_York".to_string(),
            hardware_concurrency: Some(8),
            webrtc_policy: WebRtcPolicy::DisableNonProxiedUdp,
            disabled_spoofing: Vec::new(),
        }
    }

    #[test]
    fn emits_brand_and_platform_versions_when_present() {
        let capabilities = CoreCapabilities::for_major(144);
        let args = strings(&serialize_fingerprint_args(&profile(), &capabilities));

        assert!(args.iter().any(|a| a == "--fingerprint=4242"));
        assert!(args.iter().any(|a| a == "--fingerprint-brand=Chrome"));
        assert!(
            args.iter()
                .any(|a| a == "--fingerprint-brand-version=144.0.7559.132")
        );
        assert!(args.iter().any(|a| a == "--fingerprint-platform=windows"));
        assert!(
            args.iter()
                .any(|a| a == "--fingerprint-platform-version=10.0.0")
        );
    }

    #[test]
    fn omits_version_switches_when_absent_or_blank() {
        let capabilities = CoreCapabilities::for_major(144);
        let mut fp = profile();
        fp.brand_version = None;
        fp.platform_version = Some("   ".to_string());

        let args = strings(&serialize_fingerprint_args(&fp, &capabilities));

        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("--fingerprint-brand-version"))
        );
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("--fingerprint-platform-version"))
        );
    }

    #[test]
    fn version_switch_follows_its_brand_and_platform_switch() {
        let capabilities = CoreCapabilities::for_major(144);
        let args = strings(&serialize_fingerprint_args(&profile(), &capabilities));

        let position = |needle: &str| {
            args.iter()
                .position(|a| a == needle)
                .unwrap_or_else(|| panic!("missing {needle}"))
        };

        assert_eq!(
            position("--fingerprint-brand-version=144.0.7559.132"),
            position("--fingerprint-brand=Chrome") + 1
        );
        assert_eq!(
            position("--fingerprint-platform-version=10.0.0"),
            position("--fingerprint-platform=windows") + 1
        );
    }

    #[test]
    fn capability_gates_canvas_noise_and_disable_spoofing() {
        let mut fp = profile();
        fp.disabled_spoofing = vec![SpoofingFeature::Font, SpoofingFeature::Canvas];

        let mut capabilities = CoreCapabilities::for_major(144);
        capabilities.supports_canvas_noise = false;
        capabilities.supports_disable_spoofing = false;

        let args = strings(&serialize_fingerprint_args(&fp, &capabilities));

        assert!(
            !args
                .iter()
                .any(|a| a == "--fingerprinting-canvas-image-data-noise")
        );
        assert!(
            !args
                .iter()
                .any(|a| a == "--fingerprinting-client-rects-noise")
        );
        assert!(!args.iter().any(|a| a.starts_with("--disable-spoofing")));
    }

    #[test]
    fn a_brand_the_core_does_not_declare_is_omitted_with_its_version() {
        let mut fp = profile();
        fp.brand = BrowserBrand::Opera;

        let mut capabilities = CoreCapabilities::for_major(144);
        capabilities.supported_brands = vec![BrowserBrand::Chrome];

        let args = strings(&serialize_fingerprint_args(&fp, &capabilities));

        assert!(
            !args.iter().any(|a| a.starts_with("--fingerprint-brand")),
            "neither the brand nor its version may be claimed: {args:?}"
        );
        assert!(args.iter().any(|a| a == "--fingerprint=4242"));
    }

    #[test]
    fn an_unsupported_version_switch_is_omitted() {
        let mut capabilities = CoreCapabilities::for_major(144);
        capabilities.supports_brand_version = false;
        capabilities.supports_platform_version = false;

        let args = strings(&serialize_fingerprint_args(&profile(), &capabilities));

        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("--fingerprint-brand-version"))
        );
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("--fingerprint-platform-version"))
        );
        assert!(args.iter().any(|a| a == "--fingerprint-brand=Chrome"));
        assert!(args.iter().any(|a| a == "--fingerprint-platform=windows"));
    }

    #[test]
    fn the_legacy_generation_omits_the_noise_switches() {
        let args = strings(&serialize_fingerprint_args(
            &profile(),
            &CoreCapabilities::for_major(128),
        ));

        assert!(
            !args
                .iter()
                .any(|a| a == "--fingerprinting-canvas-image-data-noise")
        );
        assert!(args.iter().any(|a| a == "--fingerprint=4242"));
    }

    #[test]
    fn disable_spoofing_lists_every_requested_feature() {
        let capabilities = CoreCapabilities::for_major(144);
        let mut fp = profile();
        fp.disabled_spoofing = vec![SpoofingFeature::Font, SpoofingFeature::Gpu];

        let args = strings(&serialize_fingerprint_args(&fp, &capabilities));

        assert!(args.iter().any(|a| a == "--disable-spoofing=font,gpu"));
    }
}
