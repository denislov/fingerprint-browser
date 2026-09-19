//! Opt-in integration: XRAY_BIN=/absolute/path/xray cargo test -p runtime --test xray_real -- --ignored
use domain::{HttpOutbound, ProxyId, ProxyOutbound, ProxyProfile, Socks5Outbound};
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
