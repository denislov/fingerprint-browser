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

/// An `http://` address, split into the parts the two protocols need.
///
/// HTTPS is refused rather than downgraded; see [`check_echo_url`] for why.
#[derive(Debug)]
struct HttpTarget {
    host: String,
    port: u16,
    path: String,
    /// Whether the engine has to resolve the name, which is the case whenever
    /// the host is not already a literal address.
    named: bool,
}

/// The prefix an address endpoint has to start with.
pub const ECHO_PREFIX: &str = "http://";

/// Whether `url` is an address endpoint that can be asked, and why not when it
/// cannot.
///
/// Plain HTTP is required rather than preferred, and an `https://` endpoint is
/// refused rather than quietly downgraded. Every caller asks the same question
/// of it - whether traffic left by a particular path - so putting TLS in the
/// same request would let a certificate problem be reported as a path problem,
/// which is the most expensive kind of wrong answer to give someone debugging a
/// proxy. The rule lives here because both the pre-flight, which asks over a
/// socket this process owns, and the runtime read-back, which asks the browser,
/// have to give the same answer to the same setting.
pub fn check_echo_url(url: &str) -> Result<(), Fault> {
    if url.starts_with("https://") {
        return Err(Fault::new(
            FaultClass::Config,
            "the address endpoint must be plain http: a TLS request would report a certificate problem as a proxy problem",
        ));
    }
    if !url.starts_with(ECHO_PREFIX) {
        return Err(Fault::new(
            FaultClass::Config,
            format!("the address endpoint is not an http address: {url}"),
        ));
    }
    Ok(())
}

impl HttpTarget {
    fn parse(url: &str) -> Result<Self, Fault> {
        check_echo_url(url)?;
        // Safe to slice: the prefix is ASCII and was just checked.
        let rest = &url[ECHO_PREFIX.len()..];

        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "/"),
        };
        let (host, port) = split_authority(authority)?;
        if host.is_empty() {
            return Err(Fault::new(
                FaultClass::Config,
                format!("the address endpoint names no host: {url}"),
            ));
        }

        Ok(Self {
            named: host.parse::<IpAddr>().is_err(),
            host: host.to_string(),
            port,
            path: path.to_string(),
        })
    }
}

/// The host and port of an authority, with a bracketed IPv6 literal allowed.
fn split_authority(authority: &str) -> Result<(&str, u16), Fault> {
    let unusable = || {
        Fault::new(
            FaultClass::Config,
            format!("the address endpoint has no usable port: {authority}"),
        )
    };

    if let Some(rest) = authority.strip_prefix('[') {
        let (address, tail) = rest.split_once(']').ok_or_else(|| {
            Fault::new(
                FaultClass::Config,
                format!("the address endpoint has an unclosed IPv6 literal: {authority}"),
            )
        })?;
        return match tail.strip_prefix(':') {
            Some(port) => Ok((address, port.parse::<u16>().map_err(|_| unusable())?)),
            None => Ok((address, 80)),
        };
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => Ok((host, port.parse::<u16>().map_err(|_| unusable())?)),
        None => Ok((authority, 80)),
    }
}

/// A socket whose every read and write is bounded by one deadline.
///
/// A per-call timeout alone is not enough: several reads make up one answer, so
/// the deadline is kept here and each call is given only what is left of it.
struct Bounded {
    stream: TcpStream,
    deadline: Instant,
}

impl Bounded {
    fn connect(socks_port: u16, timeout: Duration) -> Result<Self, Fault> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, socks_port));
        let stream = TcpStream::connect_timeout(&address, timeout).map_err(|error| {
            Fault::new(
                FaultClass::Engine,
                format!("the loopback SOCKS endpoint at {address} accepted no connection: {error}"),
            )
        })?;
        Ok(Self {
            stream,
            deadline: Instant::now() + timeout,
        })
    }

    fn remaining(&self) -> Result<Duration, Fault> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Fault::new(
                FaultClass::Timeout,
                "the request did not finish in the time allowed",
            ));
        }
        Ok(left)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Fault> {
        let left = self.remaining()?;
        self.stream
            .set_write_timeout(Some(left))
            .map_err(io_fault)?;
        self.stream.write_all(bytes).map_err(io_fault)
    }

    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), Fault> {
        let left = self.remaining()?;
        self.stream.set_read_timeout(Some(left)).map_err(io_fault)?;
        // A short read is not a complete answer: the reply must arrive whole.
        let mut filled = 0;
        while filled < buffer.len() {
            match self.stream.read(&mut buffer[filled..]) {
                Ok(0) => {
                    return Err(Fault::new(
                        FaultClass::Unreachable,
                        "the engine closed the connection before answering",
                    ));
                }
                Ok(read) => filled += read,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_fault(error)),
            }
        }
        Ok(())
    }

    /// Everything up to the close, which is what `Connection: close` asks for.
    fn read_to_close(&mut self, limit: u64) -> Result<String, Fault> {
        let mut collected = Vec::new();
        let mut buffer = [0u8; 1024];
        while (collected.len() as u64) < limit {
            let left = self.remaining()?;
            self.stream.set_read_timeout(Some(left)).map_err(io_fault)?;
            match self.stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => collected.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_fault(error)),
            }
        }
        Ok(String::from_utf8_lossy(&collected).into_owned())
    }

    /// Everything up to the blank line that ends the headers, terminator
    /// included.
    ///
    /// Read a byte at a time so the terminator is never stepped over: the
    /// alternative is to read past it and then have to carry the leftover bytes
    /// into the body, which is where the off-by-one would live. Headers from an
    /// address endpoint are a few hundred bytes, so the cost is a few hundred
    /// syscalls on a request that is already waiting on a network.
    fn read_headers(&mut self, limit: u64) -> Result<String, Fault> {
        let mut collected = Vec::new();
        let mut byte = [0u8; 1];
        while (collected.len() as u64) < limit {
            let left = self.remaining()?;
            self.stream.set_read_timeout(Some(left)).map_err(io_fault)?;
            match self.stream.read(&mut byte) {
                Ok(0) => {
                    return Err(Fault::new(
                        FaultClass::Reading,
                        "the endpoint closed before it finished answering",
                    ));
                }
                Ok(_) => {
                    collected.push(byte[0]);
                    if collected.ends_with(b"\r\n\r\n") {
                        return Ok(String::from_utf8_lossy(&collected).into_owned());
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_fault(error)),
            }
        }
        Err(Fault::new(
            FaultClass::Reading,
            format!("the endpoint's headers did not end within {limit} bytes"),
        ))
    }
}

/// The class an IO failure belongs to.
///
/// A socket read that runs past its deadline reports `WouldBlock` on Linux,
/// which is a timeout and not an unreachable network - the peer is still there,
/// it simply never answered.
fn io_fault(error: std::io::Error) -> Fault {
    let class = match error.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => FaultClass::Timeout,
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::BrokenPipe
        | ErrorKind::NotConnected => FaultClass::Unreachable,
        _ => FaultClass::Other,
    };
    Fault::new(class, error.to_string())
}

/// The SOCKS5 greeting and the CONNECT request, with every step bounded.
fn handshake(socket: &mut Bounded, target: &HttpTarget) -> Result<(), Fault> {
    // One method offered, no authentication - what the engine's own inbound
    // offers, and all this diagnostic is entitled to use.
    socket.write_all(&[0x05, 0x01, 0x00])?;

    let mut choice = [0u8; 2];
    socket.read_exact(&mut choice)?;
    if choice[0] != 0x05 {
        return Err(Fault::new(
            FaultClass::Config,
            format!(
                "the endpoint is not a SOCKS5 proxy: it answered with version {}",
                choice[0]
            ),
        ));
    }
    match choice[1] {
        0x00 => {}
        // The proxy wants a credential this diagnostic does not hold, which is
        // a different problem from a proxy that is not there.
        0xff => {
            return Err(Fault::new(
                FaultClass::Auth,
                "the SOCKS endpoint accepts no method this diagnostic may use; it requires authentication",
            ));
        }
        method => {
            return Err(Fault::new(
                FaultClass::Auth,
                format!(
                    "the SOCKS endpoint selected method {method:#04x}, which this diagnostic holds no credential for"
                ),
            ));
        }
    }

    let mut connect = vec![0x05, 0x01, 0x00];
    if target.named {
        // The name is sent as a name for the engine to resolve, which is what
        // Chromium's `--proxy-server=socks5://` does. Resolving it here would
        // test this machine's resolvers and call the result the proxy's.
        let name = target.host.as_bytes();
        let length = u8::try_from(name.len()).map_err(|_| {
            Fault::new(
                FaultClass::Config,
                "the address endpoint's name is too long for SOCKS5",
            )
        })?;
        connect.extend_from_slice(&[0x03, length]);
        connect.extend_from_slice(name);
    } else {
        match target.host.parse::<IpAddr>().map_err(|_| {
            Fault::new(
                FaultClass::Config,
                format!(
                    "the address endpoint is neither a name nor an address: {}",
                    target.host
                ),
            )
        })? {
            IpAddr::V4(address) => {
                connect.push(0x01);
                connect.extend_from_slice(&address.octets());
            }
            IpAddr::V6(address) => {
                connect.push(0x04);
                connect.extend_from_slice(&address.octets());
            }
        }
    }
    connect.extend_from_slice(&target.port.to_be_bytes());
    socket.write_all(&connect)?;

    // The reply: version, code, reserved, then a bound address.
    let mut head = [0u8; 4];
    socket.read_exact(&mut head)?;
    if head[0] != 0x05 {
        return Err(Fault::new(
            FaultClass::Config,
            format!("the endpoint answered the CONNECT with version {}", head[0]),
        ));
    }
    if head[1] != 0x00 {
        return Err(Fault::new(
            class_for_reply(head[1]),
            format!(
                "the engine did not carry the request: {}",
                reply_label(head[1])
            ),
        ));
    }

    // The bound address is consumed so the stream is left at the payload.
    match head[3] {
        0x01 => socket.read_exact(&mut [0u8; 6])?,
        0x03 => {
            let mut length = [0u8; 1];
            socket.read_exact(&mut length)?;
            socket.read_exact(&mut vec![0u8; length[0] as usize + 2])?;
        }
        0x04 => socket.read_exact(&mut [0u8; 18])?,
        kind => {
            return Err(Fault::new(
                FaultClass::Config,
                format!("the engine replied with address kind {kind:#04x}"),
            ));
        }
    }
    Ok(())
}

/// One request, and the answer framed the way the response declared it.
fn request(socket: &mut Bounded, target: &HttpTarget) -> Result<String, Fault> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: */*\r\nUser-Agent: {USER_AGENT}\r\nConnection: close\r\n\r\n",
        target.path, target.host
    );
    socket.write_all(request.as_bytes())?;

    let headers = socket.read_headers(HEADER_LIMIT)?;
    let status = status_of(&headers)?;
    match status {
        200..=299 => {}
        // Only a refusal to authenticate is a credential problem rather than a
        // bad endpoint.
        401 | 407 => {
            return Err(Fault::new(
                FaultClass::Auth,
                format!("the endpoint refused the request with {status}"),
            ));
        }
        status => {
            return Err(Fault::new(
                FaultClass::Http(status),
                format!("the endpoint answered {status}"),
            ));
        }
    }
    read_body(socket, &headers)
}

/// The body, read the way the response said it was framed.
///
/// A declared length is the stronger frame and is preferred: asking for
/// `Connection: close` and then waiting for the close makes the reading depend
/// on the endpoint honouring a request it is free to ignore, and an endpoint
/// that keeps the connection open would turn a perfectly good address into a
/// timeout.
fn read_body(socket: &mut Bounded, headers: &str) -> Result<String, Fault> {
    match content_length(headers) {
        Some(length) => {
            if length > READING_LIMIT {
                return Err(Fault::new(
                    FaultClass::Reading,
                    format!(
                        "the endpoint declared a {length}-byte answer, which is not an address"
                    ),
                ));
            }
            let mut body = vec![0u8; length as usize];
            socket.read_exact(&mut body)?;
            Ok(String::from_utf8_lossy(&body).into_owned())
        }
        // No length declared: the close is then the only end there is, and the
        // read still carries the deadline.
        None => socket.read_to_close(READING_LIMIT),
    }
}

/// The status of a response, from its first line.
fn status_of(headers: &str) -> Result<u16, Fault> {
    headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            Fault::new(
                FaultClass::Reading,
                "the endpoint answered without a status",
            )
        })
}

/// The length a response declared for its body, when it declared one.
fn content_length(headers: &str) -> Option<u64> {
    headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
}

/// The class a SOCKS5 reply code belongs to.
///
/// These are the protocol's own codes, so this is a reading of the standard
/// rather than a guess at a library's wording.
fn class_for_reply(code: u8) -> FaultClass {
    match code {
        // "network unreachable", "host unreachable", "connection refused", and
        // a refusal by ruleset all mean the same thing to a user: nothing
        // carried it. Which of the four it was is in the detail.
        0x02..=0x05 => FaultClass::Unreachable,
        0x06 => FaultClass::Timeout,
        // "command not supported" and "address type not supported" are the
        // engine being asked for something it cannot do.
        0x07 | 0x08 => FaultClass::Config,
        _ => FaultClass::Other,
    }
}

/// The protocol's own name for a reply code.
fn reply_label(code: u8) -> String {
    let name = match code {
        0x01 => "general SOCKS server failure",
        0x02 => "connection not allowed by ruleset",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown reply code",
    };
    format!("{name} ({code:#04x})")
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

/// A temporary engine, alive for exactly one diagnostic.
struct TemporaryEngine {
    child: crate::process::ManagedChild,
    port: u16,
    config_path: PathBuf,
    log_path: PathBuf,
}

impl TemporaryEngine {
    fn start(
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
fn remove_if_present(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => tracing::debug!("left {} behind: {error}", path.display()),
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
mod tests {
    use super::*;
    use domain::{ProxyId, ProxyOutbound, Socks5Outbound};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn proxy() -> ProxyProfile {
        ProxyProfile {
            id: ProxyId::new(),
            name: "test".to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "localhost".to_string(),
                port: 1080,
                username: Some("user".to_string()),
                password: Some("secret".to_string()),
            }),
        }
    }

    // ---- the reading ----------------------------------------------------

    #[test]
    fn an_endpoint_answer_is_read_whatever_shape_it_arrives_in() {
        // The bare address, a JSON field, and a line of prose: all three are
        // answers to the same question.
        assert_eq!(
            extract_exit_ip("203.0.113.7"),
            Some("203.0.113.7".to_string())
        );
        assert_eq!(
            extract_exit_ip("{\"ip\":\"203.0.113.7\"}"),
            Some("203.0.113.7".to_string())
        );
        assert_eq!(
            extract_exit_ip("your address is 203.0.113.7\n"),
            Some("203.0.113.7".to_string())
        );
        assert_eq!(
            extract_exit_ip("2001:db8::1"),
            Some("2001:db8::1".to_string())
        );
    }

    #[test]
    fn an_answer_without_an_address_is_not_a_reading() {
        // Not the same as a proxy that carried nothing, and must not be
        // reported as one.
        assert_eq!(extract_exit_ip(""), None);
        assert_eq!(extract_exit_ip("no address here"), None);
        // A version string is not an address either.
        assert_eq!(extract_exit_ip("142.0.7444.175"), None);
    }

    // ---- the request target ---------------------------------------------

    #[test]
    fn an_endpoint_address_is_split_into_what_the_two_protocols_need() {
        let named = HttpTarget::parse("http://api.example/ip").expect("a named endpoint");
        assert_eq!(named.host, "api.example");
        assert_eq!(named.port, 80);
        assert_eq!(named.path, "/ip");
        assert!(named.named, "a name is left for the engine to resolve");

        let literal = HttpTarget::parse("http://127.0.0.1:8080").expect("a literal endpoint");
        assert_eq!(literal.host, "127.0.0.1");
        assert_eq!(literal.port, 8080);
        assert_eq!(literal.path, "/");
        assert!(!literal.named, "a literal needs no resolution");

        let bracketed = HttpTarget::parse("http://[::1]:8443/ip").expect("an IPv6 endpoint");
        assert_eq!(bracketed.host, "::1");
        assert_eq!(bracketed.port, 8443);
    }

    #[test]
    fn a_tls_endpoint_is_refused_rather_than_downgraded() {
        // Reporting a certificate problem as a proxy problem is the most
        // expensive wrong answer this could give.
        let fault = HttpTarget::parse("https://api.example/ip")
            .expect_err("https is not the question this asks");
        assert_eq!(fault.class, FaultClass::Config);
        assert!(fault.detail.contains("plain http"), "{fault}");

        assert_eq!(
            HttpTarget::parse("ftp://api.example/ip")
                .expect_err("not http")
                .class,
            FaultClass::Config
        );
        assert_eq!(
            HttpTarget::parse("http://:8080/ip")
                .expect_err("no host")
                .class,
            FaultClass::Config
        );
        assert_eq!(
            HttpTarget::parse("http://api.example:not-a-port/ip")
                .expect_err("no usable port")
                .class,
            FaultClass::Config
        );
    }

    // ---- the classification ---------------------------------------------

    #[test]
    fn a_refused_forward_is_not_reported_as_a_refused_credential() {
        // The classes exist to be told apart: each one is fixed somewhere else.
        assert_eq!(class_for_reply(0x05), FaultClass::Unreachable);
        assert_eq!(class_for_reply(0x02), FaultClass::Unreachable);
        assert_eq!(class_for_reply(0x04), FaultClass::Unreachable);
        assert_eq!(class_for_reply(0x06), FaultClass::Timeout);
        assert_eq!(class_for_reply(0x07), FaultClass::Config);
        assert_eq!(class_for_reply(0x08), FaultClass::Config);
        assert_eq!(class_for_reply(0x01), FaultClass::Other);
        // The detail still says which of the four it was.
        assert!(reply_label(0x05).contains("connection refused"));
        assert!(reply_label(0x02).contains("ruleset"));
    }

    #[test]
    fn a_socket_that_ran_out_of_time_is_a_timeout_and_not_a_dead_network() {
        // The peer is still there, it simply never answered.
        assert_eq!(
            io_fault(std::io::Error::from(ErrorKind::WouldBlock)).class,
            FaultClass::Timeout
        );
        assert_eq!(
            io_fault(std::io::Error::from(ErrorKind::TimedOut)).class,
            FaultClass::Timeout
        );
        assert_eq!(
            io_fault(std::io::Error::from(ErrorKind::ConnectionReset)).class,
            FaultClass::Unreachable
        );
    }

    #[test]
    fn a_status_is_read_as_the_answer_or_the_refusal_it_is() {
        assert_eq!(
            status_of("HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n").expect("a status"),
            200
        );
        assert_eq!(
            status_of("HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").expect("a status"),
            407
        );
        assert_eq!(
            status_of("no status here")
                .expect_err("not a response")
                .class,
            FaultClass::Reading
        );
    }

    #[test]
    fn a_declared_length_is_the_frame_the_body_is_read_by() {
        // A declared length is preferred over waiting for a close, because an
        // endpoint that keeps the connection open would otherwise turn a
        // perfectly good address into a timeout.
        assert_eq!(
            content_length("HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n"),
            Some(12)
        );
        assert_eq!(
            content_length("HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\n"),
            Some(3),
            "the header name is not case sensitive"
        );
        assert_eq!(
            content_length("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n"),
            None,
            "no length means the close is the end"
        );
    }

    #[test]
    fn the_engine_log_names_the_cause_a_transport_failure_cannot() {
        let rejected = "2026/09/20 21:00:00 [Warning] failed to handler mux client: invalid user";
        assert_eq!(
            classify_engine_log(rejected).map(|fault| fault.class),
            Some(FaultClass::Auth)
        );

        let unresolved = "2026/09/20 21:00:00 [Warning] failed to dial: no such host";
        assert_eq!(
            classify_engine_log(unresolved).map(|fault| fault.class),
            Some(FaultClass::Dns)
        );

        let bad_config = "2026/09/20 21:00:00 failed to parse config: unknown transport";
        assert_eq!(
            classify_engine_log(bad_config).map(|fault| fault.class),
            Some(FaultClass::Config)
        );

        let dead = "2026/09/20 21:00:00 [Warning] dial tcp 1.2.3.4:443: i/o timeout";
        assert_eq!(
            classify_engine_log(dead).map(|fault| fault.class),
            Some(FaultClass::Timeout)
        );
    }

    #[test]
    fn an_unrecognised_engine_line_is_carried_rather_than_guessed_at() {
        let unknown = "2026/09/20 21:00:00 something nobody has seen before";
        let fault = classify_engine_log(unknown).expect("a log with a line makes a fault");
        assert_eq!(fault.class, FaultClass::Other);
        assert_eq!(fault.detail, unknown);

        // Silence is not a cause.
        assert_eq!(classify_engine_log("  \n\n"), None);
    }

    #[test]
    fn a_detail_is_bounded_before_it_reaches_a_window() {
        let fault = Fault::new(FaultClass::Other, "x".repeat(10_000));
        assert!(fault.detail.chars().count() <= DETAIL_LIMIT + 1);
    }

    // ---- the probe, against a local engine -------------------------------

    /// A loopback HTTP endpoint that answers with a fixed body.
    struct EchoStub {
        port: u16,
        _listener: TcpListener,
    }

    impl EchoStub {
        fn start(body: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind the echo stub");
            let port = listener.local_addr().expect("echo stub address").port();
            let accepting = listener.try_clone().expect("clone the echo listener");
            thread::spawn(move || {
                for stream in accepting.incoming() {
                    let Ok(mut client) = stream else { continue };
                    thread::spawn(move || {
                        let mut request = [0u8; 1024];
                        let _ = client.read(&mut request);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = client.write_all(response.as_bytes());
                        let _ = client.flush();
                    });
                }
            });
            Self {
                port,
                _listener: listener,
            }
        }

        fn url(&self) -> String {
            // Addressed by literal, never by name. A name would be resolved by
            // whatever this machine's hosts file says, and the endpoint under
            // test is a socket on loopback - not a name.
            format!("http://127.0.0.1:{}/", self.port)
        }
    }

    /// What a stub SOCKS5 endpoint does with the request it is given.
    #[derive(Clone, Copy)]
    enum Answer {
        /// Carries it to the target.
        Carries,
        /// Answers the greeting, then says nothing at all and holds the socket
        /// open. This is the case no timeout in a general-purpose client could
        /// bound, and the reason this module owns its socket.
        SaysNothing,
        /// Refuses the CONNECT with this SOCKS5 reply code.
        Refuses(u8),
        /// Chooses this greeting method.
        Selects(u8),
    }

    /// A loopback SOCKS5 endpoint, so a test can tell "the port is open" apart
    /// from "the request was carried" - which is the whole point of the
    /// diagnostic.
    struct SocksStub {
        port: u16,
        _listener: TcpListener,
    }

    impl SocksStub {
        fn start(answer: Answer) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind the SOCKS stub");
            let port = listener.local_addr().expect("SOCKS stub address").port();
            let accepting = listener.try_clone().expect("clone the SOCKS listener");
            thread::spawn(move || {
                for stream in accepting.incoming() {
                    let Ok(client) = stream else { continue };
                    thread::spawn(move || serve(client, answer));
                }
            });
            Self {
                port,
                _listener: listener,
            }
        }
    }

    /// The SOCKS5 conversation, up to the point that matters: whether a
    /// connection to the target is made.
    fn serve(mut client: TcpStream, answer: Answer) {
        let mut greeting = [0u8; 2];
        if client.read_exact(&mut greeting).is_err() {
            return;
        }
        let mut offered = vec![0u8; greeting[1] as usize];
        if client.read_exact(&mut offered).is_err() {
            return;
        }

        if let Answer::Selects(method) = answer {
            let _ = client.write_all(&[0x05, method]);
            return;
        }
        // No authentication, which is what the engine's own inbound offers.
        if client.write_all(&[0x05, 0x00]).is_err() {
            return;
        }

        let mut head = [0u8; 4];
        if client.read_exact(&mut head).is_err() {
            return;
        }
        let Some(host) = read_target(&mut client, head[3]) else {
            return;
        };
        let mut port = [0u8; 2];
        if client.read_exact(&mut port).is_err() {
            return;
        }
        let port = u16::from_be_bytes(port);

        if let Answer::SaysNothing = answer {
            // Holds the socket without answering, for longer than any test's
            // deadline, so a probe that is not bounded here would hang.
            thread::sleep(Duration::from_secs(30));
            return;
        }

        let refused = |code: u8| [0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        if let Answer::Refuses(code) = answer {
            let _ = client.write_all(&refused(code));
            return;
        }

        let Some(mut upstream) = dial(&host, port) else {
            let _ = client.write_all(&refused(0x05));
            return;
        };
        if client.write_all(&refused(0x00)).is_err() {
            return;
        }

        let mut reading = client.try_clone().expect("clone the client");
        let mut writing = upstream.try_clone().expect("clone the upstream");
        let pump = thread::spawn(move || {
            let _ = std::io::copy(&mut reading, &mut writing);
        });
        let _ = std::io::copy(&mut upstream, &mut client);
        let _ = pump.join();
    }

    /// The SOCKS address field, as the name or literal it carries.
    fn read_target(stream: &mut TcpStream, kind: u8) -> Option<String> {
        match kind {
            0x01 => {
                let mut address = [0u8; 4];
                stream.read_exact(&mut address).ok()?;
                Some(std::net::Ipv4Addr::from(address).to_string())
            }
            0x03 => {
                let mut length = [0u8; 1];
                stream.read_exact(&mut length).ok()?;
                let mut name = vec![0u8; length[0] as usize];
                stream.read_exact(&mut name).ok()?;
                String::from_utf8(name).ok()
            }
            0x04 => {
                let mut address = [0u8; 16];
                stream.read_exact(&mut address).ok()?;
                Some(std::net::Ipv6Addr::from(address).to_string())
            }
            _ => None,
        }
    }

    /// Connects to the first address that accepts. A stub that resolves
    /// "localhost" must not fail because this machine answers with `::1` first.
    fn dial(host: &str, port: u16) -> Option<TcpStream> {
        use std::net::ToSocketAddrs;
        let addresses = (host, port).to_socket_addrs().ok()?;
        addresses
            .into_iter()
            .find_map(|address| TcpStream::connect(address).ok())
    }

    fn request<'a>(engine: Engine<'a>, url: &'a str) -> DiagnosticRequest<'a> {
        DiagnosticRequest {
            engine,
            echo_url: url,
            probe_timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn a_socks_target_address_is_read_in_every_shape() {
        // The three shapes a request may carry, including the name that the
        // browser's SOCKS5 mode sends. Resolving the name is the engine's job
        // and not part of the reading, so this is independent of any hosts file.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a socket pair");
        let address = listener.local_addr().expect("socket pair address");
        let mut writer = TcpStream::connect(address).expect("connect the pair");
        let (mut reader, _) = listener.accept().expect("accept the pair");

        writer.write_all(&[127, 0, 0, 1]).expect("write ipv4");
        assert_eq!(
            read_target(&mut reader, 0x01),
            Some("127.0.0.1".to_string())
        );

        writer.write_all(&[9]).expect("write a name length");
        writer.write_all(b"localhost").expect("write a name");
        assert_eq!(
            read_target(&mut reader, 0x03),
            Some("localhost".to_string())
        );

        writer
            .write_all(&std::net::Ipv6Addr::LOCALHOST.octets())
            .expect("write ipv6");
        assert_eq!(read_target(&mut reader, 0x04), Some("::1".to_string()));

        // Anything else is not a target, and is refused rather than guessed at.
        assert_eq!(read_target(&mut reader, 0x02), None);
    }

    #[test]
    fn a_request_that_was_carried_reports_the_address_that_left() {
        let echo = EchoStub::start("203.0.113.7\n");
        let socks = SocksStub::start(Answer::Carries);
        let url = echo.url();

        let reading = diagnose(
            &SocksEchoClient::new(),
            &crate::xray::DefaultXrayConfigBuilder::new(),
            &proxy(),
            &request(
                Engine::Running {
                    socks_port: socks.port,
                },
                &url,
            ),
        )
        .expect("a carried request is a reading");

        assert_eq!(reading.exit_ip, "203.0.113.7");
    }

    #[test]
    fn an_open_port_that_carries_nothing_is_a_fault_and_not_a_pass() {
        // This is the case `wait_ready` cannot see: the engine is listening,
        // the handshake succeeds, and no byte ever leaves.
        let echo = EchoStub::start("203.0.113.7\n");
        let socks = SocksStub::start(Answer::Refuses(0x05));
        let url = echo.url();

        let fault = diagnose(
            &SocksEchoClient::new(),
            &crate::xray::DefaultXrayConfigBuilder::new(),
            &proxy(),
            &request(
                Engine::Running {
                    socks_port: socks.port,
                },
                &url,
            ),
        )
        .expect_err("a port that carries nothing is not a pass");

        assert_eq!(fault.class, FaultClass::Unreachable, "got {fault}");
        assert!(fault.detail.contains("connection refused"), "{fault}");
    }

    #[test]
    fn an_endpoint_that_never_answers_is_a_timeout_and_not_a_hang() {
        // The failure this module exists to have bounded: the proxy accepts the
        // connection and then says nothing. A general-purpose client joined its
        // own handshake thread and waited the peer out instead of timing out.
        let echo = EchoStub::start("203.0.113.7\n");
        let socks = SocksStub::start(Answer::SaysNothing);
        let url = echo.url();

        let started = Instant::now();
        let fault = diagnose(
            &SocksEchoClient::new(),
            &crate::xray::DefaultXrayConfigBuilder::new(),
            &proxy(),
            &DiagnosticRequest {
                engine: Engine::Running {
                    socks_port: socks.port,
                },
                echo_url: &url,
                probe_timeout: Duration::from_millis(600),
            },
        )
        .expect_err("silence is not a reading");

        assert_eq!(fault.class, FaultClass::Timeout, "got {fault}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the probe must give up on its own deadline, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_proxy_that_demands_a_credential_is_not_reported_as_a_dead_network() {
        let echo = EchoStub::start("203.0.113.7\n");
        let socks = SocksStub::start(Answer::Selects(0xff));
        let url = echo.url();

        let fault = diagnose(
            &SocksEchoClient::new(),
            &crate::xray::DefaultXrayConfigBuilder::new(),
            &proxy(),
            &request(
                Engine::Running {
                    socks_port: socks.port,
                },
                &url,
            ),
        )
        .expect_err("a proxy that wants a credential carries nothing for us");

        assert_eq!(fault.class, FaultClass::Auth, "got {fault}");
    }

    #[test]
    fn an_endpoint_that_is_not_listening_is_an_engine_fault() {
        // Nothing is bound: the port may even have been recycled.
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a free port");
        let free = listener.local_addr().expect("free address").port();
        drop(listener);

        let fault = SocksEchoClient::new()
            .fetch(free, "http://127.0.0.1:1/", Duration::from_millis(300))
            .expect_err("a port with nothing behind it is not a reading");
        assert_eq!(fault.class, FaultClass::Engine, "got {fault}");
    }

    #[test]
    fn a_running_engine_is_probed_and_left_alone() {
        let echo = EchoStub::start("198.51.100.4\n");
        let socks = SocksStub::start(Answer::Carries);
        let url = echo.url();
        let builder = crate::xray::DefaultXrayConfigBuilder::new();
        let client = SocksEchoClient::new();

        for _ in 0..2 {
            let reading = diagnose(
                &client,
                &builder,
                &proxy(),
                &request(
                    Engine::Running {
                        socks_port: socks.port,
                    },
                    &url,
                ),
            )
            .expect("the engine was not stopped by the first diagnostic");
            assert_eq!(reading.exit_ip, "198.51.100.4");
        }
    }

    /// A temporary engine, as an actual process.
    ///
    /// The stub scripts follow the convention the supervisor's tests already
    /// use: a python script that reads the config it was handed. What is under
    /// test here is not the probe - the local stubs above cover that - but what
    /// the diagnostic leaves behind, which is where a bug would be expensive.
    #[cfg(unix)]
    mod engine_lifetime {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::path::{Path, PathBuf};

        /// Binds the inbound the config names, then does nothing with it. It
        /// records its own pid so the test can ask whether it outlived the
        /// diagnostic.
        const LISTENING: &str = "#!/usr/bin/env python3\nimport json,os,socket,sys,time\nc=json.load(open(sys.argv[3]))\nopen(os.path.join(os.path.dirname(sys.argv[3]),'engine.pid'),'w').write(str(os.getpid()))\ns=socket.socket()\ns.bind(('127.0.0.1',c['inbounds'][0]['port']))\ns.listen()\ntime.sleep(60)\n";

        /// Never binds anything, so readiness has to give up on its own.
        const SILENT: &str = "#!/usr/bin/env python3\nimport time\ntime.sleep(60)\n";

        /// A directory that takes its stub engine with it.
        struct Dir {
            path: PathBuf,
            _serial: std::sync::MutexGuard<'static, ()>,
        }

        impl Dir {
            fn new(tag: &str) -> Self {
                // Executable writes and process launches in sibling tests race
                // each other otherwise.
                static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
                let serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
                let path =
                    std::env::temp_dir().join(format!("fp-diagnostic-{tag}-{}", ProxyId::new()));
                std::fs::create_dir_all(&path).expect("create the diagnostic directory");
                Self {
                    path,
                    _serial: serial,
                }
            }

            fn engine(&self, script: &str) -> PathBuf {
                let executable = self.path.join("xray");
                std::fs::write(&executable, script).expect("write the stub engine");
                std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
                    .expect("make the stub engine executable");
                executable
            }

            /// Where the diagnostic writes its own two files.
            fn runtime(&self) -> PathBuf {
                self.path.join("runtime")
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        fn run(dir: &Dir, script: &str) -> Result<Diagnosis, Fault> {
            let echo = EchoStub::start("203.0.113.7\n");
            let url = echo.url();
            let runtime = dir.runtime();
            let executable = dir.engine(script);
            diagnose(
                &SocksEchoClient::new(),
                &crate::xray::DefaultXrayConfigBuilder::new(),
                &proxy(),
                &DiagnosticRequest {
                    engine: Engine::Temporary {
                        executable: &executable,
                        directory: &runtime,
                        ready_timeout: Duration::from_millis(2500),
                    },
                    echo_url: &url,
                    probe_timeout: Duration::from_millis(600),
                },
            )
        }

        /// The config holds the upstream credentials, so surviving the
        /// diagnostic would be worse than the failure it reported.
        fn assert_nothing_left(dir: &Dir) {
            let runtime = dir.runtime();
            assert!(
                !runtime.join(DIAGNOSTIC_CONFIG_FILE).exists(),
                "the temporary config must not survive the diagnostic"
            );
            assert!(
                !runtime.join(DIAGNOSTIC_LOG_FILE).exists(),
                "the engine log must not survive the diagnostic"
            );
        }

        #[test]
        fn an_engine_that_binds_but_carries_nothing_is_bounded_and_leaves_nothing() {
            let dir = Dir::new("carries-nothing");
            let started = Instant::now();
            let outcome = run(&dir, LISTENING);
            let took = started.elapsed();

            // Readiness passed, because the port was open; the request did not.
            let fault = outcome.expect_err("a bound port that carries nothing is not a reading");
            assert_eq!(fault.class, FaultClass::Timeout, "got {fault}");
            assert!(
                took < Duration::from_secs(10),
                "the diagnostic must not wait the engine out, took {took:?}"
            );
            assert_nothing_left(&dir);

            // Linux is where a pid can be asked about directly.
            #[cfg(target_os = "linux")]
            {
                let pid = std::fs::read_to_string(dir.runtime().join("engine.pid"))
                    .expect("the stub recorded its pid")
                    .trim()
                    .to_string();
                assert!(
                    !Path::new(&format!("/proc/{pid}")).exists(),
                    "the diagnostic engine must not outlive the diagnostic"
                );
            }
        }

        #[test]
        fn an_engine_that_never_binds_is_a_readiness_fault_that_leaves_nothing() {
            let dir = Dir::new("silent");
            let outcome = run(&dir, SILENT);

            let fault = outcome.expect_err("an engine that never binds is not a reading");
            assert_eq!(
                fault.class,
                FaultClass::Engine,
                "nothing bound, so nothing carried: got {fault}"
            );
            assert_nothing_left(&dir);
        }
    }
}
