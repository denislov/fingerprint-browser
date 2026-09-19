use crate::error::CapabilityError;
use domain::{BrowserCore, CoreCapabilities};

pub trait CapabilityResolver: Send + Sync {
    fn resolve(&self, core: &BrowserCore) -> Result<CoreCapabilities, CapabilityError>;
}

#[derive(Debug, Default)]
pub struct DefaultCapabilityResolver;

impl DefaultCapabilityResolver {
    pub fn new() -> Self {
        Self
    }
}

impl CapabilityResolver for DefaultCapabilityResolver {
    fn resolve(&self, core: &BrowserCore) -> Result<CoreCapabilities, CapabilityError> {
        // Known supported majors: 110..=144+
        if core.major == 0 {
            return Err(CapabilityError::UnsupportedMajor { major: core.major });
        }
        Ok(CoreCapabilities::default_for_major(core.major))
    }
}
