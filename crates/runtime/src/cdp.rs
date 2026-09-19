use crate::error::CdpError;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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

pub trait CdpProbe: Send + Sync {
    fn wait_ready(&self, port: u16, timeout: Duration) -> Result<CdpInfo, CdpError>;

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
        use std::net::{Ipv4Addr, SocketAddr, TcpStream};
        let deadline = std::time::Instant::now() + timeout;
        let info = self.wait_ready(port, timeout)?;
        // CDP metadata must never redirect the close request off loopback.
        let prefix = format!("ws://127.0.0.1:{port}/");
        if !info.web_socket_debugger_url.starts_with(&prefix) {
            return Err(CdpError::InvalidResponse(
                "non-loopback CDP WebSocket".into(),
            ));
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(CdpError::Timeout {
                timeout_secs: timeout.as_secs(),
            });
        }
        let stream =
            TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), remaining)
                .map_err(|e| CdpError::Http(e.to_string()))?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| CdpError::Http(e.to_string()))?;
        stream
            .set_write_timeout(Some(remaining))
            .map_err(|e| CdpError::Http(e.to_string()))?;
        let (mut socket, _) = tungstenite::client(info.web_socket_debugger_url.as_str(), stream)
            .map_err(|e| CdpError::Http(e.to_string()))?;
        socket
            .send(tungstenite::Message::Text(
                r#"{"id":1,"method":"Browser.close"}"#.into(),
            ))
            .map_err(|e| CdpError::Http(e.to_string()))
    }

    fn wait_ready(&self, port: u16, timeout: Duration) -> Result<CdpInfo, CdpError> {
        let start = std::time::Instant::now();
        let url = format!("http://127.0.0.1:{port}/json/version");

        while start.elapsed() < timeout {
            let remaining = timeout.saturating_sub(start.elapsed());
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .proxy(None)
                .timeout_global(Some(remaining))
                .build()
                .into();
            let parsed_info = agent
                .get(&url)
                .call()
                .ok()
                .and_then(|mut r| r.body_mut().read_to_string().ok())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

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
