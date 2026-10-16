use crate::status::StatusLaunch;
use domain::{BrowserCore, BrowserProfile, ProfileId, ProxyProfile, RuntimeState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartParams {
    /// Identifies this start, including its eventual cancellation acknowledgement.
    pub request_id: u64,
    pub profile: BrowserProfile,
    pub core: BrowserCore,
    pub proxy: Option<ProxyProfile>,
    /// The status page this launch opens first, when the reader asked for one.
    ///
    /// Part of the launch rather than of the supervisor's configuration, because
    /// the language and the endpoint behind it are settings a reader can change
    /// between two starts: a page that answered in the language of the previous
    /// start would be wrong in a way nobody would think to check.
    pub status: Option<StatusLaunch>,
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
            status: None,
        }
    }

    pub fn with_proxy(profile: BrowserProfile, core: BrowserCore, proxy: ProxyProfile) -> Self {
        Self {
            request_id: Self::next_request_id(),
            profile,
            core,
            proxy: Some(proxy),
            status: None,
        }
    }

    /// The same launch, opening a status page as its first tab.
    pub fn with_status(mut self, status: StatusLaunch) -> Self {
        self.status = Some(status);
        self
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
