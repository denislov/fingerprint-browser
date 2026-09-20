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

    /// A copy with everything that authorises this client to the upstream
    /// removed, and everything else - the host, the port, the method, the
    /// security, the transport and TLS settings - left alone.
    ///
    /// This is the one place what counts as a credential is written down. Host
    /// and port are kept because re-typing them is the tedious part of
    /// recreating a proxy; a secret is kept by nobody, and a secret left in a
    /// document that is only ever plain text cannot be taken back afterwards.
    /// A new variant cannot be added without deciding, because the `match` stops
    /// compiling.
    pub fn without_credentials(&self) -> Self {
        match self {
            Self::Socks5(o) => Self::Socks5(Socks5Outbound {
                username: None,
                password: None,
                ..o.clone()
            }),
            Self::Http(o) => Self::Http(HttpOutbound {
                username: None,
                password: None,
                ..o.clone()
            }),
            Self::Shadowsocks(o) => Self::Shadowsocks(ShadowsocksOutbound {
                password: String::new(),
                stream: o.stream.without_credentials(),
                ..o.clone()
            }),
            Self::Vmess(o) => Self::Vmess(VmessOutbound {
                uuid: String::new(),
                stream: o.stream.without_credentials(),
                ..o.clone()
            }),
            Self::Vless(o) => Self::Vless(VlessOutbound {
                uuid: String::new(),
                stream: o.stream.without_credentials(),
                ..o.clone()
            }),
            Self::Trojan(o) => Self::Trojan(TrojanOutbound {
                password: String::new(),
                stream: o.stream.without_credentials(),
                ..o.clone()
            }),
        }
    }

    /// Whether there is anything for
    /// [`without_credentials`](Self::without_credentials) to remove.
    ///
    /// Also this is not the same question as "can this outbound work". SOCKS5
    /// and HTTP authenticate optionally, so one of them with no credentials is a
    /// complete configuration rather than a stripped one; for the other four the
    /// credentials are mandatory and their absence is already refused by
    /// [`validate_proxy`](crate::validate_proxy).
    ///
    /// Kept beside the stripping function because the two have to agree, and a
    /// test pins that they do: an export that reports "no credentials were
    /// written" while the file still carries a password is the failure this pair
    /// exists to prevent.
    pub fn carries_credentials(&self) -> bool {
        match self {
            Self::Socks5(o) => o.username.is_some() || o.password.is_some(),
            Self::Http(o) => o.username.is_some() || o.password.is_some(),
            Self::Shadowsocks(o) => !o.password.is_empty() || o.stream.carries_credentials(),
            Self::Vmess(o) => !o.uuid.is_empty() || o.stream.carries_credentials(),
            Self::Vless(o) => !o.uuid.is_empty() || o.stream.carries_credentials(),
            Self::Trojan(o) => !o.password.is_empty() || o.stream.carries_credentials(),
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
    /// The legacy alteration count a `vmess://` link carries as `aid`.
    ///
    /// Zero is what the engine assumes when the field is absent, and what every
    /// current server uses. A link that says otherwise is honoured rather than
    /// dropped: the engine reads this value (measured: `xray run -test` accepts
    /// a non-zero one), and a client that quietly sent zero to a server
    /// expecting 64 would fail to connect for no visible reason.
    #[serde(default)]
    pub alter_id: u32,
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

    /// The same stream with the REALITY key material removed, and everything
    /// else - the network, the security, the TLS name and the ws/grpc blocks -
    /// left alone.
    pub fn without_credentials(&self) -> Self {
        Self {
            reality: self
                .reality
                .as_ref()
                .map(RealitySettings::without_credentials),
            ..self.clone()
        }
    }

    /// Whether any REALITY key material is set.
    ///
    /// `false` for a stream that carries no REALITY block at all, which is most
    /// of them: nothing was configured, so nothing is being withheld.
    pub fn carries_credentials(&self) -> bool {
        self.reality
            .as_ref()
            .is_some_and(RealitySettings::carries_credentials)
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

impl RealitySettings {
    /// The same settings without the material the client presents to the server.
    ///
    /// `public_key` is the server's key rather than this client's secret, and it
    /// goes too: it can be re-entered, and a stripped proxy that over-removes is
    /// recoverable where one that under-removes is not. `short_id` is part of the
    /// same handshake and goes with it - which also leaves the stripped proxy
    /// refused rather than merely degraded, since a REALITY stream without a
    /// public key already fails validation.
    ///
    /// `server_name`, `fingerprint` and `spider_x` stay. They are a borrowed
    /// SNI, a client-hello template name and a fallback path: things a server
    /// does not check a secret against.
    pub fn without_credentials(&self) -> Self {
        Self {
            public_key: String::new(),
            short_id: None,
            ..self.clone()
        }
    }

    /// Whether any of the material `without_credentials` removes is set.
    pub fn carries_credentials(&self) -> bool {
        !self.public_key.is_empty() || self.short_id.is_some()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A secret that would be recognisable in a serialised document, so a test
    /// that fails shows the value rather than merely a mismatch.
    const SECRET: &str = "correct horse battery staple";

    fn over_reality() -> StreamSettings {
        StreamSettings {
            network: StreamNetwork::Tcp,
            security: StreamSecurity::Reality,
            reality: Some(RealitySettings {
                server_name: Some("www.example.com".to_string()),
                public_key: "the-public-key-xray-x25519-printed".to_string(),
                short_id: Some("0a1b2c3d".to_string()),
                fingerprint: Some("chrome".to_string()),
                spider_x: Some("/".to_string()),
            }),
            ..StreamSettings::default()
        }
    }

    /// One example per protocol, each carrying what its protocol has to carry.
    ///
    /// The equivalents of `credentialled` and `stripped` are what the two
    /// properties below are stated over, so a protocol added to `ProxyOutbound`
    /// without being added here is caught by the exhaustiveness of the functions
    /// rather than by this list.
    fn credentialled() -> Vec<ProxyOutbound> {
        vec![
            ProxyOutbound::Socks5(Socks5Outbound {
                host: "10.0.0.1".to_string(),
                port: 1080,
                username: Some("alice".to_string()),
                password: Some(SECRET.to_string()),
            }),
            ProxyOutbound::Http(HttpOutbound {
                host: "10.0.0.1".to_string(),
                port: 3128,
                username: Some("alice".to_string()),
                password: Some(SECRET.to_string()),
            }),
            ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                host: "10.0.0.2".to_string(),
                port: 8388,
                password: SECRET.to_string(),
                method: "aes-256-gcm".to_string(),
                stream: StreamSettings::default(),
            }),
            ProxyOutbound::Vmess(VmessOutbound {
                host: "10.0.0.3".to_string(),
                port: 443,
                uuid: "5c1d2e3f-4a5b-6c7d-8e9f-0a1b2c3d4e5f".to_string(),
                security: "auto".to_string(),
                alter_id: 64,
                stream: StreamSettings::default(),
            }),
            ProxyOutbound::Vless(VlessOutbound {
                host: "10.0.0.4".to_string(),
                port: 443,
                uuid: "5c1d2e3f-4a5b-6c7d-8e9f-0a1b2c3d4e5f".to_string(),
                flow: Some("xtls-rprx-vision".to_string()),
                encryption: "none".to_string(),
                stream: StreamSettings::default(),
            }),
            ProxyOutbound::Trojan(TrojanOutbound {
                host: "10.0.0.5".to_string(),
                port: 443,
                password: SECRET.to_string(),
                stream: over_reality(),
            }),
        ]
    }

    #[test]
    fn stripping_socks5_removes_the_user_name_and_password() {
        let outbound = ProxyOutbound::Socks5(Socks5Outbound {
            host: "10.0.0.1".to_string(),
            port: 1080,
            username: Some("alice".to_string()),
            password: Some(SECRET.to_string()),
        });

        // Stated as the whole outbound, so a field that was meant to survive and
        // did not shows up as clearly as one that was meant to go and stayed.
        assert_eq!(
            outbound.without_credentials(),
            ProxyOutbound::Socks5(Socks5Outbound {
                host: "10.0.0.1".to_string(),
                port: 1080,
                username: None,
                password: None,
            })
        );
    }

    #[test]
    fn stripping_http_removes_the_user_name_and_password() {
        let outbound = ProxyOutbound::Http(HttpOutbound {
            host: "proxy.example".to_string(),
            port: 3128,
            username: Some("alice".to_string()),
            password: Some(SECRET.to_string()),
        });

        assert_eq!(
            outbound.without_credentials(),
            ProxyOutbound::Http(HttpOutbound {
                host: "proxy.example".to_string(),
                port: 3128,
                username: None,
                password: None,
            })
        );
    }

    #[test]
    fn stripping_shadowsocks_removes_the_password_and_keeps_the_method() {
        let outbound = ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
            host: "10.0.0.2".to_string(),
            port: 8388,
            password: SECRET.to_string(),
            method: "aes-256-gcm".to_string(),
            stream: StreamSettings::default(),
        });

        // The method is not a credential: it says how the bytes are wrapped, and
        // the server does not check a secret against it.
        assert_eq!(
            outbound.without_credentials(),
            ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                host: "10.0.0.2".to_string(),
                port: 8388,
                password: String::new(),
                method: "aes-256-gcm".to_string(),
                stream: StreamSettings::default(),
            })
        );
    }

    #[test]
    fn stripping_vmess_removes_the_uuid_and_keeps_the_security_and_alter_id() {
        let outbound = ProxyOutbound::Vmess(VmessOutbound {
            host: "10.0.0.3".to_string(),
            port: 443,
            uuid: "5c1d2e3f-4a5b-6c7d-8e9f-0a1b2c3d4e5f".to_string(),
            security: "auto".to_string(),
            alter_id: 64,
            stream: StreamSettings::default(),
        });

        // `alter_id` is kept although a stripped proxy is not startable anyway:
        // the rule is what is and is not a credential, not what happens to be
        // useless without one.
        assert_eq!(
            outbound.without_credentials(),
            ProxyOutbound::Vmess(VmessOutbound {
                host: "10.0.0.3".to_string(),
                port: 443,
                uuid: String::new(),
                security: "auto".to_string(),
                alter_id: 64,
                stream: StreamSettings::default(),
            })
        );
    }

    #[test]
    fn stripping_vless_removes_the_uuid_and_keeps_the_flow() {
        let outbound = ProxyOutbound::Vless(VlessOutbound {
            host: "10.0.0.4".to_string(),
            port: 443,
            uuid: "5c1d2e3f-4a5b-6c7d-8e9f-0a1b2c3d4e5f".to_string(),
            flow: Some("xtls-rprx-vision".to_string()),
            encryption: "none".to_string(),
            stream: StreamSettings::default(),
        });

        assert_eq!(
            outbound.without_credentials(),
            ProxyOutbound::Vless(VlessOutbound {
                host: "10.0.0.4".to_string(),
                port: 443,
                uuid: String::new(),
                flow: Some("xtls-rprx-vision".to_string()),
                encryption: "none".to_string(),
                stream: StreamSettings::default(),
            })
        );
    }

    #[test]
    fn stripping_trojan_removes_the_password_and_the_reality_key_material() {
        let outbound = ProxyOutbound::Trojan(TrojanOutbound {
            host: "10.0.0.5".to_string(),
            port: 443,
            password: SECRET.to_string(),
            stream: over_reality(),
        });

        // The stream goes with the password, because the same document carries
        // both and REALITY's key material is the other half of the secret.
        assert_eq!(
            outbound.without_credentials(),
            ProxyOutbound::Trojan(TrojanOutbound {
                host: "10.0.0.5".to_string(),
                port: 443,
                password: String::new(),
                stream: StreamSettings {
                    network: StreamNetwork::Tcp,
                    security: StreamSecurity::Reality,
                    reality: Some(RealitySettings {
                        server_name: Some("www.example.com".to_string()),
                        public_key: String::new(),
                        short_id: None,
                        fingerprint: Some("chrome".to_string()),
                        spider_x: Some("/".to_string()),
                    }),
                    ..StreamSettings::default()
                },
            })
        );
    }

    #[test]
    fn reality_material_is_what_makes_a_stream_a_credential() {
        // A REALITY block that names a site but presents no key holds nothing
        // secret, so `carries_credentials` must not simply be
        // `reality.is_some()` and stripping must be a no-op.
        let settings = StreamSettings {
            network: StreamNetwork::Tcp,
            security: StreamSecurity::Reality,
            reality: Some(RealitySettings {
                server_name: Some("www.example.com".to_string()),
                ..RealitySettings::default()
            }),
            ..StreamSettings::default()
        };

        assert!(!settings.carries_credentials());
        assert_eq!(settings.without_credentials(), settings);
    }

    #[test]
    fn a_plain_stream_carries_nothing_to_remove() {
        let plain = StreamSettings::plain();
        assert!(!plain.carries_credentials());
        assert_eq!(plain.without_credentials(), plain);
    }

    #[test]
    fn an_outbound_carries_credentials_exactly_when_stripping_changes_it() {
        // The equivalence the two functions have to keep between them. It is
        // stated over `credentialled` plus the cases that have nothing to
        // remove, including a proxy that simply needs no authentication - where
        // "no credentials" is a complete configuration rather than a stripped
        // one.
        let mut cases = credentialled();
        cases.push(ProxyOutbound::Socks5(Socks5Outbound {
            host: "10.0.0.1".to_string(),
            port: 1080,
            username: None,
            password: None,
        }));
        cases.push(ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
            host: "10.0.0.2".to_string(),
            port: 8388,
            password: String::new(),
            method: "aes-256-gcm".to_string(),
            stream: StreamSettings::default(),
        }));
        cases.push(ProxyOutbound::Trojan(TrojanOutbound {
            host: "10.0.0.5".to_string(),
            port: 443,
            password: String::new(),
            stream: over_reality(),
        }));

        for outbound in cases {
            assert_eq!(
                outbound.carries_credentials(),
                outbound.without_credentials() != outbound,
                "{} disagreed with itself about carrying credentials",
                outbound.kind()
            );
        }
    }

    #[test]
    fn stripping_twice_is_the_same_as_stripping_once() {
        for outbound in credentialled() {
            let once = outbound.without_credentials();
            assert_eq!(
                once.without_credentials(),
                once,
                "{} changed when stripped a second time",
                outbound.kind()
            );
        }
    }

    #[test]
    fn a_stripped_proxy_still_names_the_same_server() {
        // What a reader of a stripped document keeps: where to connect, and
        // which protocol to speak. Losing either would make the file useless
        // rather than merely secret-free. The stream is covered in full by the
        // protocol tests above, which compare whole outbounds.
        for outbound in credentialled() {
            let stripped = outbound.without_credentials();
            assert_eq!(stripped.kind(), outbound.kind());
            assert_eq!(stripped.host(), outbound.host());
            assert_eq!(stripped.port(), outbound.port());
        }
    }
}
