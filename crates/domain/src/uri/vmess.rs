use super::*;

// --- vmess: `vmess://base64({json})` ---

pub(super) fn parse_vmess(input: &str) -> Result<ParsedProxy, UriError> {
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
