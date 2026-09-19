use crate::error::LaunchPlanError;
use crate::fingerprint_args::serialize_fingerprint_args;
use domain::{
    BrowserCore, BrowserProfile, CoreCapabilities, LaunchPlan, ProxyProfile, StartTarget,
    XrayLaunchPlan,
};
use std::ffi::OsString;
use std::path::PathBuf;

pub struct LaunchContext<'a> {
    pub profile: &'a BrowserProfile,
    pub core: &'a BrowserCore,
    pub proxy: Option<&'a ProxyProfile>,
    pub capabilities: &'a CoreCapabilities,
    pub cdp_port: u16,
    pub socks_port: Option<u16>,
    pub xray_executable: Option<PathBuf>,
    pub xray_config_dir: Option<PathBuf>,
}

pub trait LaunchPlanner: Send + Sync {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError>;
}

#[derive(Debug, Default)]
pub struct DefaultLaunchPlanner;

impl DefaultLaunchPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl LaunchPlanner for DefaultLaunchPlanner {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
        if ctx.proxy.is_some() != ctx.socks_port.is_some()
            || ctx.profile.proxy_id != ctx.proxy.map(|p| p.id)
            || ctx.socks_port == Some(0)
            || ctx.cdp_port == 0
            || ctx.socks_port == Some(ctx.cdp_port)
        {
            return Err(LaunchPlanError::InvalidArguments(
                "inconsistent proxy configuration or invalid ports".into(),
            ));
        }
        let mut args: Vec<OsString> = Vec::new();

        // 1. user-data-dir
        let user_data_str = ctx.profile.user_data_dir.to_string_lossy();
        args.push(format!("--user-data-dir={user_data_str}").into());

        // 2. remote debugging
        args.push(format!("--remote-debugging-port={}", ctx.cdp_port).into());

        args.push("--remote-debugging-address=127.0.0.1".into());

        // 3. common launch flags
        args.push("--disable-session-crashed-bubble".into());
        args.push("--disable-sync".into());
        args.push("--no-first-run".into());

        // 4. proxy
        if let Some(socks_port) = ctx.socks_port {
            args.push(format!("--proxy-server=socks5://127.0.0.1:{socks_port}").into());
        }

        // 5. fingerprint identity, locale, hardware, WebRTC and spoofing switches
        args.extend(serialize_fingerprint_args(
            &ctx.profile.fingerprint,
            ctx.capabilities,
        ));

        // 6. window
        args.push(
            format!(
                "--window-size={},{}",
                ctx.profile.window.width, ctx.profile.window.height
            )
            .into(),
        );

        // 7. start target
        match &ctx.profile.start_target {
            StartTarget::Blank => args.push("about:blank".into()),
            StartTarget::Url(url) => args.push(url.as_str().into()),
        }

        // Build XrayLaunchPlan if proxy & socks_port are present
        let xray = match (ctx.proxy, ctx.socks_port) {
            (Some(_), Some(socks_port)) => {
                let executable = ctx.xray_executable.clone().unwrap_or_else(|| {
                    PathBuf::from(if cfg!(windows) {
                        "bin/xray.exe"
                    } else {
                        "bin/xray"
                    })
                });
                let config_dir = ctx.xray_config_dir.clone().unwrap_or_else(|| {
                    PathBuf::from("data/runtime").join(ctx.profile.id.to_string())
                });
                let config_path = config_dir.join(crate::xray::XRAY_CONFIG_FILE);

                Some(XrayLaunchPlan {
                    executable,
                    config_path,
                    socks_port,
                })
            }
            _ => None,
        };

        Ok(LaunchPlan {
            profile_id: ctx.profile.id,
            browser_executable: ctx.core.executable.clone(),
            browser_args: args,
            user_data_dir: ctx.profile.user_data_dir.clone(),
            cdp_port: ctx.cdp_port,
            xray,
        })
    }
}
