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

mod shadowsocks;
mod transport;
mod url_link;
mod vmess;
use shadowsocks::parse_ss;
use transport::Transport;
use url_link::parse_url_link;
use vmess::parse_vmess;

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

/// Credentials are opaque; whitespace decoded from a link is part of the key.
fn credential(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
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
mod tests;
