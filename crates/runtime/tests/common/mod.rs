//! Helpers shared by the opt-in real-machine suites.
//!
//! Every integration test compiles this module on its own, so a helper only
//! some of them use is not dead code in the ones that do not.
#![allow(dead_code)]

use domain::{ProxyId, ProxyOutbound, ProxyProfile, Socks5Outbound};

/// The upstream to test, taken from the environment.
///
/// Either a share link, or the plain `host:port[:user:password]` of a SOCKS5
/// upstream - which is what a self-hosted proxy usually is, and which no share
/// link expresses. `proxy_diag_real` proves the engine carries a request
/// through it; `windows_real` proves a profile's browser leaves from it.
pub fn upstream() -> ProxyProfile {
    if let Ok(uri) = std::env::var("PROXY_URI") {
        let parsed = domain::parse_proxy_uri(&uri).expect("PROXY_URI should be a share link");
        return ProxyProfile {
            id: ProxyId::new(),
            name: parsed.suggested_name(),
            outbound: parsed.outbound,
        };
    }

    let spec = std::env::var("DIAG_SOCKS5").expect(
        "set PROXY_URI to a share link, or DIAG_SOCKS5 to host:port[:user:password] of a SOCKS5 \
         upstream",
    );
    let mut parts = spec.splitn(3, ':');
    let host = parts.next().filter(|h| !h.is_empty()).expect("a host");
    let port: u16 = parts
        .next()
        .expect("a port")
        .parse()
        .expect("a port number");
    let (username, password) = match parts.next() {
        Some(credentials) => {
            let (user, pass) = credentials
                .split_once(':')
                .expect("credentials as user:password");
            (Some(user.to_string()), Some(pass.to_string()))
        }
        None => (None, None),
    };
    ProxyProfile {
        id: ProxyId::new(),
        name: format!("{host}:{port}"),
        outbound: ProxyOutbound::Socks5(Socks5Outbound {
            host: host.to_string(),
            port,
            username,
            password,
        }),
    }
}
