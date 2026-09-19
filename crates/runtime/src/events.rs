use domain::{ProfileId, RuntimeState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommand {
    Start(ProfileId),
    Stop(ProfileId),
    Restart(ProfileId),
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
