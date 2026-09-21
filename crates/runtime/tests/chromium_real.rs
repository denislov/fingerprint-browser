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
    /// Whether this run cleans up after itself. A test that hands the directory
    /// to a second run owns the cleanup, because the second run is still using it.
    cleanup: bool,
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
            cleanup: true,
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

    /// Ends this run the way the "leave browsers running" exit does: the
    /// supervisor marks the sessions and lets its handles go, and this waits for
    /// the thread so that what follows really is a second run.
    fn release_and_end_run(&mut self) -> PathBuf {
        let _ = self.sender.send(RuntimeCommand::ReleaseAll);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.cleanup = false;
        self.dir.clone()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.sender.send(RuntimeCommand::ShutdownAll);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if self.cleanup {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Whether the kernel still has this pid, read the way the supervisor reads it
/// rather than by a bare `kill(0)`.
fn alive(pid: u32) -> bool {
    matches!(
        DefaultProcessInspector.inspect(pid),
        ProcessReading::Live(_)
    )
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

/// The pids of live processes holding a profile, asked of `/proc` rather than of
/// a pid: a browser holds its profile through `--user-data-dir`, which is what
/// this looks for.
///
/// Two shapes have to be handled. An ordinary process keeps its arguments as
/// separate NUL-terminated fields; a process that rewrote its command line -
/// Chromium does - leaves one field holding the whole line, so the flag has to be
/// looked for there as a word.
fn holders(profile_dir: &std::path::Path) -> Vec<u32> {
    let needle = format!("--user-data-dir={}", profile_dir.display());
    let mut holders = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let fields = String::from_utf8_lossy(&cmdline)
            .split('\0')
            .filter(|field| !field.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let separated = fields.iter().any(|field| field == &needle);
        let rewritten =
            fields.len() == 1 && fields[0].split(' ').any(|word| word == needle.as_str());
        if separated || rewritten {
            holders.push(pid);
        }
    }
    holders
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
fn a_start_cancelled_while_waiting_for_readiness_leaves_nothing_running() {
    let h = Harness::with_planner(Box::new(UnreadyPlanner));
    let profile = h.profile(314);
    h.facade
        .start(StartParams::new(profile.clone(), h.core.clone()))
        .unwrap();

    // The browser is spawned before readiness is waited for, so seeing it hold
    // its profile proves the start is under way. Its debugging endpoint is not
    // on the port the supervisor probes, so the start cannot have succeeded.
    let deadline = Instant::now() + Duration::from_secs(15);
    while holders(&profile.user_data_dir).is_empty() {
        assert!(Instant::now() < deadline, "the browser never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        h.facade.snapshot(profile.id).unwrap().state,
        RuntimeState::Starting
    );

    // The supervisor polls for commands while it waits for readiness, so this
    // stop lands inside the start.
    h.facade.stop(profile.id).unwrap();
    h.wait(profile.id, RuntimeState::Stopped);

    assert!(
        holders(&profile.user_data_dir).is_empty(),
        "a cancelled start left a browser behind: {:?}",
        holders(&profile.user_data_dir)
    );
    assert!(
        ::runtime::journal::read(&h.dir.join("runtime"), profile.id)
            .unwrap()
            .is_none(),
        "a cancelled start left a session record to reclaim"
    );
}

/// A supervisor killed outright - `SIGKILL`, a crash - leaves its browsers
/// running and its record on disk. The next start has to stop them from that
/// record alone, against the command line a real Chromium actually runs.
#[test]
#[ignore = "requires CHROMIUM_BIN; launches real sandboxed headless Chromium"]
fn a_browser_left_by_a_killed_run_is_reclaimed() {
    let dir = std::env::temp_dir().join(format!("fp-orphan-{}", ProfileId::new()));
    let runtime_dir = dir.join("runtime");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    let profile = BrowserProfile {
        id: ProfileId::new(),
        name: "orphan acceptance".into(),
        core_id: CoreId::new(),
        user_data_dir: dir.join("profile"),
        fingerprint: FingerprintProfile::new_random(99),
        proxy_id: None,
        window: WindowProfile::new(800, 600),
        start_target: StartTarget::Blank,
    };
    let core = BrowserCore {
        id: CoreId::new(),
        name: "real Chromium".into(),
        executable: std::env::var_os("CHROMIUM_BIN")
            .expect("set CHROMIUM_BIN")
            .into(),
        version: "148".into(),
        major: 148,
    };
    let capabilities = CoreCapabilities::for_major(148);
    let reservation = TcpPortAllocator.reserve_loopback().unwrap();
    let cdp_port = reservation.port();
    let plan = DefaultLaunchPlanner
        .build(LaunchContext {
            profile: &profile,
            core: &core,
            proxy: None,
            capabilities: &capabilities,
            cdp_port,
            socks_port: None,
            xray_executable: None,
            xray_config_dir: None,
        })
        .unwrap();
    let args: Vec<String> = plan
        .browser_args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    // Spawn what the supervisor would spawn, then let the handle go: this
    // process is not the one that started the browser, which is exactly the
    // situation the record exists for.
    let mut command = std::process::Command::new(&plan.browser_executable);
    command
        .args(&plan.browser_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    drop(reservation);
    let child = command.spawn().unwrap();
    let browser_pid = child.id();
    HttpCdpProbe
        .wait_ready(cdp_port, Duration::from_secs(15))
        .expect("a real browser should come up");

    let record = SessionRecord {
        profile_id: profile.id,
        cdp_port,
        socks_port: None,
        started_at: journal::now_millis(),
        // Unmarked: a crash's leftovers, which is what this test is about. A
        // marked record would be adopted instead of reclaimed.
        left_running: false,
        browser: ProcessRecord::captured(
            browser_pid,
            &plan.browser_executable,
            &args,
            &DefaultProcessInspector,
        ),
        xray: None,
    };
    journal::write(&runtime_dir, &record).unwrap();
    // The browser keeps running: dropping the handle does not stop it, which is
    // what a killed supervisor leaves behind.
    drop(child);

    let reclaimed = recover_orphans(
        &runtime_dir,
        &DefaultProcessInspector,
        &DefaultProcessTreeController,
        Duration::from_secs(5),
    );

    assert_eq!(reclaimed.reclaimed.len(), 1, "{reclaimed:?}");
    assert_eq!(reclaimed.reclaimed[0].browser_pid, browser_pid);
    assert!(
        !reclaimed.reclaimed[0].forced,
        "a real browser should exit on request: {reclaimed:?}"
    );
    assert!(
        holders(&profile.user_data_dir).is_empty(),
        "the reclaimed browser is still holding its profile: {:?}",
        holders(&profile.user_data_dir)
    );
    assert!(
        journal::read(&runtime_dir, profile.id).unwrap().is_none(),
        "a reclaimed session leaves no record"
    );
    let _ = std::fs::remove_dir_all(&dir);
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

/// The exit mode that leaves the browsers running, end to end with a real
/// browser: released without being stopped, recorded as deliberately left, taken
/// over by the next run as a running profile, and stoppable by it like any other.
///
/// This is the pair of claims the whole feature rests on. "Keep running" is only
/// true if the browser is still there afterwards, and the next start is only safe
/// if it adopts what it finds instead of either racing it or killing it.
#[test]
#[ignore = "requires CHROMIUM_BIN and XRAY_BIN; launches real sandboxed headless Chromium"]
fn a_browser_left_running_by_one_run_is_adopted_by_the_next() {
    let mut harness = Harness::new();
    let profile = harness.profile(7);
    let running = harness.start(&profile);
    let pid = running.browser_pid.expect("a browser pid");
    let cdp_port = running.cdp_port.expect("a debugging port");
    assert_loopback_listener(cdp_port);

    // The exit that is not a cleanup.
    let dir = harness.release_and_end_run();
    let runtime_dir = dir.join("runtime");

    assert!(
        alive(pid),
        "\"leave browsers running\" must not stop the browser"
    );
    let record = journal::read(&runtime_dir, profile.id)
        .unwrap()
        .expect("a released session keeps its record");
    assert!(
        record.left_running,
        "the record has to say it was left on purpose: {record:?}"
    );
    assert_eq!(record.browser.pid, pid);

    // A second run over the same runtime directory.
    let channels = RuntimeSupervisorChannels::new(128);
    let snapshots = Arc::new(RwLock::new(HashMap::new()));
    let facade = ChannelRuntimeFacade::new(channels.command_tx.clone(), snapshots.clone());
    let mut supervisor = RuntimeSupervisor::with_components(
        channels.command_rx,
        channels.event_tx,
        snapshots,
        SupervisorComponents {
            planner: Box::new(HeadlessPlanner),
            xray_executable: std::env::var_os("XRAY_BIN").expect("set XRAY_BIN").into(),
            runtime_dir: runtime_dir.clone(),
            ..Default::default()
        },
    );
    let report = supervisor.recover_orphans();
    assert_eq!(report.adopted.len(), 1, "{report:?}");
    assert!(
        report.reclaimed.is_empty(),
        "a marked session must not be reclaimed: {report:?}"
    );
    assert!(alive(pid), "adopting must not stop it either");

    // And it is a running profile again, not a record on disk: the snapshots say
    // so, with the ports and the pid the previous run used.
    let adopted = facade
        .snapshot(profile.id)
        .expect("an adopted session is in the snapshots");
    assert_eq!(adopted.state, RuntimeState::Running);
    assert_eq!(adopted.browser_pid, Some(pid));
    assert_eq!(adopted.cdp_port, Some(cdp_port));

    let thread = supervisor.spawn();

    // An ordinary stop stops it - the adopted session has no handle, so this is
    // the path that rechecks the recorded identity and stops the tree by pid.
    facade.stop(profile.id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if facade.snapshot(profile.id).map(|s| s.state) == Some(RuntimeState::Stopped) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "waiting for the adopted session to stop: {:?}",
            facade.snapshot(profile.id)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !alive(pid),
        "a stopped adopted session must not leave its browser running"
    );
    // The supervisor puts the browser in its own group, so the leader's pid is
    // the group - which is what the other acceptance tests pass here too.
    assert_group_stopped(pid);
    assert!(
        journal::read(&runtime_dir, profile.id).unwrap().is_none(),
        "a stopped session leaves no record"
    );

    let _ = channels.command_tx.send(RuntimeCommand::ShutdownAll);
    let _ = thread.join();
    let _ = std::fs::remove_dir_all(&dir);
}
