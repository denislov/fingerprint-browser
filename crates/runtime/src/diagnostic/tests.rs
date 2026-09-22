use super::*;
use domain::{ProxyId, ProxyOutbound, Socks5Outbound};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

fn proxy() -> ProxyProfile {
    ProxyProfile {
        id: ProxyId::new(),
        name: "test".to_string(),
        outbound: ProxyOutbound::Socks5(Socks5Outbound {
            host: "localhost".to_string(),
            port: 1080,
            username: Some("user".to_string()),
            password: Some("secret".to_string()),
        }),
    }
}

// ---- the reading ----------------------------------------------------

#[test]
fn an_endpoint_answer_is_read_whatever_shape_it_arrives_in() {
    // The bare address, a JSON field, and a line of prose: all three are
    // answers to the same question.
    assert_eq!(
        extract_exit_ip("203.0.113.7"),
        Some("203.0.113.7".to_string())
    );
    assert_eq!(
        extract_exit_ip("{\"ip\":\"203.0.113.7\"}"),
        Some("203.0.113.7".to_string())
    );
    assert_eq!(
        extract_exit_ip("your address is 203.0.113.7\n"),
        Some("203.0.113.7".to_string())
    );
    assert_eq!(
        extract_exit_ip("2001:db8::1"),
        Some("2001:db8::1".to_string())
    );
}

#[test]
fn an_answer_without_an_address_is_not_a_reading() {
    // Not the same as a proxy that carried nothing, and must not be
    // reported as one.
    assert_eq!(extract_exit_ip(""), None);
    assert_eq!(extract_exit_ip("no address here"), None);
    // A version string is not an address either.
    assert_eq!(extract_exit_ip("142.0.7444.175"), None);
}

// ---- the request target ---------------------------------------------

#[test]
fn an_endpoint_address_is_split_into_what_the_two_protocols_need() {
    let named = HttpTarget::parse("http://api.example/ip").expect("a named endpoint");
    assert_eq!(named.host, "api.example");
    assert_eq!(named.port, 80);
    assert_eq!(named.path, "/ip");
    assert!(named.named, "a name is left for the engine to resolve");

    let literal = HttpTarget::parse("http://127.0.0.1:8080").expect("a literal endpoint");
    assert_eq!(literal.host, "127.0.0.1");
    assert_eq!(literal.port, 8080);
    assert_eq!(literal.path, "/");
    assert!(!literal.named, "a literal needs no resolution");

    let bracketed = HttpTarget::parse("http://[::1]:8443/ip").expect("an IPv6 endpoint");
    assert_eq!(bracketed.host, "::1");
    assert_eq!(bracketed.port, 8443);
}

#[test]
fn a_tls_endpoint_is_refused_rather_than_downgraded() {
    // Reporting a certificate problem as a proxy problem is the most
    // expensive wrong answer this could give.
    let fault = HttpTarget::parse("https://api.example/ip")
        .expect_err("https is not the question this asks");
    assert_eq!(fault.class, FaultClass::Config);
    assert!(fault.detail.contains("plain http"), "{fault}");

    assert_eq!(
        HttpTarget::parse("ftp://api.example/ip")
            .expect_err("not http")
            .class,
        FaultClass::Config
    );
    assert_eq!(
        HttpTarget::parse("http://:8080/ip")
            .expect_err("no host")
            .class,
        FaultClass::Config
    );
    assert_eq!(
        HttpTarget::parse("http://api.example:not-a-port/ip")
            .expect_err("no usable port")
            .class,
        FaultClass::Config
    );
}

// ---- the classification ---------------------------------------------

#[test]
fn a_refused_forward_is_not_reported_as_a_refused_credential() {
    // The classes exist to be told apart: each one is fixed somewhere else.
    assert_eq!(class_for_reply(0x05), FaultClass::Unreachable);
    assert_eq!(class_for_reply(0x02), FaultClass::Unreachable);
    assert_eq!(class_for_reply(0x04), FaultClass::Unreachable);
    assert_eq!(class_for_reply(0x06), FaultClass::Timeout);
    assert_eq!(class_for_reply(0x07), FaultClass::Config);
    assert_eq!(class_for_reply(0x08), FaultClass::Config);
    assert_eq!(class_for_reply(0x01), FaultClass::Other);
    // The detail still says which of the four it was.
    assert!(reply_label(0x05).contains("connection refused"));
    assert!(reply_label(0x02).contains("ruleset"));
}

#[test]
fn a_socket_that_ran_out_of_time_is_a_timeout_and_not_a_dead_network() {
    // The peer is still there, it simply never answered.
    assert_eq!(
        io_fault(std::io::Error::from(ErrorKind::WouldBlock)).class,
        FaultClass::Timeout
    );
    assert_eq!(
        io_fault(std::io::Error::from(ErrorKind::TimedOut)).class,
        FaultClass::Timeout
    );
    assert_eq!(
        io_fault(std::io::Error::from(ErrorKind::ConnectionReset)).class,
        FaultClass::Unreachable
    );
}

#[test]
fn a_status_is_read_as_the_answer_or_the_refusal_it_is() {
    assert_eq!(
        status_of("HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n").expect("a status"),
        200
    );
    assert_eq!(
        status_of("HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").expect("a status"),
        407
    );
    assert_eq!(
        status_of("no status here")
            .expect_err("not a response")
            .class,
        FaultClass::Reading
    );
}

#[test]
fn a_declared_length_is_the_frame_the_body_is_read_by() {
    // A declared length is preferred over waiting for a close, because an
    // endpoint that keeps the connection open would otherwise turn a
    // perfectly good address into a timeout.
    assert_eq!(
        content_length("HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n"),
        Some(12)
    );
    assert_eq!(
        content_length("HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\n"),
        Some(3),
        "the header name is not case sensitive"
    );
    assert_eq!(
        content_length("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n"),
        None,
        "no length means the close is the end"
    );
}

#[test]
fn the_engine_log_names_the_cause_a_transport_failure_cannot() {
    let rejected = "2026/09/20 21:00:00 [Warning] failed to handler mux client: invalid user";
    assert_eq!(
        classify_engine_log(rejected).map(|fault| fault.class),
        Some(FaultClass::Auth)
    );

    let unresolved = "2026/09/20 21:00:00 [Warning] failed to dial: no such host";
    assert_eq!(
        classify_engine_log(unresolved).map(|fault| fault.class),
        Some(FaultClass::Dns)
    );

    let bad_config = "2026/09/20 21:00:00 failed to parse config: unknown transport";
    assert_eq!(
        classify_engine_log(bad_config).map(|fault| fault.class),
        Some(FaultClass::Config)
    );

    let dead = "2026/09/20 21:00:00 [Warning] dial tcp 1.2.3.4:443: i/o timeout";
    assert_eq!(
        classify_engine_log(dead).map(|fault| fault.class),
        Some(FaultClass::Timeout)
    );
}

#[test]
fn an_unrecognised_engine_line_is_carried_rather_than_guessed_at() {
    let unknown = "2026/09/20 21:00:00 something nobody has seen before";
    let fault = classify_engine_log(unknown).expect("a log with a line makes a fault");
    assert_eq!(fault.class, FaultClass::Other);
    assert_eq!(fault.detail, unknown);

    // Silence is not a cause.
    assert_eq!(classify_engine_log("  \n\n"), None);
}

#[test]
fn a_detail_is_bounded_before_it_reaches_a_window() {
    let fault = Fault::new(FaultClass::Other, "x".repeat(10_000));
    assert!(fault.detail.chars().count() <= DETAIL_LIMIT + 1);
}

// ---- the probe, against a local engine -------------------------------

/// A loopback HTTP endpoint that answers with a fixed body.
struct EchoStub {
    port: u16,
    _listener: TcpListener,
}

impl EchoStub {
    fn start(body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the echo stub");
        let port = listener.local_addr().expect("echo stub address").port();
        let accepting = listener.try_clone().expect("clone the echo listener");
        thread::spawn(move || {
            for stream in accepting.incoming() {
                let Ok(mut client) = stream else { continue };
                thread::spawn(move || {
                    let mut request = [0u8; 1024];
                    let _ = client.read(&mut request);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = client.write_all(response.as_bytes());
                    let _ = client.flush();
                });
            }
        });
        Self {
            port,
            _listener: listener,
        }
    }

    fn url(&self) -> String {
        // Addressed by literal, never by name. A name would be resolved by
        // whatever this machine's hosts file says, and the endpoint under
        // test is a socket on loopback - not a name.
        format!("http://127.0.0.1:{}/", self.port)
    }
}

/// What a stub SOCKS5 endpoint does with the request it is given.
#[derive(Clone, Copy)]
enum Answer {
    /// Carries it to the target.
    Carries,
    /// Answers the greeting, then says nothing at all and holds the socket
    /// open. This is the case no timeout in a general-purpose client could
    /// bound, and the reason this module owns its socket.
    SaysNothing,
    /// Refuses the CONNECT with this SOCKS5 reply code.
    Refuses(u8),
    /// Chooses this greeting method.
    Selects(u8),
}

/// A loopback SOCKS5 endpoint, so a test can tell "the port is open" apart
/// from "the request was carried" - which is the whole point of the
/// diagnostic.
struct SocksStub {
    port: u16,
    _listener: TcpListener,
}

impl SocksStub {
    fn start(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the SOCKS stub");
        let port = listener.local_addr().expect("SOCKS stub address").port();
        let accepting = listener.try_clone().expect("clone the SOCKS listener");
        thread::spawn(move || {
            for stream in accepting.incoming() {
                let Ok(client) = stream else { continue };
                thread::spawn(move || serve(client, answer));
            }
        });
        Self {
            port,
            _listener: listener,
        }
    }
}

/// The SOCKS5 conversation, up to the point that matters: whether a
/// connection to the target is made.
fn serve(mut client: TcpStream, answer: Answer) {
    let mut greeting = [0u8; 2];
    if client.read_exact(&mut greeting).is_err() {
        return;
    }
    let mut offered = vec![0u8; greeting[1] as usize];
    if client.read_exact(&mut offered).is_err() {
        return;
    }

    if let Answer::Selects(method) = answer {
        let _ = client.write_all(&[0x05, method]);
        return;
    }
    // No authentication, which is what the engine's own inbound offers.
    if client.write_all(&[0x05, 0x00]).is_err() {
        return;
    }

    let mut head = [0u8; 4];
    if client.read_exact(&mut head).is_err() {
        return;
    }
    let Some(host) = read_target(&mut client, head[3]) else {
        return;
    };
    let mut port = [0u8; 2];
    if client.read_exact(&mut port).is_err() {
        return;
    }
    let port = u16::from_be_bytes(port);

    if let Answer::SaysNothing = answer {
        // Holds the socket without answering, for longer than any test's
        // deadline, so a probe that is not bounded here would hang.
        thread::sleep(Duration::from_secs(30));
        return;
    }

    let refused = |code: u8| [0x05, code, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    if let Answer::Refuses(code) = answer {
        let _ = client.write_all(&refused(code));
        return;
    }

    let Some(mut upstream) = dial(&host, port) else {
        let _ = client.write_all(&refused(0x05));
        return;
    };
    if client.write_all(&refused(0x00)).is_err() {
        return;
    }

    let mut reading = client.try_clone().expect("clone the client");
    let mut writing = upstream.try_clone().expect("clone the upstream");
    let pump = thread::spawn(move || {
        let _ = std::io::copy(&mut reading, &mut writing);
    });
    let _ = std::io::copy(&mut upstream, &mut client);
    let _ = pump.join();
}

/// The SOCKS address field, as the name or literal it carries.
fn read_target(stream: &mut TcpStream, kind: u8) -> Option<String> {
    match kind {
        0x01 => {
            let mut address = [0u8; 4];
            stream.read_exact(&mut address).ok()?;
            Some(std::net::Ipv4Addr::from(address).to_string())
        }
        0x03 => {
            let mut length = [0u8; 1];
            stream.read_exact(&mut length).ok()?;
            let mut name = vec![0u8; length[0] as usize];
            stream.read_exact(&mut name).ok()?;
            String::from_utf8(name).ok()
        }
        0x04 => {
            let mut address = [0u8; 16];
            stream.read_exact(&mut address).ok()?;
            Some(std::net::Ipv6Addr::from(address).to_string())
        }
        _ => None,
    }
}

/// Connects to the first address that accepts. A stub that resolves
/// "localhost" must not fail because this machine answers with `::1` first.
fn dial(host: &str, port: u16) -> Option<TcpStream> {
    use std::net::ToSocketAddrs;
    let addresses = (host, port).to_socket_addrs().ok()?;
    addresses
        .into_iter()
        .find_map(|address| TcpStream::connect(address).ok())
}

fn request<'a>(engine: Engine<'a>, url: &'a str) -> DiagnosticRequest<'a> {
    DiagnosticRequest {
        engine,
        echo_url: url,
        probe_timeout: Duration::from_secs(5),
    }
}

#[test]
fn a_socks_target_address_is_read_in_every_shape() {
    // The three shapes a request may carry, including the name that the
    // browser's SOCKS5 mode sends. Resolving the name is the engine's job
    // and not part of the reading, so this is independent of any hosts file.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a socket pair");
    let address = listener.local_addr().expect("socket pair address");
    let mut writer = TcpStream::connect(address).expect("connect the pair");
    let (mut reader, _) = listener.accept().expect("accept the pair");

    writer.write_all(&[127, 0, 0, 1]).expect("write ipv4");
    assert_eq!(
        read_target(&mut reader, 0x01),
        Some("127.0.0.1".to_string())
    );

    writer.write_all(&[9]).expect("write a name length");
    writer.write_all(b"localhost").expect("write a name");
    assert_eq!(
        read_target(&mut reader, 0x03),
        Some("localhost".to_string())
    );

    writer
        .write_all(&std::net::Ipv6Addr::LOCALHOST.octets())
        .expect("write ipv6");
    assert_eq!(read_target(&mut reader, 0x04), Some("::1".to_string()));

    // Anything else is not a target, and is refused rather than guessed at.
    assert_eq!(read_target(&mut reader, 0x02), None);
}

#[test]
fn a_request_that_was_carried_reports_the_address_that_left() {
    let echo = EchoStub::start("203.0.113.7\n");
    let socks = SocksStub::start(Answer::Carries);
    let url = echo.url();

    let reading = diagnose(
        &SocksEchoClient::new(),
        &crate::xray::DefaultXrayConfigBuilder::new(),
        &proxy(),
        &request(
            Engine::Running {
                socks_port: socks.port,
            },
            &url,
        ),
    )
    .expect("a carried request is a reading");

    assert_eq!(reading.exit_ip, "203.0.113.7");
}

#[test]
fn an_open_port_that_carries_nothing_is_a_fault_and_not_a_pass() {
    // This is the case `wait_ready` cannot see: the engine is listening,
    // the handshake succeeds, and no byte ever leaves.
    let echo = EchoStub::start("203.0.113.7\n");
    let socks = SocksStub::start(Answer::Refuses(0x05));
    let url = echo.url();

    let fault = diagnose(
        &SocksEchoClient::new(),
        &crate::xray::DefaultXrayConfigBuilder::new(),
        &proxy(),
        &request(
            Engine::Running {
                socks_port: socks.port,
            },
            &url,
        ),
    )
    .expect_err("a port that carries nothing is not a pass");

    assert_eq!(fault.class, FaultClass::Unreachable, "got {fault}");
    assert!(fault.detail.contains("connection refused"), "{fault}");
}

#[test]
fn an_endpoint_that_never_answers_is_a_timeout_and_not_a_hang() {
    // The failure this module exists to have bounded: the proxy accepts the
    // connection and then says nothing. A general-purpose client joined its
    // own handshake thread and waited the peer out instead of timing out.
    let echo = EchoStub::start("203.0.113.7\n");
    let socks = SocksStub::start(Answer::SaysNothing);
    let url = echo.url();

    let started = Instant::now();
    let fault = diagnose(
        &SocksEchoClient::new(),
        &crate::xray::DefaultXrayConfigBuilder::new(),
        &proxy(),
        &DiagnosticRequest {
            engine: Engine::Running {
                socks_port: socks.port,
            },
            echo_url: &url,
            probe_timeout: Duration::from_millis(600),
        },
    )
    .expect_err("silence is not a reading");

    assert_eq!(fault.class, FaultClass::Timeout, "got {fault}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the probe must give up on its own deadline, took {:?}",
        started.elapsed()
    );
}

#[test]
fn partial_reads_share_one_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        for byte in b"127.0.0.1" {
            thread::sleep(Duration::from_millis(70));
            if stream.write_all(&[*byte]).is_err() {
                break;
            }
        }
    });
    let mut socket = Bounded::connect(port, Duration::from_millis(150)).unwrap();
    let error = socket
        .read_exact(&mut [0; 9])
        .expect_err("progress does not renew the deadline");
    assert_eq!(error.class, FaultClass::Timeout);
    drop(socket);
    peer.join().unwrap();
}

#[test]
fn a_proxy_that_demands_a_credential_is_not_reported_as_a_dead_network() {
    let echo = EchoStub::start("203.0.113.7\n");
    let socks = SocksStub::start(Answer::Selects(0xff));
    let url = echo.url();

    let fault = diagnose(
        &SocksEchoClient::new(),
        &crate::xray::DefaultXrayConfigBuilder::new(),
        &proxy(),
        &request(
            Engine::Running {
                socks_port: socks.port,
            },
            &url,
        ),
    )
    .expect_err("a proxy that wants a credential carries nothing for us");

    assert_eq!(fault.class, FaultClass::Auth, "got {fault}");
}

#[test]
fn an_endpoint_that_is_not_listening_is_an_engine_fault() {
    // Nothing is bound: the port may even have been recycled.
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a free port");
    let free = listener.local_addr().expect("free address").port();
    drop(listener);

    let fault = SocksEchoClient::new()
        .fetch(free, "http://127.0.0.1:1/", Duration::from_millis(300))
        .expect_err("a port with nothing behind it is not a reading");
    assert_eq!(fault.class, FaultClass::Engine, "got {fault}");
}

#[test]
fn a_running_engine_is_probed_and_left_alone() {
    let echo = EchoStub::start("198.51.100.4\n");
    let socks = SocksStub::start(Answer::Carries);
    let url = echo.url();
    let builder = crate::xray::DefaultXrayConfigBuilder::new();
    let client = SocksEchoClient::new();

    for _ in 0..2 {
        let reading = diagnose(
            &client,
            &builder,
            &proxy(),
            &request(
                Engine::Running {
                    socks_port: socks.port,
                },
                &url,
            ),
        )
        .expect("the engine was not stopped by the first diagnostic");
        assert_eq!(reading.exit_ip, "198.51.100.4");
    }
}

/// A temporary engine, as an actual process.
///
/// The stub scripts follow the convention the supervisor's tests already
/// use: a python script that reads the config it was handed. What is under
/// test here is not the probe - the local stubs above cover that - but what
/// the diagnostic leaves behind, which is where a bug would be expensive.
#[cfg(unix)]
mod engine_lifetime {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// Binds the inbound the config names, then does nothing with it. It
    /// records its own pid so the test can ask whether it outlived the
    /// diagnostic.
    const LISTENING: &str = "#!/usr/bin/env python3\nimport json,os,socket,sys,time\nc=json.load(open(sys.argv[3]))\nopen(os.path.join(os.path.dirname(sys.argv[3]),'engine.pid'),'w').write(str(os.getpid()))\ns=socket.socket()\ns.bind(('127.0.0.1',c['inbounds'][0]['port']))\ns.listen()\ntime.sleep(60)\n";

    /// Never binds anything, so readiness has to give up on its own.
    const SILENT: &str = "#!/usr/bin/env python3\nimport time\ntime.sleep(60)\n";

    /// A directory that takes its stub engine with it.
    struct Dir {
        path: PathBuf,
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    impl Dir {
        fn new(tag: &str) -> Self {
            // Executable writes and process launches in sibling tests race
            // each other otherwise.
            static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
            let path = std::env::temp_dir().join(format!("fp-diagnostic-{tag}-{}", ProxyId::new()));
            std::fs::create_dir_all(&path).expect("create the diagnostic directory");
            Self {
                path,
                _serial: serial,
            }
        }

        fn engine(&self, script: &str) -> PathBuf {
            let executable = self.path.join("xray");
            std::fs::write(&executable, script).expect("write the stub engine");
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
                .expect("make the stub engine executable");
            executable
        }

        /// Where the diagnostic writes its own two files.
        fn runtime(&self) -> PathBuf {
            self.path.join("runtime")
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn run(dir: &Dir, script: &str) -> Result<Diagnosis, Fault> {
        let echo = EchoStub::start("203.0.113.7\n");
        let url = echo.url();
        let runtime = dir.runtime();
        let executable = dir.engine(script);
        diagnose(
            &SocksEchoClient::new(),
            &crate::xray::DefaultXrayConfigBuilder::new(),
            &proxy(),
            &DiagnosticRequest {
                engine: Engine::Temporary {
                    executable: &executable,
                    directory: &runtime,
                    ready_timeout: Duration::from_millis(2500),
                },
                echo_url: &url,
                probe_timeout: Duration::from_millis(600),
            },
        )
    }

    /// The config holds the upstream credentials, so surviving the
    /// diagnostic would be worse than the failure it reported.
    fn assert_nothing_left(dir: &Dir) {
        let runtime = dir.runtime();
        assert!(
            !runtime.join(DIAGNOSTIC_CONFIG_FILE).exists(),
            "the temporary config must not survive the diagnostic"
        );
        assert!(
            !runtime.join(DIAGNOSTIC_LOG_FILE).exists(),
            "the engine log must not survive the diagnostic"
        );
    }

    #[test]
    fn an_engine_that_binds_but_carries_nothing_is_bounded_and_leaves_nothing() {
        let dir = Dir::new("carries-nothing");
        let started = Instant::now();
        let outcome = run(&dir, LISTENING);
        let took = started.elapsed();

        // Readiness passed, because the port was open; the request did not.
        let fault = outcome.expect_err("a bound port that carries nothing is not a reading");
        assert_eq!(fault.class, FaultClass::Timeout, "got {fault}");
        assert!(
            took < Duration::from_secs(10),
            "the diagnostic must not wait the engine out, took {took:?}"
        );
        assert_nothing_left(&dir);

        // Linux is where a pid can be asked about directly.
        #[cfg(target_os = "linux")]
        {
            let pid = std::fs::read_to_string(dir.runtime().join("engine.pid"))
                .expect("the stub recorded its pid")
                .trim()
                .to_string();
            assert!(
                !Path::new(&format!("/proc/{pid}")).exists(),
                "the diagnostic engine must not outlive the diagnostic"
            );
        }
    }

    #[test]
    fn an_engine_that_never_binds_is_a_readiness_fault_that_leaves_nothing() {
        let dir = Dir::new("silent");
        let outcome = run(&dir, SILENT);

        let fault = outcome.expect_err("an engine that never binds is not a reading");
        assert_eq!(
            fault.class,
            FaultClass::Engine,
            "nothing bound, so nothing carried: got {fault}"
        );
        assert_nothing_left(&dir);
    }
}
