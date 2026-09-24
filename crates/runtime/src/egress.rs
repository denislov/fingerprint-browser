//! Reading back the address a running browser's traffic actually leaves from.
//!
//! The pre-flight in [`crate::diagnostic`] proves that a *proxy* carries a
//! request: it starts an engine, sends one request through its loopback SOCKS
//! inbound and reports the address the endpoint saw. What it cannot prove is
//! that a *browser* was pointed at that engine. Those come apart in the cases
//! that matter - a launch flag that was ignored, a profile with no proxy at all,
//! a second proxy between the browser and the engine - and the symptom is the
//! one this project exists to prevent: a fingerprint that claims one machine
//! while the traffic leaves from another.
//!
//! So this module asks the browser itself. It opens a page of its own at the
//! address endpoint, lets the browser fetch it through whatever network path it
//! was launched with, and reads the address the endpoint named. Nothing here
//! inspects a command line or a switch list; the answer comes from the far end.
//!
//! What it does not establish, and deliberately does not claim: that a page the
//! user opened takes the same path, or that any of this is the address a
//! *profile* was configured for. It reads one document, in one tab, once.
//!
//! ## Why the failure modes are kept apart
//!
//! "The browser did not get there", "it got part of the way", "the endpoint
//! answered with something that is not an address" and "the browser could not be
//! asked at all" are four different faults with four different fixes, and
//! collapsing them into "no reading" would hide the only thing worth acting on.
//! The distinction between the first two is read from the page rather than
//! guessed from a timeout, which is what [`DocumentState`] is for.

use crate::cdp::{CdpProbe as _, CdpSession, CdpTarget, DocumentState, HttpCdpProbe};
use crate::diagnostic::{check_echo_url, extract_exit_ip};
use crate::verify::Discrepancy;
use std::time::{Duration, Instant};

/// How much of an answer is read back out of the page.
///
/// The same bound the pre-flight puts on a response body, for the same reason:
/// an address endpoint answers with a line, and a URL that has been pointed
/// somewhere else must not pull a whole document into the window.
const READING_LIMIT: usize = 4 * 1024;

/// How much of an answer that held no address is carried to the user.
const EXCERPT_LIMIT: usize = 120;

/// How often the page is asked what it holds while a read-back is in flight.
const POLL: Duration = Duration::from_millis(25);

/// The schemes a page can arrive at. A plain-http endpoint may redirect, and a
/// document that arrived over TLS arrived: refusing that would report a working
/// path as a broken one. The endpoint's own scheme is still constrained to
/// plain http, so a certificate problem cannot be introduced by this module.
const ARRIVED: [&str; 2] = ["http:", "https:"];

/// Evaluates to the text of the page the browser fetched.
///
/// Read from the document rather than fetched: the request that matters is the
/// one the browser already made to get here, and asking the page to fetch
/// anything would be asking its origin instead of its network path. Built from
/// [`READING_LIMIT`] so the bound this module documents is the bound it applies.
fn body_expression() -> String {
    format!("(document.body?document.body.innerText.slice(0,{READING_LIMIT}):'')")
}

/// An address a running browser's traffic left from, and where it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressReading {
    /// The address the endpoint saw, which is the address that left.
    pub exit_ip: String,
    /// The document it was read from, so a reading can be traced to a page.
    pub url: String,
}

/// What asking a running browser about its exit address established.
///
/// Every way this can end is one of these; there is no unclassified failure,
/// because "something went wrong" is not an answer the user can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressOutcome {
    /// The browser reached the endpoint and the endpoint named an address.
    Read(EgressReading),
    /// The page never committed to the endpoint, so nothing left by this route.
    NotReached { url: String, state: String },
    /// The page was at the endpoint and had not finished loading in time.
    Timeout { url: String, state: String },
    /// The endpoint answered, and the answer held no address.
    Unreadable { url: String, excerpt: String },
    /// The endpoint is not one that can be asked.
    Unusable(String),
    /// The browser could not be asked at all: no CDP endpoint, or it refused.
    NotAsked(String),
}

impl EgressOutcome {
    /// The address, when there was a reading.
    pub fn exit_ip(&self) -> Option<&str> {
        match self {
            Self::Read(reading) => Some(&reading.exit_ip),
            _ => None,
        }
    }

    /// How to read the outcome, for a window or a log line.
    pub fn detail(&self) -> String {
        match self {
            Self::Read(reading) => format!("left from {}", reading.exit_ip),
            Self::NotReached { url, state } => {
                format!("the browser's page never committed to {url} (last state: {state})")
            }
            Self::Timeout { url, state } => {
                format!("the page at {url} had not finished loading (last state: {state})")
            }
            Self::Unreadable { url, excerpt } => {
                format!("{url} answered with something that is not an address: {excerpt}")
            }
            Self::Unusable(reason) => format!("the endpoint cannot be asked: {reason}"),
            Self::NotAsked(reason) => format!("the browser could not be asked: {reason}"),
        }
    }
}

impl std::fmt::Display for EgressOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail())
    }
}

/// Reports the claims a running browser's exit address does not support.
///
/// `expected` is the address the profile's proxy was tested at, and is `None`
/// when that proxy was never tested: with nothing established about where the
/// traffic should leave from, a reading has nothing to disagree with, and
/// inventing a discrepancy would be asserting a claim the profile never made.
pub fn verify_egress(expected: Option<&str>, outcome: &EgressOutcome) -> Vec<Discrepancy> {
    let Some(expected) = expected else {
        return Vec::new();
    };

    // Compared before the expectation is worded for a reader. The qualifier
    // below is part of the message, and comparing against it would make every
    // agreeing reading look like a disagreement.
    if let EgressOutcome::Read(reading) = outcome
        && reading.exit_ip == expected
    {
        return Vec::new();
    }

    // The qualifier is part of the expectation, not decoration: an address is
    // only wrong against somewhere else it was supposed to be, and the only
    // such place the product knows about is where the proxy was tested.
    let expectation = format!("{expected} (where the proxy was tested)");
    let observed = match outcome {
        EgressOutcome::Read(reading) => reading.exit_ip.clone(),
        // Not reaching the endpoint at all is a stronger finding than reaching
        // it from the wrong address, not a weaker one: the tested proxy carried
        // a request, and this browser's traffic did not come out of it.
        other => other.detail(),
    };
    vec![Discrepancy {
        claim: "exit address",
        expected: expectation,
        observed,
    }]
}

/// Asks a running browser where its traffic leaves from.
#[derive(Debug, Clone)]
pub struct EgressProbe {
    timeout: Duration,
}

impl EgressProbe {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// Opens a page of the probe's own at `echo_url` and reads the address back.
    ///
    /// A new target rather than the page the user is looking at, for the same
    /// reason [`crate::verify::FingerprintProbe::read_fresh`] opens one: this
    /// must not navigate, reload or otherwise touch what the user has open. The
    /// target is closed afterwards on every path, including a failure.
    ///
    /// The request is the browser's, made through whatever network path it was
    /// launched with. That is the whole point: a probe that dialled the endpoint
    /// itself would answer a question about this process, not about the browser.
    pub fn read(&self, port: u16, echo_url: &str) -> EgressOutcome {
        // Refused before anything is dialled, so a rejected endpoint cannot be
        // reported as a path that carried nothing.
        if let Err(fault) = check_echo_url(echo_url) {
            return EgressOutcome::Unusable(fault.detail);
        }

        let probe = HttpCdpProbe::new();
        if let Err(error) = probe.wait_ready(port, self.timeout) {
            return EgressOutcome::NotAsked(error.to_string());
        }
        let target = match probe.create_page(port, echo_url, self.timeout) {
            Ok(target) => target,
            Err(error) => return EgressOutcome::NotAsked(error.to_string()),
        };

        let outcome = self.read_target(port, &target, echo_url);

        // Closing is best effort: a tab left behind must not turn a reading
        // into a non-reading, and the reading is what the caller asked for.
        let _ = probe.close_page(port, target.target_id(), self.timeout);
        outcome
    }

    fn read_target(&self, port: u16, target: &CdpTarget, echo_url: &str) -> EgressOutcome {
        let mut session = match CdpSession::connect_page(port, target, self.timeout) {
            Ok(session) => session,
            Err(error) => return EgressOutcome::NotAsked(error.to_string()),
        };

        let deadline = Instant::now() + self.timeout;
        let body_expression = body_expression();
        loop {
            let state = match session.document_state(self.timeout) {
                Ok(state) => state,
                Err(error) => return EgressOutcome::NotAsked(error.to_string()),
            };

            let body = if arrived(&state) {
                match session.evaluate(&body_expression, self.timeout) {
                    Ok(value) => Some(value.as_str().unwrap_or_default().to_string()),
                    Err(error) => return EgressOutcome::NotAsked(error.to_string()),
                }
            } else {
                None
            };

            // A page that committed elsewhere and finished loading is not going
            // to arrive by waiting longer, so the wait ends now rather than at
            // the deadline: a failed navigation should be reported promptly.
            let settled =
                body.is_some() || (state.has_committed() && state.ready_state == "complete");
            if settled || Instant::now() >= deadline {
                return outcome_from(echo_url, &state, body.as_deref());
            }
            std::thread::sleep(POLL);
        }
    }
}

/// Whether the page holds a finished document of a scheme a browsing session
/// can produce.
fn arrived(state: &DocumentState) -> bool {
    state.ready_state == "complete"
        && state.has_document
        && ARRIVED.contains(&state.protocol.as_str())
}

/// Reads one page state and its text into an outcome.
fn outcome_from(url: &str, state: &DocumentState, body: Option<&str>) -> EgressOutcome {
    if let Some(body) = body {
        return match extract_exit_ip(body) {
            Some(exit_ip) => EgressOutcome::Read(EgressReading {
                exit_ip,
                url: url.to_string(),
            }),
            None => EgressOutcome::Unreadable {
                url: url.to_string(),
                excerpt: excerpt(body),
            },
        };
    }

    // No text, so the page is not at the endpoint. Whether it committed to the
    // endpoint and ran out of time, or never committed at all, is the whole
    // difference between a slow path and one that carries nothing - and it is
    // read from the page rather than inferred from having waited.
    if arrived(state) || ARRIVED.contains(&state.protocol.as_str()) {
        EgressOutcome::Timeout {
            url: url.to_string(),
            state: state.to_string(),
        }
    } else {
        EgressOutcome::NotReached {
            url: url.to_string(),
            state: state.to_string(),
        }
    }
}

/// A few words of an answer that held no address.
///
/// An endpoint that was pointed at the wrong thing answers with a page, and its
/// words are the only evidence of what happened. Collapsed to one line because
/// this reaches a window and a log.
fn excerpt(body: &str) -> String {
    let text = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= EXCERPT_LIMIT {
        return text;
    }
    let mut cut: String = text.chars().take(EXCERPT_LIMIT).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CDP endpoint, without a browser.
    ///
    /// One port has to speak both protocols, because that is what a browser
    /// does: the metadata endpoints are HTTP and the session is a WebSocket,
    /// and the session's URL is the one the metadata handed out. So the first
    /// request line decides which of the two a connection is, and the bytes
    /// already read are replayed into whichever answers.
    ///
    /// This is what exercises [`EgressProbe::read`] as a whole: readiness,
    /// opening a page of its own, polling the document, reading the answer and
    /// closing the page again. A browser cannot be run in every environment, and
    /// the orchestration should not go untested because of it.
    mod stub {
        use std::collections::VecDeque;
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        use tungstenite::WebSocket;

        /// A stream whose opening bytes were already read while working out what
        /// the request was.
        struct Peeked {
            prefix: Vec<u8>,
            offset: usize,
            inner: TcpStream,
        }

        impl Read for Peeked {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.offset < self.prefix.len() {
                    let remaining = &self.prefix[self.offset..];
                    let taken = remaining.len().min(buffer.len());
                    buffer[..taken].copy_from_slice(&remaining[..taken]);
                    self.offset += taken;
                    return Ok(taken);
                }
                self.inner.read(buffer)
            }
        }

        impl Write for Peeked {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                self.inner.write(buffer)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.inner.flush()
            }
        }

        /// Reads up to the end of the request headers and not one byte more, so
        /// a WebSocket handshake can still read its own frames afterwards.
        fn read_head(stream: &mut TcpStream) -> std::io::Result<String> {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
                if stream.read(&mut byte)? == 0 {
                    break;
                }
                head.push(byte[0]);
            }
            Ok(String::from_utf8_lossy(&head).to_string())
        }

        fn reply_http(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
            stream.write_all(
                format!(
                    "HTTP/1.1 {status} x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
        }

        /// The stub's record of what it was asked, and the port it answers on.
        pub struct Stub {
            pub port: u16,
            /// Request lines of the page creations, in order.
            pub created: Arc<Mutex<Vec<String>>>,
            /// Target ids that were closed, in order.
            pub closed: Arc<Mutex<Vec<String>>>,
        }

        impl Stub {
            pub fn created(&self) -> Vec<String> {
                self.created.lock().expect("created lock").clone()
            }

            pub fn closed(&self) -> Vec<String> {
                self.closed.lock().expect("closed lock").clone()
            }

            pub fn wait_closed(&self, timeout: Duration) -> Vec<String> {
                let start = std::time::Instant::now();
                loop {
                    let closed = self.closed();
                    if !closed.is_empty() || start.elapsed() >= timeout {
                        return closed;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }

        /// Starts a stub that answers each document-state question with the next
        /// entry of `states` and every other reading with `body`.
        pub fn cdp(states: &[&str], body: &str) -> Stub {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
            let port = listener.local_addr().expect("the port").port();
            let created = Arc::new(Mutex::new(Vec::new()));
            let closed = Arc::new(Mutex::new(Vec::new()));
            let seen_created = Arc::clone(&created);
            let seen_closed = Arc::clone(&closed);
            let states: VecDeque<String> = states.iter().map(|state| state.to_string()).collect();
            let body = body.to_string();

            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let head = match read_head(&mut stream) {
                        Ok(head) => head,
                        Err(_) => continue,
                    };
                    let request_line = head.lines().next().unwrap_or_default().to_string();
                    let path = request_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();

                    if path.starts_with("/json/version") {
                        let body = serde_json::json!({
                            "Browser": "stub/1",
                            "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/browser/stub"),
                        })
                        .to_string();
                        let _ = reply_http(&mut stream, 200, &body);
                    } else if path.starts_with("/json/new") {
                        seen_created
                            .lock()
                            .expect("created lock")
                            .push(request_line);
                        let body = serde_json::json!({
                            "id": "A1B2C3",
                            "type": "page",
                            "url": path,
                            "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/page/A1B2C3"),
                        })
                        .to_string();
                        let _ = reply_http(&mut stream, 200, &body);
                    } else if let Some(id) = path.strip_prefix("/json/close/") {
                        seen_closed
                            .lock()
                            .expect("closed lock")
                            .push(id.to_string());
                        let _ = reply_http(&mut stream, 200, "Target is closing");
                    } else if path.starts_with("/devtools/") {
                        let socket = Peeked {
                            prefix: head.into_bytes(),
                            offset: 0,
                            inner: stream,
                        };
                        if let Ok(mut socket) = tungstenite::accept(socket) {
                            let mut states = states.clone();
                            let _ = session(&mut socket, &mut states, &body);
                        }
                    } else {
                        let _ = reply_http(&mut stream, 404, "{}");
                    }
                }
            });

            Stub {
                port,
                created,
                closed,
            }
        }

        /// Answers one CDP session until the client hangs up.
        ///
        /// Which expression was asked decides the answer, exactly as a page
        /// would: the state probe gets the next queued state, and anything else
        /// gets the document's text.
        fn session<S: Read + Write>(
            socket: &mut WebSocket<S>,
            states: &mut VecDeque<String>,
            body: &str,
        ) -> Result<(), tungstenite::Error> {
            loop {
                let Ok(message) = socket.read() else {
                    return Ok(());
                };
                let tungstenite::Message::Text(text) = message else {
                    continue;
                };
                let Ok(request) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                let id = request
                    .get("id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let expression = request
                    .pointer("/params/expression")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();

                let answer = if expression.starts_with("(document.readyState") {
                    // The last state repeats: a page that has settled stays
                    // settled, and a test that queues one state should not get
                    // an empty answer on the second look.
                    if states.len() > 1 {
                        states.pop_front().unwrap_or_default()
                    } else {
                        states
                            .front()
                            .cloned()
                            .unwrap_or_else(|| "complete|about:|document".to_string())
                    }
                } else if expression.starts_with("(document.body") {
                    body.to_string()
                } else {
                    String::new()
                };

                let reply = serde_json::json!({
                    "id": id,
                    "result": { "result": { "type": "string", "value": answer } },
                })
                .to_string();
                socket.send(tungstenite::Message::Text(reply.into()))?;
            }
        }
    }

    fn state(answer: &str) -> DocumentState {
        DocumentState::parse(answer)
    }

    /// A port nothing is listening on, so asking a browser there cannot succeed.
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .expect("a loopback port")
            .local_addr()
            .expect("the port")
            .port()
    }

    #[test]
    fn a_reading_is_the_address_the_endpoint_named() {
        let outcome = outcome_from(
            "http://api.ipify.org",
            &state("complete|http:|document"),
            Some("203.0.113.7\n"),
        );

        assert_eq!(outcome.exit_ip(), Some("203.0.113.7"));
        assert_eq!(outcome.detail(), "left from 203.0.113.7");
    }

    #[test]
    fn an_answer_that_holds_no_address_is_not_a_reading() {
        let outcome = outcome_from(
            "http://api.ipify.org",
            &state("complete|http:|document"),
            Some("<html><body>Not Found</body></html>"),
        );

        assert_eq!(outcome.exit_ip(), None);
        let EgressOutcome::Unreadable { excerpt, .. } = &outcome else {
            panic!("an answer without an address is not a reading: {outcome:?}");
        };
        assert!(
            excerpt.contains("Not Found"),
            "the answer's own words are the evidence: {excerpt}"
        );
    }

    #[test]
    fn a_page_that_never_committed_did_not_arrive() {
        let outcome = outcome_from(
            "http://api.ipify.org",
            &state("complete|about:|document"),
            None,
        );

        let EgressOutcome::NotReached { state, .. } = &outcome else {
            panic!("a blank page did not arrive: {outcome:?}");
        };
        assert_eq!(state, "complete|about:|document", "the state is quoted");
    }

    #[test]
    fn a_page_that_committed_elsewhere_did_not_arrive() {
        // What a browser shows after a navigation it could not complete. It
        // committed, so waiting cannot help, and it is not the endpoint.
        let outcome = outcome_from(
            "http://api.ipify.org",
            &state("complete|chrome-error:|document"),
            None,
        );

        assert!(
            matches!(outcome, EgressOutcome::NotReached { .. }),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_page_at_the_endpoint_that_did_not_finish_ran_out_of_time() {
        // It reached the endpoint and is still loading: that is a slow path,
        // not a path that carries nothing, and the two are fixed differently.
        let outcome = outcome_from(
            "http://api.ipify.org",
            &state("loading|http:|document"),
            None,
        );

        let EgressOutcome::Timeout { state, .. } = &outcome else {
            panic!("a page at the endpoint ran out of time: {outcome:?}");
        };
        assert_eq!(state, "loading|http:|document");
    }

    #[test]
    fn an_answer_with_no_address_in_it_is_bounded_before_it_reaches_a_window() {
        let long = "x".repeat(READING_LIMIT * 2);
        let outcome = outcome_from(
            "http://api.ipify.org",
            &state("complete|http:|document"),
            Some(&long),
        );

        let EgressOutcome::Unreadable { excerpt, .. } = &outcome else {
            panic!("a wall of text holds no address: {outcome:?}");
        };
        assert_eq!(
            excerpt.chars().count(),
            EXCERPT_LIMIT + 1,
            "cut, and marked"
        );
        assert!(excerpt.ends_with('…'));
    }

    #[test]
    fn an_endpoint_that_is_not_plain_http_is_refused_before_anything_is_asked() {
        // Port 1 is not a browser, so anything that got as far as asking would
        // report NotAsked. Reporting the endpoint instead proves the refusal
        // happens first, and that no TLS request is ever made.
        let outcome = EgressProbe::new(Duration::from_millis(100)).read(1, "https://api.ipify.org");

        let EgressOutcome::Unusable(reason) = &outcome else {
            panic!("an https endpoint is refused, not attempted: {outcome:?}");
        };
        assert!(reason.contains("plain http"), "{reason}");
    }

    #[test]
    fn a_browser_that_cannot_be_asked_is_not_a_path_that_carried_nothing() {
        let outcome =
            EgressProbe::new(Duration::from_millis(150)).read(free_port(), "http://127.0.0.1:1/");

        let EgressOutcome::NotAsked(reason) = &outcome else {
            panic!("nothing was listening, so nothing was asked: {outcome:?}");
        };
        assert!(!reason.is_empty(), "the refusal says why");
    }

    #[test]
    fn a_matching_address_is_not_a_discrepancy() {
        let outcome = outcome_from(
            "http://api.ipify.org",
            &state("complete|http:|document"),
            Some("203.0.113.7"),
        );

        assert!(verify_egress(Some("203.0.113.7"), &outcome).is_empty());
    }

    #[test]
    fn a_different_address_is_reported_against_the_tested_one() {
        let outcome = outcome_from(
            "http://api.ipify.org",
            &state("complete|http:|document"),
            Some("198.51.100.9"),
        );

        let found = verify_egress(Some("203.0.113.7"), &outcome);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].claim, "exit address");
        assert_eq!(found[0].observed, "198.51.100.9");
        assert!(
            found[0].expected.contains("203.0.113.7") && found[0].expected.contains("tested"),
            "the expectation says where the address came from: {}",
            found[0].expected
        );
    }

    #[test]
    fn nothing_is_claimed_about_a_profile_whose_proxy_was_never_tested() {
        // A reading is information here, not a finding: the profile never made
        // a claim this could contradict.
        let read = outcome_from(
            "http://api.ipify.org",
            &state("complete|http:|document"),
            Some("198.51.100.9"),
        );
        let unreached = outcome_from(
            "http://api.ipify.org",
            &state("complete|about:|document"),
            None,
        );

        assert!(verify_egress(None, &read).is_empty());
        assert!(verify_egress(None, &unreached).is_empty());
    }

    #[test]
    fn not_reaching_the_endpoint_is_a_finding_against_a_tested_proxy() {
        // The proxy carried a request during the pre-flight; this browser's
        // traffic did not come out of anywhere that could reach the endpoint.
        let outcome = outcome_from(
            "http://api.ipify.org",
            &state("complete|about:|document"),
            None,
        );

        let found = verify_egress(Some("203.0.113.7"), &outcome);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].claim, "exit address");
        assert!(
            found[0].observed.contains("never committed"),
            "the finding says what happened instead: {}",
            found[0].observed
        );
    }

    #[test]
    fn the_body_expression_is_bounded_and_asks_the_page_for_nothing_new() {
        let expression = body_expression();
        assert!(
            expression.contains(&format!("slice(0,{READING_LIMIT})")),
            "an answer is read up to the bound this module states: {expression}"
        );
        for forbidden in ["fetch(", "XMLHttpRequest", "WebSocket", "importScripts"] {
            assert!(
                !expression.contains(forbidden),
                "the request that matters is the one the browser already made: {forbidden}"
            );
        }
    }

    /// The whole read, against an endpoint that speaks both of the protocols a
    /// browser does: readiness over HTTP, the session over a WebSocket.
    #[test]
    fn a_read_back_opens_a_page_of_its_own_at_the_endpoint_and_closes_it() {
        let stub = stub::cdp(
            &["loading|about:|empty", "complete|http:|document"],
            "203.0.113.7\n",
        );

        let outcome =
            EgressProbe::new(Duration::from_secs(5)).read(stub.port, "http://example.test/whoami");

        assert_eq!(outcome.exit_ip(), Some("203.0.113.7"), "{outcome:?}");
        let created = stub.created();
        assert_eq!(created.len(), 1, "one page, opened once: {created:?}");
        assert!(
            created[0].starts_with("PUT /json/new?"),
            "a page of its own is created, not one the user has open: {}",
            created[0]
        );
        assert!(
            created[0].contains("http://example.test/whoami"),
            "the endpoint is what the browser is sent to: {}",
            created[0]
        );
        assert_eq!(
            stub.closed(),
            vec!["A1B2C3".to_string()],
            "the tab is closed again, whether or not the reading worked"
        );
    }

    /// A page that committed somewhere other than where it was sent has
    /// finished: waiting longer cannot turn it into an arrival, so the wait ends
    /// as soon as the page says so rather than at the deadline.
    #[test]
    fn a_page_that_committed_elsewhere_ends_the_wait_without_a_deadline() {
        let stub = stub::cdp(&["complete|chrome-error:|document"], "");
        let started = Instant::now();

        let outcome =
            EgressProbe::new(Duration::from_secs(30)).read(stub.port, "http://example.test/");

        assert!(
            matches!(outcome, EgressOutcome::NotReached { .. }),
            "{outcome:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a settled page should not be waited out: {:?}",
            started.elapsed()
        );
        assert_eq!(
            stub.wait_closed(Duration::from_secs(1)).len(),
            1,
            "a failed page is closed too"
        );
    }

    /// A page still on the empty document cannot say whether it is about to
    /// arrive, so this one is waited out - and still reports where it never got
    /// to, with the state it was in.
    #[test]
    fn a_page_still_on_the_empty_document_ran_out_of_time() {
        let stub = stub::cdp(&["complete|about:|document"], "");

        let outcome =
            EgressProbe::new(Duration::from_millis(400)).read(stub.port, "http://example.test/");

        let EgressOutcome::NotReached { state, url } = &outcome else {
            panic!("a blank page never reached the endpoint: {outcome:?}");
        };
        assert_eq!(url, "http://example.test/");
        assert!(state.contains("about:"), "the state is quoted: {state}");
    }

    /// The answer is read from the page the browser loaded, so an endpoint that
    /// answers with prose is not confused with one that named an address.
    #[test]
    fn an_endpoint_that_answers_with_prose_is_unreadable_rather_than_an_address() {
        let stub = stub::cdp(
            &["complete|http:|document"],
            "<html><body>429 Too Many Requests</body></html>",
        );

        let outcome =
            EgressProbe::new(Duration::from_secs(5)).read(stub.port, "http://example.test/");

        let EgressOutcome::Unreadable { excerpt, .. } = &outcome else {
            panic!("prose holds no address: {outcome:?}");
        };
        assert!(excerpt.contains("Too Many Requests"), "{excerpt}");
        assert_eq!(stub.wait_closed(Duration::from_secs(1)).len(), 1);
    }
}
