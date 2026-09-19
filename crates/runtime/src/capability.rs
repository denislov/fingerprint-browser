use crate::error::CapabilityError;
use domain::{BrowserCore, CoreCapabilities};

pub trait CapabilityResolver: Send + Sync {
    fn resolve(&self, core: &BrowserCore) -> Result<CoreCapabilities, CapabilityError>;
}

/// Resolves the switch set from the core's detected major.
///
/// An undetected major is refused instead of being resolved to an assumed
/// capability set: a fingerprint browser that cannot classify its engine must
/// not launch with unverified spoofing switches. The compatibility layer
/// reports the switches a *known* major cannot honour.
#[derive(Debug, Default)]
pub struct DefaultCapabilityResolver;

impl DefaultCapabilityResolver {
    pub fn new() -> Self {
        Self
    }
}

impl CapabilityResolver for DefaultCapabilityResolver {
    fn resolve(&self, core: &BrowserCore) -> Result<CoreCapabilities, CapabilityError> {
        if core.major == 0 {
            return Err(CapabilityError::UnsupportedMajor { major: core.major });
        }
        Ok(CoreCapabilities::for_major(core.major))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{CoreId, FingerprintGeneration};
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

    #[test]
    fn resolves_the_table_for_a_detected_major() {
        let capabilities = DefaultCapabilityResolver
            .resolve(&core(148))
            .expect("resolve");

        assert_eq!(capabilities.major, 148);
        assert_eq!(
            capabilities.generation,
            FingerprintGeneration::Chrome144Plus
        );
        assert!(capabilities.supports_canvas_noise);
    }

    #[test]
    fn refuses_an_undetected_major() {
        let error = DefaultCapabilityResolver
            .resolve(&core(0))
            .expect_err("major 0 is not a generation");

        assert!(matches!(
            error,
            CapabilityError::UnsupportedMajor { major: 0 }
        ));
    }
}
