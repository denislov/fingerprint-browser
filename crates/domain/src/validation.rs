use crate::core::BrowserCore;
use crate::error::ValidationError;
use crate::fingerprint::FingerprintProfile;
use crate::profile::BrowserProfile;
use crate::proxy::ProxyProfile;
use crate::window::WindowProfile;

pub fn validate_profile(profile: &BrowserProfile) -> Result<(), ValidationError> {
    if profile.name.trim().is_empty() {
        return Err(ValidationError::EmptyProfileName);
    }
    validate_window(&profile.window)?;
    validate_fingerprint(&profile.fingerprint)?;
    Ok(())
}

pub fn validate_window(window: &WindowProfile) -> Result<(), ValidationError> {
    if window.width == 0 || window.height == 0 {
        return Err(ValidationError::InvalidWindowDimensions {
            width: window.width,
            height: window.height,
        });
    }
    Ok(())
}

pub fn validate_fingerprint(fp: &FingerprintProfile) -> Result<(), ValidationError> {
    if fp.language.trim().is_empty() {
        return Err(ValidationError::EmptyLanguage);
    }
    if fp.timezone.trim().is_empty() {
        return Err(ValidationError::EmptyTimezone);
    }
    if let Some(hw) = fp.hardware_concurrency
        && !(1..=128).contains(&hw)
    {
        return Err(ValidationError::InvalidHardwareConcurrency(hw));
    }
    Ok(())
}

pub fn validate_proxy(proxy: &ProxyProfile) -> Result<(), ValidationError> {
    if proxy.name.trim().is_empty() {
        return Err(ValidationError::EmptyProxyName);
    }
    Ok(())
}

pub fn validate_core(core: &BrowserCore) -> Result<(), ValidationError> {
    if core.name.trim().is_empty() {
        return Err(ValidationError::EmptyCoreName);
    }
    Ok(())
}
