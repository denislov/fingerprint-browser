//! Asking a proxy whether traffic actually leaves through it.
//!
//! [`crate::xray::wait_ready`] answers a different question. It proves the
//! engine bound its loopback inbound, and says nothing about whether the
//! upstream took the credentials, resolved the target name or carried a byte.
//! The difference matters more here than anywhere else in this project: a
//! profile whose proxy silently carries nothing is a profile that leaks, and
//! the fingerprint it presents then contradicts the address it is seen from.
//!
//! So this module sends one real request through the engine and reports what
//! came back. It is split so the interesting parts are testable without a
//! network: [`EchoClient`] carries the request and can be replaced, while the
//! classification, the reading and the engine's lifetime hold no sockets.

mod engine;
mod transport;
use engine::*;
use transport::*;
pub use transport::{ECHO_PREFIX, check_echo_url};

use crate::ports::{PortAllocator, TcpPortAllocator};
use crate::xray::XrayConfigBuilder;
use domain::{ProxyProfile, XrayLaunchPlan};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The temporary engine's config, written beside the profile's own.
///
/// It holds the upstream credentials, so it is removed as soon as the
/// diagnostic ends - on every path, including a failure.
pub const DIAGNOSTIC_CONFIG_FILE: &str = "xray-diagnostic.json";

/// Where the temporary engine's stderr is kept while the diagnostic runs.
///
/// An upstream rejection does not fail the handshake to the local inbound, so
/// the transport failure alone cannot name the cause. This file is the engine's
/// own account of it, read only after a request has already failed.
pub const DIAGNOSTIC_LOG_FILE: &str = "xray-diagnostic.log";

/// The address endpoint used when the user has not named one.
///
/// It answers with the caller's address as plain text. Plain HTTP on purpose:
/// the question here is whether the engine forwards, and a TLS request would
/// report a certificate problem as a proxy problem. Which endpoint is asked is
/// a setting rather than a property of this design, because the endpoint sees
/// the exit address and is therefore the user's choice to make.
pub const DEFAULT_ECHO_URL: &str = "http://api.ipify.org";

/// What the diagnostic calls itself. Named rather than blank, so an operator
/// reading an endpoint's logs can tell this traffic apart from a browser's.
const USER_AGENT: &str = "fingerprint-browser-diagnostic";

/// A reading from an address endpoint is a line, not a document.
const READING_LIMIT: u64 = 4 * 1024;

/// Headers are bounded too. An endpoint that never ends its headers is not one
/// this diagnostic is going to get a reading out of.
const HEADER_LIMIT: u64 = 8 * 1024;

/// Engine output reaches the window and the activity log, so it is bounded. The
/// full line stays in the engine's own log until the diagnostic removes it.
const DETAIL_LIMIT: usize = 240;

/// What a failed diagnostic actually established.
///
/// The classes are kept apart because they are fixed in different places. A
/// credential the upstream refused is not a network that is down, and neither
/// is a machine with no route to the internet, so collapsing them into "the
/// proxy failed" would hide the only thing the user needs to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultClass {
    /// The engine could not be started, or never came up.
    Engine,
    /// The configuration, the request or the endpoint is unusable.
    Config,
    /// The far end refused the credentials it was offered.
    Auth,
    /// The target name could not be resolved.
    ///
    /// Only an engine can report this, never a SOCKS reply: the request carries
    /// the name for the engine to resolve, and the protocol has no reply code
    /// that means "dns". See [`classify_engine_log`].
    Dns,
    /// Nothing carried the connection.
    Unreachable,
    /// Readiness or the request ran out of time.
    Timeout,
    /// TLS failed between the engine and the endpoint.
    Tls,
    /// The endpoint answered with a status that carries no reading.
    Http(u16),
    /// The endpoint answered, and the answer held no address.
    Reading,
    /// Nothing above fits, so the engine's own words are carried instead.
    Other,
}

impl FaultClass {
    /// A short name for a window or a log line.
    pub fn label(self) -> String {
        match self {
            Self::Engine => "engine".to_string(),
            Self::Config => "configuration".to_string(),
            Self::Auth => "authentication".to_string(),
            Self::Dns => "name resolution".to_string(),
            Self::Unreachable => "unreachable".to_string(),
            Self::Timeout => "timeout".to_string(),
            Self::Tls => "tls".to_string(),
            Self::Http(code) => format!("http {code}"),
            Self::Reading => "unreadable answer".to_string(),
            Self::Other => "unclassified".to_string(),
        }
    }
}

impl std::fmt::Display for FaultClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.label())
    }
}

/// A failed diagnostic, with its class and the evidence behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub class: FaultClass,
    /// Why this class, in the words of whatever produced it.
    pub detail: String,
}

impl Fault {
    pub fn new(class: FaultClass, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: shorten(&detail.into()),
        }
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.class.label(), self.detail)
    }
}

/// What a diagnostic that succeeded observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    /// The address the endpoint saw, which is the address that left.
    pub exit_ip: String,
    /// How long the request took, which is what a user compares between proxies.
    pub elapsed: Duration,
}

/// Sends one request through a loopback SOCKS endpoint and returns its body.
///
/// The trait exists so classification and parsing can be tested against a local
/// engine. A test must not depend on an upstream being reachable, and the
/// answer has to be the same on a machine with no network at all.
pub trait EchoClient: Send + Sync {
    fn fetch(&self, socks_port: u16, url: &str, timeout: Duration) -> Result<String, Fault>;
}

/// The real client: a SOCKS5 handshake and one HTTP request, over a socket this
/// module owns.
///
/// The transport is written out rather than delegated because no timeout from a
/// general-purpose HTTP client survived the test. `ureq`'s SOCKS connector
/// performs the handshake on a worker thread and joins it, so a proxy that
/// accepts the connection and then says nothing holds the call open however the
/// timeouts are configured - measured at 60s, waiting for the peer to die. A
/// diagnostic that can hang is worse than no diagnostic, so every read and
/// write below carries a deadline of its own.
///
/// Writing the conversation out has a second benefit: the SOCKS5 reply codes
/// are the protocol's own account of why a request was not carried, and they
/// are exact, where matching on a library's error strings would be a guess at
/// someone else's wording.
#[derive(Debug, Default, Clone, Copy)]
pub struct SocksEchoClient;

impl SocksEchoClient {
    pub fn new() -> Self {
        Self
    }
}

impl EchoClient for SocksEchoClient {
    fn fetch(&self, socks_port: u16, url: &str, timeout: Duration) -> Result<String, Fault> {
        let target = HttpTarget::parse(url)?;
        let mut socket = Bounded::connect(socks_port, timeout)?;
        handshake(&mut socket, &target)?;
        request(&mut socket, &target)
    }
}

/// The address an endpoint's answer carries.
///
/// Address endpoints do not agree on a shape: some answer with the bare
/// address, some with a line of prose, some with JSON. The first token that
/// parses as an address is the reading. An answer holding no such token is not
/// a reading - which is a different outcome from a proxy that carried nothing,
/// and is reported as one.
pub fn extract_exit_ip(body: &str) -> Option<String> {
    body.split(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '.' || character == ':')
    })
    .find(|token| token.parse::<std::net::IpAddr>().is_ok())
    .map(str::to_owned)
}

/// The engine's own account of an upstream it could not reach.
///
/// These lines are the only place the specific cause appears, because the
/// handshake with the local inbound succeeds whether or not the upstream
/// accepted anything. The first marker that matches wins; a log with no marker
/// still yields its last line, unclassified. An engine message nobody has seen
/// before is carried to the user rather than mapped onto a class it may not
/// belong to.
pub fn classify_engine_log(text: &str) -> Option<Fault> {
    const MARKERS: &[(FaultClass, &[&str])] = &[
        (
            FaultClass::Config,
            &[
                "failed to parse",
                "invalid config",
                "failed to load config",
                "unknown transport",
                "unsupported protocol",
                "failed to build",
            ],
        ),
        (
            FaultClass::Auth,
            &[
                "invalid user",
                "invalid password",
                "authentication failed",
                "unknown user",
                "unauthorized",
            ],
        ),
        (
            FaultClass::Dns,
            &["no such host", "name resolution", "server misbehaving"],
        ),
        (
            FaultClass::Unreachable,
            &[
                "connection refused",
                "no route to host",
                "network is unreachable",
                "connection reset",
            ],
        ),
        (
            FaultClass::Timeout,
            &["i/o timeout", "deadline exceeded", "timed out"],
        ),
        (FaultClass::Tls, &["tls handshake", "certificate"]),
    ];

    let lowered = text.to_lowercase();
    for (class, markers) in MARKERS {
        if markers.iter().any(|marker| lowered.contains(marker)) {
            return Some(Fault::new(*class, last_line(text)?));
        }
    }
    Some(Fault::new(FaultClass::Other, last_line(text)?))
}

/// The last line that says anything.
fn last_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Reads an engine log and names the fault in it.
fn fault_from_log(path: &Path) -> Option<Fault> {
    classify_engine_log(&std::fs::read_to_string(path).ok()?)
}

/// Where the diagnostic sends its request through.
#[derive(Debug, Clone)]
pub enum Engine<'a> {
    /// A port something else already owns, such as a running profile's engine.
    ///
    /// It is probed and otherwise left alone: whoever started it stops it, and
    /// a diagnostic that killed a running profile's engine would be a worse
    /// bug than the one it is looking for.
    Running { socks_port: u16 },
    /// An engine this diagnostic starts, waits for, probes and stops.
    ///
    /// A second engine rather than the profile's own is the point of a
    /// pre-flight: a proxy has to be testable before anything is launched.
    Temporary {
        executable: &'a Path,
        /// Where the temporary config and the engine's log are written. The
        /// diagnostic creates it and removes the two files it wrote.
        directory: &'a Path,
        ready_timeout: Duration,
    },
}

/// What to ask, through what, and how long to wait.
#[derive(Debug, Clone)]
pub struct DiagnosticRequest<'a> {
    pub engine: Engine<'a>,
    pub echo_url: &'a str,
    pub probe_timeout: Duration,
}

/// Sends one request through `proxy` and reports what left.
///
/// A temporary engine's life is bounded by this call on every path, including a
/// failure and a panic: it never outlives its diagnostic, and neither does the
/// config that carries the upstream credentials.
pub fn diagnose(
    client: &dyn EchoClient,
    builder: &dyn XrayConfigBuilder,
    proxy: &ProxyProfile,
    request: &DiagnosticRequest<'_>,
) -> Result<Diagnosis, Fault> {
    match &request.engine {
        Engine::Running { socks_port } => probe(client, *socks_port, request, None),
        Engine::Temporary {
            executable,
            directory,
            ready_timeout,
        } => {
            let engine =
                TemporaryEngine::start(builder, proxy, executable, directory, *ready_timeout)?;
            // The engine drops at the end of this arm, on both outcomes.
            probe(client, engine.port, request, Some(&engine.log_path))
        }
    }
}

/// One request, and the reading or the fault that came back from it.
fn probe(
    client: &dyn EchoClient,
    socks_port: u16,
    request: &DiagnosticRequest<'_>,
    engine_log: Option<&Path>,
) -> Result<Diagnosis, Fault> {
    let started = Instant::now();
    let outcome = client.fetch(socks_port, request.echo_url, request.probe_timeout);
    let elapsed = started.elapsed();

    match outcome {
        Ok(body) => match extract_exit_ip(&body) {
            Some(exit_ip) => Ok(Diagnosis { exit_ip, elapsed }),
            None => Err(Fault::new(
                FaultClass::Reading,
                format!("the endpoint answered without an address: {body}"),
            )),
        },
        // The engine's log is consulted after the transport failure, never
        // instead of it: it is the more specific account of the same failure.
        // With no log to read, what the socket reported is the answer.
        Err(fault) => Err(engine_log.and_then(fault_from_log).unwrap_or(fault)),
    }
}

/// Bounds a detail before it reaches a window or the activity log.
fn shorten(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= DETAIL_LIMIT {
        return trimmed.to_owned();
    }
    let mut cut: String = trimmed.chars().take(DETAIL_LIMIT).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests;
