use crate::fingerprint::BrowserBrand;
use crate::id::CoreId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserCore {
    pub id: CoreId,
    pub name: String,
    pub executable: PathBuf,
    pub version: String,
    pub major: u32,
}

/// The name a core gets when the user does not choose one: `<binary> <major>`.
///
/// A core whose version could not be read says so instead of guessing a major.
pub fn suggested_name(executable: &Path, major: u32) -> String {
    let stem = executable
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "browser".to_string());
    if major == 0 {
        format!("{stem} (version unknown)")
    } else {
        format!("{stem} {major}")
    }
}

impl BrowserCore {
    /// Whether the name is still the one this binary and major suggest.
    ///
    /// Used to decide if a detected major change may rewrite the name: a name
    /// the user chose is theirs and is never rewritten.
    pub fn has_suggested_name(&self) -> bool {
        self.name == suggested_name(&self.executable, self.major)
    }

    /// The capability table for this core, or `None` when the version was never
    /// detected.
    ///
    /// Major `0` means "no version was read". It is not a generation, so it has
    /// no capability table: callers must refuse rather than assume one, or a
    /// profile would launch claiming switches nobody verified for this engine.
    pub fn capabilities(&self) -> Option<CoreCapabilities> {
        (self.major > 0).then(|| CoreCapabilities::for_major(self.major))
    }
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

/// Brands the verified generation honours.
///
/// The engine only knows two brand tokens: `Chrome` and `Edge` (matched case
/// insensitively). `google chrome`, `microsoft edge`, `opera`, `vivaldi` and
/// `brave` are accepted on the command line and ignored, leaving the default
/// Chromium brand list in place. Claiming them would let a profile ask for a
/// brand the engine silently drops, so the table carries only the honoured
/// two and the compatibility layer reports the rest as unsupported.
const VERIFIED_BRANDS: [BrowserBrand; 2] = [BrowserBrand::Chrome, BrowserBrand::Edge];

impl CoreCapabilities {
    /// Capability table for a detected major.
    ///
    /// Only switches measured against a real engine are enabled. The stable set
    /// (seed, brand, platform, both version switches, language, timezone,
    /// hardware concurrency, WebRTC policy) is carried by both generations.
    ///
    /// The two switches that are not stable turned out to sit on opposite sides
    /// of the pivot, which is worth spelling out because the table said the
    /// reverse until both were measured:
    ///
    /// - `--fingerprinting-canvas-image-data-noise` changes `toDataURL` and
    ///   leaves `getImageData` alone on **142 and 148**, so both generations
    ///   carry it. The pivot was never its boundary: a major below 144 was
    ///   being denied a switch the engine honours.
    /// - `--disable-spoofing=<feature>` is honoured by **148** and ignored by
    ///   **142**, which accepts the switch and applies the noise anyway. Only
    ///   the verified generation is asked for it; the omission is reported.
    ///
    /// 144 to 147 were not measured here: exclusions are claimed for them on
    /// the strength of the sibling product's 144 measurement, and the noise
    /// switch is claimed for every major on the strength of 142 and 148.
    pub fn for_major(major: u32) -> Self {
        let generation = FingerprintGeneration::for_major(major);
        let verified = generation == FingerprintGeneration::Chrome144Plus;

        Self {
            major,
            generation,
            // Measured: honoured on 148, ignored on 142.
            supports_disable_spoofing: verified,
            // Measured: honoured on 142 and on 148.
            supports_canvas_noise: true,
            supports_brand_version: true,
            supports_platform_version: true,
            supported_brands: VERIFIED_BRANDS.to_vec(),
        }
    }

    /// Whether this major's switch set was verified against a real engine.
    pub fn is_verified(&self) -> bool {
        self.generation == FingerprintGeneration::Chrome144Plus
    }

    /// Which switch generation this is, as the window writes it.
    pub fn generation_label(&self) -> String {
        match self.generation {
            FingerprintGeneration::Legacy => format!(
                "Chrome {} and older",
                FingerprintGeneration::PIVOT_MAJOR - 1
            ),
            FingerprintGeneration::Chrome144Plus => {
                format!("Chrome {}+", FingerprintGeneration::PIVOT_MAJOR)
            }
        }
    }

    /// Whether the engine honours `--disable-spoofing` for this generation.
    ///
    /// This is the switch that separates the generations: measured honoured on
    /// 148, ignored on 142. The noise switch is carried by both, so it is not
    /// what the label describes.
    pub fn exclusion_label(&self) -> &'static str {
        if self.supports_disable_spoofing {
            "spoofing exclusions honoured"
        } else {
            "spoofing exclusions not honoured"
        }
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

    /// Measured on 142 and 148: the noise switch is honoured by both, while
    /// `--disable-spoofing` is honoured only by the verified generation.
    #[test]
    fn the_two_unstable_switches_sit_on_opposite_sides_of_the_pivot() {
        let modern = CoreCapabilities::for_major(148);
        assert!(modern.is_verified());
        assert!(modern.supports_canvas_noise);
        assert!(modern.supports_disable_spoofing);

        let legacy = CoreCapabilities::for_major(142);
        assert!(!legacy.is_verified());
        assert!(
            legacy.supports_canvas_noise,
            "142 honours --fingerprinting-canvas-image-data-noise: toDataURL moves, \
             getImageData does not"
        );
        assert!(
            !legacy.supports_disable_spoofing,
            "142 accepts --disable-spoofing=canvas and applies the noise anyway"
        );
    }

    #[test]
    fn the_stable_switch_set_is_carried_by_both_generations() {
        for major in [110, 143, 144, 148] {
            let capabilities = CoreCapabilities::for_major(major);
            assert!(capabilities.supports_canvas_noise, "major {major}");
            assert!(capabilities.supports_brand_version, "major {major}");
            assert!(capabilities.supports_platform_version, "major {major}");
            for brand in VERIFIED_BRANDS {
                assert!(capabilities.supports_brand(brand), "major {major} {brand}");
            }
        }
    }

    #[test]
    fn only_brands_the_engine_honours_are_claimed() {
        let capabilities = CoreCapabilities::for_major(148);

        assert!(capabilities.supports_brand(BrowserBrand::Chrome));
        assert!(capabilities.supports_brand(BrowserBrand::Edge));
        // Measured on 148: these tokens are accepted and ignored.
        for brand in [BrowserBrand::Opera, BrowserBrand::Vivaldi] {
            assert!(
                !capabilities.supports_brand(brand),
                "{brand} is ignored by the engine and must not be claimed"
            );
        }
    }

    #[test]
    fn the_generation_is_written_the_same_way_everywhere() {
        let modern = CoreCapabilities::for_major(148);
        assert_eq!(modern.generation_label(), "Chrome 144+");
        assert_eq!(modern.exclusion_label(), "spoofing exclusions honoured");

        let legacy = CoreCapabilities::for_major(142);
        assert_eq!(legacy.generation_label(), "Chrome 143 and older");
        assert_eq!(legacy.exclusion_label(), "spoofing exclusions not honoured");
    }

    #[test]
    fn a_brand_outside_the_table_is_not_supported() {
        let mut capabilities = CoreCapabilities::for_major(148);
        capabilities.supported_brands = vec![BrowserBrand::Chrome];

        assert!(capabilities.supports_brand(BrowserBrand::Chrome));
        assert!(!capabilities.supports_brand(BrowserBrand::Opera));
    }
}
