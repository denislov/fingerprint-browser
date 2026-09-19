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
}

#[derive(Debug, Default)]
pub struct HttpCdpProbe;

impl HttpCdpProbe {
    pub fn new() -> Self {
        Self
    }
}

impl CdpProbe for HttpCdpProbe {
    fn wait_ready(&self, port: u16, timeout: Duration) -> Result<CdpInfo, CdpError> {
        let start = std::time::Instant::now();
        let url = format!("http://127.0.0.1:{port}/json/version");

        while start.elapsed() < timeout {
            let parsed_info = ureq::get(&url)
                .call()
                .ok()
                .and_then(|mut r| r.body_mut().read_to_string().ok())
                .and_then(|body| serde_json::from_str::<CdpInfo>(&body).ok());

            if let Some(info) = parsed_info {
                return Ok(info);
            }
            std::thread::sleep(Duration::from_millis(150));
        }

        Err(CdpError::Timeout {
            timeout_secs: timeout.as_secs(),
        })
    }
}
