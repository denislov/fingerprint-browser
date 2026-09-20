use super::*;
use crate::journal::{self, ProcessRecord, SessionRecord};
use crate::process::{
    DefaultProcessInspector, DefaultProcessTreeController, ProcessInspector, ProcessTreeController,
};
use domain::ProfileId;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("fp-windows-{}", ProfileId::new()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn command(&self, role: &str) -> Command {
        helper(&self.0, role)
    }
    fn pid(&self, name: &str) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(value) = std::fs::read_to_string(self.0.join(name))
                && let Ok(pid) = value.parse()
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "helper never wrote {name}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only remove the unique fixture directory this test created.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn helper(dir: &Path, role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "process::windows::tests::process_helper",
            "--nocapture",
        ])
        .env("FP_TEST_WINDOWS_ROLE", role)
        .env("FP_TEST_WINDOWS_DIR", dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    command
}

#[test]
// Intentionally exits before its leaf in the exiting-parent scenario. The
// owning test verifies job cleanup; waiting here would invalidate that test.
#[allow(clippy::zombie_processes)]
fn process_helper() {
    let Ok(role) = std::env::var("FP_TEST_WINDOWS_ROLE") else {
        return;
    };
    let dir = PathBuf::from(std::env::var_os("FP_TEST_WINDOWS_DIR").unwrap());
    if role == "owner" {
        let child = spawn(&mut helper(&dir, "parent")).unwrap();
        std::fs::write(dir.join("parent"), child.id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
        drop(child);
    } else if role == "parent" || role == "exiting-parent" {
        let mut child = helper(&dir, "leaf").spawn().unwrap();
        std::fs::write(dir.join("leaf"), child.id().to_string()).unwrap();
        if role == "parent" {
            let _ = child.wait();
        }
    } else if role == "arguments" {
        std::fs::write(
            dir.join("arguments.json"),
            serde_json::to_vec(&std::env::args().collect::<Vec<_>>()).unwrap(),
        )
        .unwrap();
    } else {
        std::fs::write(dir.join("ready"), std::process::id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn native_spawn_preserves_unicode_quotes_empty_arguments_and_trailing_slashes() {
    let fixture = Fixture::new();
    let values = [
        "",
        "中文 path with spaces\\",
        "embedded\"quote",
        "slashes\\\\\"quote",
    ];
    let mut command = fixture.command("arguments");
    // libtest accepts multiple exact test filters. Only the helper name matches.
    command.args(values);
    let mut child = spawn(&mut command).unwrap();
    assert!(child.wait().unwrap().success());
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(fixture.0.join("arguments.json")).unwrap()).unwrap();
    assert_eq!(&args[args.len() - values.len()..], values);
}

fn gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if inspect(pid) == ProcessReading::Absent {
            return;
        }
        assert!(Instant::now() < deadline, "process {pid} survived cleanup");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn dropping_job_kills_descendants_after_the_leader_exits() {
    let fixture = Fixture::new();
    let mut child = spawn(&mut fixture.command("exiting-parent")).unwrap();
    let leaf = fixture.pid("ready");
    assert!(child.wait().unwrap().success());
    assert!(matches!(inspect(leaf), ProcessReading::Live(_)));
    drop(child);
    gone(leaf);
}

#[test]
fn killing_the_manager_closes_its_jobs_and_preserves_unrelated_processes() {
    let fixture = Fixture::new();
    let unrelated = Fixture::new();
    let mut stranger = spawn(&mut unrelated.command("leaf")).unwrap();
    unrelated.pid("ready");
    let mut owner = fixture.command("owner").spawn().unwrap();
    let parent = fixture.pid("parent");
    let leaf = fixture.pid("ready");
    owner.kill().unwrap();
    owner.wait().unwrap();
    gone(parent);
    gone(leaf);
    assert!(stranger.try_wait().unwrap().is_none());
    stranger.terminate_tree().unwrap();
    stranger.wait().unwrap();
}

#[test]
fn creation_time_is_available_immediately_and_exited_handles_are_absent() {
    let fixture = Fixture::new();
    let mut child = spawn(&mut fixture.command("leaf")).unwrap();
    let start = DefaultProcessInspector.start_time(child.id()).unwrap();
    fixture.pid("ready");
    assert_eq!(DefaultProcessInspector.start_time(child.id()), Some(start));
    child.terminate_tree().unwrap();
    child.wait().unwrap();
    assert_eq!(inspect(child.id()), ProcessReading::Absent);
    assert_eq!(inspect(0), ProcessReading::Unknown);
}

fn record(child: &ManagedChild) -> SessionRecord {
    SessionRecord {
        profile_id: ProfileId::new(),
        cdp_port: 0,
        socks_port: None,
        started_at: journal::now_millis(),
        xray: None,
        browser: ProcessRecord::captured(
            child.id(),
            &std::env::current_exe().unwrap(),
            &[],
            &DefaultProcessInspector,
        ),
    }
}

#[test]
fn an_identified_legacy_session_is_reclaimed_but_a_mismatched_one_is_preserved() {
    let fixture = Fixture::new();
    let mut child = spawn(&mut fixture.command("parent")).unwrap();
    let leaf = fixture.pid("ready");
    let mut entry = record(&child);
    let actual = entry.browser.start_time.unwrap();
    entry.browser.start_time = Some(actual + 1);
    journal::write(&fixture.0, &entry).unwrap();
    let report = journal::reclaim(
        &fixture.0,
        &DefaultProcessInspector,
        &DefaultProcessTreeController,
        Duration::ZERO,
    );
    assert_eq!(report.unresolved.len(), 1);
    assert!(child.try_wait().unwrap().is_none());
    assert!(
        journal::read(&fixture.0, entry.profile_id)
            .unwrap()
            .is_some()
    );
    assert!(
        DefaultProcessTreeController
            .terminate_instance(child.id(), Some(actual + 1))
            .is_err()
    );
    entry.browser.start_time = None;
    journal::write(&fixture.0, &entry).unwrap();
    let report = journal::reclaim(
        &fixture.0,
        &DefaultProcessInspector,
        &DefaultProcessTreeController,
        Duration::ZERO,
    );
    assert_eq!(
        report.unresolved.len(),
        1,
        "legacy record without identity must stay"
    );
    assert!(child.try_wait().unwrap().is_none());
    entry.browser.start_time = Some(actual);
    journal::write(&fixture.0, &entry).unwrap();
    let report = journal::reclaim(
        &fixture.0,
        &DefaultProcessInspector,
        &DefaultProcessTreeController,
        Duration::ZERO,
    );
    assert_eq!(report.reclaimed.len(), 1, "{report:?}");
    child.wait().unwrap();
    gone(leaf);
    assert!(
        journal::read(&fixture.0, entry.profile_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn failed_cleanup_keeps_the_record_for_retry() {
    struct Refuse;
    impl ProcessTreeController for Refuse {
        fn terminate_tree(&self, _: u32) -> Result<(), crate::error::ProcessError> {
            Err(crate::error::ProcessError::TerminationFailed(
                "test refusal".into(),
            ))
        }
    }
    let fixture = Fixture::new();
    let mut child = spawn(&mut fixture.command("leaf")).unwrap();
    let entry = record(&child);
    journal::write(&fixture.0, &entry).unwrap();
    let report = journal::reclaim(
        &fixture.0,
        &DefaultProcessInspector,
        &Refuse,
        Duration::ZERO,
    );
    assert_eq!(report.unresolved.len(), 1);
    assert!(report.reclaimed.is_empty());
    assert!(
        journal::read(&fixture.0, entry.profile_id)
            .unwrap()
            .is_some()
    );
    child.terminate_tree().unwrap();
    child.wait().unwrap();
    let report = journal::reclaim(
        &fixture.0,
        &DefaultProcessInspector,
        &Refuse,
        Duration::ZERO,
    );
    assert_eq!(report.stale.len(), 1);
}
