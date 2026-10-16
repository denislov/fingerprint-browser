//! The page a profile opens to see what it is.
//!
//! A fingerprint browser exists so that a page sees one machine while the
//! traffic leaves from another. The whole program is built around not confusing
//! those two, and until now the only way to ask "which one am I, right now?" was
//! the **Verify** action: a one-shot reading taken from the window, about a
//! profile that has to be running, reported in a panel.
//!
//! This module is the standing version of that question. A profile whose launch
//! asks for it gets a loopback HTTP page as its first tab, served by this
//! program, which answers with the identity the session was started with and the
//! address one request leaves from.
//!
//! Three things about it are deliberate and are the reasons it is not simply a
//! second window:
//!
//! - **The page never talks to the network.** Every number it shows comes from
//!   this program over loopback. A page that fetched an echo endpoint itself
//!   would need that endpoint to send CORS headers - and the endpoint is the
//!   user's setting ([`StatusLaunch::echo_url`]), so whether it does is not
//!   something this program can promise. A reading that arrives only when a
//!   third party cooperates is a reading that disappears exactly when the reader
//!   is looking for it. The content security policy the page is served with
//!   (`default-src 'none'`, `connect-src 'self'`) is that decision, enforced by
//!   the browser rather than left as a comment.
//! - **It is read-only.** There is no endpoint that changes anything. The page
//!   cannot start, stop, edit or re-probe a profile, and it cannot be made to
//!   ask about an address of the caller's choosing.
//! - **It is loopback and nothing else.** The listener comes from a
//!   [`crate::ports::PortReservation`], which binds `127.0.0.1`, and the socket
//!   is taken rather than released - so the port the browser is sent to is the
//!   port this service is already answering on, with no window in between for
//!   something else to take it.
//!
//! ## What the page shows, and what each number proves
//!
//! The address on the page is read the way the **Test** action on the Proxies
//! page reads one: one request to the configured endpoint, through the port the
//! profile's engine is listening on. It is the *engine's* exit address, which is
//! the path a proxied browser's traffic takes - not a proof that the browser was
//! pointed at the engine. That second question is what the window's **Verify**
//! reads back out of the browser itself, and the two are kept apart on purpose
//! (see [traffic verification](../../../docs/subsystems/traffic-verification.md)).
//! The page says which of the two it is showing instead of leaving the reader to
//! guess.
//!
//! A profile with no proxy gets no address, and says so: it claims nothing about
//! where its traffic leaves from, so there is nothing to read back. That is the
//! same rule the rest of the program applies.

use crate::diagnostic::{EchoClient, FaultClass, SocksEchoClient, extract_exit_ip};
use crate::ports::PortReservation;
use serde::Serialize;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long one connection may take, from its first byte to its answer.
///
/// The deadline is absolute and is re-checked before every read and write, so a
/// peer that sends one byte per timeout cannot renew it - the rule the
/// diagnostic transport learned the hard way. The service answers one connection
/// at a time, so this is also the longest a second reader can be kept waiting.
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

/// How long the page's one outbound request may take.
///
/// The same order as the window's proxy test: long enough for a slow upstream,
/// short enough that a reader who presses the button twice does not wait twice.
pub const EGRESS_TIMEOUT: Duration = Duration::from_secs(10);

/// The most of a request head that will be read, so a client cannot make the
/// service read forever by never sending the blank line that ends it.
const MAX_REQUEST_HEAD: usize = 8 * 1024;

/// How often the accept loop looks at its shutdown flag.
const POLL: Duration = Duration::from_millis(50);

/// The languages the page is written in, as [`StatusLaunch::lang`] spells them.
pub const LANGS: [&str; 2] = ["en", "zh"];

/// What a launch asks the status page to be.
///
/// Carried by the launch rather than read from a component, because the language
/// and the endpoint are settings a reader can change between two starts, and a
/// page that answered in the language of the previous start would be wrong in a
/// way nobody would think to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLaunch {
    /// The endpoint one request is sent to. Plain HTTP on purpose, for the
    /// reason [`crate::diagnostic::check_echo_url`] gives.
    pub echo_url: String,
    /// The language the page is written in, as [`LANGS`] spells it.
    pub lang: &'static str,
}

/// What the session is, as the page reports it.
///
/// Every field is something the launch already knows. Nothing here is read from
/// the database at request time, so the page cannot disagree with the window
/// about a profile that was edited while its browser was running: it answers
/// what this session was started as, which is the question a fingerprint reader
/// is asking.
#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    /// The profile's name, as the reader chose it.
    pub profile: String,
    /// Its identifier, for telling two profiles with one name apart.
    pub profile_id: String,
    /// The core's version as it was read from the binary.
    pub core: String,
    /// Its major, when one could be read.
    pub core_major: Option<u32>,
    /// The proxy's **name**. Never its URI and never its credentials: this is
    /// the page a reader may well leave open on a second monitor.
    pub proxy: Option<String>,
    /// The port the browser's debugging endpoint is on, loopback only.
    pub cdp_port: u16,
    /// The port the profile's engine is listening on, when it has one.
    pub socks_port: Option<u16>,
    /// When this session started, in seconds since the Unix epoch. The page
    /// renders it in the reader's own locale rather than having this program
    /// guess at one.
    pub started_at: u64,
    /// What the compatibility layer reported about this launch, if anything:
    /// the switches this core will not honour, so the page cannot show a
    /// fingerprint claim the engine is silently ignoring.
    pub warning: Option<String>,
    /// Where the address is read from, so the number has a source.
    pub echo_url: String,
}

/// The answer to "where does one request leave from".
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatusEgress {
    /// An address came back, and how long the round trip took.
    Address { address: String, elapsed_ms: u64 },
    /// The request stopped somewhere, and the class says where.
    Fault { class: String, detail: String },
    /// Nothing was asked, because this profile claims nothing about an address.
    NotAsked { reason: String },
}

/// Everything the service needs to answer, fixed for the life of a session.
pub struct StatusSession {
    pub report: StatusReport,
    /// The engine's port, when the profile has a proxy. `None` means the page
    /// reports that nothing was asked.
    pub socks_port: Option<u16>,
    /// The language the page is written in, baked in when the page is rendered.
    pub lang: &'static str,
}

impl StatusSession {
    /// One request to the configured endpoint, through the profile's engine.
    ///
    /// Blocking, and bounded by [`EGRESS_TIMEOUT`]: it is called on the service's
    /// own thread, which answers one connection at a time, so a request that
    /// hangs delays the next reader rather than spawning another worker.
    fn egress(&self) -> StatusEgress {
        let Some(socks_port) = self.socks_port else {
            return StatusEgress::NotAsked {
                reason: "this profile has no proxy".to_string(),
            };
        };
        // The engine is already running and is not this service's to stop: this
        // probes its port and leaves it alone, exactly as the window's proxy
        // test does for a live profile.
        let started = Instant::now();
        match SocksEchoClient.fetch(socks_port, &self.report.echo_url, EGRESS_TIMEOUT) {
            Ok(body) => match extract_exit_ip(&body) {
                Some(address) => StatusEgress::Address {
                    address,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                },
                None => StatusEgress::Fault {
                    class: FaultClass::Reading.label(),
                    detail: "the endpoint answered without an address".to_string(),
                },
            },
            Err(fault) => StatusEgress::Fault {
                class: fault.class.label(),
                detail: fault.detail,
            },
        }
    }

    /// The page, in this session's language.
    fn page(&self) -> String {
        render_page(self.lang, &self.report.echo_url)
    }
}

/// The page, served by this program on a loopback port for one session.
///
/// Dropping it stops the thread and closes the socket, which is what makes the
/// port free again - the running session holds one of these, so a stop, a crash
/// and a reclaim all end the page the same way, with nothing to remember.
pub struct StatusService {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl StatusService {
    /// Takes a reserved port and starts answering on it.
    ///
    /// The reservation is consumed rather than dropped: this service binds the
    /// port itself, so unlike a browser or an engine there is no release-to-bind
    /// window for anything else to take.
    pub fn start(reservation: PortReservation, session: Arc<StatusSession>) -> io::Result<Self> {
        Self::start_with(reservation, session, REQUEST_DEADLINE)
    }

    fn start_with(
        reservation: PortReservation,
        session: Arc<StatusSession>,
        deadline: Duration,
    ) -> io::Result<Self> {
        let port = reservation.port();
        let listener = reservation.into_listener();
        // The accept loop has to look at its shutdown flag, so it cannot sit in
        // a blocking accept: it polls, the way the supervisor's own loop does.
        listener.set_nonblocking(true)?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("status-page".to_string())
                .spawn(move || accept_loop(listener, session, stop, deadline))?
        };

        Ok(Self {
            port,
            stop,
            thread: Some(thread),
        })
    }

    /// The port the page is on, which is the port the browser was sent to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The address a profile opens, as it goes on the browser's command line.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for StatusService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for StatusService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StatusService")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// Accepts one connection at a time until the service is dropped.
///
/// Serial on purpose. The work behind a request is one page render and, at most,
/// one outbound request with its own deadline; a thread per connection would be
/// an unbounded number of them, handed to whoever can open sockets fastest.
fn accept_loop(
    listener: TcpListener,
    session: Arc<StatusSession>,
    stop: Arc<AtomicBool>,
    deadline: Duration,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                // On some platforms an accepted socket inherits the listener's
                // non-blocking flag; the per-operation timeouts below are what
                // bound the conversation, and they need a blocking socket.
                let _ = stream.set_nonblocking(false);
                if let Err(error) = serve(stream, &session, deadline) {
                    tracing::debug!(%error, "status page connection ended early");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
            }
            Err(error) => {
                // A listener this program owns cannot be closed underneath it,
                // so this is a transient failure - a connection abandoned before
                // it was accepted, or a file-descriptor limit reached. Ending
                // the loop would take the page away silently, which is worse
                // than a pause.
                tracing::warn!(%error, "status page could not accept");
                std::thread::sleep(POLL);
            }
        }
    }
}

/// Reads one request and writes one answer.
fn serve(mut stream: TcpStream, session: &StatusSession, deadline: Duration) -> io::Result<()> {
    let until = Instant::now() + deadline;
    let head = match read_head(&mut stream, until) {
        Ok(head) => head,
        Err(HeadError::TooLarge) => {
            return respond(&mut stream, until, Status::TooLarge, Body::Text, "");
        }
        Err(HeadError::Malformed) => {
            return respond(&mut stream, until, Status::BadRequest, Body::Text, "");
        }
        // The reader went away, or never finished asking. Nothing to answer.
        Err(HeadError::Closed) | Err(HeadError::Timeout) => return Ok(()),
        Err(HeadError::Io(error)) => return Err(error),
    };

    let Some(request) = Request::parse(&head) else {
        return respond(&mut stream, until, Status::BadRequest, Body::Text, "");
    };

    // The host check comes before routing, so a request that arrived from a page
    // that is not this one learns nothing at all - not even which paths exist.
    if !request.host_names(stream.local_addr()?.port()) {
        return respond(&mut stream, until, Status::Forbidden, Body::Text, "");
    }
    // Nothing about a request may steer this service, and a query string is the
    // one way a caller could try: a page that could name the endpoint here would
    // have turned the reader's own machine into a proxy carrying their profile's
    // credentials. What holds that guarantee is the exact-match routing below -
    // `GET /api/egress?url=...` matches no arm and is a 404 whatever happens
    // here, which is what removing this check was measured to do. The check is
    // kept because the reason it is safe is a property of a `match` three lines
    // down, and a later change from an exact match to a prefix would leave the
    // opening and take the guarantee with it. Refusing the parameter by name
    // makes the refusal explicit, and a test says which answer a caller gets.
    if request.target.contains('?') {
        return respond(&mut stream, until, Status::BadRequest, Body::Text, "");
    }
    if request.method != "GET" {
        return respond(&mut stream, until, Status::MethodNotAllowed, Body::Text, "");
    }

    match request.target.as_str() {
        "/" => {
            let page = session.page();
            respond(&mut stream, until, Status::Ok, Body::Html, &page)
        }
        "/api/status" => {
            let body = json(&session.report);
            respond(&mut stream, until, Status::Ok, Body::Json, &body)
        }
        "/api/egress" => {
            let body = json(&session.egress());
            respond(&mut stream, until, Status::Ok, Body::Json, &body)
        }
        _ => respond(&mut stream, until, Status::NotFound, Body::Text, ""),
    }
}

/// Serializes a value the page reads.
///
/// A failure here cannot be reported as a fault of its own: both types are built
/// by this module out of strings and numbers, so the only way to reach the
/// fallback is a bug in them, and an answer the page can read beats a panic in a
/// thread nobody is watching.
fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| r#"{"kind":"unreadable"}"#.to_string())
}

/// Why a request head was not read.
enum HeadError {
    /// The client went away without finishing.
    Closed,
    /// The deadline passed.
    Timeout,
    /// The head is longer than [`MAX_REQUEST_HEAD`].
    TooLarge,
    /// The bytes are not a request this service reads.
    Malformed,
    Io(io::Error),
}

impl From<io::Error> for HeadError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => Self::Timeout,
            io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset => Self::Closed,
            _ => Self::Io(error),
        }
    }
}

/// Reads up to the blank line that ends a request head.
///
/// Every read is armed with what is left of one absolute deadline rather than a
/// fresh timeout, so a peer that drips a byte at a time cannot hold the service
/// open by making progress.
fn read_head(stream: &mut TcpStream, until: Instant) -> Result<String, HeadError> {
    let mut head: Vec<u8> = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    let mut searched = 0usize;
    loop {
        let remaining = until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(HeadError::Timeout);
        }
        stream.set_read_timeout(Some(remaining))?;
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(HeadError::Closed);
        }
        head.extend_from_slice(&chunk[..read]);
        // From three bytes before what has already been looked at, because the
        // terminator may straddle the boundary between two reads.
        let from = searched.saturating_sub(3);
        if let Some(offset) = head[from..]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            let end = from + offset;
            return String::from_utf8(head[..end].to_vec()).map_err(|_| HeadError::Malformed);
        }
        searched = head.len();
        if head.len() > MAX_REQUEST_HEAD {
            return Err(HeadError::TooLarge);
        }
    }
}

/// The parts of a request this service reads, and nothing else.
struct Request {
    method: String,
    target: String,
    host: Option<String>,
}

impl Request {
    /// Parses a head, or `None` if it is not one.
    fn parse(head: &str) -> Option<Self> {
        let mut lines = head.split("\r\n");
        let mut request_line = lines.next()?.split(' ');
        let method = request_line.next()?.to_string();
        let target = request_line.next()?.to_string();
        let version = request_line.next()?;
        if request_line.next().is_some() {
            return None;
        }
        if !version.starts_with("HTTP/1.") || method.is_empty() || target.is_empty() {
            return None;
        }

        let mut host = None;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                // A header without a colon is not a header, and a head that
                // holds one is not a head this service reads.
                return None;
            };
            if name.eq_ignore_ascii_case("host") {
                // Two `Host` headers are a request that means two things, and
                // the check below can only be about one of them.
                if host.is_some() {
                    return None;
                }
                host = Some(value.trim().to_string());
            }
        }

        Some(Self {
            method,
            target,
            host,
        })
    }

    /// Whether the `Host` header names this service.
    ///
    /// This is the guard against DNS rebinding: a page on another origin can
    /// make the reader's browser send a request to `127.0.0.1`, and the browser
    /// will send it. What that page cannot do is make the browser lie about the
    /// name it is asking for, so a request whose `Host` is not this service is
    /// refused however it arrived. Without this check the page's contents - the
    /// profile, the ports, the exit address - would be readable by any web page
    /// the reader visits.
    fn host_names(&self, port: u16) -> bool {
        let Some(host) = self.host.as_deref() else {
            return false;
        };
        [format!("127.0.0.1:{port}"), format!("localhost:{port}")]
            .iter()
            .any(|name| host.eq_ignore_ascii_case(name))
    }
}

/// The response statuses this service can answer with.
enum Status {
    Ok,
    BadRequest,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    TooLarge,
}

impl Status {
    fn line(&self) -> &'static str {
        match self {
            Self::Ok => "200 OK",
            Self::BadRequest => "400 Bad Request",
            Self::Forbidden => "403 Forbidden",
            Self::NotFound => "404 Not Found",
            Self::MethodNotAllowed => "405 Method Not Allowed",
            Self::TooLarge => "431 Request Header Fields Too Large",
        }
    }

    /// Whether this status carries a body a browser will render or a page will
    /// parse, as opposed to an empty answer that says only "no".
    fn carries_content(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

/// What the body of an answer is, which decides its content type.
enum Body {
    Html,
    Json,
    Text,
}

impl Body {
    fn content_type(&self) -> &'static str {
        match self {
            Self::Html => "text/html; charset=utf-8",
            Self::Json => "application/json; charset=utf-8",
            Self::Text => "text/plain; charset=utf-8",
        }
    }
}

/// The policy the page is served under.
///
/// `default-src 'none'` with `connect-src 'self'` is this module's first promise
/// made enforceable: the page can read this service and nothing else, so no
/// future edit to it can quietly start asking the network a question the reader
/// did not ask for.
const CSP: &str = "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; \
                   script-src 'unsafe-inline'; connect-src 'self'\r\n";

/// Writes one answer and closes the connection.
///
/// One request per connection: this service has nothing to gain from keeping a
/// socket open, and a connection that is never reused is a connection that
/// cannot be confused about which request it is answering.
fn respond(
    stream: &mut TcpStream,
    until: Instant,
    status: Status,
    body_kind: Body,
    body: &str,
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        status.line(),
        if status.carries_content() {
            body_kind.content_type()
        } else {
            Body::Text.content_type()
        },
        body.len()
    );
    if matches!(status, Status::MethodNotAllowed) {
        head.push_str("Allow: GET\r\n");
    }
    if status.carries_content() && matches!(body_kind, Body::Html) {
        head.push_str(CSP);
    }
    head.push_str("\r\n");

    write_all(stream, until, head.as_bytes())?;
    write_all(stream, until, body.as_bytes())
}

/// Writes with the deadline that is left, rather than with a timeout of its own.
fn write_all(stream: &mut TcpStream, until: Instant, bytes: &[u8]) -> io::Result<()> {
    let remaining = until.saturating_duration_since(Instant::now());
    stream.set_write_timeout(Some(remaining.max(Duration::from_millis(1))))?;
    stream.write_all(bytes)
}

/// The page's own text, in the languages it is written in.
///
/// `(key, en, zh)`. The tests below walk [`PAGE`] and fail on a placeholder with
/// no row here, so a message added to the page cannot be added to one language
/// only.
const STRINGS: &[(&str, &str, &str)] = &[
    ("title", "Identity", "身份"),
    (
        "lede",
        "This page is served by the program on this machine, to the profile that opened it. Nothing here leaves the machine except the one request described at the bottom.",
        "本页由本机程序提供，只服务于打开它的那个档案。除了页面底部说明的那一次请求，这里的内容不会离开本机。",
    ),
    ("session", "This session", "本次会话"),
    ("profile", "Profile", "档案"),
    ("core", "Browser core", "浏览器内核"),
    ("proxy", "Proxy", "代理"),
    ("engine_port", "Engine port", "引擎端口"),
    ("started", "Started", "启动于"),
    ("notices", "Fingerprint notices", "指纹提示"),
    ("egress", "Where one request leaves from", "一次请求的出口"),
    (
        "egress_note",
        "One request is sent to the endpoint below, through this profile's engine. That gives the engine's exit address, not a reading taken from the browser itself - the Verify action in the window reads that one back out of the browser.",
        "程序会经这个档案的引擎，向下面的端点发送一次请求。得到的是引擎的出口地址，不是从浏览器本身读回的结果——浏览器自己的出口地址要用窗口里的「校验」读回。",
    ),
    ("endpoint", "Endpoint", "端点"),
    ("recheck", "Check again", "重新检测"),
    ("checking", "Asking…", "正在检测…"),
    (
        "unavailable",
        "this page could not read the session",
        "本页读不到会话信息",
    ),
];

/// Renders the page in one language.
///
/// The language is baked into the served bytes, so a page already open does not
/// change language under the reader when the setting does; the next launch opens
/// the new one.
fn render_page(lang: &str, echo_url: &str) -> String {
    let mut page = PAGE.to_string();
    for (key, en, zh) in STRINGS {
        let text = if lang == "zh" { zh } else { en };
        page = page.replace(&format!("{{{{{key}}}}}"), &escape(text));
    }
    page.replace("{{lang}}", if lang == "zh" { "zh" } else { "en" })
        .replace("{{echo_url}}", &escape(echo_url))
}

/// Escapes text for HTML, so a profile named with a `<` is a name and not markup.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page itself. `{{key}}` is a message from [`STRINGS`], `{{lang}}` and
/// `{{echo_url}}` are filled from the session.
const PAGE: &str = r#"<!DOCTYPE html>
<html lang="{{lang}}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{title}}</title>
<style>
:root { color-scheme: light dark; --bg:#ffffff; --panel:#f6f7f9; --line:#e3e6ea; --text:#16181d; --soft:#5c636e; --fault:#c0392b; }
@media (prefers-color-scheme: dark) { :root { --bg:#16181d; --panel:#1e2128; --line:#2c3038; --text:#eef0f3; --soft:#9aa2ae; --fault:#ff8a80; } }
* { box-sizing: border-box; }
body { margin:0; padding:32px 20px; background:var(--bg); color:var(--text);
  font:14px/1.5 system-ui, -apple-system, "Segoe UI", "Noto Sans CJK SC", sans-serif; }
main { max-width:640px; margin:0 auto; }
h1 { font-size:20px; margin:0 0 8px; }
h2 { font-size:13px; text-transform:uppercase; letter-spacing:.04em; color:var(--soft); margin:28px 0 8px; }
p.lede { color:var(--soft); margin:0 0 4px; }
.card { background:var(--panel); border:1px solid var(--line); border-radius:8px; padding:4px 16px; }
.row { display:flex; gap:16px; padding:10px 0; border-top:1px solid var(--line); }
.row:first-child { border-top:0; }
.row dt { color:var(--soft); flex:0 0 168px; margin:0; }
.row dd { margin:0; word-break:break-all; }
.address { font-size:22px; font-variant-numeric:tabular-nums; }
.fault { color:var(--fault); }
button { font:inherit; color:var(--text); background:var(--bg); border:1px solid var(--line);
  border-radius:6px; padding:5px 12px; cursor:pointer; }
button:hover { border-color:var(--soft); }
button:disabled { color:var(--soft); cursor:default; }
.note { color:var(--soft); font-size:13px; }
</style>
</head>
<body>
<main>
<h1>{{title}}</h1>
<p class="lede">{{lede}}</p>

<h2>{{session}}</h2>
<div class="card"><dl>
  <div class="row"><dt>{{profile}}</dt><dd id="profile">&hellip;</dd></div>
  <div class="row"><dt>{{core}}</dt><dd id="core">&hellip;</dd></div>
  <div class="row"><dt>{{proxy}}</dt><dd id="proxy">&hellip;</dd></div>
  <div class="row"><dt>{{engine_port}}</dt><dd id="socks">&hellip;</dd></div>
  <div class="row"><dt>{{started}}</dt><dd id="started">&hellip;</dd></div>
  <div class="row" id="warning-row" hidden><dt>{{notices}}</dt><dd id="warning"></dd></div>
</dl></div>

<h2>{{egress}}</h2>
<div class="card"><dl>
  <div class="row"><dt>{{endpoint}}</dt><dd id="endpoint">{{echo_url}}</dd></div>
  <div class="row"><dt>{{egress}}</dt><dd id="egress" class="address">&hellip;</dd></div>
</dl></div>
<p class="note">{{egress_note}}</p>
<p><button id="recheck" type="button">{{recheck}}</button></p>
</main>
<script>
(function () {
  "use strict";
  var t = { checking: "{{checking}}", unavailable: "{{unavailable}}" };
  function set(id, text) {
    var el = document.getElementById(id);
    if (el) { el.textContent = text; }
  }

  function loadSession() {
    return fetch("/api/status", { cache: "no-store" }).then(function (r) { return r.json(); })
      .then(function (s) {
        set("profile", s.profile + " (" + s.profile_id + ")");
        set("core", s.core + (s.core_major === null ? "" : " · " + s.core_major));
        set("proxy", s.proxy === null ? "—" : s.proxy);
        set("socks", s.socks_port === null ? "—" : String(s.socks_port));
        set("started", new Date(s.started_at * 1000).toLocaleString());
        if (s.warning) {
          set("warning", s.warning);
          document.getElementById("warning-row").hidden = false;
        }
      })
      .catch(function () { set("profile", t.unavailable); });
  }

  function loadEgress() {
    var button = document.getElementById("recheck");
    var el = document.getElementById("egress");
    button.disabled = true;
    el.className = "address";
    el.textContent = t.checking;
    return fetch("/api/egress", { cache: "no-store" }).then(function (r) { return r.json(); })
      .then(function (e) {
        if (e.kind === "address") {
          el.textContent = e.address;
        } else if (e.kind === "fault") {
          el.textContent = e.class + ": " + e.detail;
          el.className = "address fault";
        } else {
          el.textContent = e.reason;
        }
      })
      .catch(function (error) {
        el.textContent = String(error);
        el.className = "address fault";
      })
      .then(function () { button.disabled = false; });
  }

  document.getElementById("recheck").addEventListener("click", loadEgress);
  loadSession();
  loadEgress();
})();
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{PortAllocator, TcpPortAllocator};

    /// A session with nothing running behind it, so a test never dials out.
    fn session(socks_port: Option<u16>) -> Arc<StatusSession> {
        Arc::new(StatusSession {
            report: StatusReport {
                profile: "shop-1".to_string(),
                profile_id: "8f1c".to_string(),
                core: "148.0.7778.215".to_string(),
                core_major: Some(148),
                proxy: Some("tokyo".to_string()),
                cdp_port: 9222,
                socks_port,
                started_at: 1_700_000_000,
                warning: None,
                echo_url: "http://api.ipify.org".to_string(),
            },
            socks_port,
            lang: "en",
        })
    }

    fn service(session: Arc<StatusSession>) -> StatusService {
        let reservation = TcpPortAllocator
            .reserve_loopback()
            .expect("a loopback port");
        StatusService::start_with(reservation, session, Duration::from_millis(300))
            .expect("the service starts")
    }

    /// Sends raw bytes and reads the whole answer, so a test can ask for things
    /// a browser never would.
    fn ask(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a read timeout");
        stream.write_all(request.as_bytes()).expect("write");
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        answer
    }

    fn get(port: u16, path: &str) -> String {
        ask(
            port,
            &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
    }

    fn status_line(answer: &str) -> &str {
        answer.lines().next().unwrap_or_default()
    }

    fn body(answer: &str) -> &str {
        answer.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("")
    }

    #[test]
    fn the_page_is_served_on_the_port_that_was_reserved() {
        let service = service(session(Some(1080)));
        let answer = get(service.port(), "/");

        assert!(status_line(&answer).contains("200 OK"), "{answer}");
        assert!(answer.contains("text/html"), "{answer}");
        assert!(body(&answer).starts_with("<!DOCTYPE"), "{answer}");
        // The address the browser is sent to is the address it can reach.
        assert_eq!(
            service.url(),
            format!("http://127.0.0.1:{}/", service.port())
        );
        assert!(TcpStream::connect(("127.0.0.1", service.port())).is_ok());
    }

    /// The guard against DNS rebinding, and the reason the host is checked before
    /// the path is: a page on another origin can make the reader's browser send a
    /// request here, but it cannot make it claim this service's name.
    #[test]
    fn a_request_that_names_another_host_is_refused() {
        let service = service(session(Some(1080)));
        let port = service.port();

        for host in [
            "evil.example",
            "127.0.0.1.nip.io",
            // The right name on the wrong port is a different service.
            "127.0.0.1:1",
        ] {
            let answer = ask(
                port,
                &format!("GET /api/status HTTP/1.1\r\nHost: {host}\r\n\r\n"),
            );
            assert!(
                status_line(&answer).contains("403 Forbidden"),
                "{host}: {answer}"
            );
            assert!(
                !body(&answer).contains("shop-1"),
                "a refused request was answered anyway: {answer}"
            );
        }

        // And the two names this service does answer to, whatever their case.
        for host in [format!("127.0.0.1:{port}"), format!("LOCALHOST:{port}")] {
            let answer = ask(
                port,
                &format!("GET /api/status HTTP/1.1\r\nHost: {host}\r\n\r\n"),
            );
            assert!(status_line(&answer).contains("200 OK"), "{host}: {answer}");
        }
    }

    /// A request with no `Host` at all cannot be shown to be this service's.
    #[test]
    fn a_request_without_a_host_is_refused() {
        let service = service(session(Some(1080)));
        let answer = ask(service.port(), "GET /api/status HTTP/1.1\r\n\r\n");
        assert!(status_line(&answer).contains("403 Forbidden"), "{answer}");
    }

    /// Two `Host` headers mean two things, and the check can only be about one.
    #[test]
    fn a_request_with_two_hosts_is_refused() {
        let service = service(session(Some(1080)));
        let port = service.port();
        let answer = ask(
            port,
            &format!(
                "GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nHost: evil.example\r\n\r\n"
            ),
        );
        assert!(status_line(&answer).contains("400 Bad Request"), "{answer}");
    }

    /// The endpoint the page asks about is fixed by the launch. A query string is
    /// the one way a caller could try to name a different one, and a service that
    /// took one would be an open proxy holding the profile's credentials.
    #[test]
    fn a_query_string_is_refused_rather_than_ignored() {
        let service = service(session(Some(1080)));
        let port = service.port();

        for target in [
            "/api/egress?url=http://example.com",
            "/api/egress?",
            "/?q=1",
        ] {
            let answer = ask(
                port,
                &format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            );
            assert!(
                status_line(&answer).contains("400 Bad Request"),
                "{target}: {answer}"
            );
        }
    }

    #[test]
    fn an_unknown_path_is_not_found_and_another_method_is_refused() {
        let service = service(session(Some(1080)));
        let port = service.port();

        let answer = get(port, "/admin");
        assert!(status_line(&answer).contains("404 Not Found"), "{answer}");

        let answer = ask(
            port,
            &format!("POST /api/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        );
        assert!(
            status_line(&answer).contains("405 Method Not Allowed"),
            "{answer}"
        );
        assert!(answer.contains("Allow: GET"), "{answer}");
    }

    /// Nothing the caller sent comes back: a service that echoed a header would
    /// be one that can be made to carry a value somewhere else.
    #[test]
    fn nothing_the_request_carried_is_echoed_back() {
        let service = service(session(Some(1080)));
        let port = service.port();
        let answer = ask(
            port,
            &format!(
                "GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                 X-Secret: hunter2\r\nCookie: session=abc\r\n\r\n"
            ),
        );
        assert!(!answer.contains("hunter2"), "{answer}");
        assert!(!answer.contains("session=abc"), "{answer}");
    }

    #[test]
    fn the_report_is_json_and_carries_no_proxy_credentials() {
        let service = service(session(Some(1080)));
        let answer = get(service.port(), "/api/status");
        let body = body(&answer);

        let parsed: serde_json::Value = serde_json::from_str(body).expect("json");
        assert_eq!(parsed["profile"], "shop-1");
        assert_eq!(parsed["proxy"], "tokyo");
        assert_eq!(parsed["core_major"], 148);
        // The name is the whole of what the page is told about the proxy.
        assert!(!body.contains("ss://"), "{body}");
        assert!(!body.contains("password"), "{body}");
    }

    /// A profile with no proxy claims nothing about an address, so the page is
    /// told that rather than given a number it did not earn.
    #[test]
    fn a_profile_without_a_proxy_is_not_asked_about_an_address() {
        let service = service(session(None));
        let answer = get(service.port(), "/api/egress");
        let parsed: serde_json::Value = serde_json::from_str(body(&answer)).expect("json");

        assert_eq!(parsed["kind"], "not_asked");
        assert!(parsed["reason"].as_str().unwrap_or("").contains("no proxy"));
    }

    /// A head that never ends is refused, and the service is still there after.
    #[test]
    fn a_head_that_is_too_long_is_refused_and_the_service_survives() {
        let service = service(session(Some(1080)));
        let port = service.port();

        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a read timeout");
        let filler = "X-Filler: ".to_string() + &"a".repeat(400) + "\r\n";
        let head = format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{}",
            filler.repeat(32)
        );
        stream.write_all(head.as_bytes()).expect("write");

        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        assert!(
            status_line(&answer).contains("431"),
            "the oversized head was not refused: {answer}"
        );

        // The service answered another reader afterwards.
        assert!(status_line(&get(port, "/")).contains("200 OK"));
    }

    /// One reader who connects and says nothing must not take the page away from
    /// everybody else for longer than the deadline.
    #[test]
    fn a_client_that_never_asks_does_not_hold_the_service() {
        let service = service(session(Some(1080)));
        let port = service.port();

        let idle = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let started = Instant::now();
        let answer = get(port, "/");
        let waited = started.elapsed();

        assert!(status_line(&answer).contains("200 OK"), "{answer}");
        assert!(
            waited < Duration::from_secs(3),
            "the second reader waited {waited:?} behind a silent one"
        );
        drop(idle);
    }

    /// Dropping the service is what frees the port, so a stop, a crash and a
    /// reclaim all end the page the same way.
    #[test]
    fn dropping_the_service_frees_its_port() {
        let service = service(session(Some(1080)));
        let port = service.port();
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());

        drop(service);
        assert!(
            TcpListener::bind(("127.0.0.1", port)).is_ok(),
            "the port was not released"
        );
    }

    /// The page is written in the language the launch asked for, and the two
    /// languages really are two.
    #[test]
    fn the_page_is_rendered_in_the_language_it_was_asked_for() {
        let english = render_page("en", "http://api.ipify.org");
        let chinese = render_page("zh", "http://api.ipify.org");

        assert!(english.contains("This session"), "{english}");
        assert!(!english.contains("本次会话"), "{english}");
        assert!(chinese.contains("本次会话"), "{chinese}");
        assert!(chinese.contains(r#"<html lang="zh">"#), "{chinese}");
        assert_ne!(english, chinese);
    }

    /// Every message the page uses exists in both languages, and the two differ:
    /// the rule the window's catalog is held to, for the same reason.
    #[test]
    fn the_page_is_written_in_every_language_it_claims() {
        for (key, en, zh) in STRINGS {
            assert!(!en.is_empty(), "{key} has no English");
            assert!(!zh.is_empty(), "{key} has no Chinese");
            assert_ne!(en, zh, "{key} is the same in both languages");
        }
        for key in placeholders(PAGE) {
            if key == "echo_url" || key == "lang" {
                continue;
            }
            assert!(
                STRINGS.iter().any(|(name, ..)| *name == key),
                "the page uses {key} but no language has a message for it"
            );
        }
    }

    /// And every message a language declares is one the page uses, so an edit to
    /// the markup cannot leave a translation behind.
    #[test]
    fn every_message_is_used_by_the_page() {
        let used = placeholders(PAGE);
        for (key, ..) in STRINGS {
            assert!(used.contains(&key.to_string()), "{key} is never used");
        }
    }

    /// The rendered page holds no placeholder, which is what a reader would see
    /// if a message were added to the markup and not to the table.
    #[test]
    fn no_placeholder_reaches_the_reader() {
        let page = render_page("en", "http://api.ipify.org");
        assert!(!page.contains("{{"), "a placeholder was left in the page");
        assert!(
            page.contains("http://api.ipify.org"),
            "the endpoint is named"
        );
    }

    /// A profile's name is text, not markup.
    #[test]
    fn a_name_with_markup_in_it_is_escaped() {
        let page = render_page("en", "http://example.com/<script>");
        assert!(!page.contains("<script>alert"), "{page}");
        assert!(page.contains("&lt;script&gt;"), "{page}");
    }

    fn placeholders(template: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            found.push(after[..end].to_string());
            rest = &after[end..];
        }
        found
    }
}
