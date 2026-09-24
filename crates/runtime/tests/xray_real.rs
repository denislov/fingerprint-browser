//! Opt-in integration: XRAY_BIN=/absolute/path/xray cargo test -p runtime --test xray_real -- --ignored
use domain::{
    GrpcSettings, HttpOutbound, ProxyId, ProxyOutbound, ProxyProfile, RealitySettings,
    ShadowsocksOutbound, Socks5Outbound, StreamNetwork, StreamSecurity, StreamSettings,
    TlsSettings, TrojanOutbound, VlessOutbound, VmessOutbound, WsSettings,
};
use runtime::{DefaultXrayConfigBuilder, XrayConfigBuilder};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Session {
    child: Child,
    config: PathBuf,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.config);
    }
}

fn read_bytes(stream: &mut TcpStream, count: usize) -> Vec<u8> {
    let mut bytes = vec![0; count];
    stream.read_exact(&mut bytes).unwrap();
    bytes
}

#[test]
#[ignore = "requires XRAY_BIN pointing to a real Xray executable"]
fn authenticated_socks_and_http_upstreams_forward_payload() {
    let executable = std::env::var_os("XRAY_BIN").expect("set XRAY_BIN");
    for http in [false, true] {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream_port = upstream.local_addr().unwrap().port();
        upstream.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match upstream.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "Xray did not connect to upstream"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("upstream accept: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            if http {
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    header.extend(read_bytes(&mut stream, 1));
                    assert!(header.len() < 8192);
                }
                let header = String::from_utf8(header).unwrap().to_ascii_lowercase();
                assert!(header.starts_with("connect 127.0.0.1:80 http/1.1"));
                assert!(header.contains("proxy-authorization: basic ywxpy2u6c2vjcmv0"));
                stream
                    .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .unwrap();
            } else {
                let greeting = read_bytes(&mut stream, 2);
                assert_eq!(greeting[0], 5);
                assert!(read_bytes(&mut stream, greeting[1] as usize).contains(&2));
                stream.write_all(&[5, 2]).unwrap();
                let auth = read_bytes(&mut stream, 2);
                assert_eq!(auth[0], 1);
                assert_eq!(read_bytes(&mut stream, auth[1] as usize), b"alice");
                let len = read_bytes(&mut stream, 1)[0];
                assert_eq!(read_bytes(&mut stream, len as usize), b"secret");
                stream.write_all(&[1, 0]).unwrap();
                assert_eq!(
                    read_bytes(&mut stream, 10),
                    [5, 1, 0, 1, 127, 0, 0, 1, 0, 80]
                );
                stream
                    .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                    .unwrap();
            }
            assert_eq!(read_bytes(&mut stream, 4), b"ping");
            stream.write_all(b"pong").unwrap();
        });
        let outbound = if http {
            ProxyOutbound::Http(HttpOutbound {
                host: "127.0.0.1".into(),
                port: upstream_port,
                username: Some("alice".into()),
                password: Some("secret".into()),
            })
        } else {
            ProxyOutbound::Socks5(Socks5Outbound {
                host: "127.0.0.1".into(),
                port: upstream_port,
                username: Some("alice".into()),
                password: Some("secret".into()),
            })
        };
        let proxy = ProxyProfile {
            id: ProxyId::new(),
            name: "integration".into(),
            outbound,
        };
        let config = std::env::temp_dir().join(format!("xray-integration-{}.json", proxy.id));
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reservation.local_addr().unwrap().port();
        DefaultXrayConfigBuilder
            .build(&proxy, port, &config)
            .unwrap();
        assert!(
            Command::new(&executable)
                .args(["run", "-test", "-config"])
                .arg(&config)
                .status()
                .unwrap()
                .success()
        );
        drop(reservation);
        let child = Command::new(&executable)
            .args(["run", "-config"])
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut session = Session { child, config };
        let start = Instant::now();
        let mut client = loop {
            assert!(session.child.try_wait().unwrap().is_none(), "Xray exited");
            if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)) {
                break stream;
            }
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "Xray readiness timeout"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        client.write_all(&[5, 1, 0]).unwrap();
        assert_eq!(read_bytes(&mut client, 2), [5, 0]);
        client
            .write_all(&[5, 1, 0, 1, 127, 0, 0, 1, 0, 80])
            .unwrap();
        let response = read_bytes(&mut client, 4);
        assert_eq!(&response[..3], &[5, 0, 0]);
        let address_len = match response[3] {
            1 => 4,
            4 => 16,
            3 => read_bytes(&mut client, 1)[0] as usize,
            _ => panic!("bad address"),
        };
        read_bytes(&mut client, address_len + 2);
        client.write_all(b"ping").unwrap();
        assert_eq!(read_bytes(&mut client, 4), b"pong");
        server.join().unwrap();
    }
}

fn proxy(name: &str, outbound: ProxyOutbound) -> ProxyProfile {
    ProxyProfile {
        id: ProxyId::new(),
        name: name.to_string(),
        outbound,
    }
}

/// A share link, all the way to a config the engine accepts.
///
/// The parser's own tests check the model it produces; this checks that what a
/// provider hands out actually becomes a config Xray will run - the one path a
/// paste goes through end to end.
#[test]
#[ignore = "requires XRAY_BIN pointing to a real Xray executable"]
fn links_from_the_wild_become_configs_the_engine_accepts() {
    let executable = std::env::var_os("XRAY_BIN").expect("set XRAY_BIN");
    let uuid = "b831381d-6324-4d53-ad4f-8cda48b30811";
    let public_key = "LOLTG162EtSegCnMAofVY3oKrbrCvH8zOTZEPd1GRQU";
    let links = [
        format!("vless://{uuid}@node.example:443?encryption=none&security=reality&sni=front.example&fp=chrome&pbk={public_key}&sid=ab12&type=tcp#Reality"),
        format!("vless://{uuid}@node.example:443?encryption=none&flow=xtls-rprx-vision&security=tls&sni=front.example#Vision"),
        format!("vless://{uuid}@node.example:443?encryption=none&security=tls&sni=front.example&type=ws&path=%2Fws&host=front.example#Websocket"),
        format!("vless://{uuid}@node.example:443?encryption=none&type=grpc&serviceName=svc&security=tls#Grpc"),
        // A vmess link is base64 JSON, and the `aid` it carries is read back out
        // of the config this builds.
        format!(
            "vmess://{}",
            base64_of(&format!(
                r#"{{"v":"2","ps":"Vm","add":"node.example","port":"443","id":"{uuid}","aid":"64","scy":"auto","net":"ws","type":"none","host":"front.example","path":"/ws","tls":"tls","sni":"front.example"}}"#
            ))
        ),
        "trojan://secret@node.example:443?sni=front.example#Trojan".to_string(),
        "trojan://secret@node.example:443?security=reality&sni=front.example&pbk=LOLTG162EtSegCnMAofVY3oKrbrCvH8zOTZEPd1GRQU&sid=ab12".to_string(),
        "ss://YWVzLTI1Ni1nY206c2VjcmV0@node.example:8388#Shadowsocks".to_string(),
        "ss://YWVzLTI1Ni1nY206c2VjcmV0@node.example:8388?plugin=obfs-local".to_string(),
    ];

    for link in &links[..links.len() - 1] {
        let parsed = domain::parse_proxy_uri(link).expect("the link should parse");
        let profile = proxy(&parsed.suggested_name(), parsed.outbound);
        if let Err(reason) = engine_verdict(&executable, &profile) {
            panic!(
                "the engine refused the config from {}: {reason}",
                profile.name
            );
        }
    }

    // The one link in that list this program refuses: it asks for a plugin, and
    // a config built without it would connect to nothing.
    let plugin = links.last().expect("the plugin link");
    let error = domain::parse_proxy_uri(plugin).expect_err("a plugin link is refused");
    assert!(error.to_string().contains("plugin"), "{error}");
}

fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value)
}

/// What the engine says about a config this builder wrote.
///
/// `xray run -test` parses the file and builds every handler without opening a
/// socket, so the answer costs no server and no network - which is what makes it
/// the right authority for names the engine defines (cipher methods, transport
/// names, whether a block is read at all).
fn engine_verdict(executable: &std::ffi::OsStr, proxy: &ProxyProfile) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("fp-xray-verdict-{}.json", proxy.id));
    DefaultXrayConfigBuilder
        .build(proxy, 10800, &path)
        .map_err(|error| error.to_string())?;
    let verdict = test_config(executable, &path);
    let _ = std::fs::remove_file(&path);
    verdict
}

fn test_config(executable: &std::ffi::OsStr, path: &std::path::Path) -> Result<(), String> {
    let output = Command::new(executable)
        .args(["run", "-test", "-c"])
        .arg(path)
        .output()
        .expect("run xray -test");
    if output.status.success() {
        Ok(())
    } else {
        // The engine writes its refusal to stdout, not to stderr: taking only
        // stderr reads as "refused, reason unknown", which is what the first
        // version of this did.
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Err(detail.trim().to_string())
    }
}

/// Every shape the config builder can make, held up against the engine's own
/// validator.
///
/// The negative cases are what make it worth running. One shows the validator
/// can refuse at all (a config the builder wrote, with the switch this engine
/// removed put back by hand); the other shows a name we do *not* check
/// ourselves - the cipher - being refused by the engine instead. Without them,
/// "exit 0" would not be evidence of anything.
///
/// REALITY over a websocket is not here: that one never reaches the engine,
/// because domain validation refuses it first (see `validate_proxy`).
#[test]
#[ignore = "requires XRAY_BIN pointing to a real Xray executable"]
fn every_shape_the_builder_makes_is_accepted_by_the_engine() {
    let executable = std::env::var_os("XRAY_BIN").expect("set XRAY_BIN");

    let tls_over_ws = StreamSettings {
        network: StreamNetwork::Ws,
        security: StreamSecurity::Tls,
        tls: Some(TlsSettings {
            server_name: Some("front.example".into()),
            fingerprint: Some("chrome".into()),
            alpn: vec!["h2".into(), "http/1.1".into()],
        }),
        ws: Some(WsSettings {
            path: Some("/ws".into()),
            host: Some("front.example".into()),
        }),
        ..Default::default()
    };
    // Produced by `xray x25519` (the line it prints as `Password`).
    let reality_stream = StreamSettings {
        security: StreamSecurity::Reality,
        reality: Some(RealitySettings {
            server_name: Some("front.example".into()),
            public_key: "LOLTG162EtSegCnMAofVY3oKrbrCvH8zOTZEPd1GRQU".into(),
            short_id: Some("ab12".into()),
            fingerprint: Some("chrome".into()),
            spider_x: None,
        }),
        ..Default::default()
    };
    let accepted = vec![
        proxy(
            "socks5",
            ProxyOutbound::Socks5(Socks5Outbound {
                host: "127.0.0.1".into(),
                port: 1080,
                username: Some("alice".into()),
                password: Some("secret".into()),
            }),
        ),
        proxy(
            "http",
            ProxyOutbound::Http(HttpOutbound {
                host: "127.0.0.1".into(),
                port: 8080,
                username: Some("alice".into()),
                password: Some("secret".into()),
            }),
        ),
        proxy(
            "shadowsocks aes-256-gcm",
            ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                host: "127.0.0.1".into(),
                port: 8388,
                password: "secret".into(),
                method: "aes-256-gcm".into(),
                stream: StreamSettings::plain(),
            }),
        ),
        proxy(
            "shadowsocks 2022 with a base64 key",
            ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                host: "127.0.0.1".into(),
                port: 8388,
                // 32 bytes, base64: what a 2022 cipher requires of `password`.
                password: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
                method: "2022-blake3-aes-256-gcm".into(),
                stream: StreamSettings::plain(),
            }),
        ),
        proxy(
            "vmess plain",
            ProxyOutbound::Vmess(VmessOutbound {
                host: "127.0.0.1".into(),
                port: 10086,
                uuid: "b831381d-6324-4d53-ad4f-8cda48b30811".into(),
                security: "auto".into(),
                alter_id: 0,
                stream: StreamSettings::plain(),
            }),
        ),
        proxy(
            "vmess tls over ws",
            ProxyOutbound::Vmess(VmessOutbound {
                host: "127.0.0.1".into(),
                port: 443,
                uuid: "b831381d-6324-4d53-ad4f-8cda48b30811".into(),
                security: "auto".into(),
                alter_id: 0,
                stream: tls_over_ws.clone(),
            }),
        ),
        proxy(
            "vless tls over grpc",
            ProxyOutbound::Vless(VlessOutbound {
                host: "127.0.0.1".into(),
                port: 443,
                uuid: "b831381d-6324-4d53-ad4f-8cda48b30811".into(),
                flow: None,
                encryption: "none".into(),
                stream: StreamSettings {
                    network: StreamNetwork::Grpc,
                    security: StreamSecurity::Tls,
                    grpc: Some(GrpcSettings {
                        service_name: "svc".into(),
                    }),
                    ..Default::default()
                },
            }),
        ),
        proxy(
            "vless reality",
            ProxyOutbound::Vless(VlessOutbound {
                host: "127.0.0.1".into(),
                port: 443,
                uuid: "b831381d-6324-4d53-ad4f-8cda48b30811".into(),
                flow: Some("xtls-rprx-vision".into()),
                encryption: "none".into(),
                stream: reality_stream,
            }),
        ),
        proxy(
            "trojan tls",
            ProxyOutbound::Trojan(TrojanOutbound {
                host: "127.0.0.1".into(),
                port: 443,
                password: "secret".into(),
                stream: StreamSettings {
                    security: StreamSecurity::Tls,
                    ..Default::default()
                },
            }),
        ),
        // The engine honours trojan without TLS rather than ignoring the
        // setting, so it is a config it accepts, and this records that.
        proxy(
            "trojan without tls",
            ProxyOutbound::Trojan(TrojanOutbound {
                host: "127.0.0.1".into(),
                port: 443,
                password: "secret".into(),
                stream: StreamSettings::plain(),
            }),
        ),
    ];

    for proxy in &accepted {
        if let Err(reason) = engine_verdict(&executable, proxy) {
            panic!("the engine refused {}: {reason}", proxy.name);
        }
    }

    // A cipher this project does not enumerate: the engine is the list.
    let unknown_cipher = proxy(
        "shadowsocks with an unknown cipher",
        ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
            host: "127.0.0.1".into(),
            port: 8388,
            password: "secret".into(),
            method: "definitely-not-a-cipher".into(),
            stream: StreamSettings::plain(),
        }),
    );
    assert!(
        engine_verdict(&executable, &unknown_cipher).is_err(),
        "the engine accepted a cipher it does not know"
    );

    // The switch Xray 26.2.6 removed, put back into a config this builder wrote.
    // It is added as text because there is no profile field that could produce
    // it - which is the point.
    let trojan_tls = accepted
        .iter()
        .find(|candidate| candidate.name == "trojan tls")
        .expect("the trojan case above");
    let path = std::env::temp_dir().join(format!("fp-xray-removed-{}.json", trojan_tls.id));
    DefaultXrayConfigBuilder
        .build(trojan_tls, 10800, &path)
        .expect("build");
    let with_the_removed_switch = std::fs::read_to_string(&path).expect("read").replace(
        "\"security\": \"tls\"",
        "\"security\": \"tls\", \"tlsSettings\": { \"allowInsecure\": true }",
    );
    // Without this, a config that never held the switch would be "accepted" and
    // the case would read as a pass while testing nothing.
    assert_ne!(
        std::fs::read_to_string(&path).expect("read"),
        with_the_removed_switch,
        "nothing was injected, so the engine was never asked about the removed switch"
    );
    std::fs::write(&path, with_the_removed_switch).expect("write");
    let verdict = test_config(&executable, &path);
    let _ = std::fs::remove_file(&path);
    let reason = verdict.expect_err("the engine accepted a switch it removed");
    assert!(
        reason.contains("allowInsecure"),
        "the refusal should name the switch: {reason}"
    );
}
