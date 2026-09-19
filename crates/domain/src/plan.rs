use crate::id::ProfileId;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub profile_id: ProfileId,
    pub browser_executable: PathBuf,
    pub browser_args: Vec<OsString>,
    pub user_data_dir: PathBuf,
    pub cdp_port: u16,
    pub xray: Option<XrayLaunchPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrayLaunchPlan {
    pub executable: PathBuf,
    pub config_path: PathBuf,
    pub socks_port: u16,
}

impl XrayLaunchPlan {
    /// The arguments Xray is started with, after `argv[0]`. One source for the
    /// process that gets spawned and for the session record that has to
    /// recognise it later.
    pub fn args(&self) -> Vec<String> {
        vec![
            "run".to_string(),
            "-config".to_string(),
            self.config_path.to_string_lossy().into_owned(),
        ]
    }
}
