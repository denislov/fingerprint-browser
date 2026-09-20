//! Asking a proxy whether traffic leaves through it, and from where.
//!
//! Starting an engine, sending one request and waiting for the answer takes
//! seconds and can block on a proxy that accepts a connection and then says
//! nothing, so it runs on a worker thread and reports through the channel the
//! view drains. The trait exists so the view can be tested without a proxy, and
//! so a test never has to reach the network.
//!
//! The question here is deliberately narrow: *did a byte leave through this
//! proxy, and what address did it leave from*. [`crate::verifier`] answers the
//! neighbouring question - what the running browser claims about itself - and
//! neither replaces the other. A profile can reproduce every fingerprint claim
//! and still be leaking, because a fingerprint is what the page is told while
//! the exit address is what the far end sees.

use domain::{ProxyId, ProxyProfile};
use runtime::{
    DefaultXrayConfigBuilder, Diagnosis, DiagnosticRequest, Engine, Fault, SocksEchoClient,
    diagnose_proxy,
};
use std::path::PathBuf;
use std::time::Duration;

/// How long one request may take before it is reported as a timeout.
///
/// This bounds the request, not the engine's start-up, which has its own
/// deadline below.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a temporary engine may take to bind its loopback inbound.
///
/// The same budget the launch path gives it, so a proxy that would be too slow
/// to launch is also too slow to pass a test - the test must not be the more
/// forgiving of the two.
pub const ENGINE_READY_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything a worker needs to test one proxy without touching the view.
#[derive(Debug, Clone)]
pub struct ProxyTestJob {
    pub proxy_id: ProxyId,
    pub proxy: ProxyProfile,
    /// The address endpoint to ask. Read from the settings when the test is
    /// started, so the endpoint that was asked is the one that was configured.
    pub echo_url: String,
    /// The SOCKS port of an engine that is already running for this proxy.
    ///
    /// When a profile is up, its engine is the one that would have to forward,
    /// so testing that engine tests the live path. `None` means nothing is
    /// running and the test starts an engine of its own.
    pub live_port: Option<u16>,
}

impl ProxyTestJob {
    /// True when this will probe an engine that is already up.
    pub fn is_live(&self) -> bool {
        self.live_port.is_some()
    }
}

/// Sends one request through a proxy and reports what came back.
pub trait ProxyTester: Send + Sync {
    /// The reading, or the fault that stopped it.
    ///
    /// An `Err` is not a working proxy: it is a proxy whose traffic did not
    /// arrive, and the fault says where it stopped.
    fn test(&self, job: &ProxyTestJob) -> Result<Diagnosis, Fault>;
}

/// The real tester: one request through the engine, temporary or already up.
pub struct XrayProxyTester {
    executable: PathBuf,
    runtime_dir: PathBuf,
}

impl XrayProxyTester {
    pub fn new(executable: impl Into<PathBuf>, runtime_dir: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            runtime_dir: runtime_dir.into(),
        }
    }
}

impl ProxyTester for XrayProxyTester {
    fn test(&self, job: &ProxyTestJob) -> Result<Diagnosis, Fault> {
        // One directory per proxy, under the runtime directory the supervisor
        // already uses: a temporary engine is not a second kind of thing, it is
        // the same thing without a profile behind it.
        let directory = self.runtime_dir.join(job.proxy_id.to_string());
        let engine = match job.live_port {
            Some(socks_port) => Engine::Running { socks_port },
            None => Engine::Temporary {
                executable: &self.executable,
                directory: &directory,
                ready_timeout: ENGINE_READY_TIMEOUT,
            },
        };
        diagnose_proxy(
            &SocksEchoClient,
            &DefaultXrayConfigBuilder::new(),
            &job.proxy,
            &DiagnosticRequest {
                engine,
                echo_url: &job.echo_url,
                probe_timeout: PROBE_TIMEOUT,
            },
        )
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The address a passing fake reports, from the block reserved for
    /// documentation, so it can never be mistaken for a real one.
    pub const FAKE_EXIT_IP: &str = "203.0.113.7";

    /// A tester a test drives: it answers with whatever it was given, and
    /// records which engine it was asked about.
    #[derive(Default)]
    pub struct FakeProxyTester {
        outcome: Mutex<Option<Result<Diagnosis, Fault>>>,
        calls: AtomicUsize,
        live_ports: Mutex<Vec<Option<u16>>>,
    }

    impl FakeProxyTester {
        pub fn passing() -> Self {
            Self::with_outcome(Ok(Diagnosis {
                exit_ip: FAKE_EXIT_IP.to_string(),
                elapsed: Duration::from_millis(250),
            }))
        }

        pub fn passing_from(exit_ip: &str) -> Self {
            Self::with_outcome(Ok(Diagnosis {
                exit_ip: exit_ip.to_string(),
                elapsed: Duration::from_millis(120),
            }))
        }

        pub fn with_outcome(outcome: Result<Diagnosis, Fault>) -> Self {
            Self {
                outcome: Mutex::new(Some(outcome)),
                calls: AtomicUsize::new(0),
                live_ports: Mutex::new(Vec::new()),
            }
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// Which engine each call was asked about, oldest first.
        pub fn ports_asked(&self) -> Vec<Option<u16>> {
            self.live_ports.lock().expect("ports lock").clone()
        }
    }

    impl ProxyTester for FakeProxyTester {
        fn test(&self, job: &ProxyTestJob) -> Result<Diagnosis, Fault> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.live_ports
                .lock()
                .expect("ports lock")
                .push(job.live_port);
            self.outcome
                .lock()
                .expect("outcome lock")
                .clone()
                .unwrap_or(Ok(Diagnosis {
                    exit_ip: FAKE_EXIT_IP.to_string(),
                    elapsed: Duration::from_millis(250),
                }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real tester on a proxy that leads nowhere must report a fault rather
    /// than a reading, and must not need a network to do it.
    ///
    /// `192.0.2.1` is in the block reserved for documentation, so it is never a
    /// reachable host on any machine this runs on.
    #[test]
    fn a_proxy_that_leads_nowhere_reports_a_fault_not_a_reading() {
        let tester = XrayProxyTester::new("bin/definitely-not-xray", "data/runtime");
        let job = ProxyTestJob {
            proxy_id: ProxyId::new(),
            proxy: ProxyProfile {
                id: ProxyId::new(),
                name: "Nowhere".to_string(),
                outbound: domain::ProxyOutbound::Socks5(domain::Socks5Outbound {
                    host: "192.0.2.1".to_string(),
                    port: 1080,
                    username: None,
                    password: None,
                }),
            },
            echo_url: "http://127.0.0.1:1/".to_string(),
            live_port: None,
        };

        let outcome = tester.test(&job);

        let fault = outcome.expect_err("an engine that is not there cannot forward");
        assert_eq!(
            fault.class,
            runtime::FaultClass::Engine,
            "the engine is the thing that was missing: {fault}"
        );
    }

    /// A job that names a port says so, because which engine was probed is the
    /// difference between "this proxy works" and "this profile is actually
    /// using it".
    #[test]
    fn a_job_with_a_port_is_a_live_one() {
        let mut job = ProxyTestJob {
            proxy_id: ProxyId::new(),
            proxy: ProxyProfile {
                id: ProxyId::new(),
                name: "Office".to_string(),
                outbound: domain::ProxyOutbound::Socks5(domain::Socks5Outbound {
                    host: "10.0.0.1".to_string(),
                    port: 1080,
                    username: None,
                    password: None,
                }),
            },
            echo_url: "http://127.0.0.1:1/".to_string(),
            live_port: None,
        };
        assert!(!job.is_live(), "nothing is running, so nothing is live");

        job.live_port = Some(51234);
        assert!(job.is_live());
    }
}
