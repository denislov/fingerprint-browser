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

#[test]
fn encoded_credentials_preserve_leading_and_trailing_whitespace() {
    assert_eq!(
        trojan("trojan://%20secret%20@example.org:443").password,
        " secret "
    );
    for link in [
        format!("ss://{}@example.org:8388", base64("aes-256-gcm: secret ")),
        format!("ss://{}", base64("aes-256-gcm: secret @example.org:8388")),
    ] {
        let ProxyOutbound::Shadowsocks(proxy) = outbound(&link) else {
            panic!("shadowsocks")
        };
        assert_eq!(proxy.password, " secret ");
    }
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
