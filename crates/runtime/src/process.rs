use crate::error::ProcessError;
use std::process::Command;

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

        #[cfg(not(target_os = "windows"))]
        {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
            Ok(())
        }
    }
}
