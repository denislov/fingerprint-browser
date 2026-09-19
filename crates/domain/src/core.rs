use crate::fingerprint::BrowserBrand;
use crate::id::CoreId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserCore {
    pub id: CoreId,
    pub name: String,
    pub executable: PathBuf,
    pub version: String,
    pub major: u32,
}

/// Which fingerprint-chromium switch generation a core belongs to.
///
/// The split sits at the oldest major whose switch set this project verified
/// against a real engine; see `docs/fingerprint-matrix.md`. A core whose
/// version could not be detected has no generation at all: the capability
/// resolver rejects major `0` before this table is consulted, so an
/// unclassified binary never launches with an assumed capability set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FingerprintGeneration {
    /// Older than the verified generation: only the stable switch set is passed.
    Legacy,
    /// The verified generation (`FingerprintGeneration::PIVOT_MAJOR` and later).
    Chrome144Plus,
}

impl FingerprintGeneration {
    /// First major whose switch set was verified against a real engine.
    pub const PIVOT_MAJOR: u32 = 144;

    /// Classifies a detected major.
    ///
    /// Callers must reject major `0` (undetected version) before asking for
    /// capabilities, because "unknown" is not a generation.
    pub fn for_major(major: u32) -> Self {
        debug_assert!(
            major > 0,
            "unknown majors are rejected before capability resolution"
        );
        if major >= Self::PIVOT_MAJOR {
            Self::Chrome144Plus
        } else {
            Self::Legacy
        }
    }
}

/// The fingerprint switches a specific core can honour.
///
/// Every field is a claim about the engine, not about the profile. A field that
/// is `false` means "not verified for this major": the serializer omits the
/// switch and the compatibility layer reports the omission, so a fingerprint is
/// never silently claimed by an engine that ignores the switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreCapabilities {
    pub major: u32,
    pub generation: FingerprintGeneration,
    /// `--disable-spoofing=<feature,...>`
    pub supports_disable_spoofing: bool,
    /// `--fingerprinting-canvas-image-data-noise` and
    /// `--fingerprinting-client-rects-noise`.
    pub supports_canvas_noise: bool,
    /// `--fingerprint-brand-version=<version>`
    pub supports_brand_version: bool,
    /// `--fingerprint-platform-version=<version>`
    pub supports_platform_version: bool,
    /// Brands `--fingerprint-brand` accepts.
    pub supported_brands: Vec<BrowserBrand>,
}

impl CoreCapabilities {
    /// Capability table for a detected major.
    ///
    /// Only switches verified against a real engine are enabled. The stable set
    /// (seed, brand, platform, both version switches, language, timezone,
    /// hardware concurrency, WebRTC policy, `--disable-spoofing`) is carried by
    /// both generations; the canvas/rects noise switches were verified from
    /// [`FingerprintGeneration::PIVOT_MAJOR`] onwards.
    pub fn for_major(major: u32) -> Self {
        let generation = FingerprintGeneration::for_major(major);
        let verified = generation == FingerprintGeneration::Chrome144Plus;

        Self {
            major,
            generation,
            supports_disable_spoofing: true,
            supports_canvas_noise: verified,
            supports_brand_version: true,
            supports_platform_version: true,
            supported_brands: vec![
                BrowserBrand::Chrome,
                BrowserBrand::Edge,
                BrowserBrand::Opera,
                BrowserBrand::Vivaldi,
            ],
        }
    }

    /// Whether this major's switch set was verified against a real engine.
    pub fn is_verified(&self) -> bool {
        self.generation == FingerprintGeneration::Chrome144Plus
    }

    pub fn supports_brand(&self, brand: BrowserBrand) -> bool {
        self.supported_brands.contains(&brand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_splits_at_the_verified_pivot() {
        assert_eq!(
            FingerprintGeneration::for_major(143),
            FingerprintGeneration::Legacy
        );
        assert_eq!(
            FingerprintGeneration::for_major(FingerprintGeneration::PIVOT_MAJOR),
            FingerprintGeneration::Chrome144Plus
        );
        assert_eq!(
            FingerprintGeneration::for_major(148),
            FingerprintGeneration::Chrome144Plus
        );
    }

    #[test]
    fn the_verified_generation_carries_the_noise_switches() {
        let modern = CoreCapabilities::for_major(148);
        assert!(modern.is_verified());
        assert!(modern.supports_canvas_noise);
        assert!(modern.supports_disable_spoofing);

        let legacy = CoreCapabilities::for_major(128);
        assert!(!legacy.is_verified());
        assert!(
            !legacy.supports_canvas_noise,
            "the noise switches are only verified from the pivot major"
        );
    }

    #[test]
    fn the_stable_switch_set_is_carried_by_both_generations() {
        for major in [110, 143, 144, 148] {
            let capabilities = CoreCapabilities::for_major(major);
            assert!(capabilities.supports_disable_spoofing, "major {major}");
            assert!(capabilities.supports_brand_version, "major {major}");
            assert!(capabilities.supports_platform_version, "major {major}");
            for brand in [
                BrowserBrand::Chrome,
                BrowserBrand::Edge,
                BrowserBrand::Opera,
                BrowserBrand::Vivaldi,
            ] {
                assert!(capabilities.supports_brand(brand), "major {major} {brand}");
            }
        }
    }

    #[test]
    fn a_brand_outside_the_table_is_not_supported() {
        let mut capabilities = CoreCapabilities::for_major(148);
        capabilities.supported_brands = vec![BrowserBrand::Chrome];

        assert!(capabilities.supports_brand(BrowserBrand::Chrome));
        assert!(!capabilities.supports_brand(BrowserBrand::Opera));
    }
}
