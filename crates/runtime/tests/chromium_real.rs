//! CHROMIUM_BIN=/path/chrome XRAY_BIN=/path/xray cargo test -p runtime --test chromium_real -- --ignored
#![cfg(target_os = "linux")]

use ::runtime::supervisor::SupervisorComponents;
use ::runtime::*;
use domain::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
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

struct Harness {
    facade: ChannelRuntimeFacade,
    sender: crossbeam_channel::Sender<RuntimeCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
    dir: PathBuf,
    core: BrowserCore,
}
impl Harness {
    fn new() -> Self {
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
                planner: Box::new(HeadlessPlanner),
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

fn assert_loopback_listener(port: u16) {
    let table = std::fs::read_to_string("/proc/net/tcp").unwrap();
    let suffix = format!(":{port:04X}");
    let addresses: Vec<_> = table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields[3] == "0A" && fields[1].ends_with(&suffix)).then_some(fields[1])
        })
        .collect();
    assert!(!addresses.is_empty());
    assert!(
        addresses
            .iter()
            .all(|address| address.starts_with("0100007F:"))
    );
}

fn assert_group_stopped(group: u32) {
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            continue;
        };
        let fields: Vec<_> = fields.split_whitespace().collect();
        if fields[2].parse::<u32>() == Ok(group) {
            assert!(
                matches!(fields[0], "Z" | "X"),
                "process group {group} still active: {stat}"
            );
        }
    }
}

/// Local authenticated HTTP upstream: records and answers the browser's request.
struct Upstream {
    port: u16,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Upstream {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, stopping) = (hits.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_millis(500)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_millis(500)))
                    .unwrap();
                let Some(header) = read_header(&mut stream) else {
                    continue;
                };
                if !header.starts_with("CONNECT acceptance.test:80 ") {
                    continue;
                }
                assert!(
                    header
                        .to_ascii_lowercase()
                        .contains("proxy-authorization: basic ywxpy2u6c2vjcmv0")
                );
                if stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").is_err() {
                    continue;
                }
                let Some(request) = read_header(&mut stream) else {
                    continue;
                };
                if request.starts_with("GET /acceptance ") {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                );
            }
        });
        Self {
            port,
            hits,
            stop,
            thread: Some(thread),
        }
    }
}
fn read_header(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).ok()?;
        bytes.push(byte[0]);
        if bytes.len() > 16384 {
            return None;
        }
    }
    String::from_utf8(bytes).ok()
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
#[ignore = "requires CHROMIUM_BIN and XRAY_BIN; launches real sandboxed headless Chromium"]
fn profiles_persist_cookies_and_xray_failure_closes_browser() {
    let mut h = Harness::new();
    let first = h.profile(101);
    let second = h.profile(202);
    let a = h.start(&first);
    let b = h.start(&second);
    assert_ne!(a.browser_pid, b.browser_pid);
    assert_ne!(a.cdp_port, b.cdp_port);
    assert_loopback_listener(a.cdp_port.unwrap());
    assert_loopback_listener(b.cdp_port.unwrap());
    assert_ne!(first.user_data_dir, second.user_data_dir);
    assert!(
        a.effective_args
            .iter()
            .any(|arg| arg == "--fingerprint=101")
    );
    assert!(
        b.effective_args
            .iter()
            .any(|arg| arg == "--fingerprint=202")
    );
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
    assert_group_stopped(a.browser_pid.unwrap());
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
        "persistent cookie lost on stop/restart: {cookies}"
    );

    let upstream = Upstream::new();
    let proxy = ProxyProfile {
        id: ProxyId::new(),
        name: "local acceptance".into(),
        outbound: ProxyOutbound::Http(HttpOutbound {
            host: "127.0.0.1".into(),
            port: upstream.port,
            username: Some("alice".into()),
            password: Some("secret".into()),
        }),
    };
    let mut proxied = h.profile(303);
    proxied.proxy_id = Some(proxy.id);
    proxied.start_target = StartTarget::Url("http://acceptance.test/acceptance".into());
    h.facade
        .start(StartParams::with_proxy(
            proxied.clone(),
            h.core.clone(),
            proxy,
        ))
        .unwrap();
    let running = h.wait(proxied.id, RuntimeState::Running);
    let deadline = Instant::now() + Duration::from_secs(5);
    while upstream.hits.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "browser request did not traverse Xray and authenticated upstream"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_loopback_listener(running.socks_port.unwrap());
    let port = running.cdp_port.unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let body = ureq::get(format!("http://127.0.0.1:{port}/json/list"))
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        let targets: Value = serde_json::from_str(&body).unwrap();
        let page = targets
            .as_array()
            .unwrap()
            .iter()
            .find(|target| target["type"] == "page")
            .unwrap();
        let result = cdp_socket(
            port,
            page["webSocketDebuggerUrl"].as_str().unwrap(),
            "Runtime.evaluate",
            json!({"expression": "document.body && document.body.innerText", "returnByValue": true}),
        );
        if result["result"]["value"] == "OK" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "proxied response never rendered: {result}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    // SAFETY: PID comes from this harness's live Xray session.
    assert_eq!(
        unsafe { libc::kill(running.xray_pid.unwrap() as i32, libc::SIGKILL) },
        0
    );
    let stopped = h.wait(proxied.id, RuntimeState::Stopped);
    assert!(stopped.last_error.as_ref().unwrap().contains("Xray"));
    assert!(!PathBuf::from(format!("/proc/{}", running.browser_pid.unwrap())).exists());
    assert_group_stopped(running.browser_pid.unwrap());
    assert!(
        !h.dir
            .join("runtime")
            .join(proxied.id.to_string())
            .join("xray.json")
            .exists()
    );
    // SAFETY: PID belongs to the second live browser started by this test.
    assert_eq!(
        unsafe { libc::kill(b.browser_pid.unwrap() as i32, libc::SIGKILL) },
        0
    );
    h.wait(second.id, RuntimeState::Stopped);
    assert_group_stopped(b.browser_pid.unwrap());
    h.sender.send(RuntimeCommand::ShutdownAll).unwrap();
    h.wait(first.id, RuntimeState::Stopped);
    h.thread.take().unwrap().join().unwrap();
    assert_group_stopped(restarted.browser_pid.unwrap());
}
