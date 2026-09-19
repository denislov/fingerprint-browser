pub mod capability;
pub mod cdp;
pub mod error;
pub mod events;
pub mod facade;
pub mod planner;
pub mod ports;
pub mod process;
pub mod supervisor;
pub mod xray;

pub use capability::{CapabilityResolver, DefaultCapabilityResolver};
pub use cdp::{CdpInfo, CdpProbe, HttpCdpProbe};
pub use error::{
    CapabilityError, CdpError, LaunchPlanError, PortError, ProcessError, ProxyError,
    RuntimeCommandError, RuntimeError,
};
pub use events::{RuntimeCommand, RuntimeComponent, RuntimeEvent};
pub use facade::{RuntimeFacade, RuntimeSnapshot};
pub use planner::{DefaultLaunchPlanner, LaunchContext, LaunchPlanner};
pub use ports::{PortAllocator, TcpPortAllocator};
pub use process::{DefaultProcessTreeController, ProcessTreeController};
pub use supervisor::{ChannelRuntimeFacade, RuntimeSupervisor, RuntimeSupervisorChannels};
pub use xray::{DefaultXrayConfigBuilder, XrayConfigBuilder};

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        BrowserBrand, BrowserCore, BrowserProfile, CoreCapabilities, CoreId, FingerprintProfile,
        Platform, ProfileId, ProxyId, ProxyOutbound, ProxyProfile, Socks5Outbound, StartTarget,
        WebRtcPolicy, WindowProfile,
    };
    use std::path::PathBuf;

    fn test_profile() -> BrowserProfile {
        BrowserProfile {
            id: ProfileId::new(),
            name: "Test Planner Profile".to_string(),
            core_id: CoreId::new(),
            user_data_dir: PathBuf::from("data/profiles/test_planner"),
            fingerprint: FingerprintProfile {
                seed: 4242,
                brand: BrowserBrand::Chrome,
                brand_version: None,
                platform: Platform::Windows,
                platform_version: None,
                language: "en-US".to_string(),
                accept_language: "en-US,en;q=0.9".to_string(),
                timezone: "America/New_York".to_string(),
                hardware_concurrency: Some(8),
                webrtc_policy: WebRtcPolicy::DisableNonProxiedUdp,
                disabled_spoofing: vec![],
            },
            proxy_id: None,
            window: WindowProfile::new(1920, 1080),
            start_target: StartTarget::Blank,
        }
    }

    fn test_core() -> BrowserCore {
        BrowserCore {
            id: CoreId::new(),
            name: "Chromium 128".to_string(),
            executable: PathBuf::from("chrome/chrome.exe"),
            version: "128.0.0.0".to_string(),
            major: 128,
        }
    }

    #[test]
    fn test_planner_without_proxy() {
        let profile = test_profile();
        let core = test_core();
        let capabilities = CoreCapabilities::default_for_major(128);
        let planner = DefaultLaunchPlanner::new();

        let ctx = LaunchContext {
            profile: &profile,
            core: &core,
            proxy: None,
            capabilities: &capabilities,
            cdp_port: 9222,
            socks_port: None,
            xray_executable: None,
            xray_config_dir: None,
        };

        let plan = planner.build(ctx).expect("build launch plan");
        assert_eq!(plan.cdp_port, 9222);
        assert!(plan.xray.is_none());

        let args_str: Vec<String> = plan
            .browser_args
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        // Check no proxy flag
        assert!(!args_str.iter().any(|a| a.starts_with("--proxy-server=")));
        // Check fingerprint args
        assert!(args_str.iter().any(|a| a == "--fingerprint=4242"));
        assert!(args_str.iter().any(|a| a == "--fingerprint-brand=Chrome"));
        assert!(
            args_str
                .iter()
                .any(|a| a == "--fingerprint-platform=windows")
        );
        assert!(args_str.iter().any(|a| a == "--window-size=1920,1080"));
        assert!(args_str.iter().any(|a| a == "about:blank"));
    }

    #[test]
    fn test_planner_with_proxy() {
        let profile = test_profile();
        let core = test_core();
        let capabilities = CoreCapabilities::default_for_major(128);
        let planner = DefaultLaunchPlanner::new();

        let proxy = ProxyProfile {
            id: ProxyId::new(),
            name: "Test Socks5".to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "1.2.3.4".to_string(),
                port: 1080,
                username: None,
                password: None,
            }),
        };

        let ctx = LaunchContext {
            profile: &profile,
            core: &core,
            proxy: Some(&proxy),
            capabilities: &capabilities,
            cdp_port: 9222,
            socks_port: Some(51234),
            xray_executable: Some(PathBuf::from("bin/xray.exe")),
            xray_config_dir: Some(PathBuf::from("data/runtime/test")),
        };

        let plan = planner.build(ctx).expect("build launch plan");
        assert!(plan.xray.is_some());
        let xray_plan = plan.xray.unwrap();
        assert_eq!(xray_plan.socks_port, 51234);

        let args_str: Vec<String> = plan
            .browser_args
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        // Must point to local SOCKS only
        assert!(
            args_str
                .iter()
                .any(|a| a == "--proxy-server=socks5://127.0.0.1:51234")
        );
    }

    #[test]
    fn test_planner_deterministic_output() {
        let profile = test_profile();
        let core = test_core();
        let capabilities = CoreCapabilities::default_for_major(128);
        let planner = DefaultLaunchPlanner::new();

        let ctx1 = LaunchContext {
            profile: &profile,
            core: &core,
            proxy: None,
            capabilities: &capabilities,
            cdp_port: 9222,
            socks_port: None,
            xray_executable: None,
            xray_config_dir: None,
        };

        let ctx2 = LaunchContext {
            profile: &profile,
            core: &core,
            proxy: None,
            capabilities: &capabilities,
            cdp_port: 9222,
            socks_port: None,
            xray_executable: None,
            xray_config_dir: None,
        };

        let plan1 = planner.build(ctx1).unwrap();
        let plan2 = planner.build(ctx2).unwrap();

        assert_eq!(plan1.browser_args, plan2.browser_args);
    }

    #[test]
    fn test_port_allocator() {
        let allocator = TcpPortAllocator::new();
        let port1 = allocator
            .allocate_loopback()
            .expect("allocate loopback port");
        let port2 = allocator
            .allocate_loopback()
            .expect("allocate loopback port");
        assert!(port1 > 0);
        assert!(port2 > 0);
    }
}
