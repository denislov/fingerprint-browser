//! Reading a running profile's fingerprint back and comparing it with the
//! profile.
//!
//! The window cannot do this on its own: a reading takes seconds and blocks on
//! a browser that may never answer, so it runs on a worker thread and reports
//! through a channel the view drains. The trait exists so the view can be
//! tested without a browser.

use domain::{CoreCapabilities, FingerprintProfile};
use runtime::{Discrepancy, FingerprintProbe, verify_fingerprint};
use std::time::Duration;

/// How long a verification may take before it is reported as failed.
pub const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(25);

/// Compares a running browser's fingerprint with what a profile asked for.
pub trait FingerprintVerifier: Send + Sync {
    /// Takes a reading from the browser on `port`.
    ///
    /// Returns the claims the reading did not support; an empty vector means
    /// the profile was reproduced. An `Err` means no reading could be taken at
    /// all, which is not the same as a profile that was reproduced.
    fn verify(
        &self,
        port: u16,
        profile: &FingerprintProfile,
        capabilities: &CoreCapabilities,
    ) -> Result<Vec<Discrepancy>, String>;
}

/// The real verifier: reads the fingerprint over CDP and compares it.
#[derive(Debug, Clone)]
pub struct CdpFingerprintVerifier {
    timeout: Duration,
}

impl Default for CdpFingerprintVerifier {
    fn default() -> Self {
        Self::new(VERIFICATION_TIMEOUT)
    }
}

impl CdpFingerprintVerifier {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl FingerprintVerifier for CdpFingerprintVerifier {
    fn verify(
        &self,
        port: u16,
        profile: &FingerprintProfile,
        capabilities: &CoreCapabilities,
    ) -> Result<Vec<Discrepancy>, String> {
        let observed = FingerprintProbe::new(self.timeout)
            .read_fresh(port)
            .map_err(|error| error.to_string())?;
        Ok(verify_fingerprint(profile, capabilities, &observed))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A verifier a test drives: it answers with whatever it was given, and
    /// records what it was asked.
    #[derive(Default)]
    pub struct FakeVerifier {
        outcome: Mutex<Option<Result<Vec<Discrepancy>, String>>>,
        calls: AtomicUsize,
    }

    impl FakeVerifier {
        pub fn passing() -> Self {
            Self::with_outcome(Ok(Vec::new()))
        }

        pub fn with_outcome(outcome: Result<Vec<Discrepancy>, String>) -> Self {
            Self {
                outcome: Mutex::new(Some(outcome)),
                calls: AtomicUsize::new(0),
            }
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl FingerprintVerifier for FakeVerifier {
        fn verify(
            &self,
            _port: u16,
            _profile: &FingerprintProfile,
            _capabilities: &CoreCapabilities,
        ) -> Result<Vec<Discrepancy>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.outcome
                .lock()
                .expect("outcome lock")
                .clone()
                .unwrap_or(Ok(Vec::new()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cdp_verifier_reports_an_unreachable_port_rather_than_passing() {
        // Nothing is listening: the failure must be an error, never an empty
        // discrepancy list, which would read as "verified".
        let verifier = CdpFingerprintVerifier::new(Duration::from_millis(200));
        let profile = FingerprintProfile::new_random(1);
        let capabilities = CoreCapabilities::for_major(148);

        let outcome = verifier.verify(1, &profile, &capabilities);

        assert!(outcome.is_err(), "an unread profile is not a verified one");
    }
}
