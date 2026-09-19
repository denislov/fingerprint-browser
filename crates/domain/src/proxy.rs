use crate::id::ProxyId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyProfile {
    pub id: ProxyId,
    pub name: String,
    pub outbound: ProxyOutbound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyOutbound {
    Socks5(Socks5Outbound),
    Http(HttpOutbound),
    Shadowsocks(ShadowsocksOutbound),
    Vmess(VmessOutbound),
    Vless(VlessOutbound),
    Trojan(TrojanOutbound),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Socks5Outbound {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpOutbound {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowsocksOutbound {
    pub host: String,
    pub port: u16,
    pub password: String,
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmessOutbound {
    pub host: String,
    pub port: u16,
    pub uuid: String,
    pub security: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VlessOutbound {
    pub host: String,
    pub port: u16,
    pub uuid: String,
    pub flow: Option<String>,
    pub encryption: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrojanOutbound {
    pub host: String,
    pub port: u16,
    pub password: String,
}
