use crate::error::LaunchPlanError;
use domain::{
    BrowserCore, BrowserProfile, CoreCapabilities, LaunchPlan, ProxyProfile, StartTarget,
    WebRtcPolicy, XrayLaunchPlan,
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
        let mut args: Vec<OsString> = Vec::new();

        // 1. user-data-dir
        let user_data_str = ctx.profile.user_data_dir.to_string_lossy();
        args.push(format!("--user-data-dir={user_data_str}").into());

        // 2. remote debugging
        args.push(format!("--remote-debugging-port={}", ctx.cdp_port).into());

        // 3. common launch flags
        args.push("--disable-session-crashed-bubble".into());
        args.push("--disable-sync".into());
        args.push("--no-first-run".into());

        // 4. proxy
        if let Some(socks_port) = ctx.socks_port {
            args.push(format!("--proxy-server=socks5://127.0.0.1:{socks_port}").into());
        }

        // 5. fingerprint seed
        args.push(format!("--fingerprint={}", ctx.profile.fingerprint.seed).into());

        // 6. identity
        args.push(
            format!(
                "--fingerprint-brand={}",
                ctx.profile.fingerprint.brand.as_arg_value()
            )
            .into(),
        );
        args.push(
            format!(
                "--fingerprint-platform={}",
                ctx.profile.fingerprint.platform.as_arg_value()
            )
            .into(),
        );

        // 7. locale / timezone
        args.push(format!("--lang={}", ctx.profile.fingerprint.language).into());
        args.push(format!("--accept-lang={}", ctx.profile.fingerprint.accept_language).into());
        args.push(format!("--timezone={}", ctx.profile.fingerprint.timezone).into());

        // 8. hardware
        if let Some(concurrency) = ctx.profile.fingerprint.hardware_concurrency {
            args.push(format!("--fingerprint-hardware-concurrency={concurrency}").into());
        }

        // 9. WebRTC / spoofing switches
        if ctx.profile.fingerprint.webrtc_policy == WebRtcPolicy::DisableNonProxiedUdp {
            args.push("--disable-non-proxied-udp".into());
        }

        if ctx.capabilities.supports_canvas_noise_flag {
            args.push("--fingerprinting-canvas-image-data-noise".into());
            args.push("--fingerprinting-client-rects-noise".into());
        }

        if ctx.capabilities.supports_disable_spoofing
            && !ctx.profile.fingerprint.disabled_spoofing.is_empty()
        {
            let features: Vec<String> = ctx
                .profile
                .fingerprint
                .disabled_spoofing
                .iter()
                .map(|f| f.as_flag_name().to_string())
                .collect();
            args.push(format!("--disable-spoofing={}", features.join(",")).into());
        }

        // 10. window
        args.push(
            format!(
                "--window-size={},{}",
                ctx.profile.window.width, ctx.profile.window.height
            )
            .into(),
        );

        // 11. start target
        match &ctx.profile.start_target {
            StartTarget::Blank => args.push("about:blank".into()),
            StartTarget::Url(url) => args.push(url.as_str().into()),
        }

        // Build XrayLaunchPlan if proxy & socks_port are present
        let xray = match (ctx.proxy, ctx.socks_port) {
            (Some(_), Some(socks_port)) => {
                let executable = ctx
                    .xray_executable
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("bin/xray.exe"));
                let config_dir = ctx.xray_config_dir.clone().unwrap_or_else(|| {
                    PathBuf::from("data/runtime").join(ctx.profile.id.to_string())
                });
                let config_path = config_dir.join("xray.json");

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
