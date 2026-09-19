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

impl ProxyOutbound {
    /// The protocol name, as a config file or a summary line would spell it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Socks5(_) => "socks5",
            Self::Http(_) => "http",
            Self::Shadowsocks(_) => "shadowsocks",
            Self::Vmess(_) => "vmess",
            Self::Vless(_) => "vless",
            Self::Trojan(_) => "trojan",
        }
    }

    pub fn host(&self) -> &str {
        match self {
            Self::Socks5(o) => &o.host,
            Self::Http(o) => &o.host,
            Self::Shadowsocks(o) => &o.host,
            Self::Vmess(o) => &o.host,
            Self::Vless(o) => &o.host,
            Self::Trojan(o) => &o.host,
        }
    }

    pub fn port(&self) -> u16 {
        match self {
            Self::Socks5(o) => o.port,
            Self::Http(o) => o.port,
            Self::Shadowsocks(o) => o.port,
            Self::Vmess(o) => o.port,
            Self::Vless(o) => o.port,
            Self::Trojan(o) => o.port,
        }
    }

    /// Whether a user name and password are both present or both absent.
    ///
    /// Only the two credential-carrying protocols can be incomplete; the rest
    /// carry their own secrets and are never half set.
    pub fn credentials_complete(&self) -> bool {
        match self {
            Self::Socks5(o) => o.username.is_some() == o.password.is_some(),
            Self::Http(o) => o.username.is_some() == o.password.is_some(),
            _ => true,
        }
    }
}

impl ProxyProfile {
    /// `kind://host:port`, for a row or a refusal message.
    pub fn endpoint(&self) -> String {
        format!(
            "{}://{}:{}",
            self.outbound.kind(),
            self.outbound.host(),
            self.outbound.port()
        )
    }
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
