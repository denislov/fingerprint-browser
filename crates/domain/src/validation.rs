use crate::core::BrowserCore;
use crate::error::ValidationError;
use crate::fingerprint::FingerprintProfile;
use crate::profile::BrowserProfile;
use crate::proxy::{ProxyOutbound, ProxyProfile};
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
        HttpOutbound, ProxyOutbound, ProxyProfile, ShadowsocksOutbound, Socks5Outbound,
        TrojanOutbound, VlessOutbound, VmessOutbound,
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

    #[test]
    fn the_per_protocol_secrets_are_required() {
        let cases = [
            (
                ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                    host: "h".to_string(),
                    port: 8388,
                    password: "  ".to_string(),
                    method: "aes-256-gcm".to_string(),
                }),
                "password",
            ),
            (
                ProxyOutbound::Vmess(VmessOutbound {
                    host: "h".to_string(),
                    port: 443,
                    uuid: "uuid".to_string(),
                    security: String::new(),
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
                }),
                "uuid",
            ),
            (
                ProxyOutbound::Trojan(TrojanOutbound {
                    host: "h".to_string(),
                    port: 443,
                    password: String::new(),
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
