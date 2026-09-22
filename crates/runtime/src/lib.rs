pub mod capability;
pub mod cdp;
pub mod compat;
pub mod diagnostic;
pub mod egress;
pub mod error;
pub mod events;
pub mod facade;
pub mod fingerprint_args;
pub mod journal;
pub mod planner;
pub mod ports;
pub mod process;
pub mod supervisor;
pub mod verify;
pub mod version;
pub mod xray;

pub use capability::{CapabilityResolver, DefaultCapabilityResolver};
pub use cdp::{CdpInfo, CdpProbe, CdpSession, CdpTarget, DocumentState, HttpCdpProbe};
pub use compat::{CompatibilityReport, Finding as CompatibilityFinding};
pub use diagnostic::{
    DEFAULT_ECHO_URL, Diagnosis, DiagnosticRequest, EchoClient, Engine, Fault, FaultClass,
    SocksEchoClient, check_echo_url, classify_engine_log, diagnose as diagnose_proxy,
    extract_exit_ip,
};
pub use egress::{EgressOutcome, EgressProbe, EgressReading, verify_egress};
pub use error::{
    CapabilityError, CdpError, JournalError, LaunchPlanError, PortError, ProcessError, ProxyError,
    RuntimeCommandError, RuntimeError,
};
pub use events::{RuntimeCommand, RuntimeComponent, RuntimeEvent, StartParams};
pub use facade::{RuntimeFacade, RuntimeSnapshot};
pub use fingerprint_args::serialize_fingerprint_args;
pub use journal::{
    Adopted, ProcessRecord, ReclaimReport, Reclaimed, SessionRecord,
    Unresolved as UnresolvedSession, recover as recover_orphans,
};
pub use planner::{DefaultLaunchPlanner, LaunchContext, LaunchPlanner};
pub use ports::{PortAllocator, PortReservation, TcpPortAllocator};
pub use process::{
    DefaultProcessInspector, DefaultProcessTreeController, ProcessIdentity, ProcessInspector,
    ProcessReading, ProcessTreeController,
};
pub use supervisor::{
    ChannelRuntimeFacade, RuntimeSupervisor, RuntimeSupervisorChannels, SupervisorComponents,
};
pub use verify::{
    Discrepancy, FingerprintProbe, ObservedFingerprint, PROBE_EXPRESSION,
    verify as verify_fingerprint,
};
pub use version::{DEFAULT_TIMEOUT as DEFAULT_VERSION_TIMEOUT, VersionReport, parse_major};
pub use xray::{DefaultXrayConfigBuilder, XrayConfigBuilder};

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        BrowserBrand, BrowserCore, BrowserProfile, CoreCapabilities, CoreId, FingerprintProfile,
        Platform, ProfileId, ProxyId, ProxyOutbound, ProxyProfile, RuntimeState, Socks5Outbound,
        SpoofingFeature, StartTarget, WebRtcPolicy, WindowProfile,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;

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
        let capabilities = CoreCapabilities::for_major(128);
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
        let mut profile = test_profile();
        let core = test_core();
        let capabilities = CoreCapabilities::for_major(128);
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

        profile.proxy_id = Some(proxy.id);

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
    fn test_planner_emits_brand_and_platform_versions() {
        let mut profile = test_profile();
        profile.fingerprint.brand_version = Some("144.0.7559.132".to_string());
        profile.fingerprint.platform_version = Some("10.0.0".to_string());

        let core = test_core();
        let capabilities = CoreCapabilities::for_major(128);
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
        let args_str: Vec<String> = plan
            .browser_args
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        assert!(
            args_str
                .iter()
                .any(|a| a == "--fingerprint-brand-version=144.0.7559.132"),
            "brand version must reach the launch plan, got {args_str:?}"
        );
        assert!(
            args_str
                .iter()
                .any(|a| a == "--fingerprint-platform-version=10.0.0"),
            "platform version must reach the launch plan, got {args_str:?}"
        );
    }

    /// The value a profile's start target becomes on the command line. A target
    /// that spells a switch *is* that switch - Chromium does not care which
    /// argument it is - so the planner refuses it rather than appending it.
    #[test]
    fn a_start_page_that_is_a_switch_never_becomes_an_argument() {
        let mut profile = test_profile();
        let core = test_core();
        let capabilities = CoreCapabilities::for_major(128);
        let planner = DefaultLaunchPlanner::new();
        profile.start_target = StartTarget::Url("--no-proxy-server".to_string());

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

        let error = planner
            .build(ctx)
            .expect_err("a switch is not a start page");
        assert!(
            matches!(error, LaunchPlanError::StartTarget(_)),
            "{error:?}"
        );

        // And a page is still the last argument, so the check did not change what
        // a usable target produces.
        let mut profile = test_profile();
        profile.start_target = StartTarget::Url("https://example.com".to_string());
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
        let plan = planner.build(ctx).expect("a page is a plan");
        assert_eq!(
            plan.browser_args.last().expect("arguments"),
            "https://example.com"
        );
    }

    #[test]
    fn test_planner_deterministic_output() {
        let profile = test_profile();
        let core = test_core();
        let capabilities = CoreCapabilities::for_major(128);
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

    #[test]
    fn test_supervisor_start_nonexistent_executable_fails() {
        let channels = RuntimeSupervisorChannels::new(16);
        let snapshots = Arc::new(RwLock::new(HashMap::new()));
        let supervisor = RuntimeSupervisor::new(
            channels.command_rx,
            channels.event_tx,
            Arc::clone(&snapshots),
        );

        let facade = ChannelRuntimeFacade::new(channels.command_tx.clone(), Arc::clone(&snapshots));
        let handle = supervisor.spawn();

        let mut profile = test_profile();
        // A legacy core has something to report: it ignores the exclusions.
        profile.fingerprint.disabled_spoofing = vec![SpoofingFeature::Font];
        let mut core = test_core();
        core.executable = PathBuf::from("non_existent_chrome_binary_xyz123.exe");

        let profile_id = profile.id;
        facade
            .start(StartParams::new(profile, core))
            .expect("send start command");

        // First event should be StateChanged(Starting)
        let event1 = channels
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first event");
        assert!(matches!(
            event1,
            RuntimeEvent::StateChanged {
                state: RuntimeState::Starting,
                ..
            }
        ));

        // A legacy core reports the switches it cannot honour before planning.
        let warning = channels
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("compatibility warning");
        match warning {
            RuntimeEvent::Warning { message, .. } => {
                assert!(
                    message.contains("verified fingerprint generation"),
                    "{message}"
                );
            }
            other => panic!("expected a compatibility warning, got {other:?}"),
        }

        // Next event is EffectiveLaunchArgs
        let _event2 = channels
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("effective args");

        // Next event should be StateChanged(Failed)
        let event3 = channels
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("failed event");
        match event3 {
            RuntimeEvent::StateChanged {
                profile_id: pid,
                state: RuntimeState::Failed { message },
            } => {
                assert_eq!(pid, profile_id);
                assert!(message.contains("failed to spawn executable"));
            }
            other => panic!("expected failed state, got {other:?}"),
        }

        // Test stop idempotent
        facade.stop(profile_id).expect("send stop command");
        let stop_event = channels
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("stop event");
        assert!(matches!(stop_event, RuntimeEvent::Stopped { .. }));

        // Shutdown
        channels
            .command_tx
            .send(RuntimeCommand::ShutdownAll)
            .expect("shutdown");
        handle.join().expect("join supervisor thread");
    }
}

/// Builders for the values tests start things with.
///
/// Portable on purpose. The parts of the supervisor that can be driven with fake
/// components are tested on every platform, and anything those tests need has to
/// build without a shell script or a Unix permission bit - which is what kept the
/// command queue's own rules inside the Unix-only test module before.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::events::StartParams;
    use domain::{
        BrowserCore, BrowserProfile, CoreId, FingerprintProfile, ProxyId, ProxyOutbound,
        ProxyProfile, Socks5Outbound, StartTarget, WindowProfile,
    };
    use std::path::Path;

    /// The parameters a test starts a profile with, named after nothing: the
    /// identifier is minted here.
    pub(crate) fn start_params(dir: &Path) -> StartParams {
        start_params_for(domain::ProfileId::new(), dir)
    }

    /// The same, with the identifier given - for a test that has already named a
    /// directory after it.
    pub(crate) fn start_params_for(id: domain::ProfileId, dir: &Path) -> StartParams {
        let proxy = ProxyProfile {
            id: ProxyId::new(),
            name: "test".into(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "localhost".into(),
                port: 1080,
                username: Some("user".into()),
                password: Some("secret".into()),
            }),
        };
        let core = BrowserCore {
            id: CoreId::new(),
            name: "test".into(),
            executable: "/bin/sleep".into(),
            version: "128".into(),
            major: 128,
        };
        let profile = BrowserProfile {
            id,
            name: "test".into(),
            core_id: core.id,
            user_data_dir: dir.join("profile"),
            fingerprint: FingerprintProfile::new_random(42),
            proxy_id: Some(proxy.id),
            window: WindowProfile::new(800, 600),
            start_target: StartTarget::Blank,
        };
        StartParams::with_proxy(profile, core, proxy)
    }
}
