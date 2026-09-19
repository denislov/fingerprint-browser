pub mod core;
pub mod error;
pub mod fingerprint;
pub mod id;
pub mod plan;
pub mod profile;
pub mod proxy;
pub mod runtime;
pub mod start_target;
pub mod validation;
pub mod window;

pub use core::{BrowserCore, CoreCapabilities, FingerprintGeneration, suggested_name};
pub use error::{DomainError, ValidationError};
pub use fingerprint::{BrowserBrand, FingerprintProfile, Platform, SpoofingFeature, WebRtcPolicy};
pub use id::{CoreId, ProfileId, ProxyId};
pub use plan::{LaunchPlan, XrayLaunchPlan};
pub use profile::BrowserProfile;
pub use proxy::{
    HttpOutbound, ProxyOutbound, ProxyProfile, ShadowsocksOutbound, Socks5Outbound, TrojanOutbound,
    VlessOutbound, VmessOutbound,
};
pub use runtime::{RuntimeSession, RuntimeState};
pub use start_target::StartTarget;
pub use validation::{
    validate_core, validate_fingerprint, validate_profile, validate_proxy, validate_window,
};
pub use window::WindowProfile;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_fingerprint_seed_serialization() {
        let fp = FingerprintProfile::new_random(123456);
        let serialized = serde_json::to_string(&fp).expect("failed to serialize fingerprint");
        assert!(serialized.contains("\"seed\":123456"));

        let deserialized: FingerprintProfile =
            serde_json::from_str(&serialized).expect("failed to deserialize fingerprint");
        assert_eq!(deserialized.seed, 123456);
        assert_eq!(deserialized.brand, BrowserBrand::Chrome);
    }

    #[test]
    fn test_accept_language_generation() {
        assert_eq!(
            FingerprintProfile::default_accept_language_for("en-US"),
            "en-US,en;q=0.9"
        );
        assert_eq!(
            FingerprintProfile::default_accept_language_for("zh-CN"),
            "zh-CN,zh;q=0.9,en;q=0.8"
        );
        assert_eq!(
            FingerprintProfile::default_accept_language_for("ja"),
            "ja,en;q=0.8"
        );
    }

    #[test]
    fn test_launch_switch_vocabulary_matches_the_engine() {
        // Measured on the verified generation: `mac` is ignored and the profile
        // keeps the host platform, `macos` switches it.
        assert_eq!(Platform::MacOs.as_arg_value(), "macos");
        assert_eq!(Platform::Windows.as_arg_value(), "windows");
        assert_eq!(Platform::Linux.as_arg_value(), "linux");

        // `--disable-spoofing` takes the unhyphenated token for client rects.
        assert_eq!(SpoofingFeature::ClientRects.as_flag_name(), "clientrects");
        assert_eq!(SpoofingFeature::Canvas.as_flag_name(), "canvas");
        assert_eq!(SpoofingFeature::Gpu.as_flag_name(), "gpu");
        assert_eq!(SpoofingFeature::Font.as_flag_name(), "font");
        assert_eq!(SpoofingFeature::Audio.as_flag_name(), "audio");
    }

    #[test]
    fn test_window_validation() {
        let valid_window = WindowProfile::new(1920, 1080);
        assert!(validate_window(&valid_window).is_ok());

        let invalid_window = WindowProfile::new(0, 1080);
        assert!(matches!(
            validate_window(&invalid_window),
            Err(ValidationError::InvalidWindowDimensions {
                width: 0,
                height: 1080
            })
        ));

        let invalid_height = WindowProfile::new(1080, 0);
        assert!(matches!(
            validate_window(&invalid_height),
            Err(ValidationError::InvalidWindowDimensions {
                width: 1080,
                height: 0
            })
        ));
    }

    #[test]
    fn test_hardware_concurrency_validation() {
        let mut fp = FingerprintProfile::new_random(42);
        fp.hardware_concurrency = Some(0);
        assert_eq!(
            validate_fingerprint(&fp),
            Err(ValidationError::InvalidHardwareConcurrency(0))
        );

        fp.hardware_concurrency = Some(129);
        assert_eq!(
            validate_fingerprint(&fp),
            Err(ValidationError::InvalidHardwareConcurrency(129))
        );

        fp.hardware_concurrency = Some(16);
        assert!(validate_fingerprint(&fp).is_ok());
    }

    #[test]
    fn test_profile_duplication() {
        let original = BrowserProfile {
            id: ProfileId::new(),
            name: "Original Profile".to_string(),
            core_id: CoreId::new(),
            user_data_dir: PathBuf::from("data/profiles/orig"),
            fingerprint: FingerprintProfile::new_random(1111),
            proxy_id: None,
            window: WindowProfile::new(1440, 900),
            start_target: StartTarget::Blank,
        };

        let new_id = ProfileId::new();
        let new_name = "Cloned Profile".to_string();
        let new_path = PathBuf::from("data/profiles/clone");
        let new_seed = 9999;

        let duplicated = original.duplicate(new_id, new_name.clone(), new_path.clone(), new_seed);

        assert_eq!(duplicated.id, new_id);
        assert_ne!(duplicated.id, original.id);
        assert_eq!(duplicated.name, new_name);
        assert_eq!(duplicated.user_data_dir, new_path);
        assert_eq!(duplicated.fingerprint.seed, new_seed);
        assert_ne!(duplicated.fingerprint.seed, original.fingerprint.seed);
        // Persona and window preserved
        assert_eq!(duplicated.window, original.window);
        assert_eq!(duplicated.core_id, original.core_id);
    }
}
