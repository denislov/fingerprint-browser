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
            ProxyOutbound::Shadowsocks(ss) => json!({
                "protocol": "shadowsocks",
                "settings": {
                    "servers": [{
                        "address": ss.host,
                        "port": ss.port,
                        "password": ss.password,
                        "method": ss.method
                    }]
                }
            }),
            ProxyOutbound::Vmess(vm) => json!({
                "protocol": "vmess",
                "settings": {
                    "vnext": [{
                        "address": vm.host,
                        "port": vm.port,
                        "users": [{
                            "id": vm.uuid,
                            "security": vm.security
                        }]
                    }]
                }
            }),
            ProxyOutbound::Vless(vl) => json!({
                "protocol": "vless",
                "settings": {
                    "vnext": [{
                        "address": vl.host,
                        "port": vl.port,
                        "users": [{
                            "id": vl.uuid,
                            "flow": vl.flow,
                            "encryption": vl.encryption
                        }]
                    }]
                }
            }),
            ProxyOutbound::Trojan(tr) => json!({
                "protocol": "trojan",
                "settings": {
                    "servers": [{
                        "address": tr.host,
                        "port": tr.port,
                        "password": tr.password
                    }]
                }
            }),
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
        fs::write(output_path, config_str)?;

        Ok(())
    }
}
