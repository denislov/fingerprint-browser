use crate::error::ProcessError;
use std::process::{Child, Command};

/// All supervised children must lead their own process group on Unix.
pub(crate) fn spawn_managed(command: &mut Command) -> std::io::Result<Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn()
}

/// What a live process reports about itself, read back rather than assumed.
///
/// A pid is not evidence: between a crash and the next start it can be recycled
/// onto an unrelated process. The argument vector and the kernel start time are
/// what a session record is checked against before anything is signalled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    /// The argument vector as the kernel holds it, `argv[0]` included.
    pub argv: Vec<String>,
    /// Kernel start time in clock ticks since boot, where the platform exposes
    /// it. Together with the pid this survives pid recycling.
    pub start_time: Option<u64>,
}

impl ProcessIdentity {
    /// Everything that was handed to spawn, `argv[0]` excluded. An executable
    /// that `exec`s a wrapper or a real binary keeps this vector, so this is the
    /// part of a command line that a record can be compared against.
    pub fn args(&self) -> &[String] {
        self.argv.get(1..).unwrap_or_default()
    }
}

/// What was learned about a process, with "cannot be asked" kept apart from
/// "not there".
///
/// The two must never be collapsed. On a platform that cannot read another
/// process's command line, every record would otherwise look like a record for
/// a process that had exited, and reclaiming would delete the only trace of a
/// browser that is still running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessReading {
    /// The pid is not in use, or holds a process that has already exited.
    Absent,
    /// This platform cannot be asked, or the process is still between `fork`
    /// and `execve` and has no argument vector yet.
    Unknown,
    /// What the live process reports about itself.
    Live(ProcessIdentity),
}

pub trait ProcessInspector: Send + Sync {
    fn inspect(&self, pid: u32) -> ProcessReading;
}

#[derive(Debug, Default)]
pub struct DefaultProcessInspector;

impl DefaultProcessInspector {
    pub fn new() -> Self {
        Self
    }
}

impl ProcessInspector for DefaultProcessInspector {
    #[cfg(target_os = "linux")]
    fn inspect(&self, pid: u32) -> ProcessReading {
        let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(error) => return reading_for_error(&error),
        };
        // The second field of `stat` is the executable name in parentheses and
        // may itself contain spaces, so only what follows the last ')' can be
        // split. The first token there is field 3, which makes field 22 - the
        // start time - the twentieth.
        let (state, start_time) = stat
            .rsplit_once(") ")
            .map(|(_, rest)| {
                let mut fields = rest.split_whitespace();
                (
                    fields.next().and_then(|state| state.chars().next()),
                    fields.nth(18).and_then(|value| value.parse().ok()),
                )
            })
            .unwrap_or((None, None));
        // A zombie still holds its pid and its entry in `stat`, but it cannot
        // run and it holds no argument vector to compare against.
        if matches!(state, Some('Z' | 'X')) {
            return ProcessReading::Absent;
        }

        let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
            Ok(cmdline) => cmdline,
            Err(error) => return reading_for_error(&error),
        };
        let argv: Vec<String> = cmdline
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect();
        if argv.is_empty() {
            // A process that has not reached `execve` yet has no command line to
            // read. Nobody can say what it is, so this is not evidence of
            // absence.
            return ProcessReading::Unknown;
        }
        ProcessReading::Live(ProcessIdentity { argv, start_time })
    }

    #[cfg(not(target_os = "linux"))]
    fn inspect(&self, _pid: u32) -> ProcessReading {
        // Reading another process's command line and start time portably would
        // mean shelling out to `ps` and parsing its output. Rather than guess,
        // this platform reports that it cannot be asked, and reclaim then leaves
        // its records alone instead of deciding they describe dead processes.
        ProcessReading::Unknown
    }
}

#[cfg(target_os = "linux")]
fn reading_for_error(error: &std::io::Error) -> ProcessReading {
    match error.kind() {
        std::io::ErrorKind::NotFound => ProcessReading::Absent,
        _ => ProcessReading::Unknown,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    #[test]
    fn cleanup_kills_descendants_after_group_leader_exits() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 60 & echo $!; exit 0"])
            .stdout(Stdio::piped());
        let mut child = spawn_managed(&mut command).unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let descendant: u32 = line.trim().parse().unwrap();
        child.wait().unwrap();
        DefaultProcessTreeController
            .terminate_tree(child.id())
            .unwrap();
        let start = Instant::now();
        loop {
            let status = std::fs::read_to_string(format!("/proc/{descendant}/stat"));
            // An orphan zombie awaits the host's init reaper but cannot run.
            if status
                .as_ref()
                .map_or(true, |s| s.rsplit_once(") ").unwrap().1.starts_with('Z'))
            {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "descendant survived group cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn spawn(command: &mut Command) -> Child {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        spawn_managed(command).unwrap()
    }

    /// Waits for a process to reach `execve`, which is the only moment its
    /// command line becomes readable.
    fn live_identity(pid: u32) -> ProcessIdentity {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let ProcessReading::Live(identity) = DefaultProcessInspector.inspect(pid) {
                return identity;
            }
            assert!(
                Instant::now() < deadline,
                "no command line for pid {pid}: {:?}",
                DefaultProcessInspector.inspect(pid)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_managed_child_reports_the_arguments_it_was_given() {
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let mut child = spawn(&mut command);

        let identity = live_identity(child.id());
        assert_eq!(identity.args(), ["60"]);
        assert_eq!(identity.argv[0], "/bin/sleep");
        assert!(identity.start_time.is_some(), "{identity:?}");

        DefaultProcessTreeController
            .terminate_tree(child.id())
            .unwrap();
        child.wait().unwrap();
    }

    /// The window between `fork` and `execve` must not read as absence: a
    /// record written for a process in it would otherwise be treated as a
    /// record for a process that is gone.
    #[test]
    fn a_just_spawned_child_is_never_reported_absent() {
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let mut child = spawn(&mut command);

        let reading = DefaultProcessInspector.inspect(child.id());
        assert!(
            matches!(reading, ProcessReading::Live(_) | ProcessReading::Unknown),
            "{reading:?}"
        );

        DefaultProcessTreeController
            .terminate_tree(child.id())
            .unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_reaped_child_is_not_there() {
        let mut command = Command::new("/bin/true");
        let mut child = spawn(&mut command);
        child.wait().unwrap();
        let pid = child.id();
        // The pid may have been handed to another process in the meantime; the
        // readings that matter are that nothing claims the child's own command
        // line is still there.
        match DefaultProcessInspector.inspect(pid) {
            ProcessReading::Live(identity) => assert_ne!(identity.argv, ["/bin/true"]),
            ProcessReading::Absent | ProcessReading::Unknown => {}
        }
    }

    #[test]
    fn a_zombie_is_not_there() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        let mut child = spawn(&mut command);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match DefaultProcessInspector.inspect(child.id()) {
                ProcessReading::Absent => break,
                reading => {
                    assert!(
                        Instant::now() < deadline,
                        "child never became a zombie: {reading:?}"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        child.wait().unwrap();
    }

    #[test]
    fn asking_a_tree_to_exit_stops_it_without_a_kill() {
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let mut child = spawn(&mut command);

        DefaultProcessTreeController
            .request_tree_exit(child.id())
            .unwrap();
        let status = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }
}

pub trait ProcessTreeController: Send + Sync {
    fn terminate_tree(&self, pid: u32) -> Result<(), ProcessError>;

    /// Ask a managed tree to exit on its own before [`Self::terminate_tree`]
    /// escalates to a kill.
    ///
    /// A reclaimed orphan gets the same polite request a normal stop gets, so
    /// Chromium can flush the profile it holds. Platforms and test doubles that
    /// do not model signals keep the default, which does nothing and lets the
    /// caller escalate immediately.
    fn request_tree_exit(&self, _pid: u32) -> Result<(), ProcessError> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct DefaultProcessTreeController;

impl DefaultProcessTreeController {
    pub fn new() -> Self {
        Self
    }

    /// Resolves the process group a managed child leads. `spawn_managed` makes
    /// the child's own pid its group id, never the application's group.
    #[cfg(unix)]
    fn group_of(pid: u32) -> Result<i32, ProcessError> {
        i32::try_from(pid)
            .ok()
            .filter(|pid| *pid > 1)
            .ok_or_else(|| ProcessError::TerminationFailed("invalid process group".into()))
    }

    #[cfg(unix)]
    fn signal_group(pid: u32, signal: i32) -> Result<(), ProcessError> {
        let group = Self::group_of(pid)?;
        // SAFETY: kill takes integer arguments only.
        if unsafe { libc::kill(-group, signal) } == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(ProcessError::TerminationFailed(error.to_string()));
            }
        }
        Ok(())
    }
}

impl ProcessTreeController for DefaultProcessTreeController {
    fn terminate_tree(&self, pid: u32) -> Result<(), ProcessError> {
        #[cfg(target_os = "windows")]
        {
            let status = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .status()
                .map_err(ProcessError::SpawnFailed)?;

            if !status.success() {
                // Non-zero exit code might mean process already terminated
                tracing::debug!("taskkill exited with non-zero status for pid {}", pid);
            }
            Ok(())
        }

        #[cfg(unix)]
        {
            Self::signal_group(pid, libc::SIGKILL)
        }
    }

    fn request_tree_exit(&self, pid: u32) -> Result<(), ProcessError> {
        #[cfg(unix)]
        {
            Self::signal_group(pid, libc::SIGTERM)
        }

        #[cfg(not(unix))]
        {
            // `taskkill /T` without `/F` only reaches windowed applications, so
            // there is no request to make here; the caller escalates to a kill.
            let _ = pid;
            Ok(())
        }
    }
}
