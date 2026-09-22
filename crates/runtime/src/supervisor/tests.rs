use super::*;
use crate::process::{ProcessIdentity, ProcessReading};
use crate::{CdpError, CdpInfo, LaunchPlanError, ProcessError};
use domain::*;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

struct Probe(bool);
impl CdpProbe for Probe {
    fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
        if self.0 {
            Ok(CdpInfo::default())
        } else {
            Err(CdpError::Timeout { timeout_secs: 0 })
        }
    }
}
struct Planner(bool);
impl LaunchPlanner for Planner {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
        let mut plan = DefaultLaunchPlanner.build(ctx)?;
        plan.browser_executable = if self.0 {
            "/bin/sleep"
        } else {
            "/nonexistent/browser"
        }
        .into();
        plan.browser_args = vec!["60".into()];
        Ok(plan)
    }
}
struct Fixture {
    _serial: std::sync::MutexGuard<'static, ()>,
    commands: Option<Sender<RuntimeCommand>>,
    supervisor: RuntimeSupervisor,
    events: Receiver<RuntimeEvent>,
    params: StartParams,
    dir: PathBuf,
}
impl Fixture {
    fn new(browser: bool, cdp: bool, script: &str) -> Self {
        // Avoid fork/exec races with another test writing its executable
        // and immediate port-rebind assertions racing sibling fixtures.
        static FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let serial = FIXTURES.lock().unwrap_or_else(|error| error.into_inner());
        let id = ProfileId::new();
        let dir = std::env::temp_dir().join(format!("fp-runtime-{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        let executable = dir.join("xray");
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let components = SupervisorComponents {
            planner: Box::new(Planner(browser)),
            cdp_probe: Box::new(Probe(cdp)),
            xray_executable: executable,
            runtime_dir: dir.clone(),
            xray_ready_timeout: Duration::from_millis(500),
            cdp_ready_timeout: Duration::from_millis(200),
            ..Default::default()
        };
        let channels = RuntimeSupervisorChannels::new(128);
        let supervisor = RuntimeSupervisor::with_components(
            channels.command_rx,
            channels.event_tx,
            Arc::new(RwLock::new(HashMap::new())),
            components,
        );
        Self {
            _serial: serial,
            commands: Some(channels.command_tx),
            supervisor,
            events: channels.event_rx,
            params: crate::test_support::start_params_for(id, &dir),
            dir,
        }
    }
    fn start(&mut self) {
        self.supervisor.start_profile(self.params.clone());
    }
    fn config(&self) -> PathBuf {
        self.dir
            .join(self.params.profile.id.to_string())
            .join("xray.json")
    }
    fn snapshot(&self) -> RuntimeSnapshot {
        self.supervisor.snapshots.read().unwrap()[&self.params.profile.id].clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.supervisor.cleanup_all();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
const XRAY: &str = "#!/usr/bin/env python3\nimport json,socket,sys,time\nc=json.load(open(sys.argv[3]))\ns=socket.socket()\ns.bind(('127.0.0.1',c['inbounds'][0]['port']))\ns.listen()\ntime.sleep(60)\n";

#[test]
fn a_running_session_writes_a_record_and_stopping_it_removes_it() {
    let mut f = Fixture::new(true, true, XRAY);
    f.start();

    let record = journal::read(&f.dir, f.params.profile.id)
        .unwrap()
        .expect("a running session writes a record");
    let session = &f.supervisor.active_sessions[&f.params.profile.id];
    assert_eq!(record.browser.pid, session.browser.id());
    assert_eq!(
        record.xray.as_ref().map(|xray| xray.pid),
        session.xray.as_ref().map(|xray| xray.id())
    );
    assert_eq!(record.cdp_port, f.snapshot().cdp_port.unwrap());
    assert_eq!(record.browser.args, ["60"]);
    let xray = record.xray.as_ref().unwrap();
    assert_eq!(xray.args[0], "run");
    assert_eq!(xray.args[2], f.config().to_string_lossy());

    f.supervisor.stop_profile(f.params.profile.id);
    assert!(
        journal::read(&f.dir, f.params.profile.id)
            .unwrap()
            .is_none(),
        "a stopped session leaves no record to reclaim"
    );
}

/// A start that rolls back has to take its record with it: a record naming
/// processes that are already gone would be reported as a leftover at the
/// next start.
#[test]
fn a_start_that_never_reaches_readiness_leaves_no_record() {
    // The browser starts (it is `/bin/sleep`) but its debugging endpoint
    // never answers, so the start fails and rolls back.
    let mut f = Fixture::new(true, false, XRAY);
    f.start();

    assert!(
        matches!(f.snapshot().state, RuntimeState::Failed { .. }),
        "{:?}",
        f.snapshot().state
    );
    assert!(
        journal::read(&f.dir, f.params.profile.id)
            .unwrap()
            .is_none(),
        "a rolled back start left a record behind"
    );
    assert!(!f.config().exists());
    assert!(
        f.supervisor.recover_orphans().is_empty(),
        "a rolled back start leaves nothing to reclaim"
    );
}

/// A process that was killed - `SIGKILL`, a crash, a window destroyed
/// without the close protocol - leaves its children running and its record
/// on disk. The next start has to find them from that record alone.
#[test]
fn reclaiming_orphans_stops_what_a_killed_run_left_running() {
    let mut f = Fixture::new(true, true, XRAY);
    f.start();
    let record = journal::read(&f.dir, f.params.profile.id)
        .unwrap()
        .expect("a running session writes a record");

    // Taking the handles away does not stop the children; that is what
    // makes this the crash case rather than a stop.
    let session = f
        .supervisor
        .active_sessions
        .remove(&f.params.profile.id)
        .unwrap();
    drop(session);
    assert!(
        matches!(
            DefaultProcessInspector.inspect(record.browser.pid),
            ProcessReading::Live(_)
        ),
        "the browser should still be running for this test to mean anything"
    );

    let report = f.supervisor.recover_orphans();

    assert_eq!(report.reclaimed.len(), 1, "{report:?}");
    assert_eq!(report.reclaimed[0].browser_pid, record.browser.pid);
    assert_eq!(
        report.reclaimed[0].xray_pid,
        record.xray.as_ref().map(|xray| xray.pid)
    );
    assert_eq!(
        DefaultProcessInspector.inspect(record.browser.pid),
        ProcessReading::Absent
    );
    assert!(
        journal::read(&f.dir, f.params.profile.id)
            .unwrap()
            .is_none()
    );
    assert!(
        !f.config().exists(),
        "a killed run leaves the upstream credentials in its temporary config"
    );
}

#[test]
fn normal_stop_attempts_graceful_close_but_xray_crash_skips_it() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct ClosingProbe(Arc<AtomicUsize>);
    impl CdpProbe for ClosingProbe {
        fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
            Ok(CdpInfo::default())
        }
        fn close_browser(&self, _: u16, _: Duration) -> Result<(), CdpError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(CdpError::Http("simulate unavailable CDP".into()))
        }
    }
    let mut f = Fixture::new(true, true, XRAY);
    let calls = Arc::new(AtomicUsize::new(0));
    f.supervisor.cdp_probe = Box::new(ClosingProbe(calls.clone()));
    f.start();
    f.supervisor.stop_profile(f.params.profile.id);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    f.start();
    let xray = f
        .supervisor
        .active_sessions
        .get_mut(&f.params.profile.id)
        .unwrap()
        .xray
        .as_mut()
        .unwrap();
    xray.owned_mut().kill().unwrap();
    xray.owned_mut().wait().unwrap();
    f.supervisor.poll_active_sessions();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
}

#[test]
fn a_legacy_core_reports_the_switches_it_cannot_honour() {
    let mut f = Fixture::new(true, true, XRAY);
    // Measured on 142: the exclusions are what a legacy core ignores. The
    // noise switch is carried below the pivot, so it is not the omission to
    // expect here.
    f.params.profile.fingerprint.disabled_spoofing = vec![SpoofingFeature::Font];
    f.start();

    let warning = f
        .snapshot()
        .last_warning
        .clone()
        .expect("compatibility warning");
    assert!(warning.contains("--disable-spoofing"), "{warning}");
    assert!(warning.contains("major 144"), "{warning}");

    // The same fixture with a verified core reports nothing.
    f.supervisor.stop_profile(f.params.profile.id);
    f.params.core.major = 148;
    f.params.core.version = "148.0.7778.215".into();
    f.start();

    assert_eq!(f.snapshot().state, RuntimeState::Running);
    assert_eq!(f.snapshot().last_warning, None);
}

#[test]
fn full_event_queue_does_not_block_stop_crash_or_shutdown() {
    let mut f = Fixture::new(true, true, XRAY);
    // A warning has to exist for the checks below to be about replacing it.
    f.params.profile.fingerprint.disabled_spoofing = vec![SpoofingFeature::Font];
    let (sender, receiver) = crossbeam_channel::bounded(1);
    f.supervisor.event_tx = sender;
    f.events = receiver;
    f.start();
    assert_eq!(f.snapshot().state, RuntimeState::Running);
    assert!(f.snapshot().dropped_events > 0);
    assert!(!f.snapshot().effective_args.is_empty());
    f.start();
    let already_running = f.snapshot().last_warning.clone().expect("warning");
    assert!(
        already_running.contains("already running"),
        "{already_running}"
    );
    f.supervisor.stop_profile(f.params.profile.id);
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(!f.config().exists());
    f.start();
    let fresh = f.snapshot().last_warning.clone().expect("warning");
    assert!(
        !fresh.contains("already running"),
        "a new start must replace the previous diagnostics: {fresh}"
    );
    let child = f
        .supervisor
        .active_sessions
        .get_mut(&f.params.profile.id)
        .unwrap()
        .xray
        .as_mut()
        .unwrap();
    child.owned_mut().kill().unwrap();
    child.owned_mut().wait().unwrap();
    f.supervisor.poll_active_sessions();
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(f.snapshot().last_error.as_ref().unwrap().contains("Xray"));
    assert!(!f.config().exists());
    f.start();
    assert!(f.snapshot().last_error.is_none());
    assert!(!f.supervisor.handle_command(RuntimeCommand::ShutdownAll));
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(f.supervisor.active_sessions.is_empty());
    assert!(!f.config().exists());
}

#[test]
fn disconnected_event_receiver_preserves_failure_diagnostics() {
    let mut f = Fixture::new(false, true, XRAY);
    let (sender, receiver) = crossbeam_channel::bounded(1);
    drop(receiver);
    f.supervisor.event_tx = sender;
    f.start();
    let snapshot = f.snapshot();
    assert!(matches!(snapshot.state, RuntimeState::Failed { .. }));
    assert!(
        snapshot
            .last_error
            .unwrap()
            .contains("failed to spawn executable")
    );
    assert!(!snapshot.effective_args.is_empty());
    assert!(snapshot.dropped_events >= 3);
    assert!(!f.config().exists());
}

#[test]
fn startup_holds_distinct_ports_and_releases_them_on_planning_failure() {
    use std::net::TcpListener;
    use std::sync::Mutex;
    struct CheckingPlanner(Arc<Mutex<Vec<u16>>>);
    impl LaunchPlanner for CheckingPlanner {
        fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
            let ports = vec![ctx.cdp_port, ctx.socks_port.unwrap()];
            assert_ne!(ports[0], ports[1]);
            for port in &ports {
                assert!(TcpListener::bind(("127.0.0.1", *port)).is_err());
            }
            *self.0.lock().unwrap() = ports;
            Err(LaunchPlanError::InvalidArguments(
                "test planning failure".into(),
            ))
        }
    }
    let mut f = Fixture::new(false, true, XRAY);
    let ports = Arc::new(Mutex::new(Vec::new()));
    f.supervisor.planner = Box::new(CheckingPlanner(ports.clone()));
    f.start();
    assert!(matches!(f.snapshot().state, RuntimeState::Failed { .. }));
    for port in ports.lock().unwrap().iter() {
        assert!(TcpListener::bind(("127.0.0.1", *port)).is_ok());
    }
}

struct CommandProbe {
    sender: Sender<RuntimeCommand>,
    command: RuntimeCommand,
}
impl CdpProbe for CommandProbe {
    fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
        self.sender.send(self.command.clone()).unwrap();
        // Cancellation wins even if the same probe reports readiness.
        Ok(CdpInfo::default())
    }
}

#[test]
fn stop_during_cdp_readiness_rolls_back_without_started_or_failed() {
    let mut f = Fixture::new(true, true, XRAY);
    f.supervisor.cdp_ready_timeout = Duration::from_secs(30);
    f.supervisor.cdp_probe = Box::new(CommandProbe {
        sender: f.commands.as_ref().unwrap().clone(),
        command: RuntimeCommand::Stop(f.params.profile.id),
    });
    let start = std::time::Instant::now();
    f.start();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(!f.config().exists());
    assert!(f.supervisor.active_sessions.is_empty());
    assert!(!f.events.try_iter().any(|event| matches!(
        event,
        RuntimeEvent::Started { .. }
            | RuntimeEvent::StateChanged {
                state: RuntimeState::Failed { .. },
                ..
            }
    )));
}

#[test]
fn stop_during_xray_wait_reaps_process_before_timeout() {
    let mut f = Fixture::new(
        true,
        true,
        "#!/bin/sh\necho $$ > \"$3.pid\"\nexec sleep 60\n",
    );
    f.supervisor.xray_ready_timeout = Duration::from_secs(30);
    let marker = f.config().with_file_name("xray.json.pid");
    let sender = f.commands.as_ref().unwrap().clone();
    let id = f.params.profile.id;
    let worker = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let pid = loop {
            if let Ok(text) = std::fs::read_to_string(&marker)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        sender.send(RuntimeCommand::Stop(id)).unwrap();
        pid
    });
    let start = std::time::Instant::now();
    f.start();
    let pid = worker.join().unwrap();
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(!f.config().exists());
    #[cfg(target_os = "linux")]
    assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
}

#[test]
fn shutdown_during_start_exits_run_loop_and_discards_deferred_start() {
    let mut f = Fixture::new(true, true, XRAY);
    f.supervisor.cdp_probe = Box::new(CommandProbe {
        sender: f.commands.as_ref().unwrap().clone(),
        command: RuntimeCommand::ShutdownAll,
    });
    let mut next = f.params.clone();
    next.profile.id = ProfileId::new();
    let next_id = next.profile.id;
    let sender = f.commands.as_ref().unwrap();
    sender
        .send(RuntimeCommand::Start(f.params.clone()))
        .unwrap();
    sender.send(RuntimeCommand::Start(next)).unwrap();
    f.supervisor.run();
    assert!(f.supervisor.shutting_down);
    assert!(f.supervisor.pending_commands.is_empty());
    assert!(f.supervisor.active_sessions.is_empty());
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(!f.config().exists());
    assert!(
        !f.supervisor
            .snapshots
            .read()
            .unwrap()
            .contains_key(&next_id)
    );
}

#[test]
fn disconnected_command_channel_cancels_start() {
    let mut f = Fixture::new(true, true, XRAY);
    f.commands.take();
    f.start();
    assert!(f.supervisor.shutting_down);
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(!f.config().exists());
}

#[test]
fn restart_during_readiness_cancels_then_starts_again_without_recursion() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct RestartProbe {
        sender: Sender<RuntimeCommand>,
        params: StartParams,
        calls: AtomicUsize,
    }
    impl CdpProbe for RestartProbe {
        fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
            let command = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                RuntimeCommand::Restart(self.params.clone())
            } else {
                RuntimeCommand::ShutdownAll
            };
            self.sender.send(command).unwrap();
            Ok(CdpInfo::default())
        }
    }
    let mut f = Fixture::new(true, true, XRAY);
    f.supervisor.cdp_probe = Box::new(RestartProbe {
        sender: f.commands.as_ref().unwrap().clone(),
        params: f.params.clone(),
        calls: AtomicUsize::new(0),
    });
    f.commands
        .as_ref()
        .unwrap()
        .send(RuntimeCommand::Start(f.params.clone()))
        .unwrap();
    f.supervisor.run();
    let events: Vec<_> = f.events.try_iter().collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                RuntimeEvent::StateChanged {
                    state: RuntimeState::Starting,
                    ..
                }
            ))
            .count(),
        2
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::Started { .. }))
    );
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(!f.config().exists());
}

#[test]
fn stop_other_profile_removes_earlier_deferred_start_and_preserves_later_start() {
    let mut f = Fixture::new(true, true, XRAY);
    f.start();
    let id = f.params.profile.id;
    let sender = f.commands.as_ref().unwrap();
    sender
        .send(RuntimeCommand::Start(f.params.clone()))
        .unwrap();
    sender.send(RuntimeCommand::Stop(id)).unwrap();
    sender
        .send(RuntimeCommand::Start(f.params.clone()))
        .unwrap();
    assert!(!f.supervisor.poll_start_commands(ProfileId::new()));
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(!f.config().exists());
    assert_eq!(f.supervisor.pending_commands.len(), 1);
    assert!(
        matches!(f.supervisor.pending_commands.front(), Some(RuntimeCommand::Start(p)) if p.profile_id() == id)
    );
}

#[test]
fn starting_another_profile_still_reaps_existing_crashed_session() {
    struct CheckingProbe {
        snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
        crashed: ProfileId,
    }
    impl CdpProbe for CheckingProbe {
        fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
            assert_eq!(
                self.snapshots.read().unwrap()[&self.crashed].state,
                RuntimeState::Stopped
            );
            Ok(CdpInfo::default())
        }
    }
    let mut f = Fixture::new(true, true, XRAY);
    f.start();
    let id = f.params.profile.id;
    let xray = f
        .supervisor
        .active_sessions
        .get_mut(&id)
        .unwrap()
        .xray
        .as_mut()
        .unwrap();
    xray.owned_mut().kill().unwrap();
    xray.owned_mut().wait().unwrap();
    f.supervisor.cdp_probe = Box::new(CheckingProbe {
        snapshots: f.supervisor.snapshots.clone(),
        crashed: id,
    });
    let mut next = f.params.clone();
    next.profile.id = ProfileId::new();
    next.profile.user_data_dir = f.dir.join("second-profile");
    let next_id = next.profile.id;
    f.supervisor.start_profile(next);
    assert_eq!(
        f.supervisor.snapshots.read().unwrap()[&next_id].state,
        RuntimeState::Running
    );
}

#[test]
fn xray_crash_terminates_browser_and_clears_session() {
    let mut f = Fixture::new(true, true, XRAY);
    f.start();
    assert_eq!(f.snapshot().state, RuntimeState::Running);
    assert!(f.snapshot().xray_pid.is_some());
    let id = f.params.profile.id;
    let session = f.supervisor.active_sessions.get_mut(&id).unwrap();
    let browser_pid = session.browser.id();
    session.xray.as_mut().unwrap().owned_mut().kill().unwrap();
    session.xray.as_mut().unwrap().owned_mut().wait().unwrap();
    f.supervisor.poll_active_sessions();
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(f.snapshot().browser_pid.is_none());
    assert!(!f.config().exists());
    #[cfg(target_os = "linux")]
    assert!(!PathBuf::from(format!("/proc/{browser_pid}")).exists());
    assert!(f.events.try_iter().any(|e| matches!(
        e,
        RuntimeEvent::Crashed {
            component: RuntimeComponent::Xray,
            ..
        }
    )));
}

#[test]
fn startup_failures_rollback_xray_and_config() {
    for (browser, cdp, script) in [
        (false, true, XRAY),
        (true, false, XRAY),
        (true, true, "#!/bin/sh\nexit 1\n"),
        (true, true, "#!/bin/sh\nexec sleep 60\n"),
    ] {
        let mut f = Fixture::new(browser, cdp, script);
        f.start();
        assert!(matches!(f.snapshot().state, RuntimeState::Failed { .. }));
        assert!(f.supervisor.active_sessions.is_empty());
        assert!(!f.config().exists());
        assert!(
            !f.events
                .try_iter()
                .any(|e| matches!(e, RuntimeEvent::Started { .. }))
        );
        // The record and the processes are the other two things a start
        // acquires. Which of the four cases reached the record differs - two
        // fail before both children exist - but none of them may leave one
        // behind, and none may leave a process for the next run to reclaim.
        assert!(
            journal::read(&f.dir, f.params.profile.id)
                .expect("the journal is readable")
                .is_none(),
            "a rolled back start left its record behind"
        );
        assert!(
            f.supervisor.recover_orphans().is_empty(),
            "a rolled back start left a process running"
        );
    }
}

#[test]
fn normal_stop_reclaims_both_children_and_missing_xray_fails_closed() {
    let mut f = Fixture::new(true, true, XRAY);
    f.start();
    assert_eq!(f.snapshot().state, RuntimeState::Running);
    f.supervisor.stop_profile(f.params.profile.id);
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(f.supervisor.active_sessions.is_empty());
    assert!(!f.config().exists());
    std::fs::remove_file(&f.supervisor.xray_executable).unwrap();
    f.start();
    assert!(matches!(f.snapshot().state, RuntimeState::Failed { .. }));
    assert!(!f.config().exists());
}

#[test]
fn browser_crash_reclaims_xray_and_stop_is_idempotent() {
    let mut f = Fixture::new(true, true, XRAY);
    f.start();
    let id = f.params.profile.id;
    let session = f.supervisor.active_sessions.get_mut(&id).unwrap();
    let xray_pid = session.xray.as_ref().unwrap().id();
    session.browser.owned_mut().kill().unwrap();
    session.browser.owned_mut().wait().unwrap();
    f.supervisor.poll_active_sessions();
    f.supervisor.stop_profile(id);
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(f.snapshot().xray_pid.is_none());
    assert!(!f.config().exists());
    #[cfg(target_os = "linux")]
    assert!(!PathBuf::from(format!("/proc/{xray_pid}")).exists());
}

/// An inspector that reports every pid as a process with someone else's
/// command line: "this is not the process the record describes", which is one
/// of the answers a stop has to be able to give.
struct SomeoneElse;
impl ProcessInspector for SomeoneElse {
    fn inspect(&self, _: u32) -> ProcessReading {
        ProcessReading::Live(ProcessIdentity {
            argv: vec!["/usr/bin/something-else".to_string()],
            start_time: Some(1),
        })
    }
}

/// An inspector that reports every pid as gone.
struct Nobody;
impl ProcessInspector for Nobody {
    fn inspect(&self, _: u32) -> ProcessReading {
        ProcessReading::Absent
    }
}

/// A tree controller that refuses to terminate anything, for the other half
/// of "the stop did not happen".
struct Refuses;
impl ProcessTreeController for Refuses {
    fn terminate_tree(&self, pid: u32) -> Result<(), ProcessError> {
        Err(ProcessError::TerminationFailed(format!(
            "pid {pid} refused"
        )))
    }

    fn terminate_instance(&self, pid: u32, _: Option<u64>) -> Result<(), ProcessError> {
        Err(ProcessError::TerminationFailed(format!(
            "pid {pid} refused"
        )))
    }
}

/// An adopted session as recovery installs one: the record on disk, the
/// temporary Xray config beside it, and no handle.
fn adopt(f: &mut Fixture, browser_pid: u32, with_xray: bool) -> journal::Adopted {
    let profile_id = f.params.profile.id;
    let browser = ProcessRecord {
        pid: browser_pid,
        executable: "/bin/sleep".into(),
        args: vec!["60".into()],
        start_time: Some(7),
    };
    let xray = with_xray.then(|| ProcessRecord {
        pid: browser_pid + 1,
        executable: "/bin/sleep".into(),
        args: vec!["60".into()],
        start_time: Some(8),
    });
    let record = SessionRecord {
        profile_id,
        cdp_port: 9222,
        socks_port: Some(1080),
        started_at: 1_700_000_000_000,
        left_running: true,
        browser: browser.clone(),
        xray: xray.clone(),
    };
    journal::write(&f.dir, &record).expect("write the record");
    let config = f
        .dir
        .join(profile_id.to_string())
        .join(crate::xray::XRAY_CONFIG_FILE);
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "{}").unwrap();

    let adopted = journal::Adopted {
        profile_id,
        cdp_port: record.cdp_port,
        socks_port: record.socks_port,
        started_at: record.started_at,
        browser,
        xray,
    };
    f.supervisor.install_adopted(&adopted);
    adopted
}

/// A session this run adopted has no handle, so it is stopped by pid against
/// a record that may no longer describe what is running there. That answer has
/// to reach the caller: the record is the only thing that can still reach
/// whatever is running, and publishing Stopped over it would leave a live
/// browser that nothing tracks, that no start refuses, and that the next run
/// has no reason to look for.
#[test]
fn a_stop_that_cannot_reach_an_adopted_process_keeps_the_session_and_its_record() {
    let mut f = Fixture::new(false, false, XRAY);
    let adopted = adopt(&mut f, 4242, true);
    let profile_id = adopted.profile_id;
    f.supervisor.process_inspector = Arc::new(SomeoneElse);

    f.supervisor.stop_profile(profile_id);

    // Not Stopped, and nothing was thrown away.
    assert_eq!(f.snapshot().state, RuntimeState::Running);
    assert!(f.supervisor.active_sessions.contains_key(&profile_id));
    assert!(
        journal::read(&f.dir, profile_id).unwrap().is_some(),
        "the record still describes the running process"
    );
    assert!(f.config().exists(), "the Xray config was not removed");
    let warning = std::iter::from_fn(|| f.events.try_recv().ok())
        .find_map(|event| match event {
            RuntimeEvent::Warning { message, .. } => Some(message),
            _ => None,
        })
        .expect("the window is told the stop did not happen");
    assert!(warning.contains("could not be stopped"), "{warning}");
    assert!(
        !std::iter::from_fn(|| f.events.try_recv().ok())
            .any(|event| matches!(event, RuntimeEvent::Stopped { .. })),
        "a stop that did not happen must not report one"
    );

    // The process is gone now, so the retry the session stayed for works:
    // everything a normal stop cleans up is cleaned up.
    f.supervisor.process_inspector = Arc::new(Nobody);
    f.supervisor.stop_profile(profile_id);
    assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    assert!(!f.supervisor.active_sessions.contains_key(&profile_id));
    assert!(journal::read(&f.dir, profile_id).unwrap().is_none());
    assert!(!f.config().exists());
}

/// A tree that refuses to terminate a process it did recognise is the same
/// answer by the other route, and the same handling.
#[test]
fn a_stop_a_process_tree_refuses_keeps_the_session_and_its_record() {
    let mut f = Fixture::new(false, false, XRAY);
    let adopted = adopt(&mut f, 4343, false);
    let profile_id = adopted.profile_id;
    f.supervisor.process_tree = Arc::new(Refuses);
    // The inspector says the pid is still ours, so the stop gets as far as
    // asking the tree to end it - and the grace period is what it spends
    // waiting first.
    f.supervisor.process_inspector = Arc::new(Ours);

    f.supervisor.stop_profile(profile_id);

    assert_eq!(f.snapshot().state, RuntimeState::Running);
    assert!(f.supervisor.active_sessions.contains_key(&profile_id));
    assert!(journal::read(&f.dir, profile_id).unwrap().is_some());
}

/// An inspector that reports the recorded process, so a stop escalates to the
/// tree.
struct Ours;
impl ProcessInspector for Ours {
    fn inspect(&self, _: u32) -> ProcessReading {
        ProcessReading::Live(ProcessIdentity {
            argv: vec!["/bin/sleep".to_string(), "60".to_string()],
            start_time: Some(7),
        })
    }
}
