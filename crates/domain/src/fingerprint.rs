use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserBrand {
    Chrome,
    Edge,
    Opera,
    Vivaldi,
}

impl BrowserBrand {
    pub fn as_arg_value(&self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
            Self::Opera => "Opera",
            Self::Vivaldi => "Vivaldi",
        }
    }
}

impl fmt::Display for BrowserBrand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_arg_value())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
}

impl Platform {
    pub fn as_arg_value(&self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::MacOs => "mac",
            Self::Linux => "linux",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_arg_value())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebRtcPolicy {
    #[default]
    DisableNonProxiedUdp,
    DefaultPublicInterfaceOnly,
    DefaultPublicAndPrivateInterfaces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpoofingFeature {
    Font,
    Audio,
    Canvas,
    ClientRects,
    Gpu,
}

impl SpoofingFeature {
    pub fn as_flag_name(&self) -> &'static str {
        match self {
            Self::Font => "font",
            Self::Audio => "audio",
            Self::Canvas => "canvas",
            Self::ClientRects => "client-rects",
            Self::Gpu => "gpu",
        }
    }
}

impl fmt::Display for SpoofingFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_flag_name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FingerprintProfile {
    pub seed: u32,
    pub brand: BrowserBrand,
    pub brand_version: Option<String>,
    pub platform: Platform,
    pub platform_version: Option<String>,
    pub language: String,
    pub accept_language: String,
    pub timezone: String,
    pub hardware_concurrency: Option<u8>,
    pub webrtc_policy: WebRtcPolicy,
    pub disabled_spoofing: Vec<SpoofingFeature>,
}

impl FingerprintProfile {
    pub fn new_random(seed: u32) -> Self {
        Self {
            seed,
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
        }
    }

    pub fn default_accept_language_for(lang: &str) -> String {
        if lang.starts_with("zh") {
            format!("{lang},zh;q=0.9,en;q=0.8")
        } else if let Some((primary, _)) = lang.split_once('-') {
            format!("{lang},{primary};q=0.9")
        } else {
            format!("{lang},en;q=0.8")
        }
    }
}
