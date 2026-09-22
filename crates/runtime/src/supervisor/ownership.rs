//! Resource ownership for launch attempts and live sessions.
use super::*;

/// A component the supervisor is responsible for, and how it came to be one.
///
/// A run that was told to leave its children running leaves no handle behind -
/// the handle died with that run - so the next run has only the session record: a
/// pid, the kernel start time that tells one process instance from the next, and
/// the ports. That is enough to watch, to stop and to restart, and it is *all*
/// there is, which is why the two cases are one type here rather than a special
/// case in every method that touches a child.
pub(super) enum Held {
    /// Started by this run. The handle is the authority: it knows whether the
    /// process is alive without asking the kernel, and stopping it goes through
    /// the job or the process group this run created.
    Owned(crate::process::ManagedChild),
    /// Started by a run that deliberately left it running. The record is the
    /// authority, and every question about it is a question about identity -
    /// which is why it is answered by the same rule the journal uses.
    Adopted(ProcessRecord),
}

impl Held {
    pub(super) fn id(&self) -> u32 {
        match self {
            Self::Owned(child) => child.id(),
            Self::Adopted(record) => record.pid,
        }
    }

    /// Why it is no longer running, or `None` while it is.
    ///
    /// "Cannot be asked" is deliberately not "gone": a platform that cannot read
    /// another process's identity must not turn into a crash report and a stopped
    /// profile.
    pub(super) fn exited(&mut self, inspector: &dyn ProcessInspector) -> Option<Gone> {
        match self {
            Self::Owned(child) => match child.try_wait() {
                Ok(None) => None,
                Ok(Some(status)) => Some(Gone {
                    detail: status.to_string(),
                    succeeded: Some(status.success()),
                }),
                Err(error) => Some(Gone {
                    detail: error.to_string(),
                    succeeded: None,
                }),
            },
            Self::Adopted(record) => match journal::verdict(record, inspector) {
                journal::Verdict::Gone => Some(Gone {
                    detail: "the process is gone".to_string(),
                    // A process this run did not spawn has no exit status to
                    // read, so whether it left cleanly cannot be known. It is
                    // reported as a normal stop rather than guessed at as a
                    // crash: the process being gone is the fact, and inventing
                    // an alarm from it is the false report the clean flag exists
                    // to prevent.
                    succeeded: None,
                }),
                journal::Verdict::Ours => None,
                journal::Verdict::Foreign(reason) => Some(Gone {
                    detail: reason,
                    succeeded: None,
                }),
                // Nothing could be learned, so nothing may be decided.
                journal::Verdict::Unreadable(_) => None,
            },
        }
    }

    /// The handle behind an owned component, for a test that has to end a child
    /// the way only its owner can. An adopted component has no handle - which is
    /// the whole distinction - so asking for one is a mistake worth a panic.
    ///
    /// Gated exactly like the test module that is its only caller: `cfg(test)`
    /// alone leaves it unused on Windows, where the supervisor's tests do not
    /// compile because they spawn Unix processes, and the gate refuses warnings.
    #[cfg(all(test, unix))]
    pub(super) fn owned_mut(&mut self) -> &mut crate::process::ManagedChild {
        match self {
            Self::Owned(child) => child,
            Self::Adopted(_) => panic!("an adopted session has no handle"),
        }
    }

    /// Stops it, and everything it started, or says why it could not.
    ///
    /// The answer matters for an adopted session and only for one: it is stopped
    /// by pid against a record that may no longer describe what is running there,
    /// so "the process is not this one" and "it did not exit" have to reach the
    /// caller rather than a log line - the caller is what decides whether the
    /// record may be thrown away. An owned child is a handle this process holds,
    /// so its `wait` is the confirmation and the result is always `Ok`.
    pub(super) fn kill(
        &mut self,
        inspector: &dyn ProcessInspector,
        tree: &dyn ProcessTreeController,
    ) -> Result<(), String> {
        match self {
            Self::Owned(child) => {
                terminate_owned(tree, child);
                Ok(())
            }
            // No handle, so the recorded identity is rechecked at the moment of
            // termination and the tree is stopped by pid.
            Self::Adopted(record) => {
                journal::stop(record, inspector, tree, journal::DEFAULT_GRACE).map(|_| ())
            }
        }
    }

    /// Prepares to leave it running, and gives up the handle without stopping it.
    ///
    /// On Unix a child is not killed when its handle is dropped, so there is
    /// nothing to do. On Windows there is: every child is in a job whose limit is
    /// "kill everything in it when the last handle closes", which is exactly what
    /// makes a crashed manager take its browsers with it. Leaving them on purpose
    /// means clearing that limit first, and then the handle may close.
    pub(super) fn release(&mut self) -> std::io::Result<()> {
        match self {
            Self::Owned(child) => crate::process::release_child(child),
            Self::Adopted(_) => Ok(()),
        }
    }
}

/// Everything one start acquires, in one value.
///
/// A start takes a CDP port, sometimes a SOCKS port, sometimes an Xray process
/// with a temporary config on disk, a browser process, and a session record. Each
/// of those has to be given back if anything later in the start goes wrong, and
/// there are five places in the start that can go wrong - which is five
/// hand-written rollbacks, each of which has to remember every resource acquired
/// before it. This is that list in one place: [`StartAttempt::drop`] ends what it
/// still holds, so a path out of the start that forgets to clean up does not
/// exist, and a sixth failure point added later cannot leak a browser.
///
/// On success the children are taken out and [`StartAttempt::keep`] is called:
/// the record and the config are the running session's then, and must not be
/// undone.
pub(super) struct StartAttempt {
    pub(super) profile_id: ProfileId,
    pub(super) runtime_dir: std::path::PathBuf,
    /// The same tree controller the supervisor uses, shared rather than borrowed
    /// so this can outlive a borrow of it.
    pub(super) tree: Arc<dyn ProcessTreeController>,
    /// Held until the child that will bind it starts. A reservation is a bound
    /// socket, so it cannot simply be kept: the browser has to be able to take
    /// the port.
    pub(super) cdp: Option<crate::ports::PortReservation>,
    pub(super) socks: Option<crate::ports::PortReservation>,
    pub(super) browser: Option<crate::process::ManagedChild>,
    pub(super) xray: Option<crate::process::ManagedChild>,
    /// The ports the children were given, and the arguments the browser was
    /// launched with: what the running session is described by.
    pub(super) cdp_port: u16,
    pub(super) socks_port: Option<u16>,
    pub(super) effective_args: Vec<String>,
    /// The temporary Xray config, which holds upstream credentials. Removed on
    /// every path, including the ones that fail before it is written.
    pub(super) xray_config: Option<std::path::PathBuf>,
    /// Whether a session record was written for this attempt.
    pub(super) recorded: bool,
    /// Set by the cancellation polls, so the caller can tell "the start was
    /// called off" from "the start failed".
    pub(super) cancelled: bool,
    pub(super) kept: bool,
}

impl StartAttempt {
    pub(super) fn new(
        profile_id: ProfileId,
        tree: Arc<dyn ProcessTreeController>,
        runtime_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            profile_id,
            runtime_dir,
            tree,
            cdp: None,
            socks: None,
            browser: None,
            xray: None,
            cdp_port: 0,
            socks_port: None,
            effective_args: Vec::new(),
            xray_config: None,
            recorded: false,
            cancelled: false,
            kept: false,
        }
    }

    /// The ports, once the plan has been built from them.
    pub(super) fn note_ports(&mut self, cdp_port: u16, socks_port: Option<u16>, args: Vec<String>) {
        self.cdp_port = cdp_port;
        self.socks_port = socks_port;
        self.effective_args = args;
    }

    pub(super) fn cdp(&self) -> u16 {
        self.cdp
            .as_ref()
            .expect("the CDP port is held before the plan is built from it")
            .port()
    }

    pub(super) fn socks(&self) -> Option<u16> {
        self.socks.as_ref().map(crate::ports::PortReservation::port)
    }

    pub(super) fn browser_id(&self) -> u32 {
        self.browser
            .as_ref()
            .expect("the browser is held before its identifier is asked for")
            .id()
    }

    pub(super) fn xray_id(&self) -> Option<u32> {
        self.xray.as_ref().map(crate::process::ManagedChild::id)
    }

    pub(super) fn hold_cdp(&mut self, reservation: crate::ports::PortReservation) {
        self.cdp = Some(reservation);
    }

    pub(super) fn hold_socks(&mut self, reservation: crate::ports::PortReservation) {
        self.socks = Some(reservation);
    }

    /// Lets go of a port so the child that is about to start can bind it.
    pub(super) fn release_cdp(&mut self) {
        drop(self.cdp.take());
    }

    pub(super) fn release_socks(&mut self) {
        drop(self.socks.take());
    }

    pub(super) fn hold_browser(&mut self, child: crate::process::ManagedChild) {
        self.browser = Some(child);
    }

    pub(super) fn hold_xray(&mut self, child: crate::process::ManagedChild) {
        self.xray = Some(child);
    }

    pub(super) fn browser_mut(&mut self) -> &mut crate::process::ManagedChild {
        self.browser
            .as_mut()
            .expect("the browser is held before anything asks about it")
    }

    pub(super) fn xray_mut(&mut self) -> Option<&mut crate::process::ManagedChild> {
        self.xray.as_mut()
    }

    pub(super) fn xray_status(
        &mut self,
    ) -> Option<std::io::Result<Option<std::process::ExitStatus>>> {
        self.xray
            .as_mut()
            .map(crate::process::ManagedChild::try_wait)
    }

    /// Remembers the config this attempt will have written, before it is written:
    /// a failure partway through writing it still has to remove it.
    pub(super) fn note_config(&mut self, path: Option<std::path::PathBuf>) {
        self.xray_config = path;
    }

    pub(super) fn note_record(&mut self) {
        self.recorded = true;
    }

    /// Somewhere the start checks whether it has been called off.
    pub(super) fn cancelled(&self) -> bool {
        self.cancelled
    }

    pub(super) fn note_cancelled(&mut self, cancelled: bool) {
        self.cancelled = cancelled;
    }

    /// The start succeeded: these children are the running session's now, and the
    /// record and config describe a live session rather than a failed attempt.
    pub(super) fn keep(&mut self) {
        self.kept = true;
    }

    pub(super) fn take_browser(&mut self) -> crate::process::ManagedChild {
        self.browser.take().expect("a kept attempt has a browser")
    }

    pub(super) fn take_xray(&mut self) -> Option<crate::process::ManagedChild> {
        self.xray.take()
    }

    /// The config path, for the session that will own it.
    pub(super) fn config(&self) -> Option<&std::path::Path> {
        self.xray_config.as_deref()
    }
}

impl Drop for StartAttempt {
    /// Undoes whatever the start acquired and did not hand over.
    ///
    /// Children first, then the files that describe them: a record removed before
    /// its process is gone is a process nothing can find, and the terminal moment
    /// of a failed start is exactly when that matters.
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        if let Some(child) = self.browser.as_mut() {
            terminate_owned(self.tree.as_ref(), child);
        }
        if let Some(child) = self.xray.as_mut() {
            terminate_owned(self.tree.as_ref(), child);
        }
        remove_config(self.xray_config.as_deref());
        if self.recorded {
            journal::remove(&self.runtime_dir, self.profile_id);
        }
    }
}

/// The CDP port of an attempt, for the probe that is waiting on it.
///
/// A free function because the probe takes the port while the attempt is borrowed
/// mutably by the check that ran just before it.
pub(super) fn cdp_port_of(attempt: &StartAttempt) -> u16 {
    attempt.cdp_port
}

/// Why a start did not become a session.
pub(super) struct StartFailure {
    pub(super) message: String,
    /// Whether the start was called off rather than failing: a cancelled start is
    /// not an error to show the user, and it leaves the profile stopped.
    pub(super) cancelled: bool,
}

impl StartFailure {
    pub(super) fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
        }
    }

    pub(super) fn cancelled(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: true,
        }
    }
}

/// Ends a child this program started, and the tree under it.
///
/// One function, called by the start's own value on its way out and by the
/// supervisor for a session that is being stopped, because a browser is a process
/// group and a job object rather than one process: two rules for ending one would
/// be two chances to leave a renderer behind.
pub(super) fn terminate_owned(
    tree: &dyn ProcessTreeController,
    child: &mut crate::process::ManagedChild,
) {
    #[cfg(windows)]
    if let Err(error) = child.terminate_tree() {
        tracing::error!("failed to terminate managed job: {error}");
    }
    if matches!(child.try_wait(), Ok(None)) {
        #[cfg(unix)]
        let _ = tree.terminate_tree(child.id());
        // A direct kill is a fallback if the platform tree controller fails.
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// Removes a temporary Xray config, which holds the upstream credentials.
pub(super) fn remove_config(path: Option<&std::path::Path>) {
    let Some(path) = path else {
        return;
    };
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("failed to remove the temporary Xray config {path:?}: {error}");
    }
}

/// A component that is no longer running, and how much can be said about why.
pub(super) struct Gone {
    pub(super) detail: String,
    /// The exit status, where there is one to read. `None` for a process this run
    /// did not spawn, and for a status that could not be read.
    pub(super) succeeded: Option<bool>,
}

pub(super) struct ActiveSession {
    pub(super) _profile_id: ProfileId,
    pub(super) browser: Held,
    pub(super) xray: Option<Held>,
    pub(super) xray_config: Option<std::path::PathBuf>,
    pub(super) _cdp_port: u16,
    pub(super) _socks_port: Option<u16>,
    pub(super) _effective_args: Vec<String>,
    pub(super) stopping: bool,
    /// Why the last attempt to stop this session failed, if one did.
    ///
    /// A session that could not be stopped stays here, and this is what keeps the
    /// poll from reporting the same failure on every tick: the process is still
    /// there and there is nothing new to learn about it. An explicit stop is what
    /// tries again.
    pub(super) stop_failed: Option<String>,
}
