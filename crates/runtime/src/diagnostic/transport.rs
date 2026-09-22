//! SOCKS5 and HTTP with a single absolute deadline.
use super::*;

/// An `http://` address, split into the parts the two protocols need.
///
/// HTTPS is refused rather than downgraded; see [`check_echo_url`] for why.
#[derive(Debug)]
pub(super) struct HttpTarget {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) path: String,
    /// Whether the engine has to resolve the name, which is the case whenever
    /// the host is not already a literal address.
    pub(super) named: bool,
}

/// The prefix an address endpoint has to start with.
pub const ECHO_PREFIX: &str = "http://";

/// Whether `url` is an address endpoint that can be asked, and why not when it
/// cannot.
///
/// Plain HTTP is required rather than preferred, and an `https://` endpoint is
/// refused rather than quietly downgraded. Every caller asks the same question
/// of it - whether traffic left by a particular path - so putting TLS in the
/// same request would let a certificate problem be reported as a path problem,
/// which is the most expensive kind of wrong answer to give someone debugging a
/// proxy. The rule lives here because both the pre-flight, which asks over a
/// socket this process owns, and the runtime read-back, which asks the browser,
/// have to give the same answer to the same setting.
pub fn check_echo_url(url: &str) -> Result<(), Fault> {
    if url.starts_with("https://") {
        return Err(Fault::new(
            FaultClass::Config,
            "the address endpoint must be plain http: a TLS request would report a certificate problem as a proxy problem",
        ));
    }
    if !url.starts_with(ECHO_PREFIX) {
        return Err(Fault::new(
            FaultClass::Config,
            format!("the address endpoint is not an http address: {url}"),
        ));
    }
    Ok(())
}

impl HttpTarget {
    pub(super) fn parse(url: &str) -> Result<Self, Fault> {
        check_echo_url(url)?;
        // Safe to slice: the prefix is ASCII and was just checked.
        let rest = &url[ECHO_PREFIX.len()..];

        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "/"),
        };
        let (host, port) = split_authority(authority)?;
        if host.is_empty() {
            return Err(Fault::new(
                FaultClass::Config,
                format!("the address endpoint names no host: {url}"),
            ));
        }

        Ok(Self {
            named: host.parse::<IpAddr>().is_err(),
            host: host.to_string(),
            port,
            path: path.to_string(),
        })
    }
}

/// The host and port of an authority, with a bracketed IPv6 literal allowed.
pub(super) fn split_authority(authority: &str) -> Result<(&str, u16), Fault> {
    let unusable = || {
        Fault::new(
            FaultClass::Config,
            format!("the address endpoint has no usable port: {authority}"),
        )
    };

    if let Some(rest) = authority.strip_prefix('[') {
        let (address, tail) = rest.split_once(']').ok_or_else(|| {
            Fault::new(
                FaultClass::Config,
                format!("the address endpoint has an unclosed IPv6 literal: {authority}"),
            )
        })?;
        return match tail.strip_prefix(':') {
            Some(port) => Ok((address, port.parse::<u16>().map_err(|_| unusable())?)),
            None => Ok((address, 80)),
        };
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => Ok((host, port.parse::<u16>().map_err(|_| unusable())?)),
        None => Ok((authority, 80)),
    }
}

/// A socket whose every read and write is bounded by one deadline.
///
/// A per-call timeout alone is not enough: several reads make up one answer, so
/// the deadline is kept here and each call is given only what is left of it.
pub(super) struct Bounded {
    pub(super) stream: TcpStream,
    pub(super) deadline: Instant,
}

impl Bounded {
    pub(super) fn connect(socks_port: u16, timeout: Duration) -> Result<Self, Fault> {
        let deadline = Instant::now() + timeout;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, socks_port));
        let stream = TcpStream::connect_timeout(&address, timeout).map_err(|error| {
            Fault::new(
                FaultClass::Engine,
                format!("the loopback SOCKS endpoint at {address} accepted no connection: {error}"),
            )
        })?;
        Ok(Self { stream, deadline })
    }

    pub(super) fn remaining(&self) -> Result<Duration, Fault> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Fault::new(
                FaultClass::Timeout,
                "the request did not finish in the time allowed",
            ));
        }
        Ok(left)
    }

    pub(super) fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), Fault> {
        while !bytes.is_empty() {
            let left = self.remaining()?;
            self.stream
                .set_write_timeout(Some(left))
                .map_err(io_fault)?;
            match self.stream.write(bytes) {
                Ok(0) => return Err(io_fault(std::io::Error::from(ErrorKind::WriteZero))),
                Ok(written) => bytes = &bytes[written..],
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_fault(error)),
            }
        }
        self.remaining()?;
        Ok(())
    }

    pub(super) fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), Fault> {
        // A short read is not a complete answer: the reply must arrive whole.
        let mut filled = 0;
        while filled < buffer.len() {
            let left = self.remaining()?;
            self.stream.set_read_timeout(Some(left)).map_err(io_fault)?;
            match self.stream.read(&mut buffer[filled..]) {
                Ok(0) => {
                    return Err(Fault::new(
                        FaultClass::Unreachable,
                        "the engine closed the connection before answering",
                    ));
                }
                Ok(read) => filled += read,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_fault(error)),
            }
        }
        self.remaining()?;
        Ok(())
    }

    /// Everything up to the close, which is what `Connection: close` asks for.
    pub(super) fn read_to_close(&mut self, limit: u64) -> Result<String, Fault> {
        let mut collected = Vec::new();
        let mut buffer = [0u8; 1024];
        while (collected.len() as u64) < limit {
            let left = self.remaining()?;
            self.stream.set_read_timeout(Some(left)).map_err(io_fault)?;
            match self.stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => collected.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_fault(error)),
            }
        }
        Ok(String::from_utf8_lossy(&collected).into_owned())
    }

    /// Everything up to the blank line that ends the headers, terminator
    /// included.
    ///
    /// Read a byte at a time so the terminator is never stepped over: the
    /// alternative is to read past it and then have to carry the leftover bytes
    /// into the body, which is where the off-by-one would live. Headers from an
    /// address endpoint are a few hundred bytes, so the cost is a few hundred
    /// syscalls on a request that is already waiting on a network.
    pub(super) fn read_headers(&mut self, limit: u64) -> Result<String, Fault> {
        let mut collected = Vec::new();
        let mut byte = [0u8; 1];
        while (collected.len() as u64) < limit {
            let left = self.remaining()?;
            self.stream.set_read_timeout(Some(left)).map_err(io_fault)?;
            match self.stream.read(&mut byte) {
                Ok(0) => {
                    return Err(Fault::new(
                        FaultClass::Reading,
                        "the endpoint closed before it finished answering",
                    ));
                }
                Ok(_) => {
                    collected.push(byte[0]);
                    if collected.ends_with(b"\r\n\r\n") {
                        return Ok(String::from_utf8_lossy(&collected).into_owned());
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_fault(error)),
            }
        }
        Err(Fault::new(
            FaultClass::Reading,
            format!("the endpoint's headers did not end within {limit} bytes"),
        ))
    }
}

/// The class an IO failure belongs to.
///
/// A socket read that runs past its deadline reports `WouldBlock` on Linux,
/// which is a timeout and not an unreachable network - the peer is still there,
/// it simply never answered.
pub(super) fn io_fault(error: std::io::Error) -> Fault {
    let class = match error.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => FaultClass::Timeout,
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::BrokenPipe
        | ErrorKind::NotConnected => FaultClass::Unreachable,
        _ => FaultClass::Other,
    };
    Fault::new(class, error.to_string())
}

/// The SOCKS5 greeting and the CONNECT request, with every step bounded.
pub(super) fn handshake(socket: &mut Bounded, target: &HttpTarget) -> Result<(), Fault> {
    // One method offered, no authentication - what the engine's own inbound
    // offers, and all this diagnostic is entitled to use.
    socket.write_all(&[0x05, 0x01, 0x00])?;

    let mut choice = [0u8; 2];
    socket.read_exact(&mut choice)?;
    if choice[0] != 0x05 {
        return Err(Fault::new(
            FaultClass::Config,
            format!(
                "the endpoint is not a SOCKS5 proxy: it answered with version {}",
                choice[0]
            ),
        ));
    }
    match choice[1] {
        0x00 => {}
        // The proxy wants a credential this diagnostic does not hold, which is
        // a different problem from a proxy that is not there.
        0xff => {
            return Err(Fault::new(
                FaultClass::Auth,
                "the SOCKS endpoint accepts no method this diagnostic may use; it requires authentication",
            ));
        }
        method => {
            return Err(Fault::new(
                FaultClass::Auth,
                format!(
                    "the SOCKS endpoint selected method {method:#04x}, which this diagnostic holds no credential for"
                ),
            ));
        }
    }

    let mut connect = vec![0x05, 0x01, 0x00];
    if target.named {
        // The name is sent as a name for the engine to resolve, which is what
        // Chromium's `--proxy-server=socks5://` does. Resolving it here would
        // test this machine's resolvers and call the result the proxy's.
        let name = target.host.as_bytes();
        let length = u8::try_from(name.len()).map_err(|_| {
            Fault::new(
                FaultClass::Config,
                "the address endpoint's name is too long for SOCKS5",
            )
        })?;
        connect.extend_from_slice(&[0x03, length]);
        connect.extend_from_slice(name);
    } else {
        match target.host.parse::<IpAddr>().map_err(|_| {
            Fault::new(
                FaultClass::Config,
                format!(
                    "the address endpoint is neither a name nor an address: {}",
                    target.host
                ),
            )
        })? {
            IpAddr::V4(address) => {
                connect.push(0x01);
                connect.extend_from_slice(&address.octets());
            }
            IpAddr::V6(address) => {
                connect.push(0x04);
                connect.extend_from_slice(&address.octets());
            }
        }
    }
    connect.extend_from_slice(&target.port.to_be_bytes());
    socket.write_all(&connect)?;

    // The reply: version, code, reserved, then a bound address.
    let mut head = [0u8; 4];
    socket.read_exact(&mut head)?;
    if head[0] != 0x05 {
        return Err(Fault::new(
            FaultClass::Config,
            format!("the endpoint answered the CONNECT with version {}", head[0]),
        ));
    }
    if head[1] != 0x00 {
        return Err(Fault::new(
            class_for_reply(head[1]),
            format!(
                "the engine did not carry the request: {}",
                reply_label(head[1])
            ),
        ));
    }

    // The bound address is consumed so the stream is left at the payload.
    match head[3] {
        0x01 => socket.read_exact(&mut [0u8; 6])?,
        0x03 => {
            let mut length = [0u8; 1];
            socket.read_exact(&mut length)?;
            socket.read_exact(&mut vec![0u8; length[0] as usize + 2])?;
        }
        0x04 => socket.read_exact(&mut [0u8; 18])?,
        kind => {
            return Err(Fault::new(
                FaultClass::Config,
                format!("the engine replied with address kind {kind:#04x}"),
            ));
        }
    }
    Ok(())
}

/// One request, and the answer framed the way the response declared it.
pub(super) fn request(socket: &mut Bounded, target: &HttpTarget) -> Result<String, Fault> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: */*\r\nUser-Agent: {USER_AGENT}\r\nConnection: close\r\n\r\n",
        target.path, target.host
    );
    socket.write_all(request.as_bytes())?;

    let headers = socket.read_headers(HEADER_LIMIT)?;
    let status = status_of(&headers)?;
    match status {
        200..=299 => {}
        // Only a refusal to authenticate is a credential problem rather than a
        // bad endpoint.
        401 | 407 => {
            return Err(Fault::new(
                FaultClass::Auth,
                format!("the endpoint refused the request with {status}"),
            ));
        }
        status => {
            return Err(Fault::new(
                FaultClass::Http(status),
                format!("the endpoint answered {status}"),
            ));
        }
    }
    read_body(socket, &headers)
}

/// The body, read the way the response said it was framed.
///
/// A declared length is the stronger frame and is preferred: asking for
/// `Connection: close` and then waiting for the close makes the reading depend
/// on the endpoint honouring a request it is free to ignore, and an endpoint
/// that keeps the connection open would turn a perfectly good address into a
/// timeout.
pub(super) fn read_body(socket: &mut Bounded, headers: &str) -> Result<String, Fault> {
    match content_length(headers) {
        Some(length) => {
            if length > READING_LIMIT {
                return Err(Fault::new(
                    FaultClass::Reading,
                    format!(
                        "the endpoint declared a {length}-byte answer, which is not an address"
                    ),
                ));
            }
            let mut body = vec![0u8; length as usize];
            socket.read_exact(&mut body)?;
            Ok(String::from_utf8_lossy(&body).into_owned())
        }
        // No length declared: the close is then the only end there is, and the
        // read still carries the deadline.
        None => socket.read_to_close(READING_LIMIT),
    }
}

/// The status of a response, from its first line.
pub(super) fn status_of(headers: &str) -> Result<u16, Fault> {
    headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            Fault::new(
                FaultClass::Reading,
                "the endpoint answered without a status",
            )
        })
}

/// The length a response declared for its body, when it declared one.
pub(super) fn content_length(headers: &str) -> Option<u64> {
    headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
}

/// The class a SOCKS5 reply code belongs to.
///
/// These are the protocol's own codes, so this is a reading of the standard
/// rather than a guess at a library's wording.
pub(super) fn class_for_reply(code: u8) -> FaultClass {
    match code {
        // "network unreachable", "host unreachable", "connection refused", and
        // a refusal by ruleset all mean the same thing to a user: nothing
        // carried it. Which of the four it was is in the detail.
        0x02..=0x05 => FaultClass::Unreachable,
        0x06 => FaultClass::Timeout,
        // "command not supported" and "address type not supported" are the
        // engine being asked for something it cannot do.
        0x07 | 0x08 => FaultClass::Config,
        _ => FaultClass::Other,
    }
}

/// The protocol's own name for a reply code.
pub(super) fn reply_label(code: u8) -> String {
    let name = match code {
        0x01 => "general SOCKS server failure",
        0x02 => "connection not allowed by ruleset",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown reply code",
    };
    format!("{name} ({code:#04x})")
}
