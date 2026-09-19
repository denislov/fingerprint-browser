use thiserror::Error;

#[derive(Debug, Error)]
pub enum CapabilityError {
    #[error("unsupported browser major version: {major}")]
    UnsupportedMajor { major: u32 },

    #[error("browser brand {brand} not supported by core {major}")]
    UnsupportedBrand { brand: String, major: u32 },

    #[error("capability error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum LaunchPlanError {
    #[error("core capability error: {0}")]
    Capability(#[from] CapabilityError),

    #[error("missing browser executable at path: {path}")]
    ExecutableNotFound { path: String },

    #[error("missing xray executable at path: {path}")]
    XrayExecutableNotFound { path: String },

    #[error("user data dir creation failed: {0}")]
    UserDataDirError(String),

    #[error("invalid launch arguments: {0}")]
    InvalidArguments(String),

    #[error("launch plan error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("unsupported proxy outbound type: {0}")]
    UnsupportedOutbound(String),

    #[error("proxy config serialization failed: {0}")]
    ConfigSerialization(#[from] serde_json::Error),

    #[error("proxy IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("proxy error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum PortError {
    #[error("failed to allocate loopback port: {0}")]
    AllocationFailed(#[from] std::io::Error),

    #[error("no free port available")]
    NoPortAvailable,
}

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to spawn process: {0}")]
    SpawnFailed(#[from] std::io::Error),

    #[error("process termination failed: {0}")]
    TerminationFailed(String),

    #[error("process not found: pid {0}")]
    ProcessNotFound(u32),
}

#[derive(Debug, Error)]
pub enum CdpError {
    #[error("CDP readiness probe timed out after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },

    #[error("CDP HTTP request failed: {0}")]
    Http(String),

    #[error("CDP invalid JSON response: {0}")]
    InvalidResponse(String),
}

#[derive(Debug, Error)]
pub enum RuntimeCommandError {
    #[error("profile {profile_id} already running or starting")]
    AlreadyRunning { profile_id: domain::ProfileId },

    #[error("profile {profile_id} not running")]
    NotRunning { profile_id: domain::ProfileId },

    #[error("supervisor channel closed")]
    ChannelClosed,

    #[error("runtime command error: {0}")]
    Other(String),
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Capability(#[from] CapabilityError),

    #[error(transparent)]
    LaunchPlan(#[from] LaunchPlanError),

    #[error(transparent)]
    Proxy(#[from] ProxyError),

    #[error(transparent)]
    Port(#[from] PortError),

    #[error(transparent)]
    Process(#[from] ProcessError),

    #[error(transparent)]
    Cdp(#[from] CdpError),

    #[error(transparent)]
    Command(#[from] RuntimeCommandError),

    #[error("runtime error: {0}")]
    Other(String),
}
