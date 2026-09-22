//! Copying browser data off the UI thread.
//!
//! A profile's browser data is hundreds of megabytes, so copying it is not the
//! local-and-quick work a configuration export is: it can block for seconds, and
//! blocking the window while it runs would freeze the interface. So it runs on a
//! worker thread and reports through the channel the view drains, exactly as a
//! proxy test and a directory open do.
//!
//! The trait exists for the same reason the neighbouring workers have one: a test
//! must be able to press the button without hundreds of megabytes being written,
//! and without the machine it runs on being touched.

use crate::text::Text;
use application::{BrowserDataReport, Direction, copy_browser_data, restore_browser_data};
use domain::{BrowserProfile, ProfileId};
use std::collections::HashSet;
use std::path::PathBuf;

/// A notice for the profiles whose data an interrupted restore had left aside,
/// or `None` when there was none to put back.
///
/// A success rather than a problem, which is why the caller shows it as a toast:
/// the data is where it belongs either way, and the line is there because a
/// directory that came back on its own is otherwise indistinguishable from one
/// that was never lost.
pub fn recovery_notice(put_back: &[String], t: &Text) -> Option<(String, bool)> {
    if put_back.is_empty() {
        return None;
    }
    Some((t.data_recovered(&t.names(put_back)), false))
}

/// Everything a worker needs to copy browser data without touching the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserDataJob {
    /// Which way the copy goes: out to the backup, or back in.
    pub direction: Direction,
    /// The profiles to copy, as storage holds them.
    pub profiles: Vec<BrowserProfile>,
    /// The identifiers of profiles that are active, which the copy refuses.
    pub running: Vec<ProfileId>,
    /// The backup directory: written to when copying out, read from when copying
    /// back in.
    pub directory: PathBuf,
}

/// Copies browser data, and reports what it did or why it could not.
pub trait BrowserDataCopier: Send + Sync {
    /// Runs one copy to completion. Blocks; callers run it off the UI thread.
    ///
    /// An `Err` is the reason, already phrased for the window.
    fn run(&self, job: &BrowserDataJob) -> Result<BrowserDataReport, String>;
}

/// The real copier: the filesystem-backed function in `application`.
#[derive(Debug, Default, Clone, Copy)]
pub struct DiskBrowserDataCopier;

impl BrowserDataCopier for DiskBrowserDataCopier {
    fn run(&self, job: &BrowserDataJob) -> Result<BrowserDataReport, String> {
        let running: HashSet<ProfileId> = job.running.iter().copied().collect();
        let outcome = match job.direction {
            Direction::ToBackup => copy_browser_data(&job.profiles, &running, &job.directory),
            Direction::FromBackup => restore_browser_data(&job.profiles, &running, &job.directory),
        };
        outcome.map_err(|error| error.to_string())
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// A copier a test drives: it records the jobs it was given and answers with
    /// whatever it was told to.
    ///
    /// [`FakeBrowserDataCopier::paused`] adds a gate, which is how a test holds a
    /// copy open while it asks the window what it thinks of a second one. The
    /// other constructors leave the gate open, so tests that are not about that
    /// are not slowed down by it.
    #[derive(Default)]
    pub struct FakeBrowserDataCopier {
        outcome: Mutex<Option<Result<BrowserDataReport, String>>>,
        jobs: Mutex<Vec<BrowserDataJob>>,
        calls: AtomicUsize,
        gate: Option<Gate>,
    }

    /// The two ends of the gate: the worker says it is inside the copy, and waits
    /// until the test lets it finish.
    struct Gate {
        entered: std::sync::mpsc::Sender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl FakeBrowserDataCopier {
        /// A copier that reports having copied one profile.
        pub fn passing() -> Self {
            Self::with_outcome(Ok(BrowserDataReport {
                directory: PathBuf::from("/backups/browser-data"),
                copied: vec!["Work laptop".to_string()],
                skipped: Vec::new(),
                bytes: 1024,
            }))
        }

        pub fn with_outcome(outcome: Result<BrowserDataReport, String>) -> Self {
            Self {
                outcome: Mutex::new(Some(outcome)),
                jobs: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                gate: None,
            }
        }

        /// A copier that stops inside the copy until [`Pause::release`], and the
        /// handle the test drives it with.
        ///
        /// This is what makes "a copy is in flight" a state a test can hold still
        /// rather than race: the refusal it is about is only interesting while the
        /// first copy is still running.
        pub fn paused() -> (Self, Pause) {
            let (entered, waiting) = std::sync::mpsc::channel();
            let (release, released) = std::sync::mpsc::channel();
            let copier = Self {
                gate: Some(Gate {
                    entered,
                    release: Mutex::new(released),
                }),
                ..Self::passing()
            };
            (copier, Pause { waiting, release })
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// The jobs the worker was given, oldest first.
        pub fn jobs(&self) -> Vec<BrowserDataJob> {
            self.jobs.lock().expect("jobs lock").clone()
        }

        /// Waits at the gate, when this copier has one.
        fn hold(&self) {
            let Some(gate) = &self.gate else {
                return;
            };
            let _ = gate.entered.send(());
            // Bounded rather than open-ended: a test that forgets to release must
            // fail rather than hang the suite, and what it is holding open is a
            // fake copy.
            if let Ok(released) = gate.release.lock() {
                let _ = released.recv_timeout(Duration::from_secs(10));
            }
        }
    }

    /// The test's end of a paused copier.
    pub struct Pause {
        waiting: std::sync::mpsc::Receiver<()>,
        release: std::sync::mpsc::Sender<()>,
    }

    impl Pause {
        /// Waits until the worker is inside the copy, so that what follows is
        /// about a copy that is genuinely in flight.
        pub fn wait_until_copying(&self) {
            self.waiting
                .recv_timeout(Duration::from_secs(10))
                .expect("the worker reaches the copy");
        }

        /// Lets it finish.
        pub fn release(&self) {
            let _ = self.release.send(());
        }
    }

    impl BrowserDataCopier for FakeBrowserDataCopier {
        fn run(&self, job: &BrowserDataJob) -> Result<BrowserDataReport, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.jobs.lock().expect("jobs lock").push(job.clone());
            self.hold();
            self.outcome
                .lock()
                .expect("outcome lock")
                .clone()
                .unwrap_or_else(|| {
                    Ok(BrowserDataReport {
                        directory: job.directory.clone(),
                        ..Default::default()
                    })
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Lang, text};

    /// The line that says a directory came back, and the case that produces
    /// nothing: a start with nothing to put back must not invent a notice. The
    /// recovery itself is the important half, but a repair nobody is told about
    /// looks like data that was never lost.
    #[test]
    fn a_recovery_is_reported_only_when_something_was_put_back() {
        for lang in Lang::ALL {
            let t = text(lang);
            assert!(recovery_notice(&[], t).is_none(), "{lang:?}");

            let (message, error) = recovery_notice(&["Work laptop".to_string()], t)
                .expect("a profile that was put back is reported");
            assert!(!error, "a repair that worked is not a problem: {message}");
            assert!(message.contains("Work laptop"), "{message}");
        }

        // Several profiles are one phrase, not a list of lines.
        let (message, _) =
            recovery_notice(&["Alice".to_string(), "Bob".to_string()], text(Lang::En))
                .expect("two profiles");
        assert!(message.contains("Alice and Bob"), "{message}");
    }
}
