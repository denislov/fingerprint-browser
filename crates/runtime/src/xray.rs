use crate::error::ProxyError;
use domain::{ProxyOutbound, ProxyProfile, StreamSettings};
use serde_json::json;
use std::fs;
use std::path::Path;

/// The temporary config a profile's Xray is started with, written inside that
/// profile's directory under the runtime directory. It holds the upstream
/// credentials, so a run that is killed has to clear it on the way back in.
pub const XRAY_CONFIG_FILE: &str = "xray.json";

/// Whether the editor is allowed to offer this protocol and the proxy service
/// to store it.
///
/// This is about the *form*, not about the config: [`DefaultXrayConfigBuilder`]
/// builds all six protocols, but the proxy editor cannot fill in the stream
/// settings four of them carry yet, so nothing offers them until it can. A
/// protocol with no form would be a row a user could create and never complete.
pub fn is_supported(outbound: &ProxyOutbound) -> bool {
    matches!(outbound, ProxyOutbound::Socks5(_) | ProxyOutbound::Http(_))
}

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
        if local_socks_port == 0 {
            return Err(ProxyError::Other("invalid local SOCKS port".into()));
        }
        // The same rules the editor refuses a save on, so what reaches the
        // engine is what was checked. A config built here and rejected at
        // startup would be a start failure with a worse message.
        domain::validate_proxy(proxy)
            .map_err(|error| ProxyError::Other(format!("the proxy cannot be launched: {error}")))?;
        let inbound = json!({
            "listen": "127.0.0.1",
            "port": local_socks_port,
            "protocol": "socks",
            "settings": {
                "auth": "noauth",
                "udp": true
            }
        });

        let mut outbound = match &proxy.outbound {
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
            ProxyOutbound::Shadowsocks(s) => json!({
                "protocol": "shadowsocks",
                "settings": {
                    "servers": [{
                        "address": s.host,
                        "port": s.port,
                        "method": s.method,
                        "password": s.password
                    }]
                }
            }),
            ProxyOutbound::Vmess(v) => json!({
                "protocol": "vmess",
                "settings": {
                    "vnext": [{
                        "address": v.host,
                        "port": v.port,
                        "users": [{
                            "id": v.uuid,
                            "security": v.security
                        }]
                    }]
                }
            }),
            ProxyOutbound::Vless(v) => json!({
                "protocol": "vless",
                "settings": {
                    "vnext": [{
                        "address": v.host,
                        "port": v.port,
                        "users": [{
                            "id": v.uuid,
                            "flow": v.flow.clone().unwrap_or_default(),
                            "encryption": v.encryption
                        }]
                    }]
                }
            }),
            ProxyOutbound::Trojan(t) => json!({
                "protocol": "trojan",
                "settings": {
                    "servers": [{
                        "address": t.host,
                        "port": t.port,
                        "password": t.password
                    }]
                }
            }),
        };

        if let Some(stream) = proxy.outbound.stream().and_then(stream_json) {
            outbound["streamSettings"] = stream;
        }

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

/// The `streamSettings` object, or `None` when nothing but the engine's own
/// defaults would be written.
///
/// Every name here was checked against Xray 26.2.6 with `xray run -test`: the
/// shapes below are accepted, and the contradictions domain validation refuses
/// (REALITY over ws) are refused by that same command. One name is deliberately
/// absent - `allowInsecure` has been *removed* from that engine (it exits 23 and
/// says to use `pinnedPeerCertSha256`), so there is no way to express "do not
/// check the certificate" here.
fn stream_json(stream: &StreamSettings) -> Option<serde_json::Value> {
    if stream.is_plain() {
        return None;
    }

    let mut settings = serde_json::Map::new();
    settings.insert("network".into(), json!(stream.network.as_str()));
    settings.insert("security".into(), json!(stream.security.as_str()));

    if let Some(tls) = &stream.tls {
        let mut tls_json = serde_json::Map::new();
        if let Some(server_name) = &tls.server_name {
            tls_json.insert("serverName".into(), json!(server_name));
        }
        if let Some(fingerprint) = &tls.fingerprint {
            tls_json.insert("fingerprint".into(), json!(fingerprint));
        }
        if !tls.alpn.is_empty() {
            tls_json.insert("alpn".into(), json!(tls.alpn));
        }
        settings.insert("tlsSettings".into(), json!(tls_json));
    }

    if let Some(reality) = &stream.reality {
        let mut reality_json = serde_json::Map::new();
        if let Some(server_name) = &reality.server_name {
            reality_json.insert("serverName".into(), json!(server_name));
        }
        reality_json.insert("publicKey".into(), json!(reality.public_key));
        if let Some(short_id) = &reality.short_id {
            reality_json.insert("shortId".into(), json!(short_id));
        }
        if let Some(fingerprint) = &reality.fingerprint {
            reality_json.insert("fingerprint".into(), json!(fingerprint));
        }
        if let Some(spider_x) = &reality.spider_x {
            reality_json.insert("spiderX".into(), json!(spider_x));
        }
        settings.insert("realitySettings".into(), json!(reality_json));
    }

    if let Some(ws) = &stream.ws {
        let mut ws_json = serde_json::Map::new();
        if let Some(path) = &ws.path {
            ws_json.insert("path".into(), json!(path));
        }
        if let Some(host) = &ws.host {
            ws_json.insert("headers".into(), json!({ "Host": host }));
        }
        settings.insert("wsSettings".into(), json!(ws_json));
    }

    if let Some(grpc) = &stream.grpc {
        settings.insert(
            "grpcSettings".into(),
            json!({ "serviceName": grpc.service_name }),
        );
    }

    Some(serde_json::Value::Object(settings))
}

/// Require a live child and a listening loopback endpoint before launching Chromium.
pub(crate) fn wait_ready(
    child: &mut std::process::Child,
    port: u16,
    timeout: std::time::Duration,
    mut keep_waiting: impl FnMut() -> bool,
) -> Result<(), ProxyError> {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::{Duration, Instant};
    let start = Instant::now();
    loop {
        if !keep_waiting() {
            return Err(ProxyError::Other("Xray startup cancelled".into()));
        }
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
    use domain::{
        GrpcSettings, HttpOutbound, ProxyId, RealitySettings, ShadowsocksOutbound, Socks5Outbound,
        StreamNetwork, StreamSecurity, StreamSettings, TlsSettings, TrojanOutbound, VlessOutbound,
        VmessOutbound, WsSettings,
    };

    fn config_for(outbound: ProxyOutbound) -> serde_json::Value {
        let proxy = ProxyProfile {
            id: ProxyId::new(),
            name: "test".into(),
            outbound,
        };
        let path = std::env::temp_dir().join(format!("fp-xray-check-{}.json", proxy.id));
        DefaultXrayConfigBuilder
            .build(&proxy, 12345, &path)
            .unwrap();
        let config = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        fs::remove_file(path).unwrap();
        config
    }

    /// The secret of each protocol goes where that protocol's engine code looks
    /// for it: `servers` for the two server-shaped ones, `vnext[].users` for the
    /// two user-shaped ones.
    #[test]
    fn each_protocol_writes_its_secret_where_the_engine_reads_it() {
        let shadowsocks = config_for(ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
            host: "node.example".into(),
            port: 8388,
            password: "secret".into(),
            method: "aes-256-gcm".into(),
            stream: StreamSettings::plain(),
        }));
        let outbound = &shadowsocks["outbounds"][0];
        assert_eq!(outbound["protocol"], "shadowsocks");
        assert_eq!(outbound["settings"]["servers"][0]["method"], "aes-256-gcm");
        assert_eq!(outbound["settings"]["servers"][0]["password"], "secret");
        // A plain stream is the engine's own default, so nothing is written.
        assert!(outbound.get("streamSettings").is_none());

        let vmess = config_for(ProxyOutbound::Vmess(VmessOutbound {
            host: "node.example".into(),
            port: 443,
            uuid: "the-uuid".into(),
            security: "auto".into(),
            stream: StreamSettings::plain(),
        }));
        let outbound = &vmess["outbounds"][0];
        assert_eq!(outbound["protocol"], "vmess");
        assert_eq!(
            outbound["settings"]["vnext"][0]["users"][0]["id"],
            "the-uuid"
        );
        assert_eq!(
            outbound["settings"]["vnext"][0]["users"][0]["security"],
            "auto"
        );

        let vless = config_for(ProxyOutbound::Vless(VlessOutbound {
            host: "node.example".into(),
            port: 443,
            uuid: "the-uuid".into(),
            flow: Some("xtls-rprx-vision".into()),
            encryption: "none".into(),
            stream: StreamSettings::plain(),
        }));
        let outbound = &vless["outbounds"][0];
        assert_eq!(outbound["protocol"], "vless");
        assert_eq!(
            outbound["settings"]["vnext"][0]["users"][0]["flow"],
            "xtls-rprx-vision"
        );
        assert_eq!(
            outbound["settings"]["vnext"][0]["users"][0]["encryption"],
            "none"
        );

        // No flow is an empty string rather than an absent field: the engine
        // reads the field, and `null` is not what it expects there.
        let flowless = config_for(ProxyOutbound::Vless(VlessOutbound {
            host: "node.example".into(),
            port: 443,
            uuid: "the-uuid".into(),
            flow: None,
            encryption: "none".into(),
            stream: StreamSettings::plain(),
        }));
        assert_eq!(
            flowless["outbounds"][0]["settings"]["vnext"][0]["users"][0]["flow"],
            ""
        );

        let trojan = config_for(ProxyOutbound::Trojan(TrojanOutbound {
            host: "node.example".into(),
            port: 443,
            password: "secret".into(),
            stream: StreamSettings::plain(),
        }));
        let outbound = &trojan["outbounds"][0];
        assert_eq!(outbound["protocol"], "trojan");
        assert_eq!(outbound["settings"]["servers"][0]["password"], "secret");
    }

    #[test]
    fn stream_settings_are_written_under_the_names_the_engine_reads() {
        let tls_over_ws = config_for(ProxyOutbound::Vmess(VmessOutbound {
            host: "node.example".into(),
            port: 443,
            uuid: "the-uuid".into(),
            security: "auto".into(),
            stream: StreamSettings {
                network: StreamNetwork::Ws,
                security: StreamSecurity::Tls,
                tls: Some(TlsSettings {
                    server_name: Some("front.example".into()),
                    fingerprint: Some("chrome".into()),
                    alpn: vec!["h2".into(), "http/1.1".into()],
                }),
                ws: Some(WsSettings {
                    path: Some("/ws".into()),
                    host: Some("front.example".into()),
                }),
                ..Default::default()
            },
        }));
        let stream = &tls_over_ws["outbounds"][0]["streamSettings"];
        assert_eq!(stream["network"], "ws");
        assert_eq!(stream["security"], "tls");
        assert_eq!(stream["tlsSettings"]["serverName"], "front.example");
        assert_eq!(stream["tlsSettings"]["fingerprint"], "chrome");
        assert_eq!(stream["tlsSettings"]["alpn"], json!(["h2", "http/1.1"]));
        assert_eq!(stream["wsSettings"]["path"], "/ws");
        assert_eq!(stream["wsSettings"]["headers"]["Host"], "front.example");

        let reality = config_for(ProxyOutbound::Vless(VlessOutbound {
            host: "node.example".into(),
            port: 443,
            uuid: "the-uuid".into(),
            flow: Some("xtls-rprx-vision".into()),
            encryption: "none".into(),
            stream: StreamSettings {
                security: StreamSecurity::Reality,
                reality: Some(RealitySettings {
                    server_name: Some("front.example".into()),
                    public_key: "the-public-key".into(),
                    short_id: Some("ab12".into()),
                    fingerprint: Some("chrome".into()),
                    spider_x: Some("/".into()),
                }),
                ..Default::default()
            },
        }));
        let stream = &reality["outbounds"][0]["streamSettings"];
        assert_eq!(stream["security"], "reality");
        assert_eq!(stream["realitySettings"]["serverName"], "front.example");
        assert_eq!(stream["realitySettings"]["publicKey"], "the-public-key");
        assert_eq!(stream["realitySettings"]["shortId"], "ab12");
        assert_eq!(stream["realitySettings"]["fingerprint"], "chrome");
        assert_eq!(stream["realitySettings"]["spiderX"], "/");

        let over_grpc = config_for(ProxyOutbound::Vless(VlessOutbound {
            host: "node.example".into(),
            port: 443,
            uuid: "the-uuid".into(),
            flow: None,
            encryption: "none".into(),
            stream: StreamSettings {
                network: StreamNetwork::Grpc,
                security: StreamSecurity::Tls,
                grpc: Some(GrpcSettings {
                    service_name: "svc".into(),
                }),
                ..Default::default()
            },
        }));
        let stream = &over_grpc["outbounds"][0]["streamSettings"];
        assert_eq!(stream["network"], "grpc");
        assert_eq!(stream["grpcSettings"]["serviceName"], "svc");
        // No tls block was set, and the engine reads that as "TLS with its own
        // defaults" rather than as nothing at all.
        assert_eq!(stream["security"], "tls");
        assert!(stream.get("tlsSettings").is_none());
    }

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
