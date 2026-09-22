use super::*;

// --- ss: `ss://base64(method:password)@host:port` and two other shapes ---

pub(super) fn parse_ss(input: &str) -> Result<ParsedProxy, UriError> {
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
    let password = credential(password).ok_or(UriError::Missing {
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
pub(super) fn decode_ss_credentials(userinfo: &str) -> String {
    match decode_base64(userinfo) {
        Some(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| userinfo.to_string()),
        None => userinfo.to_string(),
    }
}
