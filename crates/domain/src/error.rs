use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ValidationError {
    #[error("profile name cannot be empty")]
    EmptyProfileName,

    #[error("the start page cannot be empty")]
    EmptyStartUrl,

    #[error("start page {url:?} is not an address this program will open")]
    UnusableStartUrl { url: String },

    #[error("start page {url:?} is a browser switch, not a page")]
    StartUrlIsSwitch { url: String },

    #[error("start page scheme {scheme:?} is not supported; use http, https, file or about:blank")]
    UnsupportedStartScheme { scheme: String },

    #[error("proxy name cannot be empty")]
    EmptyProxyName,

    #[error("proxy host cannot be empty")]
    EmptyProxyHost,

    #[error("proxy host {host:?} must be a bare host name or address, without spaces or a scheme")]
    InvalidProxyHost { host: String },

    #[error("proxy port must be between 1 and 65535")]
    InvalidProxyPort,

    #[error("proxy credentials must be either both set or both empty")]
    IncompleteProxyCredentials,

    #[error("proxy {field} cannot be empty")]
    EmptyProxyField { field: &'static str },

    #[error("stream setting {field} is set, but {selected} never reads it")]
    UnusedStreamSetting {
        field: &'static str,
        selected: &'static str,
    },

    #[error("stream setting {field} is required when {selected}")]
    MissingStreamSetting {
        field: &'static str,
        selected: &'static str,
    },

    #[error("{detail}")]
    UnsupportedStreamCombination { detail: String },

    #[error("core name cannot be empty")]
    EmptyCoreName,

    #[error("invalid window dimensions: {width}x{height}, width and height must be greater than 0")]
    InvalidWindowDimensions { width: u32, height: u32 },

    #[error("invalid hardware concurrency: {0}, must be between 1 and 128")]
    InvalidHardwareConcurrency(u8),

    #[error("language cannot be empty")]
    EmptyLanguage,

    #[error("timezone cannot be empty")]
    EmptyTimezone,

    #[error("validation failed: {0}")]
    Custom(String),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    #[error(transparent)]
    Validation(#[from] ValidationError),

    #[error("domain error: {0}")]
    Other(String),
}
