//! Share links, turned into the model the rest of the program uses.
//!
//! A `ss://`, `vmess://`, `vless://` or `trojan://` link is how these proxies
//! actually travel: a provider hands out a URI, not a form. So the parser's job
//! is to be faithful rather than permissive - a field that is read and then
//! dropped produces a config that fails to connect with no explanation, which is
//! the same failure the capability table refuses to ship for engine switches.
//!
//! The rule for anything this model cannot carry:
//!
//! - A field we *recognise* and cannot represent is refused, by name:
//!   `plugin`, `type=kcp`, `type=xhttp`, `headerType=http`, `mode=multi`. Being
//!   silent here would mean a broken config discovered later, by a browser that
//!   simply does not load pages.
//! - A query parameter we do not recognise at all is passed over. Clients add
//!   parameters of their own (`utm_source` and the like), and refusing a link
//!   because of a name we have never seen would refuse working links.
//!
//! What each parser reads was checked against the engine where the engine has an
//! opinion: a `vmess://` link's `aid` is carried rather than dropped because
//! `xray run -test` accepts a non-zero one, and `mode=multi`, `type=xhttp`,
//! `type=raw` and `headerType=http` are refused because this model has nowhere to
//! put them - see `crates/runtime/tests/xray_real.rs` for the configs the engine
//! accepts.

use crate::proxy::{
    GrpcSettings, ProxyOutbound, RealitySettings, ShadowsocksOutbound, StreamNetwork,
    StreamSecurity, StreamSettings, TlsSettings, TrojanOutbound, VlessOutbound, VmessOutbound,
    WsSettings,
};
use crate::validation::validate_stream;
use base64::Engine as _;
use percent_encoding::percent_decode_str;
use serde_json::Value;
use std::borrow::Cow;
use thiserror::Error;
use url::Url;

/// What a link turned into, and what to call it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedProxy {
    /// The remark the link carries: `ps` for `vmess://`, else the fragment.
    pub name: Option<String>,
    pub outbound: ProxyOutbound,
}

impl ParsedProxy {
    /// The name to store this proxy under: the link's own remark, or the
    /// protocol and host when it carries none.
    pub fn suggested_name(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => format!("{} {}", self.outbound.kind(), self.outbound.host()),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UriError {
    #[error("nothing was pasted")]
    Empty,

    #[error("the link has no scheme: expected ss://, vmess://, vless:// or trojan://")]
    MissingScheme,

    #[error(
        "this program cannot read a {scheme}:// link; it reads ss://, vmess://, vless:// and trojan://"
    )]
    UnsupportedScheme { scheme: String },

    #[error("the {scheme}:// link does not carry {field}")]
    Missing {
        scheme: &'static str,
        field: &'static str,
    },

    #[error("the {field} in this {scheme}:// link is not a number: {value:?}")]
    NotANumber {
        scheme: &'static str,
        field: &'static str,
        value: String,
    },

    #[error("the {scheme}:// link is not valid base64")]
    NotBase64 { scheme: &'static str },

    #[error("the {scheme}:// link is not valid UTF-8 where text was expected")]
    NotUtf8 { scheme: &'static str },

    #[error("the {scheme}:// link could not be read: {detail}")]
    Malformed {
        scheme: &'static str,
        detail: String,
    },

    #[error("this program cannot carry {field}={value:?} from a link: {reason}")]
    Unsupported {
        field: &'static str,
        value: String,
        reason: &'static str,
    },

    #[error("the link parses, but the proxy it describes would not launch: {detail}")]
    WouldNotLaunch { detail: String },
}

/// Reads a share link into the model, or says why it cannot.
pub fn parse_proxy_uri(input: &str) -> Result<ParsedProxy, UriError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(UriError::Empty);
    }
    let (scheme, _) = input.split_once("://").ok_or(UriError::MissingScheme)?;
    match scheme.to_ascii_lowercase().as_str() {
        "ss" => parse_ss(input),
        "vmess" => parse_vmess(input),
        "vless" => parse_url_link(input, "vless", StreamSecurity::None),
        // Trojan is TLS in every deployment there is. The engine accepts a
        // plain one, so a link that says `security=none` is honoured - but a
        // link that says nothing is not asking for a config that no server runs.
        "trojan" => parse_url_link(input, "trojan", StreamSecurity::Tls),
        other => Err(UriError::UnsupportedScheme {
            scheme: other.to_ascii_lowercase(),
        }),
    }
}

// --- vless and trojan: `scheme://secret@host:port?params#name` ---

fn parse_url_link(
    input: &str,
    scheme: &'static str,
    default_security: StreamSecurity,
) -> Result<ParsedProxy, UriError> {
    let url = Url::parse(input).map_err(|error| UriError::Malformed {
        scheme,
        detail: error.to_string(),
    })?;

    // `host_str` spells an IPv6 address with the brackets the URL syntax needs;
    // the engine's `address` field takes it without them (measured: both are
    // accepted, and bare is what the rest of this program stores).
    let host = url
        .host_str()
        .map(bare_host)
        .filter(|host| !host.is_empty())
        .ok_or(UriError::Missing {
            scheme,
            field: "host",
        })?;
    let port = url
        .port()
        .filter(|port| *port > 0)
        .ok_or(UriError::Missing {
            scheme,
            field: "port",
        })?;

    let secret_field = if scheme == "vless" {
        "uuid"
    } else {
        "password"
    };
    let secret = percent_decode(url.username(), scheme)?;
    let secret = text(&secret).ok_or(UriError::Missing {
        scheme,
        field: secret_field,
    })?;

    let transport = Transport::from_pairs(url.query_pairs());
    let stream = transport.stream(scheme, default_security)?;
    let name = url
        .fragment()
        .map(|fragment| percent_decode(fragment, scheme))
        .transpose()?
        .and_then(|fragment| text(&fragment));

    let outbound = if scheme == "vless" {
        ProxyOutbound::Vless(VlessOutbound {
            host,
            port,
            uuid: secret,
            flow: transport.flow.clone(),
            encryption: transport
                .encryption
                .clone()
                .unwrap_or_else(|| "none".to_string()),
            stream,
        })
    } else {
        ProxyOutbound::Trojan(TrojanOutbound {
            host,
            port,
            password: secret,
            stream,
        })
    };

    Ok(ParsedProxy { name, outbound })
}

// --- ss: `ss://base64(method:password)@host:port` and two other shapes ---

fn parse_ss(input: &str) -> Result<ParsedProxy, UriError> {
    const SCHEME: &str = "ss";
    let rest = after_scheme(input);

    // The fragment is the remark; a plugin or a transport would follow a `?`.
    let (rest, fragment) = match rest.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (rest, None),
    };
    let (rest, query) = match rest.split_once('?') {
        Some((rest, query)) => (rest, Some(query)),
        None => (rest, None),
    };

    let name = fragment
        .map(|fragment| percent_decode(fragment, SCHEME))
        .transpose()?
        .and_then(|fragment| text(&fragment));

    // Two shapes carry the credentials before the `@`: base64 of
    // `method:password`, or the pair in the clear. A link with no `@` at all is
    // base64 of the whole `method:password@host:port`.
    let (credentials, host_port) = match rest.rsplit_once('@') {
        Some((userinfo, host_port)) => (decode_ss_credentials(userinfo), host_port.to_string()),
        None => {
            let decoded = decode_base64(rest).ok_or(UriError::NotBase64 { scheme: SCHEME })?;
            let decoded =
                String::from_utf8(decoded).map_err(|_| UriError::NotUtf8 { scheme: SCHEME })?;
            let (userinfo, host_port) = decoded.rsplit_once('@').ok_or(UriError::Missing {
                scheme: SCHEME,
                field: "host",
            })?;
            (userinfo.to_string(), host_port.to_string())
        }
    };

    let (method, password) = credentials.split_once(':').ok_or(UriError::Missing {
        scheme: SCHEME,
        field: "method",
    })?;
    let method = text(method).ok_or(UriError::Missing {
        scheme: SCHEME,
        field: "method",
    })?;
    let password = text(password).ok_or(UriError::Missing {
        scheme: SCHEME,
        field: "password",
    })?;
    let (host, port) = split_host_port(&host_port, SCHEME)?;

    let transport = Transport::from_pairs(url::form_urlencoded::parse(
        query.unwrap_or_default().as_bytes(),
    ));
    let stream = transport.stream(SCHEME, StreamSecurity::None)?;

    Ok(ParsedProxy {
        name,
        outbound: ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
            host,
            port,
            password,
            method,
            stream,
        }),
    })
}

/// `ss://` credentials are base64 in most links and in the clear in some.
fn decode_ss_credentials(userinfo: &str) -> String {
    match decode_base64(userinfo) {
        Some(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| userinfo.to_string()),
        None => userinfo.to_string(),
    }
}

// --- vmess: `vmess://base64({json})` ---

fn parse_vmess(input: &str) -> Result<ParsedProxy, UriError> {
    const SCHEME: &str = "vmess";
    let rest = after_scheme(input);
    let payload = rest.split_once('#').map_or(rest, |(payload, _)| payload);

    let decoded = decode_base64(payload).ok_or(UriError::NotBase64 { scheme: SCHEME })?;
    let json: Value = serde_json::from_slice(&decoded).map_err(|_| UriError::Malformed {
        scheme: SCHEME,
        detail: "the base64 does not hold a JSON object".to_string(),
    })?;
    let field = |name: &str| json.get(name).and_then(json_text);

    let host = field("add")
        .and_then(|value| text(&value))
        .ok_or(UriError::Missing {
            scheme: SCHEME,
            field: "add",
        })?;
    let port = count(field("port"), SCHEME, "port")?;
    let port = u16::try_from(port)
        .ok()
        .filter(|port| *port > 0)
        .ok_or(UriError::NotANumber {
            scheme: SCHEME,
            field: "port",
            value: field("port").unwrap_or_default(),
        })?;
    let uuid = field("id")
        .and_then(|value| text(&value))
        .ok_or(UriError::Missing {
            scheme: SCHEME,
            field: "id",
        })?;
    let alter_id = match field("aid") {
        Some(value) => count(Some(value), SCHEME, "aid")?,
        None => 0,
    };

    let transport = Transport {
        network: field("net"),
        path: field("path"),
        host: field("host"),
        service_name: field("serviceName"),
        // In a vmess object `tls` is the transport's security and `scy`/`security`
        // is the cipher, which are two different things with one name.
        security: field("tls"),
        server_name: field("sni"),
        fingerprint: field("fp"),
        alpn: field("alpn"),
        header_type: field("type"),
        ..Default::default()
    };
    let stream = transport.stream(SCHEME, StreamSecurity::None)?;

    let name = field("ps").and_then(|value| text(&value));
    let security = field("scy")
        .or_else(|| field("security"))
        .and_then(|value| text(&value))
        .unwrap_or_else(|| "auto".to_string());

    Ok(ParsedProxy {
        name,
        outbound: ProxyOutbound::Vmess(VmessOutbound {
            host,
            port,
            uuid,
            security,
            alter_id,
            stream,
        }),
    })
}

// --- the transport and security fields every scheme spells the same way ---

/// The half of a link that decides *how* the connection is carried, gathered
/// from a query string or from a vmess object's own field names.
#[derive(Debug, Default)]
struct Transport {
    network: Option<String>,
    path: Option<String>,
    host: Option<String>,
    service_name: Option<String>,
    security: Option<String>,
    server_name: Option<String>,
    fingerprint: Option<String>,
    alpn: Option<String>,
    public_key: Option<String>,
    short_id: Option<String>,
    spider_x: Option<String>,
    header_type: Option<String>,
    plugin: Option<String>,
    mode: Option<String>,
    flow: Option<String>,
    encryption: Option<String>,
}

impl Transport {
    fn from_pairs<'a>(pairs: impl Iterator<Item = (Cow<'a, str>, Cow<'a, str>)>) -> Self {
        let mut transport = Self::default();
        for (key, value) in pairs {
            let value = value.trim().to_string();
            if value.is_empty() {
                continue;
            }
            match key.trim().to_ascii_lowercase().as_str() {
                "type" | "network" => transport.network = Some(value),
                "path" => transport.path = Some(value),
                "host" => transport.host = Some(value),
                "servicename" | "service-name" | "service_name" => {
                    transport.service_name = Some(value)
                }
                "security" => transport.security = Some(value),
                "sni" | "peer" | "servername" | "server_name" => {
                    transport.server_name = Some(value)
                }
                "fp" | "fingerprint" | "client-fingerprint" => transport.fingerprint = Some(value),
                "alpn" => transport.alpn = Some(value),
                "pbk" | "publickey" | "public-key" => transport.public_key = Some(value),
                "sid" | "shortid" | "short-id" => transport.short_id = Some(value),
                "spx" | "spiderx" | "spider-x" => transport.spider_x = Some(value),
                "headertype" | "header-type" => transport.header_type = Some(value),
                "plugin" => transport.plugin = Some(value),
                "mode" => transport.mode = Some(value),
                "flow" => transport.flow = Some(value),
                "encryption" => transport.encryption = Some(value),
                // Anything else is another client's business.
                _ => {}
            }
        }
        transport
    }

    fn stream(
        &self,
        scheme: &'static str,
        default_security: StreamSecurity,
    ) -> Result<StreamSettings, UriError> {
        self.refuse_what_this_program_cannot_carry()?;

        let network = match lower(&self.network).as_deref() {
            None | Some("tcp") => StreamNetwork::Tcp,
            Some("ws") | Some("websocket") => StreamNetwork::Ws,
            Some("grpc") | Some("gun") => StreamNetwork::Grpc,
            Some(other) => {
                return Err(UriError::Unsupported {
                    field: "type",
                    value: other.to_string(),
                    reason: "only tcp, ws and grpc are modelled",
                });
            }
        };

        let security = match lower(&self.security).as_deref() {
            None => default_security,
            Some("none") => StreamSecurity::None,
            Some("tls") => StreamSecurity::Tls,
            Some("reality") => StreamSecurity::Reality,
            Some(other) => {
                return Err(UriError::Unsupported {
                    field: "security",
                    value: other.to_string(),
                    reason: "only none, tls and reality are modelled",
                });
            }
        };

        let tls_asked_for =
            self.server_name.is_some() || self.fingerprint.is_some() || self.alpn.is_some();
        let reality_asked_for =
            self.public_key.is_some() || self.short_id.is_some() || self.spider_x.is_some();

        let mut stream = StreamSettings {
            network,
            security,
            ..Default::default()
        };
        match security {
            StreamSecurity::None => {
                if tls_asked_for {
                    return Err(UriError::Unsupported {
                        field: "sni",
                        value: self.server_name.clone().unwrap_or_default(),
                        reason: "the link asks for no tls, so nothing would read it",
                    });
                }
                if reality_asked_for {
                    return Err(UriError::Unsupported {
                        field: "pbk",
                        value: self.public_key.clone().unwrap_or_default(),
                        reason: "the link asks for no reality, so nothing would read it",
                    });
                }
            }
            StreamSecurity::Tls => {
                if reality_asked_for {
                    return Err(UriError::Unsupported {
                        field: "pbk",
                        value: self.public_key.clone().unwrap_or_default(),
                        reason: "the link asks for tls, not reality",
                    });
                }
                if tls_asked_for {
                    stream.tls = Some(TlsSettings {
                        server_name: self.server_name.clone(),
                        fingerprint: self.fingerprint.clone(),
                        alpn: self.alpn_list(),
                    });
                }
            }
            StreamSecurity::Reality => {
                let public_key = text(self.public_key.as_deref().unwrap_or_default()).ok_or(
                    UriError::Missing {
                        scheme,
                        field: "pbk",
                    },
                )?;
                stream.reality = Some(RealitySettings {
                    server_name: self.server_name.clone(),
                    public_key,
                    short_id: self.short_id.clone(),
                    fingerprint: self.fingerprint.clone(),
                    spider_x: self.spider_x.clone(),
                });
            }
        }

        match network {
            StreamNetwork::Tcp => {
                if self.path.is_some() || self.host.is_some() {
                    return Err(UriError::Unsupported {
                        field: "path",
                        value: self.path.clone().unwrap_or_default(),
                        reason: "the link asks for no websocket, so nothing would read it",
                    });
                }
            }
            StreamNetwork::Ws => {
                stream.ws = Some(WsSettings {
                    path: self.path.clone(),
                    host: self.host.clone(),
                });
            }
            StreamNetwork::Grpc => {
                let service_name = text(self.service_name.as_deref().unwrap_or_default()).ok_or(
                    UriError::Missing {
                        scheme,
                        field: "serviceName",
                    },
                )?;
                stream.grpc = Some(GrpcSettings { service_name });
            }
        }

        // The same rules the editor refuses a save on, so a link that parses is
        // a link that could have been typed in.
        validate_stream(&stream).map_err(|error| UriError::WouldNotLaunch {
            detail: error.to_string(),
        })?;

        Ok(stream)
    }

    /// The fields this model has nowhere to put, refused by name rather than
    /// dropped. Each one changes what the server expects to receive.
    fn refuse_what_this_program_cannot_carry(&self) -> Result<(), UriError> {
        if let Some(plugin) = text(self.plugin.as_deref().unwrap_or_default()) {
            return Err(UriError::Unsupported {
                field: "plugin",
                value: plugin,
                reason: "this program starts Xray itself and does not run a shadowsocks plugin",
            });
        }
        if let Some(header_type) = lower(&self.header_type)
            && header_type != "none"
        {
            return Err(UriError::Unsupported {
                field: "headerType",
                value: header_type,
                reason: "tcp header obfuscation is not modelled",
            });
        }
        if let Some(mode) = lower(&self.mode)
            && mode != "gun"
        {
            return Err(UriError::Unsupported {
                field: "mode",
                value: mode,
                reason: "only the gRPC gun mode is modelled",
            });
        }
        Ok(())
    }

    fn alpn_list(&self) -> Vec<String> {
        self.alpn
            .as_deref()
            .map(|alpn| alpn.split(',').filter_map(text).collect::<Vec<_>>())
            .unwrap_or_default()
    }
}

// --- small shared pieces ---

/// Everything after `scheme://`.
fn after_scheme(input: &str) -> &str {
    match input.find("://") {
        Some(index) => &input[index + 3..],
        None => input,
    }
}

/// `[2001:db8::1]` -> `2001:db8::1`.
fn bare_host(host: &str) -> String {
    let host = host.trim();
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .to_string()
}

fn text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn lower(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn percent_decode(value: &str, scheme: &'static str) -> Result<String, UriError> {
    percent_decode_str(value)
        .decode_utf8()
        .map(|decoded| decoded.to_string())
        .map_err(|_| UriError::NotUtf8 { scheme })
}

/// Links arrive in both base64 alphabets, with and without padding.
fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let value = value.trim();
    let engines = [
        base64::engine::general_purpose::STANDARD,
        base64::engine::general_purpose::STANDARD_NO_PAD,
        base64::engine::general_purpose::URL_SAFE,
        base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ];
    engines.iter().find_map(|engine| engine.decode(value).ok())
}

fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text_value) => Some(text_value.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A JSON number or a JSON string holding a number, both of which appear in the
/// wild for a vmess link's `port` and `aid`.
fn count(
    value: Option<String>,
    scheme: &'static str,
    field: &'static str,
) -> Result<u32, UriError> {
    let value = value.ok_or(UriError::Missing { scheme, field })?;
    value
        .trim()
        .parse::<u32>()
        .map_err(|_| UriError::NotANumber {
            scheme,
            field,
            value: value.trim().to_string(),
        })
}

/// `host:port`, with the brackets an IPv6 address may arrive in.
fn split_host_port(value: &str, scheme: &'static str) -> Result<(String, u16), UriError> {
    let value = value.trim();
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        rest.split_once(']')
            .map(|(host, rest)| (host, rest.trim_start_matches(':')))
            .ok_or(UriError::Missing {
                scheme,
                field: "port",
            })?
    } else {
        value.rsplit_once(':').ok_or(UriError::Missing {
            scheme,
            field: "port",
        })?
    };

    let host = text(host).ok_or(UriError::Missing {
        scheme,
        field: "host",
    })?;
    let port = port
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or(UriError::NotANumber {
            scheme,
            field: "port",
            value: port.trim().to_string(),
        })?;
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::validate_proxy;
    use crate::{ProxyId, ProxyProfile};

    const UUID: &str = "b831381d-6324-4d53-ad4f-8cda48b30811";
    const PUBLIC_KEY: &str = "LOLTG162EtSegCnMAofVY3oKrbrCvH8zOTZEPd1GRQU";

    fn parsed(input: &str) -> ParsedProxy {
        parse_proxy_uri(input).expect("the link should parse")
    }

    fn outbound(input: &str) -> ProxyOutbound {
        parsed(input).outbound
    }

    fn base64(value: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(value)
    }

    fn vless(link: &str) -> VlessOutbound {
        match outbound(link) {
            ProxyOutbound::Vless(vless) => vless,
            other => panic!("expected a vless outbound, got {other:?}"),
        }
    }

    fn trojan(link: &str) -> TrojanOutbound {
        match outbound(link) {
            ProxyOutbound::Trojan(trojan) => trojan,
            other => panic!("expected a trojan outbound, got {other:?}"),
        }
    }

    fn vmess(link: &str) -> VmessOutbound {
        match outbound(link) {
            ProxyOutbound::Vmess(vmess) => vmess,
            other => panic!("expected a vmess outbound, got {other:?}"),
        }
    }

    fn shadowsocks(link: &str) -> ShadowsocksOutbound {
        match outbound(link) {
            ProxyOutbound::Shadowsocks(shadowsocks) => shadowsocks,
            other => panic!("expected a shadowsocks outbound, got {other:?}"),
        }
    }

    #[test]
    fn a_vless_reality_link_keeps_every_field_the_engine_reads() {
        let link = format!(
            "vless://{UUID}@node.example:443?encryption=none&flow=xtls-rprx-vision\
             &security=reality&sni=front.example&fp=chrome&pbk={PUBLIC_KEY}&sid=ab12\
             &spx=%2F&type=tcp#Tokyo"
        );
        let parsed = parsed(&link);
        let vless = vless(&link);
        assert_eq!(vless.host, "node.example");
        assert_eq!(vless.port, 443);
        assert_eq!(vless.uuid, UUID);
        assert_eq!(vless.flow.as_deref(), Some("xtls-rprx-vision"));
        assert_eq!(vless.encryption, "none");
        assert_eq!(vless.stream.network, StreamNetwork::Tcp);
        assert_eq!(vless.stream.security, StreamSecurity::Reality);
        let reality = vless.stream.reality.expect("reality settings");
        assert_eq!(reality.public_key, PUBLIC_KEY);
        assert_eq!(reality.server_name.as_deref(), Some("front.example"));
        assert_eq!(reality.short_id.as_deref(), Some("ab12"));
        assert_eq!(reality.fingerprint.as_deref(), Some("chrome"));
        // Percent-encoded, as a share link spells the spider path.
        assert_eq!(reality.spider_x.as_deref(), Some("/"));
        assert_eq!(parsed.suggested_name(), "Tokyo");
    }

    #[test]
    fn a_vless_websocket_link_carries_tls_and_the_host_header() {
        let vless = vless(&format!(
            "vless://{UUID}@node.example:443?encryption=none&security=tls\
             &sni=front.example&type=ws&path=%2Fws&host=front.example\
             &alpn=h2%2Chttp%2F1.1#WS"
        ));
        assert_eq!(vless.stream.network, StreamNetwork::Ws);
        assert_eq!(vless.stream.security, StreamSecurity::Tls);
        let tls = vless.stream.tls.expect("tls settings");
        assert_eq!(tls.server_name.as_deref(), Some("front.example"));
        assert_eq!(tls.alpn, vec!["h2".to_string(), "http/1.1".to_string()]);
        let ws = vless.stream.ws.expect("ws settings");
        assert_eq!(ws.path.as_deref(), Some("/ws"));
        assert_eq!(ws.host.as_deref(), Some("front.example"));
    }

    #[test]
    fn a_vless_link_without_tls_keeps_only_what_it_asked_for() {
        let vless = vless(&format!("vless://{UUID}@node.example:443?encryption=none"));
        assert_eq!(vless.stream, StreamSettings::plain());
        // The protocol needs the field written down.
        assert_eq!(vless.encryption, "none");
        assert_eq!(vless.flow, None);
    }

    #[test]
    fn a_vmess_link_is_its_json() {
        let json = format!(
            r#"{{"v":"2","ps":"Osaka","add":"node.example","port":"443","id":"{UUID}",
                "aid":"64","scy":"auto","net":"ws","type":"none","host":"front.example",
                "path":"/ws","tls":"tls","sni":"front.example","alpn":"h2"}}"#
        );
        let link = format!("vmess://{}", base64(&json));
        let parsed = parsed(&link);
        let vmess = vmess(&link);
        assert_eq!(vmess.host, "node.example");
        assert_eq!(vmess.port, 443);
        assert_eq!(vmess.uuid, UUID);
        assert_eq!(vmess.security, "auto");
        // Read by the engine, so carried rather than dropped.
        assert_eq!(vmess.alter_id, 64);
        assert_eq!(vmess.stream.network, StreamNetwork::Ws);
        assert_eq!(vmess.stream.security, StreamSecurity::Tls);
        assert_eq!(
            vmess.stream.tls.as_ref().unwrap().alpn,
            vec!["h2".to_string()]
        );
        // `ps` is the remark a vmess link carries.
        assert_eq!(parsed.suggested_name(), "Osaka");
    }

    #[test]
    fn a_vmess_link_with_nothing_extra_gets_the_engine_defaults() {
        let json = format!(r#"{{"add":"node.example","port":10086,"id":"{UUID}"}}"#);
        let vmess = vmess(&format!("vmess://{}", base64(&json)));
        assert_eq!(vmess.alter_id, 0);
        assert_eq!(vmess.security, "auto");
        assert_eq!(vmess.stream, StreamSettings::plain());
    }

    #[test]
    fn a_trojan_link_is_tls_even_when_it_does_not_say_so() {
        let trojan = trojan("trojan://secret@node.example:443?sni=front.example#Trojan");
        assert_eq!(trojan.password, "secret");
        assert_eq!(trojan.stream.security, StreamSecurity::Tls);
        assert_eq!(
            trojan.stream.tls.as_ref().unwrap().server_name.as_deref(),
            Some("front.example")
        );
    }

    #[test]
    fn a_trojan_link_that_asks_for_no_tls_is_taken_at_its_word() {
        // The engine accepts a plain trojan outbound, so this is offered rather
        // than silently turned into TLS.
        let trojan = trojan("trojan://secret@node.example:443?security=none");
        assert_eq!(trojan.stream.security, StreamSecurity::None);
        assert_eq!(trojan.stream, StreamSettings::plain());
    }

    #[test]
    fn a_trojan_link_can_ask_for_reality() {
        let trojan = trojan(&format!(
            "trojan://secret@node.example:443?security=reality&pbk={PUBLIC_KEY}&sni=front.example&sid=ab12"
        ));
        assert_eq!(trojan.stream.security, StreamSecurity::Reality);
        assert_eq!(
            trojan.stream.reality.as_ref().unwrap().public_key,
            PUBLIC_KEY
        );
    }

    #[test]
    fn a_vless_link_can_ask_for_grpc() {
        let vless = vless(&format!(
            "vless://{UUID}@node.example:443?encryption=none&type=grpc&serviceName=svc&security=tls"
        ));
        assert_eq!(vless.stream.network, StreamNetwork::Grpc);
        assert_eq!(vless.stream.grpc.as_ref().unwrap().service_name, "svc");
    }

    #[test]
    fn shadowsocks_links_arrive_in_three_shapes() {
        let credentials = base64("aes-256-gcm:secret");

        // base64 credentials before the `@`, and a percent-encoded remark.
        let first = shadowsocks(&format!("ss://{credentials}@node.example:8388#Osaka%20Two"));
        assert_eq!(first.host, "node.example");
        assert_eq!(first.port, 8388);
        assert_eq!(first.method, "aes-256-gcm");
        assert_eq!(first.password, "secret");
        assert_eq!(
            parsed(&format!("ss://{credentials}@node.example:8388#Osaka%20Two")).suggested_name(),
            "Osaka Two"
        );

        // The whole thing base64.
        let whole = base64("aes-256-gcm:secret@node.example:8388");
        let second = shadowsocks(&format!("ss://{whole}"));
        assert_eq!(second.host, "node.example");
        assert_eq!(second.password, "secret");

        // Credentials in the clear, which some clients hand out.
        let third = shadowsocks("ss://aes-256-gcm:secret@node.example:8388");
        assert_eq!(third.method, "aes-256-gcm");
        assert_eq!(third.password, "secret");
    }

    #[test]
    fn a_percent_encoded_password_and_an_ipv6_host_survive() {
        let trojan = trojan("trojan://p%40ss%3Aword@[2001:db8::1]:443?sni=front.example");
        assert_eq!(trojan.password, "p@ss:word");
        assert_eq!(trojan.host, "2001:db8::1");
        assert_eq!(trojan.port, 443);
    }

    #[test]
    fn a_link_this_program_cannot_carry_is_refused_by_name() {
        let cases = [
            ("hysteria2://secret@node.example:443", "hysteria2"),
            (
                "vless://uuid@node.example:443?encryption=none&type=kcp",
                "type",
            ),
            (
                "vless://uuid@node.example:443?encryption=none&type=xhttp&path=%2Fx",
                "type",
            ),
            (
                "vless://uuid@node.example:443?encryption=none&type=raw",
                "type",
            ),
            (
                "vless://uuid@node.example:443?encryption=none&headerType=http",
                "headerType",
            ),
            (
                "vless://uuid@node.example:443?encryption=none&type=grpc&serviceName=s&mode=multi",
                "mode",
            ),
            (
                "ss://aes-256-gcm:secret@node.example:8388?plugin=obfs-local",
                "plugin",
            ),
            // A tls field with no tls selected would be read by nobody.
            (
                "vless://uuid@node.example:443?encryption=none&sni=front.example",
                "sni",
            ),
            // And reality over a websocket is refused by the engine itself, so
            // it never reaches it from here either.
            (
                "vless://uuid@node.example:443?encryption=none&security=reality&type=ws&pbk=key",
                "REALITY",
            ),
        ];
        for (link, expected) in cases {
            let error = parse_proxy_uri(link).expect_err(link);
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "the refusal should name {expected:?}: {message}"
            );
        }
    }

    #[test]
    fn a_link_with_nothing_in_it_says_so() {
        assert_eq!(parse_proxy_uri("   "), Err(UriError::Empty));
        assert_eq!(parse_proxy_uri("not a link"), Err(UriError::MissingScheme));
        assert_eq!(
            parse_proxy_uri("vless://"),
            Err(UriError::Missing {
                scheme: "vless",
                field: "host"
            })
        );
        assert_eq!(
            parse_proxy_uri("vless://uuid@node.example"),
            Err(UriError::Missing {
                scheme: "vless",
                field: "port"
            })
        );
        assert_eq!(
            parse_proxy_uri("vmess://not-base64!!"),
            Err(UriError::NotBase64 { scheme: "vmess" })
        );
        assert_eq!(
            parse_proxy_uri(&format!("vmess://{}", base64("{}"))),
            Err(UriError::Missing {
                scheme: "vmess",
                field: "add"
            })
        );
        assert_eq!(
            parse_proxy_uri(&format!(
                "vmess://{}",
                base64(r#"{"add":"node.example","port":"eight","id":"uuid"}"#)
            )),
            Err(UriError::NotANumber {
                scheme: "vmess",
                field: "port",
                value: "eight".to_string()
            })
        );
    }

    #[test]
    fn a_parameter_this_program_has_never_seen_is_passed_over() {
        // Providers add their own parameters; refusing a link over one would
        // refuse links that work.
        let vless = vless(&format!(
            "vless://{UUID}@node.example:443?encryption=none&utm_source=share&remark=best"
        ));
        assert_eq!(vless.stream, StreamSettings::plain());
        assert!(
            parse_proxy_uri(&format!(
                "vless://{UUID}@node.example:443?encryption=none&utm_source=share"
            ))
            .is_ok()
        );
    }

    #[test]
    fn an_uppercase_scheme_is_still_a_link() {
        let trojan = trojan("TROJAN://secret@node.example:443?sni=front.example");
        assert_eq!(trojan.password, "secret");
    }

    /// The invariant that makes the refusals above mean something: anything this
    /// parser accepts is something the editor would also have accepted, so an
    /// import cannot land a row that fails at the next start.
    #[test]
    fn everything_that_parses_would_also_validate() {
        let links = [
            format!("vless://{UUID}@node.example:443?encryption=none"),
            format!(
                "vless://{UUID}@node.example:443?encryption=none&security=reality&pbk={PUBLIC_KEY}&sni=front.example"
            ),
            format!("vless://{UUID}@node.example:443?encryption=none&type=grpc&serviceName=svc"),
            format!(
                "vmess://{}",
                base64(&format!(
                    r#"{{"add":"node.example","port":443,"id":"{UUID}","net":"ws","path":"/ws","tls":"tls","sni":"front.example"}}"#
                ))
            ),
            "trojan://secret@node.example:443".to_string(),
            "ss://aes-256-gcm:secret@node.example:8388".to_string(),
        ];
        for link in links {
            let outbound = outbound(&link);
            let profile = ProxyProfile {
                id: ProxyId::new(),
                name: "imported".to_string(),
                outbound,
            };
            assert_eq!(validate_proxy(&profile), Ok(()), "{link}");
        }
    }
}
