//! Opt-in integration: a real engine, a real upstream, a real endpoint.
//!
//! The unit tests in `runtime::diagnostic` prove the classification and the
//! engine's lifetime against local stubs, which is where the interesting logic
//! is. What they cannot show is that the conversation this module writes is one
//! a real Xray answers - so this is the check that the hand-written SOCKS5
//! handshake, the address encoding and the reply decoding all agree with an
//! engine that was not written to match them.
//!
//! ```text
//! XRAY_BIN=/path/xray DIAG_SOCKS5=127.0.0.1:1080 DIAG_ECHO_URL=http://api.ipify.org \
//!   cargo test -p runtime --test proxy_diag_real -- --ignored
//!
//! XRAY_BIN=/path/xray PROXY_URI='vless://...' DIAG_ECHO_URL=http://... \
//!   cargo test -p runtime --test proxy_diag_real -- --ignored
//! ```

use domain::{ProxyId, ProxyOutbound, ProxyProfile, Socks5Outbound};
use runtime::diagnostic::{DIAGNOSTIC_CONFIG_FILE, DIAGNOSTIC_LOG_FILE};
use runtime::{
    DefaultXrayConfigBuilder, DiagnosticRequest, Engine, FaultClass, SocksEchoClient,
    diagnose_proxy,
};
use std::path::Path;
use std::time::Duration;

/// How long the probe may take. Longer than the shipped default: this runs over
/// whatever the operator's upstream actually is, not over loopback.
const PROBE: Duration = Duration::from_secs(30);

/// The upstream to test, taken from the environment.
///
/// Either a share link, or the plain `host:port[:user:password]` of a SOCKS5
/// upstream - which is what a self-hosted proxy usually is, and which no share
/// link expresses.
fn upstream() -> ProxyProfile {
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

fn engine_executable() -> std::ffi::OsString {
    std::env::var_os("XRAY_BIN").expect("set XRAY_BIN to a real Xray executable")
}

/// A directory of its own per run, removed by the caller.
fn directory(proxy: &ProxyProfile, suffix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("fp-diag-real-{suffix}-{}", proxy.id))
}

/// Where a temporary engine writes is its own business, but that it takes the
/// credentials away with it is not: this is the promise the module makes on
/// every path, and the reason the config lives beside a profile's instead of in
/// a shared place.
fn assert_nothing_left_behind(directory: &Path) {
    for name in [DIAGNOSTIC_CONFIG_FILE, DIAGNOSTIC_LOG_FILE] {
        let path = directory.join(name);
        assert!(
            !path.exists(),
            "{} outlived the diagnostic that wrote it",
            path.display()
        );
    }
}

/// The whole conversation, end to end, against an engine this project did not
/// write.
#[test]
#[ignore = "requires XRAY_BIN, a reachable upstream and DIAG_ECHO_URL"]
fn a_real_engine_reports_where_a_real_request_left_from() {
    let echo_url = std::env::var("DIAG_ECHO_URL").expect(
        "set DIAG_ECHO_URL to an http:// endpoint the upstream can reach, so the reading is a \
         measurement and not a hope",
    );
    assert!(
        echo_url.starts_with("http://"),
        "the diagnostic refuses anything but plain http on purpose: {echo_url}"
    );

    let executable = engine_executable();
    let proxy = upstream();
    let directory = directory(&proxy, "carries");
    let _ = std::fs::remove_dir_all(&directory);

    let outcome = diagnose_proxy(
        &SocksEchoClient,
        &DefaultXrayConfigBuilder,
        &proxy,
        &DiagnosticRequest {
            engine: Engine::Temporary {
                executable: Path::new(&executable),
                directory: &directory,
                ready_timeout: Duration::from_secs(10),
            },
            echo_url: &echo_url,
            probe_timeout: PROBE,
        },
    );

    let diagnosis = match outcome {
        Ok(diagnosis) => diagnosis,
        Err(fault) => panic!("{} could not carry a request: {fault}", proxy.name),
    };
    assert!(
        !diagnosis.exit_ip.trim().is_empty(),
        "a reading with no address is not a reading"
    );
    assert!(
        diagnosis.elapsed > Duration::ZERO && diagnosis.elapsed < PROBE,
        "the elapsed time is what a user compares between proxies: {:?}",
        diagnosis.elapsed
    );

    assert_nothing_left_behind(&directory);
    let _ = std::fs::remove_dir_all(&directory);
}

/// The negative case that makes the positive one mean something: a request the
/// upstream is asked to carry to somewhere nothing can be listening.
///
/// The target is loopback port 1 *on whatever machine the upstream runs on*, so
/// the failure has to come back through the engine as a SOCKS reply or as the
/// engine's own log line. Either is a classified fault - never a reading, which
/// is the distinction the whole module exists to make.
#[test]
#[ignore = "requires XRAY_BIN and a reachable upstream"]
fn an_upstream_that_cannot_reach_the_target_is_a_fault_not_a_reading() {
    let executable = engine_executable();
    let proxy = upstream();
    let directory = directory(&proxy, "cannot-reach");
    let _ = std::fs::remove_dir_all(&directory);

    let outcome = diagnose_proxy(
        &SocksEchoClient,
        &DefaultXrayConfigBuilder,
        &proxy,
        &DiagnosticRequest {
            engine: Engine::Temporary {
                executable: Path::new(&executable),
                directory: &directory,
                ready_timeout: Duration::from_secs(10),
            },
            echo_url: "http://127.0.0.1:1/",
            probe_timeout: PROBE,
        },
    );

    let fault = match outcome {
        Ok(diagnosis) => panic!(
            "nothing can be listening on the upstream's loopback port 1, yet the endpoint \
             answered from {}",
            diagnosis.exit_ip
        ),
        Err(fault) => fault,
    };
    assert!(
        !matches!(fault.class, FaultClass::Engine | FaultClass::Config),
        "the request should have reached the upstream before it failed, so this is not the \
         engine failing to start nor the config being unusable: {fault}"
    );
    assert!(
        !fault.detail.trim().is_empty(),
        "a class with no evidence behind it is a guess: {fault}"
    );

    assert_nothing_left_behind(&directory);
    let _ = std::fs::remove_dir_all(&directory);
}
