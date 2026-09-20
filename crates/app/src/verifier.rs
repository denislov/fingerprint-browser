//! Reading a running profile back out of its browser.
//!
//! Two questions are asked of the same running browser, and their answers come
//! from different places:
//!
//! - Does the browser reproduce what the profile asked for? A reading cannot
//!   answer that on its own, so [`verify_fingerprint`] compares it with the
//!   profile's claims.
//! - Does its traffic leave from where the profile's proxy was tested at? Only
//!   the browser can answer that, because the browser's own network path is the
//!   thing in question. See `runtime::egress`.
//!
//! The window cannot do either on its own: a fingerprint reading takes seconds
//! and blocks on a browser that may never answer, and the address read-back
//! waits on a page load. Both run on a worker thread and report through a
//! channel the view drains, and the trait exists so the view can be tested
//! without a browser.

use domain::{CoreCapabilities, FingerprintProfile, ProfileId};
use runtime::{
    Discrepancy, EgressOutcome, EgressProbe, FingerprintProbe, verify_egress, verify_fingerprint,
};
use std::time::Duration;

/// How long a verification may take before it is reported as failed.
pub const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(25);

/// How long the browser is given to reach the address endpoint.
///
/// Shorter than a fingerprint reading, which has a whole page of measurements
/// to take where this waits for one small document. A path that cannot carry it
/// is worth reporting rather than holding the window open for.
pub const EGRESS_TIMEOUT: Duration = Duration::from_secs(10);

/// The address question, for a profile whose traffic is meant to leave by a
/// proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressJob {
    /// The endpoint the browser is sent to.
    pub echo_url: String,
    /// The address the profile's proxy was tested at, when it was tested in
    /// this session. With no pre-flight there is nothing for a reading to
    /// disagree with, and the address is reported rather than judged.
    pub expected: Option<String>,
}

/// Everything a worker needs to verify one profile without touching the view.
#[derive(Debug, Clone)]
pub struct VerificationJob {
    pub profile_id: ProfileId,
    pub port: u16,
    pub profile: FingerprintProfile,
    pub capabilities: CoreCapabilities,
    /// `None` when the profile has no proxy. Such a profile claims nothing
    /// about an address, so nothing is asked about one - and asking would send
    /// a direct browser to an endpoint for nothing, where the endpoint learns
    /// the address it was asked from.
    pub egress: Option<EgressJob>,
}

/// Everything one verification established.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VerificationReport {
    /// The claims the reading did not support.
    pub discrepancies: Vec<Discrepancy>,
    /// The address the browser's own traffic left from, when it was read.
    pub exit_ip: Option<String>,
    /// Why the address could not be read, when it could not.
    pub exit_unreadable: Option<String>,
}

impl VerificationReport {
    /// A report from a fingerprint reading, before the path was asked.
    pub fn from_discrepancies(discrepancies: Vec<Discrepancy>) -> Self {
        Self {
            discrepancies,
            ..Self::default()
        }
    }

    /// How the address question reads, when it was asked at all.
    pub fn exit_label(&self) -> Option<String> {
        if let Some(exit_ip) = &self.exit_ip {
            return Some(format!("traffic left from {exit_ip}"));
        }
        self.exit_unreadable
            .as_ref()
            .map(|reason| format!("exit address not read: {reason}"))
    }
}

/// Reads a running browser back and compares it with what a profile asked for.
pub trait FingerprintVerifier: Send + Sync {
    /// Takes a reading from the browser on `job.port`.
    ///
    /// Returns the claims the reading did not support, along with where the
    /// traffic left from when the profile's proxy was in question; an empty
    /// discrepancy list means the profile was reproduced. An `Err` means no
    /// reading could be taken at all, which is not the same as a profile that
    /// was reproduced.
    fn verify(&self, job: &VerificationJob) -> Result<VerificationReport, String>;
}

/// The real verifier: reads the fingerprint and the exit address over CDP.
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
    fn verify(&self, job: &VerificationJob) -> Result<VerificationReport, String> {
        let observed = FingerprintProbe::new(self.timeout)
            .read_fresh(job.port)
            .map_err(|error| error.to_string())?;
        let mut report = VerificationReport::from_discrepancies(verify_fingerprint(
            &job.profile,
            &job.capabilities,
            &observed,
        ));

        // The fingerprint question is settled first, so a browser that cannot be
        // read at all is reported as that rather than as a path problem. They
        // are fixed in different places, and the reading that failed is the one
        // the user asked for.
        if let Some(egress) = &job.egress {
            let outcome = EgressProbe::new(EGRESS_TIMEOUT).read(job.port, &egress.echo_url);
            report
                .discrepancies
                .extend(verify_egress(egress.expected.as_deref(), &outcome));
            match &outcome {
                EgressOutcome::Read(reading) => report.exit_ip = Some(reading.exit_ip.clone()),
                other => report.exit_unreadable = Some(other.detail()),
            }
        }
        Ok(report)
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
        outcome: Mutex<Option<Result<VerificationReport, String>>>,
        calls: AtomicUsize,
        egrees: Mutex<Vec<Option<EgressJob>>>,
    }

    impl FakeVerifier {
        pub fn passing() -> Self {
            Self::with_outcome(Ok(VerificationReport::default()))
        }

        /// A pass that also read an address back, as a proxied profile does.
        pub fn passing_from(exit_ip: &str) -> Self {
            Self::with_outcome(Ok(VerificationReport {
                exit_ip: Some(exit_ip.to_string()),
                ..VerificationReport::default()
            }))
        }

        pub fn disagreeing(discrepancies: Vec<Discrepancy>) -> Self {
            Self::with_outcome(Ok(VerificationReport::from_discrepancies(discrepancies)))
        }

        pub fn with_outcome(outcome: Result<VerificationReport, String>) -> Self {
            Self {
                outcome: Mutex::new(Some(outcome)),
                calls: AtomicUsize::new(0),
                egrees: Mutex::new(Vec::new()),
            }
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// The address question each call was given, in order.
        pub fn egrees_asked(&self) -> Vec<Option<EgressJob>> {
            self.egrees.lock().expect("egress lock").clone()
        }
    }

    impl FingerprintVerifier for FakeVerifier {
        fn verify(&self, job: &VerificationJob) -> Result<VerificationReport, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.egrees
                .lock()
                .expect("egress lock")
                .push(job.egress.clone());
            self.outcome
                .lock()
                .expect("outcome lock")
                .clone()
                .unwrap_or(Ok(VerificationReport::default()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(port: u16) -> VerificationJob {
        VerificationJob {
            profile_id: ProfileId::new(),
            port,
            profile: FingerprintProfile::new_random(1),
            capabilities: CoreCapabilities::for_major(148),
            // No proxy, so nothing is asked about an address: this test is
            // about a browser that cannot be read at all.
            egress: None,
        }
    }

    #[test]
    fn the_cdp_verifier_reports_an_unreachable_port_rather_than_passing() {
        // Nothing is listening: the failure must be an error, never an empty
        // discrepancy list, which would read as "verified".
        let verifier = CdpFingerprintVerifier::new(Duration::from_millis(200));

        let outcome = verifier.verify(&job(1));

        assert!(outcome.is_err(), "an unread profile is not a verified one");
    }

    /// The address question is only put to a profile that makes a claim about
    /// one, and a browser that cannot be read at all is reported as that rather
    /// than as a path that carried nothing.
    #[test]
    fn a_browser_that_cannot_be_read_is_not_reported_as_a_path_problem() {
        let verifier = CdpFingerprintVerifier::new(Duration::from_millis(200));
        let mut job = job(1);
        job.egress = Some(EgressJob {
            echo_url: "http://127.0.0.1:1/".to_string(),
            expected: Some("203.0.113.7".to_string()),
        });

        let error = verifier
            .verify(&job)
            .expect_err("an unreadable browser is not a reading");

        assert!(
            !error.contains("exit address"),
            "the fingerprint reading is the one that failed: {error}"
        );
    }
}
