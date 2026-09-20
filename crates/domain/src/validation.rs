use crate::core::BrowserCore;
use crate::error::ValidationError;
use crate::fingerprint::FingerprintProfile;
use crate::profile::BrowserProfile;
use crate::proxy::{ProxyOutbound, ProxyProfile, StreamNetwork, StreamSecurity, StreamSettings};
use crate::window::WindowProfile;

pub fn validate_profile(profile: &BrowserProfile) -> Result<(), ValidationError> {
    if profile.name.trim().is_empty() {
        return Err(ValidationError::EmptyProfileName);
    }
    validate_window(&profile.window)?;
    validate_fingerprint(&profile.fingerprint)?;
    Ok(())
}

pub fn validate_window(window: &WindowProfile) -> Result<(), ValidationError> {
    if window.width == 0 || window.height == 0 {
        return Err(ValidationError::InvalidWindowDimensions {
            width: window.width,
            height: window.height,
        });
    }
    Ok(())
}

pub fn validate_fingerprint(fp: &FingerprintProfile) -> Result<(), ValidationError> {
    if fp.language.trim().is_empty() {
        return Err(ValidationError::EmptyLanguage);
    }
    if fp.timezone.trim().is_empty() {
        return Err(ValidationError::EmptyTimezone);
    }
    if let Some(hw) = fp.hardware_concurrency
        && !(1..=128).contains(&hw)
    {
        return Err(ValidationError::InvalidHardwareConcurrency(hw));
    }
    Ok(())
}

/// Checks the parts of a proxy the runtime would otherwise only discover when
/// the profile is started.
///
/// A half-set username or a port of zero used to reach the config builder and
/// come back as a start failure; catching them here means the editor can refuse
/// the save and say which field is at fault.
pub fn validate_proxy(proxy: &ProxyProfile) -> Result<(), ValidationError> {
    if proxy.name.trim().is_empty() {
        return Err(ValidationError::EmptyProxyName);
    }

    let host = proxy.outbound.host().trim();
    if host.is_empty() {
        return Err(ValidationError::EmptyProxyHost);
    }
    if host.contains(char::is_whitespace) || host.contains("://") {
        return Err(ValidationError::InvalidProxyHost {
            host: host.to_string(),
        });
    }
    if proxy.outbound.port() == 0 {
        return Err(ValidationError::InvalidProxyPort);
    }
    if !proxy.outbound.credentials_complete() {
        return Err(ValidationError::IncompleteProxyCredentials);
    }

    let required: &[(&str, &str)] = match &proxy.outbound {
        ProxyOutbound::Socks5(_) | ProxyOutbound::Http(_) => &[],
        ProxyOutbound::Shadowsocks(s) => &[("password", &s.password), ("method", &s.method)],
        ProxyOutbound::Vmess(v) => &[("uuid", &v.uuid), ("security", &v.security)],
        ProxyOutbound::Vless(v) => &[("uuid", &v.uuid), ("encryption", &v.encryption)],
        ProxyOutbound::Trojan(t) => &[("password", &t.password)],
    };
    for (field, value) in required {
        if value.trim().is_empty() {
            return Err(ValidationError::EmptyProxyField { field });
        }
    }

    if let Some(stream) = proxy.outbound.stream() {
        validate_stream(stream)?;
    }

    Ok(())
}

/// Checks the stream settings against the transport that would have to read them.
///
/// Two kinds of mistake, and the engine reports only one of them. It refuses
/// REALITY over ws at startup with a message of its own (measured: `xray run
/// -test` exits 23); it silently accepts a `tls` block under `security: none`
/// and a `ws` block under `network: tcp`, because those blocks are read by
/// nobody and reported by nobody (measured: exit 0). The second kind is why
/// this function exists: a setting that would be dropped in silence is refused
/// here instead, by name.
pub(crate) fn validate_stream(stream: &StreamSettings) -> Result<(), ValidationError> {
    match stream.security {
        StreamSecurity::None => {
            for (field, present) in [
                ("tls", stream.tls.is_some()),
                ("reality", stream.reality.is_some()),
            ] {
                if present {
                    return Err(ValidationError::UnusedStreamSetting {
                        field,
                        selected: "security=none",
                    });
                }
            }
        }
        StreamSecurity::Tls => {
            if stream.reality.is_some() {
                return Err(ValidationError::UnusedStreamSetting {
                    field: "reality",
                    selected: "security=tls",
                });
            }
        }
        StreamSecurity::Reality => {
            if stream.tls.is_some() {
                return Err(ValidationError::UnusedStreamSetting {
                    field: "tls",
                    selected: "security=reality",
                });
            }
            let public_key = stream
                .reality
                .as_ref()
                .map(|reality| reality.public_key.trim())
                .unwrap_or_default();
            if public_key.is_empty() {
                return Err(ValidationError::MissingStreamSetting {
                    field: "reality.public_key",
                    selected: "security=reality",
                });
            }
            if stream.network != StreamNetwork::Tcp {
                return Err(ValidationError::UnsupportedStreamCombination {
                    detail: format!(
                        "REALITY borrows a TLS handshake over tcp, so network={} cannot carry it",
                        stream.network.as_str()
                    ),
                });
            }
        }
    }

    match stream.network {
        StreamNetwork::Tcp => {
            for (field, present) in [("ws", stream.ws.is_some()), ("grpc", stream.grpc.is_some())] {
                if present {
                    return Err(ValidationError::UnusedStreamSetting {
                        field,
                        selected: "network=tcp",
                    });
                }
            }
        }
        StreamNetwork::Ws => {
            if stream.grpc.is_some() {
                return Err(ValidationError::UnusedStreamSetting {
                    field: "grpc",
                    selected: "network=ws",
                });
            }
        }
        StreamNetwork::Grpc => {
            if stream.ws.is_some() {
                return Err(ValidationError::UnusedStreamSetting {
                    field: "ws",
                    selected: "network=grpc",
                });
            }
            let service_name = stream
                .grpc
                .as_ref()
                .map(|grpc| grpc.service_name.trim())
                .unwrap_or_default();
            if service_name.is_empty() {
                return Err(ValidationError::MissingStreamSetting {
                    field: "grpc.service_name",
                    selected: "network=grpc",
                });
            }
        }
    }

    Ok(())
}

pub fn validate_core(core: &BrowserCore) -> Result<(), ValidationError> {
    if core.name.trim().is_empty() {
        return Err(ValidationError::EmptyCoreName);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{
        GrpcSettings, HttpOutbound, ProxyOutbound, ProxyProfile, RealitySettings,
        ShadowsocksOutbound, Socks5Outbound, StreamNetwork, StreamSecurity, StreamSettings,
        TlsSettings, TrojanOutbound, VlessOutbound, VmessOutbound, WsSettings,
    };
    use crate::{ProxyId, ValidationError};

    fn profile(outbound: ProxyOutbound) -> ProxyProfile {
        ProxyProfile {
            id: ProxyId::new(),
            name: "Office".to_string(),
            outbound,
        }
    }

    fn socks5(host: &str, port: u16) -> ProxyOutbound {
        ProxyOutbound::Socks5(Socks5Outbound {
            host: host.to_string(),
            port,
            username: None,
            password: None,
        })
    }

    #[test]
    fn a_well_formed_proxy_passes() {
        assert_eq!(validate_proxy(&profile(socks5("10.0.0.1", 1080))), Ok(()));
        let secret = ProxyProfile {
            id: ProxyId::new(),
            name: "Office".to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "10.0.0.1".to_string(),
                port: 1080,
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
            }),
        };
        assert_eq!(validate_proxy(&secret), Ok(()));
    }

    #[test]
    fn a_nameless_proxy_is_refused() {
        let mut proxy = profile(socks5("10.0.0.1", 1080));
        proxy.name = "  ".to_string();
        assert_eq!(validate_proxy(&proxy), Err(ValidationError::EmptyProxyName));
    }

    #[test]
    fn a_proxy_without_a_host_or_port_is_refused() {
        assert_eq!(
            validate_proxy(&profile(socks5("   ", 1080))),
            Err(ValidationError::EmptyProxyHost)
        );
        assert_eq!(
            validate_proxy(&profile(socks5("10.0.0.1", 0))),
            Err(ValidationError::InvalidProxyPort)
        );
    }

    #[test]
    fn a_pasted_url_is_refused_as_a_host() {
        let error = validate_proxy(&profile(socks5("http://10.0.0.1", 1080))).unwrap_err();
        assert!(matches!(error, ValidationError::InvalidProxyHost { .. }));
        assert!(error.to_string().contains("scheme"), "{error}");

        let error = validate_proxy(&profile(socks5("10.0.0.1 8080", 1080))).unwrap_err();
        assert!(matches!(error, ValidationError::InvalidProxyHost { .. }));
    }

    #[test]
    fn half_a_credential_is_refused() {
        let outbound = ProxyOutbound::Http(HttpOutbound {
            host: "10.0.0.1".to_string(),
            port: 8080,
            username: Some("user".to_string()),
            password: None,
        });
        assert_eq!(
            validate_proxy(&profile(outbound)),
            Err(ValidationError::IncompleteProxyCredentials)
        );
    }

    /// Where "a stripped proxy is not a working proxy" holds, and where it does
    /// not.
    ///
    /// It is true for the four protocols whose credentials are mandatory, and
    /// it already follows from the checks above - nothing new had to be added
    /// for a backup to be safe. It is false for SOCKS5 and HTTP, where
    /// authenticating is optional: stripping those produces a *different*
    /// proxy rather than a broken one, and whether it still works is a question
    /// for the upstream. An import cannot tell those two apart from the outbound
    /// alone, which is why an export records its own choice.
    #[test]
    fn a_stripped_proxy_is_refused_only_where_credentials_are_mandatory() {
        let mandatory = [
            ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                host: "10.0.0.2".to_string(),
                port: 8388,
                password: "secret".to_string(),
                method: "aes-256-gcm".to_string(),
                stream: StreamSettings::default(),
            }),
            ProxyOutbound::Vmess(VmessOutbound {
                host: "10.0.0.3".to_string(),
                port: 443,
                uuid: "5c1d2e3f-4a5b-6c7d-8e9f-0a1b2c3d4e5f".to_string(),
                security: "auto".to_string(),
                alter_id: 0,
                stream: StreamSettings::default(),
            }),
            ProxyOutbound::Vless(VlessOutbound {
                host: "10.0.0.4".to_string(),
                port: 443,
                uuid: "5c1d2e3f-4a5b-6c7d-8e9f-0a1b2c3d4e5f".to_string(),
                flow: None,
                encryption: "none".to_string(),
                stream: StreamSettings::default(),
            }),
            ProxyOutbound::Trojan(TrojanOutbound {
                host: "10.0.0.5".to_string(),
                port: 443,
                password: "secret".to_string(),
                stream: StreamSettings::default(),
            }),
        ];

        for outbound in &mandatory {
            let kind = outbound.kind();
            assert_eq!(
                validate_proxy(&profile(outbound.clone())),
                Ok(()),
                "{kind} should be complete before it is stripped"
            );
            assert!(
                matches!(
                    validate_proxy(&profile(outbound.without_credentials())),
                    Err(ValidationError::EmptyProxyField { .. })
                ),
                "{kind} should be refused once stripped rather than offered"
            );
        }

        for outbound in [
            ProxyOutbound::Socks5(Socks5Outbound {
                host: "10.0.0.1".to_string(),
                port: 1080,
                username: Some("alice".to_string()),
                password: Some("secret".to_string()),
            }),
            ProxyOutbound::Http(HttpOutbound {
                host: "10.0.0.1".to_string(),
                port: 3128,
                username: Some("alice".to_string()),
                password: Some("secret".to_string()),
            }),
        ] {
            assert_eq!(
                validate_proxy(&profile(outbound.without_credentials())),
                Ok(()),
                "{} should still validate once stripped",
                outbound.kind()
            );
        }
    }

    /// REALITY's public key is load-bearing on its own, so losing it does not
    /// leave a quietly degraded proxy.
    ///
    /// A whole-outbound strip trips a credential field first, so this strips the
    /// stream by itself to show which check would have caught it.
    #[test]
    fn a_stripped_reality_stream_is_refused_for_the_missing_key() {
        let proxy = profile(ProxyOutbound::Trojan(TrojanOutbound {
            host: "10.0.0.5".to_string(),
            port: 443,
            password: "secret".to_string(),
            stream: StreamSettings {
                network: StreamNetwork::Tcp,
                security: StreamSecurity::Reality,
                reality: Some(RealitySettings {
                    server_name: Some("www.example.com".to_string()),
                    public_key: "the-public-key-xray-x25519-printed".to_string(),
                    short_id: Some("0a1b2c3d".to_string()),
                    ..RealitySettings::default()
                }),
                ..StreamSettings::default()
            },
        }));
        assert_eq!(validate_proxy(&proxy), Ok(()));

        let ProxyOutbound::Trojan(trojan) = &proxy.outbound else {
            unreachable!("built as a trojan above");
        };
        let stripped = profile(ProxyOutbound::Trojan(TrojanOutbound {
            stream: trojan.stream.without_credentials(),
            ..trojan.clone()
        }));

        assert!(
            matches!(
                validate_proxy(&stripped),
                Err(ValidationError::MissingStreamSetting {
                    field: "reality.public_key",
                    ..
                })
            ),
            "{:?}",
            validate_proxy(&stripped)
        );
    }

    #[test]
    fn a_stream_setting_nobody_would_read_is_refused() {
        let with_stream = |stream: StreamSettings| ProxyProfile {
            id: ProxyId::new(),
            name: "Office".to_string(),
            outbound: ProxyOutbound::Vless(VlessOutbound {
                host: "h".to_string(),
                port: 443,
                uuid: "uuid".to_string(),
                flow: None,
                encryption: "none".to_string(),
                stream,
            }),
        };
        let tls = Some(TlsSettings {
            server_name: Some("front.example".to_string()),
            ..Default::default()
        });
        let reality = Some(RealitySettings {
            server_name: Some("front.example".to_string()),
            public_key: "the-public-key".to_string(),
            ..Default::default()
        });
        let keyless_reality = Some(RealitySettings {
            server_name: Some("front.example".to_string()),
            public_key: String::new(),
            ..Default::default()
        });
        let ws = Some(WsSettings {
            path: Some("/ws".to_string()),
            ..Default::default()
        });
        let grpc = Some(GrpcSettings {
            service_name: "svc".to_string(),
        });
        let nameless_grpc = Some(GrpcSettings {
            service_name: String::new(),
        });

        let cases = [
            (
                "a tls block under security=none",
                StreamSettings {
                    tls: tls.clone(),
                    ..Default::default()
                },
                Some(ValidationError::UnusedStreamSetting {
                    field: "tls",
                    selected: "security=none",
                }),
            ),
            (
                "a reality block under security=none",
                StreamSettings {
                    reality: reality.clone(),
                    ..Default::default()
                },
                Some(ValidationError::UnusedStreamSetting {
                    field: "reality",
                    selected: "security=none",
                }),
            ),
            (
                "a reality block under security=tls",
                StreamSettings {
                    security: StreamSecurity::Tls,
                    reality: reality.clone(),
                    ..Default::default()
                },
                Some(ValidationError::UnusedStreamSetting {
                    field: "reality",
                    selected: "security=tls",
                }),
            ),
            (
                "security=reality without a public key",
                StreamSettings {
                    security: StreamSecurity::Reality,
                    reality: keyless_reality,
                    ..Default::default()
                },
                Some(ValidationError::MissingStreamSetting {
                    field: "reality.public_key",
                    selected: "security=reality",
                }),
            ),
            (
                "a tls block beside security=reality",
                StreamSettings {
                    security: StreamSecurity::Reality,
                    tls: tls.clone(),
                    reality: reality.clone(),
                    ..Default::default()
                },
                Some(ValidationError::UnusedStreamSetting {
                    field: "tls",
                    selected: "security=reality",
                }),
            ),
            (
                "reality over a websocket",
                StreamSettings {
                    network: StreamNetwork::Ws,
                    security: StreamSecurity::Reality,
                    reality: reality.clone(),
                    ws: ws.clone(),
                    ..Default::default()
                },
                Some(ValidationError::UnsupportedStreamCombination {
                    detail:
                        "REALITY borrows a TLS handshake over tcp, so network=ws cannot carry it"
                            .to_string(),
                }),
            ),
            (
                "a ws block under network=tcp",
                StreamSettings {
                    ws: ws.clone(),
                    ..Default::default()
                },
                Some(ValidationError::UnusedStreamSetting {
                    field: "ws",
                    selected: "network=tcp",
                }),
            ),
            (
                "a grpc block under network=tcp",
                StreamSettings {
                    grpc: grpc.clone(),
                    ..Default::default()
                },
                Some(ValidationError::UnusedStreamSetting {
                    field: "grpc",
                    selected: "network=tcp",
                }),
            ),
            (
                "a grpc block under network=ws",
                StreamSettings {
                    network: StreamNetwork::Ws,
                    grpc: grpc.clone(),
                    ..Default::default()
                },
                Some(ValidationError::UnusedStreamSetting {
                    field: "grpc",
                    selected: "network=ws",
                }),
            ),
            (
                "a ws block under network=grpc",
                StreamSettings {
                    network: StreamNetwork::Grpc,
                    ws: ws.clone(),
                    grpc: grpc.clone(),
                    ..Default::default()
                },
                Some(ValidationError::UnusedStreamSetting {
                    field: "ws",
                    selected: "network=grpc",
                }),
            ),
            (
                "network=grpc without a service name",
                StreamSettings {
                    network: StreamNetwork::Grpc,
                    grpc: nameless_grpc,
                    ..Default::default()
                },
                Some(ValidationError::MissingStreamSetting {
                    field: "grpc.service_name",
                    selected: "network=grpc",
                }),
            ),
            (
                "tls over a websocket",
                StreamSettings {
                    network: StreamNetwork::Ws,
                    security: StreamSecurity::Tls,
                    tls: tls.clone(),
                    ws: ws.clone(),
                    ..Default::default()
                },
                None,
            ),
            (
                "reality over plain tcp",
                StreamSettings {
                    security: StreamSecurity::Reality,
                    reality: reality.clone(),
                    ..Default::default()
                },
                None,
            ),
            ("a plain tcp stream", StreamSettings::plain(), None),
        ];

        for (label, stream, expected) in cases {
            assert_eq!(
                validate_proxy(&with_stream(stream)),
                expected.map(Err).unwrap_or(Ok(())),
                "{label}"
            );
        }
    }

    #[test]
    fn the_per_protocol_secrets_are_required() {
        let cases = [
            (
                ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                    host: "h".to_string(),
                    port: 8388,
                    password: "  ".to_string(),
                    method: "aes-256-gcm".to_string(),
                    stream: StreamSettings::plain(),
                }),
                "password",
            ),
            (
                ProxyOutbound::Vmess(VmessOutbound {
                    host: "h".to_string(),
                    port: 443,
                    uuid: "uuid".to_string(),
                    security: String::new(),
                    alter_id: 0,
                    stream: StreamSettings::plain(),
                }),
                "security",
            ),
            (
                ProxyOutbound::Vless(VlessOutbound {
                    host: "h".to_string(),
                    port: 443,
                    uuid: String::new(),
                    flow: None,
                    encryption: "none".to_string(),
                    stream: StreamSettings::plain(),
                }),
                "uuid",
            ),
            (
                ProxyOutbound::Trojan(TrojanOutbound {
                    host: "h".to_string(),
                    port: 443,
                    password: String::new(),
                    stream: StreamSettings::plain(),
                }),
                "password",
            ),
        ];
        for (outbound, field) in cases {
            assert_eq!(
                validate_proxy(&profile(outbound)),
                Err(ValidationError::EmptyProxyField { field }),
                "{field} should be required"
            );
        }
    }

    #[test]
    fn the_endpoint_names_the_protocol_and_the_host() {
        let mut proxy = profile(socks5("10.0.0.1", 1080));
        assert_eq!(proxy.endpoint(), "socks5://10.0.0.1:1080");
        proxy.outbound = ProxyOutbound::Http(HttpOutbound {
            host: "proxy.example".to_string(),
            port: 3128,
            username: None,
            password: None,
        });
        assert_eq!(proxy.endpoint(), "http://proxy.example:3128");
    }
}
