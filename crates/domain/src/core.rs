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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreCapabilities {
    pub major: u32,
    pub supports_disable_spoofing: bool,
    pub supports_explicit_gpu: bool,
    pub supports_canvas_noise_flag: bool,
    pub supported_brands: Vec<BrowserBrand>,
}

impl CoreCapabilities {
    /// Baseline capability profile for known fingerprint-chromium majors
    pub fn default_for_major(major: u32) -> Self {
        Self {
            major,
            supports_disable_spoofing: true,
            supports_explicit_gpu: true,
            supports_canvas_noise_flag: true,
            supported_brands: vec![
                BrowserBrand::Chrome,
                BrowserBrand::Edge,
                BrowserBrand::Opera,
                BrowserBrand::Vivaldi,
            ],
        }
    }
}
