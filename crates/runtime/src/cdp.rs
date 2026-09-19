use crate::error::CdpError;
use serde::{Deserialize, Serialize};
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::WebSocket;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CdpInfo {
    #[serde(rename = "Browser", default)]
    pub browser: String,
    #[serde(rename = "Protocol-Version", default)]
    pub protocol_version: String,
    #[serde(rename = "User-Agent", default)]
    pub user_agent: String,
    #[serde(rename = "V8-Version", default)]
    pub v8_version: String,
    #[serde(rename = "webSocketDebuggerUrl", default)]
    pub web_socket_debugger_url: String,
}

/// One entry of the `/json/list` page listing.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CdpTarget {
    /// Present on `/json/new` replies and on `/json/list` entries.
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default)]
    pub target_type: String,
    #[serde(default)]
    pub url: String,
    #[serde(rename = "webSocketDebuggerUrl", default)]
    pub web_socket_debugger_url: String,
}

impl CdpTarget {
    /// The browser's id for this target, empty when it did not report one.
    pub fn target_id(&self) -> &str {
        &self.id
    }
}

/// A debugger URL that points somewhere other than the loopback port we asked
/// about must never be dialled: CDP hands out code execution.
fn loopback_websocket_url(port: u16, url: &str) -> Result<String, CdpError> {
    let prefix = format!("ws://127.0.0.1:{port}/");
    if !url.starts_with(&prefix) {
        return Err(CdpError::InvalidResponse(
            "non-loopback CDP WebSocket".into(),
        ));
    }
    Ok(url.to_string())
}

fn connect(websocket_url: &str, timeout: Duration) -> Result<WebSocket<TcpStream>, CdpError> {
    use std::net::{Ipv4Addr, SocketAddr};
    let port: u16 = websocket_url
        .split('/')
        .nth(2)
        .and_then(|authority| authority.rsplit_once(':'))
        .and_then(|(_, port)| port.parse().ok())
        .ok_or_else(|| CdpError::InvalidResponse("CDP URL without a port".into()))?;
    let stream =
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), timeout)
            .map_err(|e| CdpError::Http(e.to_string()))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| CdpError::Http(e.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| CdpError::Http(e.to_string()))?;
    let (socket, _) =
        tungstenite::client(websocket_url, stream).map_err(|e| CdpError::Http(e.to_string()))?;
    Ok(socket)
}

/// A live CDP connection to one page target.
///
/// Used to read the fingerprint back out of a running browser: the supervisor
/// can only report the switches it asked for, and a switch the engine ignores
/// looks identical to one it honours until something asks the page.
pub struct CdpSession {
    socket: WebSocket<TcpStream>,
    next_id: u64,
}

impl std::fmt::Debug for CdpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdpSession")
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl CdpSession {
    /// Dial a debugger URL that the caller already resolved.
    pub fn connect(websocket_url: &str, timeout: Duration) -> Result<Self, CdpError> {
        let socket = connect(websocket_url, timeout)?;
        Ok(Self { socket, next_id: 1 })
    }

    /// Dial a page target the caller already created.
    pub fn connect_page(
        port: u16,
        target: &CdpTarget,
        timeout: Duration,
    ) -> Result<Self, CdpError> {
        let url = loopback_websocket_url(port, &target.web_socket_debugger_url)?;
        Self::connect(&url, timeout)
    }

    /// Resolve the first page target of a running browser and dial it.
    pub fn open_page(port: u16, timeout: Duration) -> Result<Self, CdpError> {
        let target = HttpCdpProbe::new()
            .page_target(port, timeout)?
            .ok_or(CdpError::NoPageTarget)?;
        let url = loopback_websocket_url(port, &target.web_socket_debugger_url)?;
        Self::connect(&url, timeout)
    }

    /// Navigate the session's page and wait for the document to load.
    ///
    /// The user agent data surface is only exposed on a real document: it is
    /// absent on `about:blank` and `data:` URLs, so a reading taken without a
    /// navigation cannot see the brand claims at all.
    pub fn navigate(&mut self, url: &str, timeout: Duration) -> Result<(), CdpError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "id": id,
            "method": "Page.navigate",
            "params": { "url": url }
        });
        self.send_text(&request.to_string())?;
        self.await_reply(id, timeout)?;

        let scheme = url.split(':').next().unwrap_or_default().to_string();
        self.wait_for_document(&format!("{scheme}:"), timeout)
    }

    /// Wait until the page holds the loaded document of `scheme`.
    ///
    /// `readyState` alone is not enough: a brand new target reports `complete`
    /// for the empty document it starts with, before the navigation that opened
    /// it has committed. Waiting for the scheme as well means the answer is
    /// about the document the caller asked for, not the placeholder.
    pub fn wait_for_document(&mut self, scheme: &str, timeout: Duration) -> Result<(), CdpError> {
        let expression = "(document.readyState+'|'+location.protocol+'|'+             (document.documentElement?'document':'empty'))";
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let observed = self.evaluate(expression, timeout)?;
            let observed = observed.as_str().unwrap_or_default();
            if observed == format!("complete|{scheme}|document") {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout {
                    timeout_secs: timeout.as_secs(),
                });
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Evaluate an expression in the page and return its value.
    ///
    /// `awaitPromise` is on, so an expression that returns a promise resolves
    /// before the call returns.
    pub fn evaluate(
        &mut self,
        expression: &str,
        timeout: Duration,
    ) -> Result<serde_json::Value, CdpError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
            }
        });
        self.send_text(&request.to_string())?;

        let result = self.await_reply(id, timeout)?;
        if let Some(exception) = result.get("exceptionDetails") {
            let described = exception
                .pointer("/exception/description")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let text = described.unwrap_or_else(|| exception.to_string());
            return Err(CdpError::Evaluation(text));
        }
        Ok(result
            .get("result")
            .and_then(|value| value.get("value"))
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    /// Waits for the reply with `id`, ignoring events and other replies.
    fn await_reply(&mut self, id: u64, timeout: Duration) -> Result<serde_json::Value, CdpError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let message = self.read_text(deadline)?;
            let parsed: serde_json::Value = serde_json::from_str(&message)
                .map_err(|e| CdpError::InvalidResponse(e.to_string()))?;
            // Events carry no id; only a reply to our request ends the wait.
            if parsed.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = parsed.get("error") {
                return Err(CdpError::Evaluation(error.to_string()));
            }
            return Ok(parsed.get("result").cloned().unwrap_or_default());
        }
    }

    fn send_text(&mut self, text: &str) -> Result<(), CdpError> {
        self.socket
            .send(tungstenite::Message::Text(text.to_string().into()))
            .map_err(|e| CdpError::Http(e.to_string()))
    }

    fn read_text(&mut self, deadline: std::time::Instant) -> Result<String, CdpError> {
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(CdpError::Timeout { timeout_secs: 1 });
            }
            let _ = self.socket.get_ref().set_read_timeout(Some(remaining));
            match self.socket.read() {
                Ok(tungstenite::Message::Text(text)) => return Ok(text.to_string()),
                Ok(_) => continue,
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(CdpError::Timeout { timeout_secs: 1 });
                }
                Err(e) => return Err(CdpError::Http(e.to_string())),
            }
        }
    }
}

pub trait CdpProbe: Send + Sync {
    fn wait_ready(&self, port: u16, timeout: Duration) -> Result<CdpInfo, CdpError>;

    /// The page targets a running browser exposes.
    fn page_targets(&self, _port: u16, _timeout: Duration) -> Result<Vec<CdpTarget>, CdpError> {
        Err(CdpError::InvalidResponse(
            "target listing unavailable".into(),
        ))
    }

    /// Opens a new page target at `url`, as the browser's own HTTP endpoint
    /// does. Used so a probe can run somewhere the user is not looking.
    fn create_page(
        &self,
        _port: u16,
        _url: &str,
        _timeout: Duration,
    ) -> Result<CdpTarget, CdpError> {
        Err(CdpError::InvalidResponse(
            "page creation unavailable".into(),
        ))
    }

    /// Closes a page target previously returned by [`Self::create_page`].
    fn close_page(&self, _port: u16, _target_id: &str, _timeout: Duration) -> Result<(), CdpError> {
        Err(CdpError::InvalidResponse("page closing unavailable".into()))
    }

    fn page_target(&self, port: u16, timeout: Duration) -> Result<Option<CdpTarget>, CdpError> {
        Ok(self
            .page_targets(port, timeout)?
            .into_iter()
            .find(|target| target.target_type == "page"))
    }

    fn close_browser(&self, _port: u16, _timeout: Duration) -> Result<(), CdpError> {
        Err(CdpError::InvalidResponse(
            "graceful close unavailable".into(),
        ))
    }
}

#[derive(Debug, Default)]
pub struct HttpCdpProbe;

impl HttpCdpProbe {
    pub fn new() -> Self {
        Self
    }
}

impl CdpProbe for HttpCdpProbe {
    fn close_browser(&self, port: u16, timeout: Duration) -> Result<(), CdpError> {
        use std::net::{Ipv4Addr, SocketAddr};
        let info = self.wait_ready(port, timeout)?;
        // CDP metadata must never redirect the close request off loopback.
        let url = loopback_websocket_url(port, &info.web_socket_debugger_url)?;
        let stream =
            TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), timeout)
                .map_err(|e| CdpError::Http(e.to_string()))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| CdpError::Http(e.to_string()))?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|e| CdpError::Http(e.to_string()))?;
        let (mut socket, _) =
            tungstenite::client(url.as_str(), stream).map_err(|e| CdpError::Http(e.to_string()))?;
        socket
            .send(tungstenite::Message::Text(
                r#"{"id":1,"method":"Browser.close"}"#.into(),
            ))
            .map_err(|e| CdpError::Http(e.to_string()))
    }

    fn page_targets(&self, port: u16, timeout: Duration) -> Result<Vec<CdpTarget>, CdpError> {
        let body = self
            .get_json(port, "list", timeout)
            .ok_or(CdpError::NoPageTarget)?;
        serde_json::from_str(&body).map_err(|e| CdpError::InvalidResponse(e.to_string()))
    }

    fn create_page(&self, port: u16, url: &str, timeout: Duration) -> Result<CdpTarget, CdpError> {
        let body = self
            .call_json(
                port,
                &format!("new?{}", encode_target_url(url)),
                true,
                timeout,
            )
            .ok_or_else(|| CdpError::InvalidResponse("page creation refused".into()))?;
        let target: CdpTarget =
            serde_json::from_str(&body).map_err(|e| CdpError::InvalidResponse(e.to_string()))?;
        if target.target_id().is_empty() || target.web_socket_debugger_url.is_empty() {
            return Err(CdpError::InvalidResponse(
                "page creation returned no target".into(),
            ));
        }
        loopback_websocket_url(port, &target.web_socket_debugger_url)?;
        Ok(target)
    }

    fn close_page(&self, port: u16, target_id: &str, timeout: Duration) -> Result<(), CdpError> {
        // The id comes from the browser; refuse anything that is not id shaped
        // rather than putting it into a request path.
        if target_id.is_empty() || !target_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CdpError::InvalidResponse("bad page target id".into()));
        }
        self.call_json(port, &format!("close/{target_id}"), false, timeout)
            .map(|_| ())
            .ok_or_else(|| CdpError::InvalidResponse("page close refused".into()))
    }

    fn wait_ready(&self, port: u16, timeout: Duration) -> Result<CdpInfo, CdpError> {
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            let remaining = timeout.saturating_sub(start.elapsed());
            let parsed_info = self
                .get_json(port, "version", remaining)
                .and_then(|body| serde_json::from_str::<CdpInfo>(&body).ok());

            if let Some(info) = parsed_info
                .filter(|info| !info.browser.is_empty() && !info.web_socket_debugger_url.is_empty())
            {
                return Ok(info);
            }
            std::thread::sleep(
                timeout
                    .saturating_sub(start.elapsed())
                    .min(Duration::from_millis(150)),
            );
        }

        Err(CdpError::Timeout {
            timeout_secs: timeout.as_secs(),
        })
    }
}

impl HttpCdpProbe {
    /// One bounded loopback GET of the CDP metadata endpoints.
    fn get_json(&self, port: u16, endpoint: &str, timeout: Duration) -> Option<String> {
        self.call_json(port, endpoint, false, timeout)
    }

    /// One bounded loopback request to a CDP metadata endpoint.
    ///
    /// Only loopback is ever addressed, and the agent ignores any configured
    /// proxy: this is the browser's own control endpoint, not traffic.
    fn call_json(&self, port: u16, endpoint: &str, put: bool, timeout: Duration) -> Option<String> {
        let url = format!("http://127.0.0.1:{port}/json/{endpoint}");
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(None)
            .timeout_global(Some(timeout))
            .build()
            .into();
        // The create endpoint needs a bodyless PUT; the rest are GETs.
        let response = if put {
            agent.put(&url).send_empty()
        } else {
            agent.get(&url).call()
        };
        response
            .ok()
            .and_then(|mut response| response.body_mut().read_to_string().ok())
    }
}

/// Percent-encodes a URL for use as a query value, keeping the characters that
/// make it still a URL.
fn encode_target_url(url: &str) -> String {
    let mut encoded = String::with_capacity(url.len());
    for byte in url.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b':'
            | b'/'
            | b'?'
            | b'='
            | b'&' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    /// Serves one CDP WebSocket session and returns the port it listens on.
    ///
    /// `reply` receives each request text and returns the frames to answer
    /// with, so a test can send events before the reply.
    fn fake_cdp_server(reply: impl Fn(&str) -> Vec<String> + Send + 'static) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = match tungstenite::accept(stream) {
                Ok(socket) => socket,
                Err(_) => return,
            };
            while let Ok(message) = socket.read() {
                let tungstenite::Message::Text(text) = message else {
                    break;
                };
                for frame in reply(&text) {
                    if socket
                        .send(tungstenite::Message::Text(frame.into()))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
        port
    }

    #[test]
    fn evaluation_skips_events_and_returns_the_reply_value() {
        let port = fake_cdp_server(|request| {
            let id = serde_json::from_str::<serde_json::Value>(request).unwrap()["id"].clone();
            vec![
                // An event the session must not mistake for its reply.
                r#"{"method":"Runtime.consoleAPICalled","params":{}}"#.to_string(),
                serde_json::json!({
                    "id": id,
                    "result": {"result": {"type": "string", "value": "{\"platform\":\"Win32\"}"}}
                })
                .to_string(),
            ]
        });

        let mut session = CdpSession::connect(
            &format!("ws://127.0.0.1:{port}/devtools/page/1"),
            Duration::from_secs(5),
        )
        .unwrap();
        let value = session
            .evaluate("1", Duration::from_secs(5))
            .expect("evaluation succeeds");

        assert_eq!(value.as_str(), Some("{\"platform\":\"Win32\"}"));
    }

    #[test]
    fn a_page_exception_is_reported_instead_of_a_value() {
        let port = fake_cdp_server(|request| {
            let id = serde_json::from_str::<serde_json::Value>(request).unwrap()["id"].clone();
            vec![
                serde_json::json!({
                    "id": id,
                    "result": {
                        "result": {"type": "undefined"},
                        "exceptionDetails": {
                            "text": "Uncaught",
                            "exception": {"description": "TypeError: nope"}
                        }
                    }
                })
                .to_string(),
            ]
        });

        let mut session = CdpSession::connect(
            &format!("ws://127.0.0.1:{port}/devtools/page/1"),
            Duration::from_secs(5),
        )
        .unwrap();
        let error = session
            .evaluate("boom", Duration::from_secs(5))
            .expect_err("exception surfaces");

        assert!(
            matches!(&error, CdpError::Evaluation(text) if text.contains("TypeError")),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_silent_peer_cannot_hold_the_evaluation_past_its_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Complete the handshake, read the request, never answer it.
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = match tungstenite::accept(stream) {
                Ok(socket) => socket,
                Err(_) => return,
            };
            let _ = socket.read();
            std::thread::sleep(Duration::from_secs(3));
        });

        let mut session = CdpSession::connect(
            &format!("ws://127.0.0.1:{port}/devtools/page/1"),
            Duration::from_secs(2),
        )
        .expect("the handshake completes");
        let start = Instant::now();
        let error = session
            .evaluate("1", Duration::from_millis(300))
            .expect_err("deadline is enforced");

        assert!(matches!(error, CdpError::Timeout { .. }), "{error}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_peer_that_never_completes_the_handshake_fails_the_dial() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_secs(3));
        });

        let start = Instant::now();
        let error = CdpSession::connect(
            &format!("ws://127.0.0.1:{port}/devtools/page/1"),
            Duration::from_millis(400),
        )
        .expect_err("a silent peer cannot hold the dial open");

        assert!(matches!(error, CdpError::Http(_)), "{error}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_debugger_url_off_loopback_is_refused() {
        let error = loopback_websocket_url(9222, "ws://evil.example:9222/devtools/page/1")
            .expect_err("only the probed loopback port is dialled");

        assert!(matches!(error, CdpError::InvalidResponse(_)));
        assert!(loopback_websocket_url(9222, "ws://127.0.0.1:9222/devtools/page/1").is_ok());
    }

    #[test]
    fn unresponsive_http_server_cannot_exceed_readiness_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Keep the listener alive without servicing requests.
        let start = Instant::now();
        assert!(
            HttpCdpProbe
                .wait_ready(port, Duration::from_millis(100))
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_browser_without_a_page_target_reports_no_page_target() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let _ = stream.read(&mut [0; 1024]);
                        let body = r#"[{"type":"service_worker","webSocketDebuggerUrl":"ws://127.0.0.1:1/x"}]"#;
                        let _ = stream.write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        );
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        });

        let error = CdpSession::open_page(port, Duration::from_millis(500))
            .expect_err("a browser with no page target cannot be read");
        assert!(matches!(error, CdpError::NoPageTarget), "{error}");
        server.join().unwrap();
    }

    /// Serves the CDP metadata endpoints as `method path` -> body.
    fn fake_metadata_server(
        reply: impl Fn(&str, &str, u16) -> Option<(u16, String)> + Send + 'static,
    ) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let mut buffer = [0u8; 2048];
                        let read = stream.read(&mut buffer).unwrap_or(0);
                        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                        let mut parts = request.split_whitespace();
                        let method = parts.next().unwrap_or_default().to_string();
                        let path = parts.next().unwrap_or_default().to_string();
                        let (status, body) =
                            reply(&method, &path, port).unwrap_or((404, "{}".into()));
                        let _ = stream.write_all(
                            format!(
                                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        );
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        port
    }

    #[test]
    fn a_page_is_created_with_put_and_closed_by_id() {
        let port = fake_metadata_server(|method, path, port| {
            if method == "PUT" && path.starts_with("/json/new?file:///tmp/probe%20one.html") {
                Some((
                    200,
                    format!(
                        r#"{{"id":"A1B2C3","type":"page","url":"file:///tmp/probe one.html","webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/page/A1B2C3"}}"#
                    ),
                ))
            } else if method == "GET" && path == "/json/close/A1B2C3" {
                Some((200, "Target is closing".into()))
            } else {
                None
            }
        });

        let created = HttpCdpProbe
            .create_page(port, "file:///tmp/probe one.html", Duration::from_secs(2))
            .expect("the create endpoint answers");
        assert_eq!(created.target_id(), "A1B2C3");
        HttpCdpProbe
            .close_page(port, created.target_id(), Duration::from_secs(2))
            .expect("the close endpoint answers");
    }

    #[test]
    fn a_target_id_that_is_not_id_shaped_is_never_put_in_a_path() {
        let error = HttpCdpProbe
            .close_page(9222, "../version", Duration::from_secs(1))
            .expect_err("a path cannot be smuggled through the target id");

        assert!(matches!(error, CdpError::InvalidResponse(_)), "{error}");
    }

    #[test]
    fn a_debugger_url_off_loopback_is_not_dialled_after_creation() {
        let port = fake_metadata_server(|method, path, _| {
            (method == "PUT" && path.starts_with("/json/new")).then(|| {
                (
                    200,
                    r#"{"id":"AA","type":"page","webSocketDebuggerUrl":"ws://evil.example:1/x"}"#
                        .to_string(),
                )
            })
        });

        let error = HttpCdpProbe
            .create_page(port, "file:///tmp/probe.html", Duration::from_secs(2))
            .expect_err("a hijacked debugger url must not be handed back");

        assert!(matches!(error, CdpError::InvalidResponse(_)), "{error}");
    }

    #[test]
    fn a_target_url_is_percent_encoded_for_the_create_query() {
        assert_eq!(
            encode_target_url("file:///tmp/probe one.html?a=1"),
            "file:///tmp/probe%20one.html?a=1"
        );
        assert_eq!(encode_target_url("file:///t#x"), "file:///t%23x");
    }

    #[test]
    fn empty_json_is_not_ready() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let _ = stream.read(&mut [0; 1024]);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
        });
        assert!(
            HttpCdpProbe
                .wait_ready(port, Duration::from_millis(100))
                .is_err()
        );
        server.join().unwrap();
    }
}
