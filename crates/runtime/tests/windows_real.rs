//! Opt-in Windows lifecycle acceptance using local Chromium and Xray binaries.
#![cfg(windows)]
use ::runtime::*;
use domain::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
struct HeadlessPlanner;
impl LaunchPlanner for HeadlessPlanner {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
        let mut plan = DefaultLaunchPlanner.build(ctx)?;
        for flag in [
            "--headless",
            "--disable-gpu",
            "--disable-background-networking",
            "--proxy-bypass-list=<-loopback>",
        ] {
            plan.browser_args.insert(0, flag.into());
        }
        Ok(plan)
    }
}

/// Headless, and with its debugging endpoint moved off the port the supervisor
/// waits for, so a start can be held inside the readiness wait on purpose.
struct UnreadyPlanner;
impl LaunchPlanner for UnreadyPlanner {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
        let mut plan = HeadlessPlanner.build(ctx)?;
        // The last occurrence wins, so the endpoint the supervisor probes is
        // never opened and this browser stays "starting" until it is stopped.
        plan.browser_args.push("--remote-debugging-port=0".into());
        Ok(plan)
    }
}

struct Harness {
    facade: ChannelRuntimeFacade,
    sender: crossbeam_channel::Sender<RuntimeCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
    dir: PathBuf,
    core: BrowserCore,
}

fn gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while DefaultProcessInspector.inspect(pid) != ProcessReading::Absent {
        assert!(Instant::now() < deadline, "process {pid} survived cleanup");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
#[ignore = "requires CHROMIUM_BIN and XRAY_BIN"]
fn windows_profiles_preserve_cookies_and_fail_closed() {
    let mut h = Harness::new();
    let first = h.profile(101);
    let second = h.profile(202);
    let a = h.start(&first);
    let b = h.start(&second);
    assert_ne!(a.browser_pid, b.browser_pid);
    assert_ne!(a.cdp_port, b.cdp_port);
    cdp(
        a.cdp_port.unwrap(),
        "Storage.setCookies",
        json!({"cookies": [{"name": "persist", "value": "first-only", "domain": "acceptance.test", "path": "/", "expires": 4102444800.0}]}),
    );
    assert!(
        cdp(b.cdp_port.unwrap(), "Storage.getCookies", json!({}))["cookies"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    h.facade.stop(first.id).unwrap();
    h.wait(first.id, RuntimeState::Stopped);
    gone(a.browser_pid.unwrap());
    assert_eq!(
        h.facade.snapshot(second.id).unwrap().state,
        RuntimeState::Running
    );
    let restarted = h.start(&first);
    let cookies = cdp(restarted.cdp_port.unwrap(), "Storage.getCookies", json!({}));
    assert!(
        cookies["cookies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cookie| cookie["name"] == "persist" && cookie["value"] == "first-only"),
        "{cookies}"
    );

    // Only test local Xray startup/failure here; remote forwarding is a separate acceptance.
    let upstream = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = ProxyProfile {
        id: ProxyId::new(),
        name: "Windows acceptance".into(),
        outbound: ProxyOutbound::Socks5(Socks5Outbound {
            host: "127.0.0.1".into(),
            port: upstream.local_addr().unwrap().port(),
            username: None,
            password: None,
        }),
    };
    let mut proxied = h.profile(303);
    proxied.proxy_id = Some(proxy.id);
    h.facade
        .start(StartParams::with_proxy(
            proxied.clone(),
            h.core.clone(),
            proxy,
        ))
        .unwrap();
    let running = h.wait(proxied.id, RuntimeState::Running);
    DefaultProcessTreeController
        .terminate_tree(running.xray_pid.unwrap())
        .unwrap();
    let stopped = h.wait(proxied.id, RuntimeState::Stopped);
    assert!(stopped.last_error.unwrap().contains("Xray"));
    gone(running.browser_pid.unwrap());
    assert!(
        !h.dir
            .join("runtime")
            .join(proxied.id.to_string())
            .join("xray.json")
            .exists()
    );

    // Closing Chromium normally is a stop, while a killed browser reports a crash.
    HttpCdpProbe
        .close_browser(b.cdp_port.unwrap(), Duration::from_secs(2))
        .unwrap();
    let closed = h.wait(second.id, RuntimeState::Stopped);
    assert!(closed.last_error.is_none(), "{closed:?}");
    let b = h.start(&second);
    DefaultProcessTreeController
        .terminate_tree(b.browser_pid.unwrap())
        .unwrap();
    let crashed = h.wait(second.id, RuntimeState::Stopped);
    assert!(crashed.last_error.is_some(), "{crashed:?}");
    h.sender.send(RuntimeCommand::ShutdownAll).unwrap();
    h.wait(first.id, RuntimeState::Stopped);
    h.thread.take().unwrap().join().unwrap();
    gone(restarted.browser_pid.unwrap());
}

#[test]
#[ignore = "requires CHROMIUM_BIN and XRAY_BIN"]
fn windows_cancel_during_readiness_cleans_the_session() {
    let h = Harness::with_planner(Box::new(UnreadyPlanner));
    let profile = h.profile(404);
    h.facade
        .start(StartParams::new(profile.clone(), h.core.clone()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let entry = loop {
        if let Some(entry) = journal::read(&h.dir.join("runtime"), profile.id).unwrap() {
            break entry;
        }
        assert!(Instant::now() < deadline, "no startup record");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(matches!(
        DefaultProcessInspector.inspect(entry.browser.pid),
        ProcessReading::Live(_)
    ));
    h.facade.stop(profile.id).unwrap();
    h.wait(profile.id, RuntimeState::Stopped);
    gone(entry.browser.pid);
    assert!(
        journal::read(&h.dir.join("runtime"), profile.id)
            .unwrap()
            .is_none()
    );
}

#[test]
#[ignore = "internal subprocess fixture for Windows manager crash acceptance"]
fn windows_manager_process() {
    let Some(output) = std::env::var_os("FP_WINDOWS_ACCEPTANCE_OUTPUT") else {
        return;
    };
    let h = Harness::new();
    let profile = h.profile(505);
    let running = h.start(&profile);
    std::fs::write(
        output,
        serde_json::to_vec(&json!({
            "pid": running.browser_pid.unwrap(), "directory": h.dir,
            "profile": profile.id,
        }))
        .unwrap(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(60));
}

#[test]
#[ignore = "requires CHROMIUM_BIN and XRAY_BIN"]
fn windows_manager_crash_kills_browser_and_next_start_clears_record() {
    use std::os::windows::process::CommandExt;
    let h = Harness::new();
    let unrelated = h.profile(606);
    let running = h.start(&unrelated);
    let output = h.dir.join("child-manager.json");
    let mut manager = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "windows_manager_process"])
        .env("FP_WINDOWS_ACCEPTANCE_OUTPUT", &output)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let result: Value = loop {
        if let Ok(bytes) = std::fs::read(&output)
            && let Ok(result) = serde_json::from_slice(&bytes)
        {
            break result;
        }
        if Instant::now() >= deadline || manager.try_wait().unwrap().is_some() {
            let _ = manager.kill();
            let _ = manager.wait();
            panic!("manager did not start its browser");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let pid = result["pid"].as_u64().unwrap() as u32;
    manager.kill().unwrap();
    manager.wait().unwrap();
    gone(pid);
    assert!(matches!(
        DefaultProcessInspector.inspect(running.browser_pid.unwrap()),
        ProcessReading::Live(_)
    ));
    let directory = PathBuf::from(result["directory"].as_str().unwrap());
    // The helper writes only its unique temp fixture; do not remove any other path.
    assert!(directory.starts_with(std::env::temp_dir()));
    assert!(
        directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("fp-chromium-")
    );
    let report = reclaim_orphans(
        &directory.join("runtime"),
        &DefaultProcessInspector,
        &DefaultProcessTreeController,
        Duration::ZERO,
    );
    assert!(report.unresolved.is_empty(), "{report:?}");
    assert_eq!(report.stale.len(), 1, "{report:?}");
    std::fs::remove_dir_all(directory).unwrap();
}
impl Harness {
    fn new() -> Self {
        Self::with_planner(Box::new(HeadlessPlanner))
    }
    fn with_planner(planner: Box<dyn LaunchPlanner>) -> Self {
        let dir = std::env::temp_dir().join(format!("fp-chromium-{}", ProfileId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let channels = RuntimeSupervisorChannels::new(128);
        let snapshots = Arc::new(RwLock::new(HashMap::new()));
        let facade = ChannelRuntimeFacade::new(channels.command_tx.clone(), snapshots.clone());
        let supervisor = RuntimeSupervisor::with_components(
            channels.command_rx,
            channels.event_tx,
            snapshots,
            SupervisorComponents {
                planner,
                xray_executable: std::env::var_os("XRAY_BIN").expect("set XRAY_BIN").into(),
                runtime_dir: dir.join("runtime"),
                ..Default::default()
            },
        );
        Self {
            facade,
            sender: channels.command_tx,
            thread: Some(supervisor.spawn()),
            dir,
            core: BrowserCore {
                id: CoreId::new(),
                name: "real Chromium".into(),
                executable: std::env::var_os("CHROMIUM_BIN")
                    .expect("set CHROMIUM_BIN")
                    .into(),
                version: "148".into(),
                major: 148,
            },
        }
    }
    fn profile(&self, seed: u32) -> BrowserProfile {
        let id = ProfileId::new();
        BrowserProfile {
            id,
            name: format!("acceptance-{seed}"),
            core_id: self.core.id,
            user_data_dir: self.dir.join(id.to_string()),
            fingerprint: FingerprintProfile::new_random(seed),
            proxy_id: None,
            window: WindowProfile::new(800, 600),
            start_target: StartTarget::Blank,
        }
    }
    fn wait(&self, id: ProfileId, expected: RuntimeState) -> RuntimeSnapshot {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(snapshot) = self.facade.snapshot(id) {
                assert!(
                    !matches!(snapshot.state, RuntimeState::Failed { .. }),
                    "startup failed: {snapshot:?}"
                );
                if snapshot.state == expected {
                    return snapshot;
                }
            }
            assert!(
                Instant::now() < deadline,
                "waiting for {expected:?}: {:?}",
                self.facade.snapshot(id)
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    fn start(&self, profile: &BrowserProfile) -> RuntimeSnapshot {
        self.facade
            .start(StartParams::new(profile.clone(), self.core.clone()))
            .unwrap();
        self.wait(profile.id, RuntimeState::Running)
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.sender.send(RuntimeCommand::ShutdownAll);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn cdp(port: u16, method: &str, params: Value) -> Value {
    let info = HttpCdpProbe
        .wait_ready(port, Duration::from_secs(2))
        .unwrap();
    cdp_socket(port, &info.web_socket_debugger_url, method, params)
}

fn cdp_socket(port: u16, url: &str, method: &str, params: Value) -> Value {
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let (mut socket, _) = tungstenite::client(url, stream).unwrap();
    socket
        .send(tungstenite::Message::Text(
            json!({"id": 1, "method": method, "params": params})
                .to_string()
                .into(),
        ))
        .unwrap();
    loop {
        let response = socket.read().unwrap();
        if let tungstenite::Message::Text(text) = response {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["id"] == 1 {
                assert!(value.get("error").is_none(), "CDP {method}: {value}");
                return value["result"].clone();
            }
        }
    }
}
