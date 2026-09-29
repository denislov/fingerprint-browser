//! The proxy this machine says it uses.
//!
//! A profile's traffic leaves through the profile's own proxy, and this is the
//! one thing that can be put in front of it: when the reader asks for it, the
//! engine dials the profile's proxy *through* the machine's proxy, which is what
//! makes a chain out of a hop the machine already has and a hop the profile has.
//! Nothing here is used unless that switch is on - the program does not follow
//! the system proxy for its own sake, and a profile without a proxy never
//! touches this.
//!
//! What is read is the machine's own answer, most specific first: the process's
//! environment, because that is the answer for *this* process and the only kind
//! a script that started the program can set, and then the desktop or operating
//! system setting behind it. Only a TCP proxy can be read - an HTTP proxy the
//! engine can send CONNECT to, or a SOCKS5 one - because those are the only
//! first hops the engine has an outbound for. A machine that picks its proxy with
//! a script (PAC, or WPAD's automatic discovery) is reported as choosing one
//! rather than as having none: a reader who turned the switch on and watched
//! nothing happen is owed the difference between "there is nothing here" and
//! "there is something here that this program cannot read".
//!
//! Two things are deliberately not read. KDE's own store, which is a file whose
//! keys have changed shape between releases; the environment is the answer
//! there, and `README.md` says so beside the setting. And the Windows and macOS
//! readers are not verified on those machines by the person who wrote them: they
//! are parsed from `reg query` and `scutil --proxy` output, which is what their
//! tests are over, and the environment remains the source that is the same
//! everywhere.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How the engine speaks to a proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// An HTTP proxy, dialled with CONNECT.
    Http,
    /// A SOCKS5 proxy.
    Socks5,
}

impl Protocol {
    /// The name the engine gives this protocol in a config's `outbounds`.
    pub(crate) fn engine_name(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Socks5 => "socks",
        }
    }

    /// The name a reader sees: the protocol's own name, in every language,
    /// rather than a translated word.
    pub fn label(self) -> &'static str {
        match self {
            Self::Http => "HTTP",
            Self::Socks5 => "SOCKS5",
        }
    }

    /// The port a URL of this kind means when it names none.
    fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Socks5 => 1080,
        }
    }
}

/// A proxy the engine can dial before it reaches the profile's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProxy {
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    /// Read only where the machine keeps them in the clear: a URL's userinfo,
    /// and GNOME's own keys. There is nowhere else to read them from - Windows
    /// keeps them in the credential manager, and `scutil` does not print them.
    pub username: Option<String>,
    pub password: Option<String>,
}

impl SystemProxy {
    /// `host:port`, as a reader and a log line name it.
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// What reading the machine answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// A proxy the engine can dial through.
    Found(SystemProxy),
    /// The machine chooses its proxy with a script, which this program does not
    /// evaluate. Reported rather than ignored: it is the one case where turning
    /// the switch on does nothing at all, and the reader cannot see why.
    Automatic,
    /// Nothing is configured, or nothing this program can read is.
    None,
}

/// Reads the machine's proxy.
///
/// Called when a profile starts and when the switch is flipped, and not while a
/// page is drawn: every source but the environment is a process to spawn, and a
/// page is drawn many times a second.
pub fn detect() -> Reading {
    match environment() {
        Answer::Nothing => platform(),
        answer => answer,
    }
    .reading()
}

/// One source's answer, before it is known to be the machine's.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Proxy(SystemProxy),
    Automatic,
    Nothing,
}

impl Answer {
    fn reading(self) -> Reading {
        match self {
            Self::Proxy(proxy) => Reading::Found(proxy),
            Self::Automatic => Reading::Automatic,
            Self::Nothing => Reading::None,
        }
    }
}

/// The variables a proxy is set in, in the order one is preferred.
///
/// `all_proxy` leads because it is the one that means every protocol, and the
/// pair below it is the one most tools are configured with. Both spellings of
/// each, because a lowercase name is a convention rather than a rule.
const ENV_VARS: [&str; 6] = [
    "all_proxy",
    "ALL_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "http_proxy",
    "HTTP_PROXY",
];

/// The process's own answer.
///
/// Preferred over the machine's because it is the more specific statement, and
/// because it is the one this program can be started with: a shell that exports
/// a proxy for one command means that command.
fn environment() -> Answer {
    for name in ENV_VARS {
        let Ok(value) = std::env::var(name) else {
            continue;
        };
        // An empty variable is one someone cleared, not a proxy.
        if value.trim().is_empty() {
            continue;
        }
        if let Some(proxy) = parse_url(value.trim()) {
            return Answer::Proxy(proxy);
        }
    }
    Answer::Nothing
}

/// A proxy URL: `scheme://[user:password@]host[:port]`, or a bare `host:port`.
///
/// A scheme the engine has no outbound for - SOCKS4, whose name is the one
/// people reach for and whose protocol the engine cannot speak - is not guessed
/// at: the variable is passed over as though it named another program's proxy,
/// which is what it does.
///
/// An `https://` proxy is read as a plain HTTP one on that host and port. What
/// `https` means in a URL like this is that the connection *to the proxy* is
/// TLS, and the engine has no setting for that; the port is kept, so the value
/// still names the endpoint, and the engine's own failure to speak to it is
/// reported rather than hidden.
fn parse_url(text: &str) -> Option<SystemProxy> {
    let (scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
        None => (None, text),
    };
    let protocol = match scheme.as_deref() {
        None | Some("http") | Some("https") => Protocol::Http,
        Some("socks") | Some("socks5") | Some("socks5h") => Protocol::Socks5,
        Some(_) => return None,
    };
    // `rsplit`, because a password may contain an `@` and the host may not.
    let (credentials, authority) = match rest.rsplit_once('@') {
        Some((credentials, authority)) => (Some(credentials), authority),
        None => (None, rest),
    };
    let (host, port) = split_authority(authority)?;
    let (username, password) = match credentials.and_then(|credentials| credentials.split_once(':'))
    {
        Some((user, password)) => (Some(user.to_string()), Some(password.to_string())),
        // A username with no password is not a credential pair an HTTP proxy
        // accepts, and sending half of one is worse than sending none.
        None => (None, None),
    };
    Some(SystemProxy {
        protocol,
        port: port.unwrap_or_else(|| protocol.default_port()),
        host,
        username,
        password,
    })
}

/// `host`, `host:port`, `[::1]:port`, and nothing else.
fn split_authority(authority: &str) -> Option<(String, Option<u16>)> {
    // A path after the authority belongs to the URL's protocol, not to the
    // proxy: `http://host:8080/` is the same endpoint as without the slash.
    let authority = authority
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        if host.is_empty() {
            return None;
        }
        let port = match rest.trim() {
            "" => None,
            rest => Some(rest.strip_prefix(':')?.parse().ok()?),
        };
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            // A second colon in an authority that was not bracketed is an IPv6
            // address written without its brackets, where what follows the last
            // colon is a segment of the address rather than a port.
            if host.contains(':') {
                return None;
            }
            Some((host.to_string(), Some(port.trim().parse().ok()?)))
        }
        // An empty host before a colon, which is nothing to dial.
        Some(_) => None,
        None => Some((authority.to_string(), None)),
    }
}

#[cfg(target_os = "linux")]
fn platform() -> Answer {
    gnome()
}

/// The desktop's own answer, read through `gsettings`.
///
/// The one desktop store on Linux with a command in front of it. `auto` is the
/// mode whose proxy is a script; `manual` is the one this can use, and both
/// halves of it are read, SOCKS first because a SOCKS proxy carries a tunnel
/// better than an HTTP one that has to be asked for CONNECT each time.
#[cfg(target_os = "linux")]
fn gnome() -> Answer {
    gnome_from(|args| command_output("gsettings", args))
}

/// Compiled wherever the tests run, so the parser is covered on the platform
/// that runs the gate as well as on the one that has `gsettings`.
#[cfg(any(target_os = "linux", test))]
fn gnome_from(read: impl FnMut(&[&str]) -> Option<String>) -> Answer {
    let mut read = read;
    match gnome_key(&mut read, "org.gnome.system.proxy", "mode").as_deref() {
        Some("manual") => {
            if let Some(proxy) = gnome_proxy(&mut read, "org.gnome.system.proxy.socks") {
                return Answer::Proxy(proxy);
            }
            // An HTTP proxy is the same shape of answer with a different
            // protocol, so the reading is shared and only the name differs.
            if let Some(proxy) = gnome_proxy(&mut read, "org.gnome.system.proxy.http") {
                return Answer::Proxy(SystemProxy {
                    protocol: Protocol::Http,
                    ..proxy
                });
            }
            Answer::Nothing
        }
        Some("auto") => Answer::Automatic,
        _ => Answer::Nothing,
    }
}

#[cfg(any(target_os = "linux", test))]
fn gnome_key(
    read: &mut impl FnMut(&[&str]) -> Option<String>,
    schema: &str,
    name: &str,
) -> Option<String> {
    // `gsettings` prints strings quoted, and a value it does not have prints as
    // nothing at all, which is also how a missing key reads.
    read(&["get", schema, name])
        .map(|value| value.trim().trim_matches('\'').to_string())
        .filter(|value| !value.is_empty())
}

/// The proxy one GNOME schema names, if it names one.
#[cfg(any(target_os = "linux", test))]
fn gnome_proxy(
    read: &mut impl FnMut(&[&str]) -> Option<String>,
    schema: &str,
) -> Option<SystemProxy> {
    let host = gnome_key(read, schema, "host")?;
    let port: u16 = gnome_key(read, schema, "port")?.parse().ok()?;
    if port == 0 {
        return None;
    }
    let (username, password) =
        if gnome_key(read, schema, "use-authentication").as_deref() == Some("true") {
            (
                gnome_key(read, schema, "authentication-user"),
                gnome_key(read, schema, "authentication-password"),
            )
        } else {
            (None, None)
        };
    Some(SystemProxy {
        protocol: Protocol::Socks5,
        host,
        port,
        username,
        password,
    })
}

#[cfg(target_os = "macos")]
fn platform() -> Answer {
    match command_output("scutil", &["--proxy"]) {
        Some(text) => parse_scutil(&text),
        None => Answer::Nothing,
    }
}

/// `scutil --proxy`'s output: `Key : value` lines.
///
/// HTTPS before HTTP before SOCKS: a profile's own proxy is nearly always
/// reached over TLS, and both HTTP entries name the same kind of endpoint, so
/// the one that would carry that connection is the one to take. The enables are
/// what decide - a machine keeps the addresses of a proxy it is no longer using
/// in the keys beside them.
#[cfg(any(target_os = "macos", test))]
fn parse_scutil(text: &str) -> Answer {
    let values: Vec<(&str, &str)> = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect();
    let get = |name: &str| -> Option<&str> {
        values
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
    };
    let enabled = |name: &str| get(name) == Some("1");
    let named = |host_key: &str, port_key: &str, protocol: Protocol| -> Option<SystemProxy> {
        let host = get(host_key).filter(|host| !host.is_empty())?;
        let port = get(port_key)?.parse().ok()?;
        if port == 0 {
            return None;
        }
        Some(SystemProxy {
            protocol,
            host: host.to_string(),
            port,
            username: None,
            password: None,
        })
    };

    if enabled("HTTPSEnable")
        && let Some(proxy) = named("HTTPSProxy", "HTTPSPort", Protocol::Http)
    {
        return Answer::Proxy(proxy);
    }
    if enabled("HTTPEnable")
        && let Some(proxy) = named("HTTPProxy", "HTTPPort", Protocol::Http)
    {
        return Answer::Proxy(proxy);
    }
    if enabled("SOCKSEnable")
        && let Some(proxy) = named("SOCKSProxy", "SOCKSPort", Protocol::Socks5)
    {
        return Answer::Proxy(proxy);
    }
    if enabled("ProxyAutoConfigEnable") || enabled("ProxyAutoDiscoveryEnable") {
        return Answer::Automatic;
    }
    Answer::Nothing
}

#[cfg(windows)]
fn platform() -> Answer {
    // The whole key in one call: `reg query` without `/v` prints every value in
    // it, which is one process instead of three.
    match command_output(
        "reg",
        &[
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
        ],
    ) {
        Some(text) => parse_registry(&text),
        None => Answer::Nothing,
    }
}

/// `reg query`'s output: a name, a type and a value, separated by runs of
/// spaces.
///
/// Windows spells a per-protocol proxy as `http=host:port;https=host:port`, and
/// the bare form as `host:port`. The plain entries are preferred over `socks=`,
/// which is Windows' legacy SOCKS4 setting: read as a proxy it works only if the
/// far end also speaks SOCKS5, and read as nothing it would do nothing at all.
/// `AutoConfigURL` is a PAC script, and is reported as one.
#[cfg(any(windows, test))]
fn parse_registry(text: &str) -> Answer {
    let values: Vec<(&str, &str)> = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            match (fields.next(), fields.next(), fields.next()) {
                (Some(name), Some(_kind), Some(value)) => Some((name, value)),
                _ => None,
            }
        })
        .collect();
    let get = |name: &str| -> Option<&str> {
        values
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
    };

    let enabled = get("ProxyEnable").is_some_and(|value| value != "0x0");
    if enabled
        && let Some(server) = get("ProxyServer").filter(|server| !server.is_empty())
        && let Some(proxy) = proxy_server(server)
    {
        return Answer::Proxy(proxy);
    }
    if get("AutoConfigURL").is_some_and(|url| !url.is_empty()) {
        return Answer::Automatic;
    }
    Answer::Nothing
}

/// The endpoint inside a Windows `ProxyServer` value.
#[cfg(any(windows, test))]
fn proxy_server(server: &str) -> Option<SystemProxy> {
    // By kind rather than by position: a machine that lists `ftp=` first still
    // uses its HTTP proxy, and `socks=` is the one taken last.
    let mut plain: Option<SystemProxy> = None;
    let mut socks: Option<SystemProxy> = None;
    for entry in server.split(';') {
        let (scheme, endpoint) = match entry.split_once('=') {
            Some((scheme, endpoint)) => (Some(scheme.trim().to_ascii_lowercase()), endpoint),
            None => (None, entry),
        };
        let Some((host, port)) = split_authority(endpoint.trim()) else {
            continue;
        };
        let Some(port) = port else { continue };
        let proxy = SystemProxy {
            protocol: Protocol::Socks5,
            host,
            port,
            username: None,
            password: None,
        };
        match scheme.as_deref() {
            None | Some("http") | Some("https") => {
                plain.get_or_insert(SystemProxy {
                    protocol: Protocol::Http,
                    ..proxy
                });
            }
            Some("socks") => {
                socks.get_or_insert(proxy);
            }
            Some(_) => {}
        }
    }
    plain.or(socks)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform() -> Answer {
    Answer::Nothing
}

/// How long a source is given to answer.
///
/// Every source is a local process reading a local setting, so this is not a
/// budget for a slow answer - it is the ceiling that keeps a source which never
/// answers from freezing the window, which is drawn by the same thread that asks.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);

/// One command's standard output, when it ran and succeeded.
///
/// A source that is not installed on this machine, or that fails because there
/// is no session to ask, answers nothing rather than failing: "not configured"
/// and "could not read it" both mean the engine dials the profile's proxy
/// directly, and neither is worth a banner.
fn command_output(program: &str, args: &[&str]) -> Option<String> {
    command_output_within(program, args, ANSWER_TIMEOUT)
}

/// The same, with a wait a test can shorten.
fn command_output_within(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: `reg` is a console program, and a console window
        // appearing over the window for a setting this quiet is not acceptable.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.stdout(Stdio::piped()).spawn().ok()?;
    let stdout = child.stdout.take()?;
    // Read on a worker rather than here: waiting on the process would make the
    // process the thing that decides when this returns, and one that never
    // answers would hold the window with it. A source that runs past the wait is
    // killed instead, which closes the pipe the worker is reading and lets it
    // end - and the reading is "nothing", which is a machine to dial directly.
    let (sender, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = String::new();
        let read = std::io::BufReader::new(stdout).read_to_string(&mut buffer);
        let _ = sender.send(read.map(|_| buffer));
    });
    let output = match answer.recv_timeout(timeout) {
        Ok(read) => read.ok(),
        Err(_) => {
            let _ = child.kill();
            None
        }
    };
    let succeeded = child.wait().is_ok_and(|status| status.success());
    output.filter(|_| succeeded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(host: &str, port: u16) -> SystemProxy {
        SystemProxy {
            protocol: Protocol::Http,
            host: host.to_string(),
            port,
            username: None,
            password: None,
        }
    }

    fn socks5(host: &str, port: u16) -> SystemProxy {
        SystemProxy {
            protocol: Protocol::Socks5,
            ..http(host, port)
        }
    }

    /// A source that answers is read, one that is not installed answers
    /// nothing, and one that never answers is given up on rather than waited
    /// for - the window is drawn by the thread that asks.
    #[cfg(unix)]
    #[test]
    fn a_source_is_read_when_it_answers_and_given_up_on_when_it_does_not() {
        assert_eq!(
            command_output_within("echo", &["hello"], Duration::from_secs(5)).as_deref(),
            Some("hello\n")
        );
        assert_eq!(
            command_output_within("no-such-program-here", &[], Duration::from_secs(5)),
            None,
            "a source this machine does not have is not an error"
        );

        let started = std::time::Instant::now();
        assert_eq!(
            command_output_within("sleep", &["30"], Duration::from_millis(150)),
            None
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the wait was bounded"
        );
    }

    #[test]
    fn a_variable_is_read_as_whatever_kind_of_proxy_it_names() {
        assert_eq!(
            parse_url("http://127.0.0.1:7890"),
            Some(http("127.0.0.1", 7890))
        );
        assert_eq!(
            parse_url("socks5://127.0.0.1:1080"),
            Some(socks5("127.0.0.1", 1080))
        );
        // A URL with a path is the same endpoint as without one, and a port that
        // is not written is the scheme's own.
        assert_eq!(
            parse_url("http://proxy.example/"),
            Some(http("proxy.example", 80))
        );
        assert_eq!(
            parse_url("socks5h://proxy.example"),
            Some(socks5("proxy.example", 1080))
        );
        // `https` names an endpoint this reads as an HTTP proxy on that port.
        assert_eq!(
            parse_url("https://proxy.example:8443"),
            Some(http("proxy.example", 8443))
        );
    }

    #[test]
    fn credentials_are_read_from_a_url_and_only_as_a_pair() {
        assert_eq!(
            parse_url("http://user:secret@127.0.0.1:8080"),
            Some(SystemProxy {
                username: Some("user".into()),
                password: Some("secret".into()),
                ..http("127.0.0.1", 8080)
            })
        );
        // A password may hold the `@` and the `:` that separate the parts.
        assert_eq!(
            parse_url("http://user:p@ss:word@127.0.0.1:8080"),
            Some(SystemProxy {
                username: Some("user".into()),
                password: Some("p@ss:word".into()),
                ..http("127.0.0.1", 8080)
            })
        );
        assert_eq!(
            parse_url("http://user@127.0.0.1:8080"),
            Some(http("127.0.0.1", 8080))
        );
    }

    #[test]
    fn an_ipv6_address_is_read_only_in_brackets() {
        assert_eq!(parse_url("http://[::1]:7890"), Some(http("::1", 7890)));
        // Without brackets the colons are not known to be a port separator, and
        // guessing is how a proxy address becomes a machine named "::1".
        assert_eq!(parse_url("http://::1"), None);
    }

    #[test]
    fn a_proxy_this_cannot_speak_to_is_passed_over_rather_than_guessed_at() {
        assert_eq!(parse_url("socks4://127.0.0.1:1080"), None);
        assert_eq!(parse_url("quic://127.0.0.1:443"), None);
        assert_eq!(parse_url(""), None);
        assert_eq!(parse_url("http://"), None);
        assert_eq!(parse_url("http://127.0.0.1:not-a-port"), None);
    }

    /// One address a stand-in answers with: host and port.
    type GnomeAddress = Option<(&'static str, u16)>;
    /// The HTTP half of the same, which can also carry credentials.
    type GnomeHttp = Option<(&'static str, u16, Option<(&'static str, &'static str)>)>;

    /// A stand-in for `gsettings`, answering the keys one test cares about.
    fn gnome_reading(
        mode: &'static str,
        socks: GnomeAddress,
        http: GnomeHttp,
    ) -> impl FnMut(&[&str]) -> Option<String> {
        move |args: &[&str]| {
            let name = *args.last()?;
            let schema = args[1];
            if name == "mode" {
                return Some(format!("'{mode}'"));
            }
            let (host, port, credentials) = match schema {
                "org.gnome.system.proxy.socks" => socks.map(|(host, port)| (host, port, None)),
                "org.gnome.system.proxy.http" => http,
                _ => None,
            }?;
            let value = match name {
                "host" => host.to_string(),
                "port" => port.to_string(),
                "use-authentication" => credentials.is_some().to_string(),
                "authentication-user" => credentials?.0.to_string(),
                "authentication-password" => credentials?.1.to_string(),
                _ => return None,
            };
            Some(format!("'{value}'"))
        }
    }

    #[test]
    fn gnome_answers_manual_auto_and_nothing() {
        assert_eq!(
            gnome_from(gnome_reading("manual", Some(("127.0.0.1", 1080)), None)),
            Answer::Proxy(socks5("127.0.0.1", 1080))
        );
        assert_eq!(
            gnome_from(gnome_reading(
                "manual",
                None,
                Some(("proxy.example", 3128, None))
            )),
            Answer::Proxy(http("proxy.example", 3128))
        );
        // The HTTP half with the credentials the same schema holds.
        assert_eq!(
            gnome_from(gnome_reading(
                "manual",
                None,
                Some(("proxy.example", 3128, Some(("user", "secret"))))
            )),
            Answer::Proxy(SystemProxy {
                username: Some("user".into()),
                password: Some("secret".into()),
                ..http("proxy.example", 3128)
            })
        );
        assert_eq!(
            gnome_from(gnome_reading("auto", None, None)),
            Answer::Automatic
        );
        assert_eq!(
            gnome_from(gnome_reading("none", None, None)),
            Answer::Nothing
        );
        // A manual mode with neither half filled in is not a proxy: the mode is
        // what is left of a machine that used to have one.
        assert_eq!(
            gnome_from(gnome_reading("manual", None, None)),
            Answer::Nothing
        );
    }

    #[test]
    fn scutil_is_read_through_the_enables() {
        let text = "\
<dictionary> {
  ExceptionsList : <array> {
    0 : *.local
  }
  HTTPEnable : 1
  HTTPPort : 3128
  HTTPProxy : proxy.example
  HTTPSEnable : 0
  HTTPSPort : 8443
  HTTPSProxy : old.example
  SOCKSEnable : 0
  SOCKSPort : 0
  SOCKSProxy :
  ProxyAutoConfigEnable : 0
}
";
        // The enabled HTTP pair, not the HTTPS one the machine stopped using,
        // and not the empty SOCKS keys beside it.
        assert_eq!(
            parse_scutil(text),
            Answer::Proxy(http("proxy.example", 3128))
        );
        assert_eq!(
            parse_scutil("  HTTPSEnable : 1\n  HTTPSProxy : tls.example\n  HTTPSPort : 8443\n"),
            Answer::Proxy(http("tls.example", 8443))
        );
        assert_eq!(
            parse_scutil("  SOCKSEnable : 1\n  SOCKSProxy : socks.example\n  SOCKSPort : 1080\n"),
            Answer::Proxy(socks5("socks.example", 1080))
        );
        assert_eq!(
            parse_scutil("  ProxyAutoConfigEnable : 1\n"),
            Answer::Automatic
        );
        assert_eq!(
            parse_scutil("  ProxyAutoDiscoveryEnable : 1\n"),
            Answer::Automatic
        );
        // Enabled with nothing to use, which is a machine mid-edit.
        assert_eq!(parse_scutil("  HTTPEnable : 1\n"), Answer::Nothing);
        assert_eq!(parse_scutil(""), Answer::Nothing);
    }

    #[test]
    fn the_registry_is_read_through_proxy_enable() {
        assert_eq!(
            parse_registry(
                "    ProxyEnable    REG_DWORD    0x1\n    ProxyServer    REG_SZ    http=proxy.example:8080;https=proxy.example:8080\n"
            ),
            Answer::Proxy(http("proxy.example", 8080))
        );
        // A bare `host:port` is the same answer, and `socks=` is taken last.
        assert_eq!(
            parse_registry(
                "    ProxyEnable    REG_DWORD    0x1\n    ProxyServer    REG_SZ    proxy.example:8080\n"
            ),
            Answer::Proxy(http("proxy.example", 8080))
        );
        assert_eq!(
            parse_registry(
                "    ProxyEnable    REG_DWORD    0x1\n    ProxyServer    REG_SZ    socks=socks.example:1080;http=proxy.example:8080\n"
            ),
            Answer::Proxy(http("proxy.example", 8080))
        );
        assert_eq!(
            parse_registry(
                "    ProxyEnable    REG_DWORD    0x1\n    ProxyServer    REG_SZ    socks=socks.example:1080\n"
            ),
            Answer::Proxy(socks5("socks.example", 1080))
        );
    }

    #[test]
    fn the_registry_reports_a_switched_off_proxy_and_a_script() {
        assert_eq!(
            parse_registry(
                "    ProxyEnable    REG_DWORD    0x0\n    ProxyServer    REG_SZ    proxy.example:8080\n"
            ),
            Answer::Nothing
        );
        assert_eq!(
            parse_registry("    AutoConfigURL    REG_SZ    http://wpad.example/proxy.pac\n"),
            Answer::Automatic
        );
        // Switched on with nothing to use it for.
        assert_eq!(
            parse_registry("    ProxyEnable    REG_DWORD    0x1\n    ProxyServer    REG_SZ\n"),
            Answer::Nothing
        );
        assert_eq!(parse_registry(""), Answer::Nothing);
    }
}
