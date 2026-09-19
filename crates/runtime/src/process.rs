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
}

pub trait ProcessTreeController: Send + Sync {
    fn terminate_tree(&self, pid: u32) -> Result<(), ProcessError>;
}

#[derive(Debug, Default)]
pub struct DefaultProcessTreeController;

impl DefaultProcessTreeController {
    pub fn new() -> Self {
        Self
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
            let group = i32::try_from(pid)
                .ok()
                .filter(|pid| *pid > 1)
                .ok_or_else(|| ProcessError::TerminationFailed("invalid process group".into()))?;
            // SAFETY: kill takes integer arguments only. spawn_managed assigns
            // the child's PID as its group ID, never the application's group.
            if unsafe { libc::kill(-group, libc::SIGKILL) } == -1 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(ProcessError::TerminationFailed(error.to_string()));
                }
            }
            Ok(())
        }
    }
}
