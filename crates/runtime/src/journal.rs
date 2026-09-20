//! A record of what a running profile's children are, left where the next run
//! can find it.
//!
//! A supervisor that is killed outright - `SIGKILL`, a crash, a window destroyed
//! without the close protocol - takes with it the only knowledge of the browsers
//! it started. The children survive, because nothing told them to stop. So every
//! running session writes down what it launched, and the next run reads those
//! records back before it accepts a command and stops what the last one left
//! behind.
//!
//! Nothing here trusts a pid. A record names a process by pid, but between a
//! crash and the next start that pid can be recycled onto an unrelated process.
//! A record is only acted on when the live process is still the process instance
//! the record captured - same pid, same kernel start time - or, where the
//! platform cannot report a start time, when its command line still carries the
//! arguments that were passed to spawn. A record whose process cannot be
//! identified is reported and left alone: killing a stranger's process is worse
//! than leaving our own behind, and the record stays readable either way.

use crate::error::JournalError;
use crate::process::{ProcessInspector, ProcessReading, ProcessTreeController};
use crate::xray::XRAY_CONFIG_FILE;
use domain::ProfileId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Written inside the per-profile runtime directory, next to `xray.json`.
pub const RECORD_FILE: &str = "session.json";

/// How long a reclaimed process is given to exit after the polite request.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub pid: u32,
    pub executable: PathBuf,
    /// The arguments after `argv[0]`: exactly what was handed to spawn, which is
    /// what the live process is compared against.
    pub args: Vec<String>,
    /// Read from the child when the record was written, where the platform
    /// exposes it. `None` means identity rests on the arguments alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<u64>,
}

impl ProcessRecord {
    /// Records a process that was just spawned. The start time is read back out
    /// of the live process rather than assumed, so a record written here can
    /// tell the same process apart from a recycled pid later.
    ///
    /// It is read from the pid, not from the command line: a record is written
    /// as soon as the child exists, which is before it reaches `execve`, and a
    /// start time lost there leaves the record identifiable only by a command
    /// line the browser will have rewritten by the time anyone reads it back.
    /// `None` means the platform cannot report it, and identity then rests on the
    /// arguments alone.
    pub fn captured(
        pid: u32,
        executable: &Path,
        args: &[String],
        inspector: &dyn ProcessInspector,
    ) -> Self {
        Self {
            pid,
            executable: executable.to_path_buf(),
            args: args.to_vec(),
            start_time: inspector.start_time(pid),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub profile_id: ProfileId,
    pub cdp_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socks_port: Option<u16>,
    /// Milliseconds since the Unix epoch, kept as a number so the record reads
    /// the same everywhere.
    pub started_at: u64,
    pub browser: ProcessRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xray: Option<ProcessRecord>,
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub fn record_path(runtime_dir: &Path, profile_id: ProfileId) -> PathBuf {
    runtime_dir.join(profile_id.to_string()).join(RECORD_FILE)
}

/// Writes the record through a temporary file, so a crash in the middle of a
/// write cannot leave half a record where the next run expects a whole one.
pub fn write(runtime_dir: &Path, record: &SessionRecord) -> Result<PathBuf, JournalError> {
    let path = record_path(runtime_dir, record.profile_id);
    let parent = path.parent().ok_or_else(|| {
        JournalError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "session record path has no parent directory",
        ))
    })?;
    std::fs::create_dir_all(parent)?;

    let temporary = path.with_file_name(format!("{RECORD_FILE}.tmp"));
    let bytes = serde_json::to_vec_pretty(record)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    std::io::Write::write_all(&mut file, &bytes)?;
    drop(file);
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

pub fn read(
    runtime_dir: &Path,
    profile_id: ProfileId,
) -> Result<Option<SessionRecord>, JournalError> {
    read_path(&record_path(runtime_dir, profile_id))
}

fn read_path(path: &Path) -> Result<Option<SessionRecord>, JournalError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Removes a record, and the per-profile directory when the record was its last
/// file. A removal that fails is logged rather than propagated: the session it
/// describes is already stopped, and the next run will look at it again.
pub fn remove(runtime_dir: &Path, profile_id: ProfileId) {
    let path = record_path(runtime_dir, profile_id);
    if let Err(error) = std::fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("failed to remove session record {path:?}: {error}");
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

/// Every record under the runtime directory. A record that cannot be read is
/// returned as an error rather than skipped: it is the only trace of a session
/// that may still be running.
pub fn list(runtime_dir: &Path) -> Vec<(PathBuf, Result<SessionRecord, JournalError>)> {
    let mut entries = Vec::new();
    let Ok(directories) = std::fs::read_dir(runtime_dir) else {
        return entries;
    };
    for directory in directories.flatten() {
        let path = directory.path().join(RECORD_FILE);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        entries.push((path, serde_json::from_slice(&bytes).map_err(Into::into)));
    }
    entries
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reclaimed {
    pub profile_id: ProfileId,
    pub browser_pid: u32,
    /// The Xray pid, when the record named one that was still alive.
    pub xray_pid: Option<u32>,
    /// True when a process ignored the polite request and had to be killed.
    pub forced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// Missing only when the record itself could not be read.
    pub profile_id: Option<ProfileId>,
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReclaimReport {
    /// Sessions whose children were still alive and have been stopped.
    pub reclaimed: Vec<Reclaimed>,
    /// Records nothing was done about, because the process they name is not the
    /// one they describe. The record and the temporary Xray config are kept.
    pub unresolved: Vec<Unresolved>,
    /// Records whose children had already exited: only leftover files were
    /// removed.
    pub stale: Vec<PathBuf>,
}

impl ReclaimReport {
    pub fn is_empty(&self) -> bool {
        self.reclaimed.is_empty() && self.unresolved.is_empty() && self.stale.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// The pid is not in use, or holds a process that has already exited.
    Gone,
    /// Live, and still running the command line this record describes.
    Ours,
    /// Live, but not the process this record describes. Carries what it runs now.
    Foreign(String),
    /// Nothing could be learned, so nothing may be decided.
    Unreadable(String),
}

impl Verdict {
    /// Why this verdict blocks touching the record, if it does.
    fn blockage(&self) -> Option<&str> {
        match self {
            Self::Foreign(reason) | Self::Unreadable(reason) => Some(reason),
            Self::Gone | Self::Ours => None,
        }
    }
}

/// Stops what an earlier run left running, then clears the records it left.
///
/// `graceful` is how long a reclaimed process may take to exit after the polite
/// request before it is killed. Anything that cannot be identified is reported
/// and left running.
pub fn reclaim(
    runtime_dir: &Path,
    inspector: &dyn ProcessInspector,
    tree: &dyn ProcessTreeController,
    graceful: Duration,
) -> ReclaimReport {
    let mut report = ReclaimReport::default();
    for (path, entry) in list(runtime_dir) {
        let record = match entry {
            Ok(record) => record,
            Err(error) => {
                report.unresolved.push(Unresolved {
                    profile_id: None,
                    path,
                    reason: format!("the session record could not be read: {error}"),
                });
                continue;
            }
        };

        let browser = verdict(&record.browser, inspector);
        let xray = record
            .xray
            .as_ref()
            .map(|process| verdict(process, inspector));

        let mut blocked = Vec::new();
        if let Some(reason) = browser.blockage() {
            blocked.push(format!("browser pid {}: {reason}", record.browser.pid));
        }
        if let (Some(process), Some(verdict)) = (record.xray.as_ref(), xray.as_ref())
            && let Some(reason) = verdict.blockage()
        {
            blocked.push(format!("xray pid {}: {reason}", process.pid));
        }
        if !blocked.is_empty() {
            report.unresolved.push(Unresolved {
                profile_id: Some(record.profile_id),
                path,
                reason: format!("nothing was killed: {}", blocked.join("; ")),
            });
            continue;
        }

        let browser_running = browser == Verdict::Ours;
        let xray_running = xray == Some(Verdict::Ours);
        if !browser_running && !xray_running {
            // The children are gone, but a run that was killed leaves its
            // temporary Xray config behind, and that file holds the upstream
            // credentials.
            remove_xray_config(runtime_dir, &record);
            remove(runtime_dir, record.profile_id);
            report.stale.push(path);
            continue;
        }

        let mut forced = false;
        let mut failures = Vec::new();
        if browser_running {
            match stop(&record.browser, inspector, tree, graceful) {
                Ok(killed) => forced |= killed,
                Err(error) => failures.push(error),
            }
        }
        let xray_pid = record
            .xray
            .as_ref()
            .filter(|_| xray_running)
            .map(|process| process.pid);
        if let Some(process) = record.xray.as_ref().filter(|_| xray_running) {
            match stop(process, inspector, tree, graceful) {
                Ok(killed) => forced |= killed,
                Err(error) => failures.push(error),
            }
        }
        if !failures.is_empty() {
            report.unresolved.push(Unresolved {
                profile_id: Some(record.profile_id),
                path,
                reason: failures.join("; "),
            });
            continue;
        }
        remove_xray_config(runtime_dir, &record);
        remove(runtime_dir, record.profile_id);

        report.reclaimed.push(Reclaimed {
            profile_id: record.profile_id,
            browser_pid: record.browser.pid,
            xray_pid,
            forced,
        });
    }
    report
}

fn verdict(record: &ProcessRecord, inspector: &dyn ProcessInspector) -> Verdict {
    let live = match inspector.inspect(record.pid) {
        ProcessReading::Absent => return Verdict::Gone,
        ProcessReading::Unknown => {
            return Verdict::Unreadable(format!(
                "pid {} cannot be identified on this platform, so it was left alone",
                record.pid
            ));
        }
        ProcessReading::Live(live) => live,
    };
    if live.argv.is_empty() && (record.start_time.is_none() || live.start_time.is_none()) {
        return Verdict::Unreadable(format!(
            "pid {} has no comparable creation time or command line; left untouched",
            record.pid
        ));
    }
    let describes_this_process = describes(&live, record);
    if describes_this_process {
        Verdict::Ours
    } else {
        Verdict::Foreign(format!(
            "pid {} now runs {:?}",
            record.pid,
            live.argv.join(" ")
        ))
    }
}

/// Whether a live process is the one a record describes.
///
/// The same pid *and* the same kernel start time is the same process instance:
/// the kernel does not hand a recycled pid the start time of the process that
/// died, so nothing else can be the process this record captured. That is the
/// identity, and it is the one that survives a process rewriting its own command
/// line - Chromium overwrites `/proc/self/cmdline` with a single string, so its
/// arguments are no longer separable at all.
///
/// When a start time is missing on either side - a record written where the
/// platform cannot report it - the command line is all there is, and it has to
/// match: as the tail of the argument vector, or as the end of that single
/// string.
fn describes(live: &crate::process::ProcessIdentity, record: &ProcessRecord) -> bool {
    match (record.start_time, live.start_time) {
        (Some(recorded), Some(live)) => recorded == live,
        _ => command_line_matches(live, record),
    }
}

fn command_line_matches(live: &crate::process::ProcessIdentity, record: &ProcessRecord) -> bool {
    let args = record.args.as_slice();
    if args.is_empty() {
        return false;
    }
    if live.argv.len() > args.len() && live.argv[live.argv.len() - args.len()..] == *args {
        return true;
    }
    // A process that rewrote its command line leaves one field holding the whole
    // line. The recorded arguments have to be the end of it, on a word boundary.
    let joint = args.join(" ");
    live.argv.len() == 1 && (live.argv[0] == joint || live.argv[0].ends_with(&format!(" {joint}")))
}

/// Returns whether the process had to be killed.
fn stop(
    record: &ProcessRecord,
    inspector: &dyn ProcessInspector,
    tree: &dyn ProcessTreeController,
    graceful: Duration,
) -> Result<bool, String> {
    let pid = record.pid;
    // Re-read after earlier components' grace periods, before touching this PID.
    match verdict(record, inspector) {
        Verdict::Gone => return Ok(false),
        Verdict::Ours => {}
        Verdict::Foreign(reason) | Verdict::Unreadable(reason) => return Err(reason),
    }
    let _ = tree.request_tree_exit(pid);
    if wait_gone(pid, inspector, graceful) {
        return Ok(false);
    }
    tree.terminate_instance(pid, record.start_time)
        .map_err(|e| format!("pid {pid}: {e}"))?;
    if !wait_gone(pid, inspector, DEFAULT_GRACE) {
        return Err(format!(
            "pid {pid} did not exit after cleanup; session record retained"
        ));
    }
    Ok(true)
}

fn wait_gone(pid: u32, inspector: &dyn ProcessInspector, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if inspector.inspect(pid) == ProcessReading::Absent {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Reads a record back out of the live processes the way the next run will, so
/// a session that could never be recognised later says so now instead of at the
/// next start. Returns a description of every component that does not answer as
/// recorded; a component nobody can be asked about is not reported.
pub fn confirm(record: &SessionRecord, inspector: &dyn ProcessInspector) -> Option<String> {
    let mut disagreements = Vec::new();
    let mut check = |component: &str, process: &ProcessRecord| {
        if let Verdict::Foreign(reason) = verdict(process, inspector) {
            disagreements.push(format!("{component} {reason}"));
        }
    };
    check("browser", &record.browser);
    if let Some(xray) = record.xray.as_ref() {
        check("xray", xray);
    }
    (!disagreements.is_empty()).then(|| disagreements.join("; "))
}

fn remove_xray_config(runtime_dir: &Path, record: &SessionRecord) {
    if record.xray.is_none() {
        return;
    }
    let path = runtime_dir
        .join(record.profile_id.to_string())
        .join(XRAY_CONFIG_FILE);
    if let Err(error) = std::fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("failed to remove the leftover Xray config {path:?}: {error}");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::process::{
        DefaultProcessInspector, DefaultProcessTreeController, ProcessIdentity, ProcessReading,
        spawn_managed,
    };
    use std::process::{Command, Stdio};

    fn runtime_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fp-journal-{name}-{}", ProfileId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn spawn_sleep() -> std::process::Child {
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        spawn_with(&mut command, Stdio::null())
    }

    /// Spawns a child in its own process group.
    ///
    /// A test that writes an executable and execs it immediately can be told
    /// `ETXTBSY` with nothing holding the file open any more; `execvp` retries
    /// the same way. Production never writes the binary it launches.
    fn spawn_with(command: &mut Command, stdout: Stdio) -> std::process::Child {
        command
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(Stdio::null());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match spawn_managed(command) {
                Ok(child) => return child,
                Err(error)
                    if error.raw_os_error() == Some(libc::ETXTBSY)
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("failed to spawn: {error}"),
            }
        }
    }

    /// A process has no command line between `fork` and `execve`.
    fn live_identity(pid: u32) -> ProcessIdentity {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let ProcessReading::Live(identity) = DefaultProcessInspector.inspect(pid) {
                return identity;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no identity for {pid}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A platform that cannot read another process's command line.
    struct BlindInspector;
    impl ProcessInspector for BlindInspector {
        fn inspect(&self, _pid: u32) -> ProcessReading {
            ProcessReading::Unknown
        }
    }

    /// A child that exists but has not reached `execve`: no command line, but
    /// its `/proc` entry - and its start time - are there.
    struct PreExecInspector;
    impl ProcessInspector for PreExecInspector {
        fn inspect(&self, _pid: u32) -> ProcessReading {
            ProcessReading::Unknown
        }

        fn start_time(&self, _pid: u32) -> Option<u64> {
            Some(424_242)
        }
    }

    /// The record is written before readiness on purpose, so it is written
    /// before the child has a command line. Losing the start time there leaves
    /// the record identified by a command line the browser rewrites, which is
    /// how a session this run started became one the next run refused to
    /// reclaim.
    #[test]
    fn a_record_keeps_the_start_time_before_the_command_line_exists() {
        let record = ProcessRecord::captured(
            4321,
            std::path::Path::new("/opt/chrome"),
            &["--fingerprint=42".to_string()],
            &PreExecInspector,
        );

        assert_eq!(record.start_time, Some(424_242));

        // And a platform that cannot report one still records the arguments.
        let blind = ProcessRecord::captured(
            4321,
            std::path::Path::new("/opt/chrome"),
            &["--fingerprint=42".to_string()],
            &BlindInspector,
        );
        assert_eq!(blind.start_time, None);
        assert_eq!(blind.args, ["--fingerprint=42"]);
    }

    fn record_for(profile_id: ProfileId, child: &std::process::Child) -> SessionRecord {
        let live = live_identity(child.id());
        SessionRecord {
            profile_id,
            cdp_port: 9222,
            socks_port: None,
            started_at: now_millis(),
            browser: ProcessRecord {
                pid: child.id(),
                executable: "/bin/sleep".into(),
                args: live.args().to_vec(),
                start_time: live.start_time,
            },
            xray: None,
        }
    }

    #[test]
    fn a_record_round_trips_and_is_private() {
        let dir = runtime_dir("round-trip");
        let profile_id = ProfileId::new();
        let record = SessionRecord {
            profile_id,
            cdp_port: 9222,
            socks_port: Some(51234),
            started_at: 1_700_000_000_000,
            browser: ProcessRecord {
                pid: 4321,
                executable: "/opt/chrome".into(),
                args: vec!["--fingerprint=42".into(), "about:blank".into()],
                start_time: Some(987_654),
            },
            xray: Some(ProcessRecord {
                pid: 4322,
                executable: "/opt/xray".into(),
                args: vec!["run".into(), "-config".into(), "/tmp/xray.json".into()],
                start_time: None,
            }),
        };
        let path = write(&dir, &record).unwrap();
        assert_eq!(read(&dir, profile_id).unwrap(), Some(record));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        remove(&dir, profile_id);
        assert_eq!(read(&dir, profile_id).unwrap(), None);
        assert!(
            !dir.join(profile_id.to_string()).exists(),
            "the per-profile directory should be gone with its last file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_left_running_is_stopped_and_its_record_removed() {
        let dir = runtime_dir("reclaim");
        let profile_id = ProfileId::new();
        let mut child = spawn_sleep();
        write(&dir, &record_for(profile_id, &child)).unwrap();

        let report = reclaim(
            &dir,
            &DefaultProcessInspector,
            &DefaultProcessTreeController,
            Duration::from_millis(500),
        );

        assert_eq!(report.reclaimed.len(), 1, "{report:?}");
        assert_eq!(report.reclaimed[0].browser_pid, child.id());
        assert!(!report.reclaimed[0].forced, "{report:?}");
        assert!(report.unresolved.is_empty(), "{report:?}");
        assert!(read(&dir, profile_id).unwrap().is_none());
        assert_eq!(child.wait().unwrap().code(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The polite request is ignored, so the process is killed instead.
    #[test]
    fn a_process_that_ignores_the_request_is_killed() {
        let dir = runtime_dir("forced");
        let profile_id = ProfileId::new();
        // A shell that installs a SIGTERM trap and keeps sleeping. The marker on
        // stdout is what makes the test wait for the trap: signalling before it
        // is installed would prove nothing about ignoring it.
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "trap '' TERM; echo ready; sleep 60"]);
        let mut child = spawn_with(&mut command, Stdio::piped());
        let mut line = String::new();
        use std::io::BufRead;
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        write(&dir, &record_for(profile_id, &child)).unwrap();

        let report = reclaim(
            &dir,
            &DefaultProcessInspector,
            &DefaultProcessTreeController,
            Duration::from_millis(200),
        );

        assert_eq!(report.reclaimed.len(), 1, "{report:?}");
        assert!(report.reclaimed[0].forced, "{report:?}");
        assert!(read(&dir, profile_id).unwrap().is_none());
        child.wait().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_pid_running_something_else_is_not_killed() {
        let dir = runtime_dir("foreign");
        let profile_id = ProfileId::new();
        let mut child = spawn_sleep();
        let mut record = record_for(profile_id, &child);
        // What a recycled pid looks like: the process is alive, but it is not
        // the one the record describes. The start time is left out the way a
        // record written by a platform that cannot report one would, because
        // that is when the command line has to decide on its own.
        record.browser.args = vec!["61".into()];
        record.browser.start_time = None;
        write(&dir, &record).unwrap();

        let report = reclaim(
            &dir,
            &DefaultProcessInspector,
            &DefaultProcessTreeController,
            Duration::from_millis(200),
        );

        assert!(report.reclaimed.is_empty(), "{report:?}");
        assert_eq!(report.unresolved.len(), 1, "{report:?}");
        assert!(
            report.unresolved[0].reason.contains("nothing was killed"),
            "{report:?}"
        );
        assert!(
            DefaultProcessInspector.inspect(child.id()) != ProcessReading::Absent,
            "a process that is not ours must survive"
        );
        assert!(
            read(&dir, profile_id).unwrap().is_some(),
            "the record is kept when it could not be honoured"
        );

        DefaultProcessTreeController
            .terminate_tree(child.id())
            .unwrap();
        child.wait().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recorded_pid_that_was_recycled_is_not_killed() {
        let dir = runtime_dir("recycled");
        let profile_id = ProfileId::new();
        let mut child = spawn_sleep();
        let mut record = record_for(profile_id, &child);
        // Same arguments, different process: the kernel start time differs.
        record.browser.start_time = record.browser.start_time.map(|started| started + 1);
        assert!(record.browser.start_time.is_some());
        write(&dir, &record).unwrap();

        let report = reclaim(
            &dir,
            &DefaultProcessInspector,
            &DefaultProcessTreeController,
            Duration::from_millis(200),
        );

        assert_eq!(report.unresolved.len(), 1, "{report:?}");
        assert!(report.reclaimed.is_empty(), "{report:?}");
        assert_ne!(
            DefaultProcessInspector.inspect(child.id()),
            ProcessReading::Absent
        );

        DefaultProcessTreeController
            .terminate_tree(child.id())
            .unwrap();
        child.wait().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_whose_processes_are_gone_is_cleared_with_its_config() {
        let dir = runtime_dir("stale");
        let profile_id = ProfileId::new();
        let mut child = spawn_sleep();
        let mut record = record_for(profile_id, &child);
        record.xray = Some(ProcessRecord {
            pid: 4_000_000_000,
            executable: "/opt/xray".into(),
            args: vec!["run".into()],
            start_time: None,
        });
        write(&dir, &record).unwrap();
        let config = dir.join(profile_id.to_string()).join(XRAY_CONFIG_FILE);
        std::fs::write(&config, "{\"credentials\":\"secret\"}").unwrap();
        child.kill().unwrap();
        child.wait().unwrap();

        let report = reclaim(
            &dir,
            &DefaultProcessInspector,
            &DefaultProcessTreeController,
            Duration::from_millis(100),
        );

        assert_eq!(report.stale.len(), 1, "{report:?}");
        assert!(report.reclaimed.is_empty(), "{report:?}");
        assert!(
            !config.exists(),
            "a killed run leaves credentials in the temporary config"
        );
        assert!(read(&dir, profile_id).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_record_is_reported_and_left_in_place() {
        let dir = runtime_dir("unreadable");
        let profile_id = ProfileId::new();
        let path = record_path(&dir, profile_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();

        let report = reclaim(
            &dir,
            &DefaultProcessInspector,
            &DefaultProcessTreeController,
            Duration::from_millis(100),
        );

        assert_eq!(report.unresolved.len(), 1, "{report:?}");
        assert_eq!(report.unresolved[0].profile_id, None);
        assert!(
            report.unresolved[0].reason.contains("could not be read"),
            "{report:?}"
        );
        assert!(path.exists(), "an unreadable record is evidence, not trash");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_runtime_directory_reclaims_nothing() {
        let report = reclaim(
            &std::env::temp_dir().join(format!("fp-journal-missing-{}", ProfileId::new())),
            &DefaultProcessInspector,
            &DefaultProcessTreeController,
            Duration::from_millis(50),
        );
        assert!(report.is_empty(), "{report:?}");
    }

    #[test]
    fn a_record_matches_a_process_started_through_an_interpreter() {
        let dir = runtime_dir("shebang");
        let profile_id = ProfileId::new();
        let script = dir.join("xray");
        std::fs::write(&script, "#!/bin/sh\nsleep 60\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut command = Command::new(&script);
        command.args(["run", "-config", "/tmp/xray.json"]);
        let mut child = spawn_with(&mut command, Stdio::null());
        let live = live_identity(child.id());
        // The kernel starts a shebang script as `/bin/sh <script> <args>`, so the
        // live command line has two more fields than the one that was passed to
        // spawn. Both are the same process.
        assert!(live.argv.len() > 3, "{live:?}");

        let record = SessionRecord {
            profile_id,
            cdp_port: 9222,
            socks_port: None,
            started_at: now_millis(),
            browser: ProcessRecord {
                pid: child.id(),
                executable: script,
                args: vec!["run".into(), "-config".into(), "/tmp/xray.json".into()],
                // Written as a platform that cannot report a start time would:
                // the command line is then the only thing carrying the identity,
                // and it has to match through the interpreter.
                start_time: None,
            },
            xray: None,
        };
        assert_eq!(confirm(&record, &DefaultProcessInspector), None);
        write(&dir, &record).unwrap();

        let report = reclaim(
            &dir,
            &DefaultProcessInspector,
            &DefaultProcessTreeController,
            Duration::from_millis(500),
        );

        assert_eq!(report.reclaimed.len(), 1, "{report:?}");
        assert!(!report.reclaimed[0].forced, "{report:?}");
        child.wait().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Chromium overwrites `/proc/self/cmdline` with one string holding the
    /// whole command line, so its arguments are no longer separate fields. The
    /// recorded arguments then have to be the end of that string.
    #[test]
    fn a_command_line_rewritten_into_one_string_is_still_recognised() {
        let record = ProcessRecord {
            pid: 1,
            executable: "/opt/chrome".into(),
            args: vec!["--user-data-dir=/tmp/profile".into(), "about:blank".into()],
            start_time: None,
        };
        let rewritten = crate::process::ProcessIdentity {
            argv: vec!["/opt/chrome --user-data-dir=/tmp/profile about:blank".into()],
            start_time: None,
        };
        assert!(command_line_matches(&rewritten, &record));
        assert!(describes(&rewritten, &record));

        let other = crate::process::ProcessIdentity {
            argv: vec!["/opt/chrome --user-data-dir=/tmp/other about:blank".into()],
            start_time: None,
        };
        assert!(!command_line_matches(&other, &record));

        // A truncated line must not match: the arguments have to be the end.
        let partial = crate::process::ProcessIdentity {
            argv: vec!["/opt/chrome --user-data-dir=/tmp/profile".into()],
            start_time: None,
        };
        assert!(!command_line_matches(&partial, &record));

        // An empty record cannot be recognised at all.
        let empty = ProcessRecord {
            args: vec![],
            ..record.clone()
        };
        assert!(!command_line_matches(&rewritten, &empty));
    }

    /// A record nobody can check - another platform, or an unreadable process -
    /// keeps its record and its process. Treating "cannot be asked" as "gone"
    /// would delete the only trace of a browser that is still running.
    #[test]
    fn a_record_nobody_can_check_is_left_alone() {
        let dir = runtime_dir("blind");
        let profile_id = ProfileId::new();
        let mut child = spawn_sleep();
        write(&dir, &record_for(profile_id, &child)).unwrap();
        let config = dir.join(profile_id.to_string()).join(XRAY_CONFIG_FILE);
        std::fs::write(&config, "{}").unwrap();

        let report = reclaim(
            &dir,
            &BlindInspector,
            &DefaultProcessTreeController,
            Duration::from_millis(50),
        );

        assert!(report.reclaimed.is_empty(), "{report:?}");
        assert!(report.stale.is_empty(), "{report:?}");
        assert_eq!(report.unresolved.len(), 1, "{report:?}");
        assert!(
            report.unresolved[0].reason.contains("cannot be identified"),
            "{report:?}"
        );
        assert!(read(&dir, profile_id).unwrap().is_some());
        assert!(config.exists());
        assert_ne!(
            DefaultProcessInspector.inspect(child.id()),
            ProcessReading::Absent
        );

        DefaultProcessTreeController
            .terminate_tree(child.id())
            .unwrap();
        child.wait().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_is_confirmed_against_the_live_processes() {
        let profile_id = ProfileId::new();
        let mut child = spawn_sleep();
        let record = record_for(profile_id, &child);
        // The live process is the instance the record captured.
        assert_eq!(confirm(&record, &DefaultProcessInspector), None);

        // A pid the kernel handed to another process: same command line, a
        // different instance.
        let mut recycled = record.clone();
        recycled.browser.start_time = recycled.browser.start_time.map(|started| started + 1);
        let disagreement = confirm(&recycled, &DefaultProcessInspector).unwrap();
        assert!(disagreement.contains("browser"), "{disagreement}");
        assert!(disagreement.contains("now runs"), "{disagreement}");

        // With no start time to compare, the command line has to decide, and it
        // does not agree either.
        let mut mismatched = record.clone();
        mismatched.browser.start_time = None;
        mismatched.browser.args = vec!["61".into()];
        let disagreement = confirm(&mismatched, &DefaultProcessInspector).unwrap();
        assert!(disagreement.contains("browser"), "{disagreement}");

        // A process nobody can be asked about is not a disagreement.
        assert_eq!(confirm(&record, &BlindInspector), None);

        DefaultProcessTreeController
            .terminate_tree(child.id())
            .unwrap();
        child.wait().unwrap();
    }
}
