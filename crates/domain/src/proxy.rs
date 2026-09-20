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

    /// How this outbound is carried, for the protocols that can carry one.
    ///
    /// `None` means the protocol has no stream to configure, not that it uses
    /// the engine's defaults: a field that could never be read is not modelled,
    /// so there is nothing to silently drop.
    pub fn stream(&self) -> Option<&StreamSettings> {
        match self {
            Self::Socks5(_) | Self::Http(_) => None,
            Self::Shadowsocks(o) => Some(&o.stream),
            Self::Vmess(o) => Some(&o.stream),
            Self::Vless(o) => Some(&o.stream),
            Self::Trojan(o) => Some(&o.stream),
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
    #[serde(default)]
    pub stream: StreamSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmessOutbound {
    pub host: String,
    pub port: u16,
    pub uuid: String,
    pub security: String,
    #[serde(default)]
    pub stream: StreamSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VlessOutbound {
    pub host: String,
    pub port: u16,
    pub uuid: String,
    pub flow: Option<String>,
    pub encryption: String,
    #[serde(default)]
    pub stream: StreamSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrojanOutbound {
    pub host: String,
    pub port: u16,
    pub password: String,
    #[serde(default)]
    pub stream: StreamSettings,
}

/// How the connection to the upstream is carried.
///
/// Both halves have to agree with each other and with the settings block below:
/// a `tls` block under `security: none`, or a `ws` block under `network: tcp`, is
/// read by nobody, and the engine says nothing about it (measured: `xray run
/// -test` accepts both). The engine's own default is `tcp` over no security.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StreamSettings {
    #[serde(default)]
    pub network: StreamNetwork,
    #[serde(default)]
    pub security: StreamSecurity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reality: Option<RealitySettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws: Option<WsSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grpc: Option<GrpcSettings>,
}

impl StreamSettings {
    /// Plain TCP with no TLS, which is what the engine does when it is told
    /// nothing. Spelled out rather than implied, so a caller has one value to
    /// compare against.
    pub fn plain() -> Self {
        Self::default()
    }

    /// Whether nothing here would have to be written down: the engine's own
    /// defaults, and no block for it to read.
    pub fn is_plain(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamNetwork {
    #[default]
    Tcp,
    Ws,
    Grpc,
}

impl StreamNetwork {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Ws => "ws",
            Self::Grpc => "grpc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamSecurity {
    #[default]
    None,
    Tls,
    Reality,
}

impl StreamSecurity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Tls => "tls",
            Self::Reality => "reality",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TlsSettings {
    /// The name the certificate is checked against, and the SNI that is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    /// The client hello to imitate (`chrome`, `firefox`, ...). The engine
    /// validates the name against its own list at startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alpn: Vec<String>,
}

/// REALITY, which authenticates the server by key instead of by certificate.
///
/// There is no "skip verification" here and no way to add one: the client
/// either holds the server's public key or it does not.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RealitySettings {
    /// The name the REALITY handshake borrows from the real site behind it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    /// The server's public key, as `xray x25519` prints it.
    #[serde(default)]
    pub public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spider_x: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WsSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The `Host` header, which is what a CDN in front of the relay routes on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GrpcSettings {
    #[serde(default)]
    pub service_name: String,
}
