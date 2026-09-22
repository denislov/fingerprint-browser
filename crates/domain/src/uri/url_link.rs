use super::*;

// --- vless and trojan: `scheme://secret@host:port?params#name` ---

pub(super) fn parse_url_link(
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
    let secret = if scheme == "trojan" {
        credential(&secret)
    } else {
        text(&secret)
    }
    .ok_or(UriError::Missing {
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
