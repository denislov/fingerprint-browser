use domain::{BrowserCore, BrowserProfile, ProfileId, ProxyProfile, RuntimeState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartParams {
    /// Identifies this start, including its eventual cancellation acknowledgement.
    pub request_id: u64,
    pub profile: BrowserProfile,
    pub core: BrowserCore,
    pub proxy: Option<ProxyProfile>,
}

impl StartParams {
    pub fn next_request_id() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
    pub fn new(profile: BrowserProfile, core: BrowserCore) -> Self {
        Self {
            request_id: Self::next_request_id(),
            profile,
            core,
            proxy: None,
        }
    }

    pub fn with_proxy(profile: BrowserProfile, core: BrowserCore, proxy: ProxyProfile) -> Self {
        Self {
            request_id: Self::next_request_id(),
            profile,
            core,
            proxy: Some(proxy),
        }
    }

    pub fn profile_id(&self) -> ProfileId {
        self.profile.id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommand {
    Start(StartParams),
    Stop(ProfileId),
    Restart(StartParams),
    /// End this run and leave every running session running, marked so the next
    /// run adopts it rather than stopping it.
    ReleaseAll,
    /// End this run and stop everything it started.
    ShutdownAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeComponent {
    Browser,
    Xray,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    StateChanged {
        profile_id: ProfileId,
        state: RuntimeState,
    },
    EffectiveLaunchArgs {
        profile_id: ProfileId,
        args: Vec<String>,
    },
    Started {
        profile_id: ProfileId,
        browser_pid: u32,
        xray_pid: Option<u32>,
        cdp_port: u16,
        socks_port: Option<u16>,
    },
    Stopped {
        profile_id: ProfileId,
    },
    Crashed {
        profile_id: ProfileId,
        component: RuntimeComponent,
        message: String,
    },
    Warning {
        profile_id: ProfileId,
        message: String,
    },
}

impl RuntimeEvent {
    pub fn profile_id(&self) -> ProfileId {
        match self {
            Self::StateChanged { profile_id, .. }
            | Self::EffectiveLaunchArgs { profile_id, .. }
            | Self::Started { profile_id, .. }
            | Self::Stopped { profile_id }
            | Self::Crashed { profile_id, .. }
            | Self::Warning { profile_id, .. } => *profile_id,
        }
    }
}
