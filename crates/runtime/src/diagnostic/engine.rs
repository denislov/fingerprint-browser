//! Temporary engine acquisition and cleanup.
use super::*;

/// A temporary engine, alive for exactly one diagnostic.
pub(super) struct TemporaryEngine {
    pub(super) child: crate::process::ManagedChild,
    pub(super) port: u16,
    pub(super) config_path: PathBuf,
    pub(super) log_path: PathBuf,
}

impl TemporaryEngine {
    pub(super) fn start(
        builder: &dyn XrayConfigBuilder,
        proxy: &ProxyProfile,
        executable: &Path,
        directory: &Path,
        ready_timeout: Duration,
    ) -> Result<Self, Fault> {
        let config_path = directory.join(DIAGNOSTIC_CONFIG_FILE);
        let log_path = directory.join(DIAGNOSTIC_LOG_FILE);
        // A previous run's words must not be read back as this one's account.
        remove_if_present(&log_path);
        remove_if_present(&config_path);

        let reservation = TcpPortAllocator::new()
            .reserve_loopback()
            .map_err(|error| {
                Fault::new(
                    FaultClass::Engine,
                    format!("no loopback port for the diagnostic engine: {error}"),
                )
            })?;
        let port = reservation.port();

        // The same validation and the same file shape the launch path writes,
        // so a diagnostic cannot pass a proxy the launcher would refuse, and
        // cannot refuse one it would accept.
        builder
            .build(proxy, port, &config_path)
            .map_err(|error| Fault::new(FaultClass::Config, error.to_string()))?;

        // Built through the plan's own `args`, so the diagnostic engine is
        // started the way the launched one is started.
        let plan = XrayLaunchPlan {
            executable: executable.to_path_buf(),
            config_path: config_path.clone(),
            socks_port: port,
        };
        let log = std::fs::File::create(&log_path).map_err(|error| {
            Fault::new(
                FaultClass::Engine,
                format!(
                    "the diagnostic log could not be created at {}: {error}",
                    log_path.display()
                ),
            )
        })?;

        let mut command = std::process::Command::new(&plan.executable);
        command
            .args(plan.args())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(log));

        // The engine binds its own socket: the reservation goes at the handoff,
        // exactly as the launch path releases it.
        drop(reservation);
        let child = crate::process::spawn_managed(&mut command).map_err(|error| {
            remove_if_present(&config_path);
            remove_if_present(&log_path);
            Fault::new(
                FaultClass::Engine,
                format!(
                    "the diagnostic engine at {} did not start: {error}",
                    plan.executable.display()
                ),
            )
        })?;

        let mut engine = Self {
            child,
            port,
            config_path,
            log_path,
        };
        if let Err(error) = crate::xray::wait_ready(&mut engine.child, port, ready_timeout, || true)
        {
            // A config the engine rejects also fails readiness, and the log is
            // where it says so. Without one, the readiness failure stands.
            let fault = fault_from_log(&engine.log_path)
                .unwrap_or_else(|| Fault::new(FaultClass::Engine, error.to_string()));
            // `engine` drops here, taking the process and both files with it.
            return Err(fault);
        }
        Ok(engine)
    }
}

impl Drop for TemporaryEngine {
    fn drop(&mut self) {
        // Unix children lead their own process group, so the group goes first.
        // On Windows the child was created inside a private kill-on-close job,
        // so closing the owned handle takes the tree even where the kill below
        // cannot. This mirrors the supervisor's teardown.
        #[cfg(unix)]
        {
            use crate::process::{DefaultProcessTreeController, ProcessTreeController};
            let _ = DefaultProcessTreeController::new().terminate_tree(self.child.id());
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        remove_if_present(&self.config_path);
        remove_if_present(&self.log_path);
    }
}

/// To a caller clearing its own files, absent and unremovable are the same.
pub(super) fn remove_if_present(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => tracing::debug!("left {} behind: {error}", path.display()),
    }
}
