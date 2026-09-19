use crate::error::ProxyError;
use domain::{ProxyOutbound, ProxyProfile};
use serde_json::json;
use std::fs;
use std::path::Path;

pub trait XrayConfigBuilder: Send + Sync {
    fn build(
        &self,
        proxy: &ProxyProfile,
        local_socks_port: u16,
        output_path: &Path,
    ) -> Result<(), ProxyError>;
}

#[derive(Debug, Default)]
pub struct DefaultXrayConfigBuilder;

impl DefaultXrayConfigBuilder {
    pub fn new() -> Self {
        Self
    }
}

impl XrayConfigBuilder for DefaultXrayConfigBuilder {
    fn build(
        &self,
        proxy: &ProxyProfile,
        local_socks_port: u16,
        output_path: &Path,
    ) -> Result<(), ProxyError> {
        let (host, port, username, password) = match &proxy.outbound {
            ProxyOutbound::Socks5(s) => (&s.host, s.port, &s.username, &s.password),
            ProxyOutbound::Http(h) => (&h.host, h.port, &h.username, &h.password),
            _ => {
                return Err(ProxyError::UnsupportedOutbound(
                    "phase 3 supports SOCKS5 and HTTP only".into(),
                ));
            }
        };
        if host.trim().is_empty()
            || port == 0
            || local_socks_port == 0
            || username.is_some() != password.is_some()
        {
            return Err(ProxyError::Other(
                "invalid proxy host, port or incomplete credentials".into(),
            ));
        }
        let inbound = json!({
            "listen": "127.0.0.1",
            "port": local_socks_port,
            "protocol": "socks",
            "settings": {
                "auth": "noauth",
                "udp": true
            }
        });

        let outbound = match &proxy.outbound {
            ProxyOutbound::Socks5(s) => {
                let mut user_list = Vec::new();
                if let (Some(u), Some(p)) = (&s.username, &s.password) {
                    user_list.push(json!({ "user": u, "pass": p }));
                }
                json!({
                    "protocol": "socks",
                    "settings": {
                        "servers": [{
                            "address": s.host,
                            "port": s.port,
                            "users": user_list
                        }]
                    }
                })
            }
            ProxyOutbound::Http(h) => {
                let mut user_list = Vec::new();
                if let (Some(u), Some(p)) = (&h.username, &h.password) {
                    user_list.push(json!({ "user": u, "pass": p }));
                }
                json!({
                    "protocol": "http",
                    "settings": {
                        "servers": [{
                            "address": h.host,
                            "port": h.port,
                            "users": user_list
                        }]
                    }
                })
            }
            _ => {
                return Err(ProxyError::UnsupportedOutbound(
                    "phase 3 supports SOCKS5 and HTTP only".into(),
                ));
            }
        };

        let config = json!({
            "log": {
                "loglevel": "warning"
            },
            "inbounds": [inbound],
            "outbounds": [outbound]
        });

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let config_str = serde_json::to_string_pretty(&config)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(output_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        std::io::Write::write_all(&mut file, config_str.as_bytes())?;

        Ok(())
    }
}

/// Require a live child and a listening loopback endpoint before launching Chromium.
pub(crate) fn wait_ready(
    child: &mut std::process::Child,
    port: u16,
    timeout: std::time::Duration,
    mut poll_sessions: impl FnMut(),
) -> Result<(), ProxyError> {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::{Duration, Instant};
    let start = Instant::now();
    loop {
        poll_sessions();
        if let Some(status) = child.try_wait()? {
            return Err(ProxyError::Other(format!(
                "Xray exited before readiness: {status}"
            )));
        }
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(ProxyError::Other("Xray SOCKS readiness timed out".into()));
        }
        if TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            remaining.min(Duration::from_millis(100)),
        )
        .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(remaining.min(Duration::from_millis(25)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{HttpOutbound, ProxyId, Socks5Outbound};

    #[test]
    fn socks_and_http_configs_preserve_credentials_and_bind_loopback() {
        for outbound in [
            ProxyOutbound::Socks5(Socks5Outbound {
                host: "proxy.example".into(),
                port: 1080,
                username: Some("alice".into()),
                password: Some("secret".into()),
            }),
            ProxyOutbound::Http(HttpOutbound {
                host: "proxy.example".into(),
                port: 8080,
                username: Some("alice".into()),
                password: Some("secret".into()),
            }),
        ] {
            let protocol = if matches!(outbound, ProxyOutbound::Http(_)) {
                "http"
            } else {
                "socks"
            };
            let proxy = ProxyProfile {
                id: ProxyId::new(),
                name: "test".into(),
                outbound,
            };
            let path = std::env::temp_dir().join(format!("fp-xray-{}.json", proxy.id));
            DefaultXrayConfigBuilder
                .build(&proxy, 12345, &path)
                .unwrap();
            let config: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(config["inbounds"][0]["listen"], "127.0.0.1");
            assert_eq!(config["inbounds"][0]["port"], 12345);
            assert_eq!(config["outbounds"][0]["protocol"], protocol);
            assert_eq!(
                config["outbounds"][0]["settings"]["servers"][0]["users"][0],
                json!({"user":"alice", "pass":"secret"})
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn incomplete_credentials_are_rejected_without_writing_config() {
        let proxy = ProxyProfile {
            id: ProxyId::new(),
            name: "test".into(),
            outbound: ProxyOutbound::Http(HttpOutbound {
                host: "proxy.example".into(),
                port: 8080,
                username: Some("alice".into()),
                password: None,
            }),
        };
        let path = std::env::temp_dir().join(format!("fp-xray-{}.json", proxy.id));
        assert!(
            DefaultXrayConfigBuilder
                .build(&proxy, 12345, &path)
                .is_err()
        );
        assert!(!path.exists());
    }
}
