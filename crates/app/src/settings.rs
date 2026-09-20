//! Process-level configuration, and where each value came from.
//!
//! Three settings can be changed from the window: the data directory, the Xray
//! executable, and the address endpoint the proxy diagnostic asks. The first two
//! decide what the *next* start does, so they cannot live in the data
//! directory's database: changing the data directory would move the database,
//! and the setting would be forgotten in the move. They live in a config file of
//! their own instead, outside the data directory. The endpoint is read fresh on
//! every test, so it takes effect immediately; it lives here because it is a
//! standing choice about a machine, not a per-profile one.
//!
//! Precedence is environment, then config file, then default. The page shows
//! which one won, and when the environment is overriding a stored value it says
//! so along with the value being ignored - a setting that looks saved and is
//! not would be worse than no setting at all.
//!
//! A config file that cannot be read is never silently treated as empty: it is
//! reported, the defaults are used, and saving is refused until it is fixed, so
//! a typo cannot cost the user the settings it still holds.

use crate::paths;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Data root: profiles, cores, the database, runtime files.
///
/// The default for it is the platform's own place for application data, which
/// lives in [`crate::paths`]; this is the environment variable that overrides
/// it.
pub const DATA_DIR_ENV: &str = "FP_BROWSER_DATA_DIR";
/// Xray executable, used when a profile has a proxy.
pub const XRAY_BIN_ENV: &str = "FP_BROWSER_XRAY_BIN";
/// Chromium binary registered at startup; cores are managed on their own page.
pub const CHROMIUM_BIN_ENV: &str = "FP_BROWSER_CHROMIUM_BIN";
/// Majors for binaries that do not answer `--version` usefully.
pub const CHROMIUM_MAJOR_ENV: &str = "FP_BROWSER_CHROMIUM_MAJOR";
/// Where the two editable settings are stored.
pub const CONFIG_ENV: &str = "FP_BROWSER_CONFIG";
/// The address endpoint the proxy diagnostic asks.
pub const ECHO_URL_ENV: &str = "FP_BROWSER_ECHO_URL";

/// A message for the window banner: the text and whether it is an error.
pub type Notice = (String, bool);

fn default_xray() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("bin/xray.exe")
    } else {
        PathBuf::from("bin/xray")
    }
}

/// Which setting a row describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKey {
    DataDir,
    XrayExecutable,
    EchoUrl,
    ChromiumBin,
    ChromiumMajor,
    ConfigFile,
    RuntimeDir,
}

impl SettingKey {
    /// Every key, in the order the page shows them.
    #[cfg(test)]
    pub const ALL: [Self; 7] = [
        Self::DataDir,
        Self::XrayExecutable,
        Self::EchoUrl,
        Self::ChromiumBin,
        Self::ChromiumMajor,
        Self::ConfigFile,
        Self::RuntimeDir,
    ];

    /// The stable id the window and the tests use.
    pub fn id(self) -> &'static str {
        match self {
            Self::DataDir => "data-dir",
            Self::XrayExecutable => "xray-executable",
            Self::EchoUrl => "echo-url",
            Self::ChromiumBin => "chromium-bin",
            Self::ChromiumMajor => "chromium-major",
            Self::ConfigFile => "config-file",
            Self::RuntimeDir => "runtime-dir",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DataDir => "Data directory",
            Self::XrayExecutable => "Xray executable",
            Self::EchoUrl => "Proxy test endpoint",
            Self::ChromiumBin => "Chromium binary",
            Self::ChromiumMajor => "Chromium major override",
            Self::ConfigFile => "Config file",
            Self::RuntimeDir => "Runtime directory",
        }
    }

    /// Whether the window may change it.
    pub fn editable(self) -> bool {
        matches!(self, Self::DataDir | Self::XrayExecutable | Self::EchoUrl)
    }

    /// When a change takes effect.
    pub fn effect(self) -> &'static str {
        match self {
            Self::DataDir => "next start",
            Self::XrayExecutable => "next start",
            Self::RuntimeDir => "derived from the data directory",
            _ => "now",
        }
    }
}

/// Where the effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Environment,
    ConfigFile,
    Default,
    /// Not chosen anywhere: computed from another setting.
    Derived,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::ConfigFile => "config file",
            Self::Default => "default",
            Self::Derived => "derived from the data directory",
        }
    }
}

/// One line of the settings page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRow {
    pub key: SettingKey,
    pub value: String,
    pub source: Source,
    /// The environment variable behind the value, when one decided it.
    pub env: Option<&'static str>,
    /// A value the environment is overriding: shown, not hidden, because a
    /// stored setting that silently does nothing is a trap.
    pub shadowed: Option<String>,
    pub note: Option<String>,
}

impl SettingRow {
    /// `set by FP_BROWSER_DATA_DIR`, `from the config file`, `default`.
    pub fn source_label(&self) -> String {
        match (self.source, self.env) {
            (Source::Environment, Some(env)) => format!("set by {env}"),
            (Source::Derived, _) => self.source.label().to_string(),
            _ => format!("from the {}", self.source.label()),
        }
    }

    /// What the window says about a stored value the environment is winning.
    pub fn shadowed_label(&self) -> Option<String> {
        self.shadowed
            .as_ref()
            .map(|value| format!("the config file holds {value}, which this overrides"))
    }
}

/// The settings as they were stored, before the environment is applied.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Stored {
    data_dir: Option<String>,
    xray_executable: Option<String>,
    echo_url: Option<String>,
}

/// The environment, read once so a test can supply its own.
#[derive(Debug, Default, Clone)]
pub struct Environment {
    pub data_dir: Option<String>,
    pub xray_executable: Option<String>,
    pub echo_url: Option<String>,
    pub chromium_bin: Option<String>,
    pub chromium_major: Option<String>,
    pub config: Option<String>,
    /// The host's own directories, so a test can say which platform it means.
    ///
    /// `Default` is a host with no home directory, which is what most tests
    /// want: the relative fallback, as the default used to be for every host.
    pub host: paths::Host,
}

impl Environment {
    pub fn from_process() -> Self {
        let read = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        Self {
            data_dir: read(DATA_DIR_ENV),
            xray_executable: read(XRAY_BIN_ENV),
            echo_url: read(ECHO_URL_ENV),
            chromium_bin: read(CHROMIUM_BIN_ENV),
            chromium_major: read(CHROMIUM_MAJOR_ENV),
            config: read(CONFIG_ENV),
            host: paths::Host::from_process(),
        }
    }
}

/// Where the config file lives when nothing overrides it.
fn default_config_path(host: &paths::Host) -> PathBuf {
    paths::config_dir(host)
        .unwrap_or_else(|| PathBuf::from(".").join(paths::CONFIG_DIR))
        .join("config.json")
}

/// The endpoint the proxy diagnostic asks what address it left from.
///
/// The default is plain HTTP on purpose. The first question a proxy test asks is
/// whether traffic left at all; over TLS a certificate that does not match the
/// endpoint would look exactly like a proxy that does not forward, and the two
/// need opposite fixes. An `https://` value is not downgraded: the diagnostic
/// refuses it with a configuration fault rather than quietly asking a different
/// question than the one the address appears to ask.
fn resolve_echo_url(stored: &Stored, env: &Environment) -> String {
    env.echo_url
        .clone()
        .or_else(|| stored.echo_url.clone())
        .unwrap_or_else(|| runtime::DEFAULT_ECHO_URL.to_string())
}

pub struct Settings {
    config_path: PathBuf,
    stored: Stored,
    env: Environment,
    /// Set when the config file exists but could not be read or parsed.
    config_error: Option<String>,
    data_dir: PathBuf,
    xray_executable: PathBuf,
    echo_url: String,
}

impl Settings {
    /// Reads the environment and the config file, without failing.
    ///
    /// A broken config file is reported and the defaults are used, so the app
    /// still starts; saving is refused until it is fixed.
    pub fn load(env: Environment) -> (Self, Option<Notice>) {
        let config_path = env
            .config
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| default_config_path(&env.host));

        let (stored, config_error) = match read_config(&config_path) {
            Ok(stored) => (stored, None),
            Err(error) => (Stored::default(), Some(error)),
        };

        let data_dir = env
            .data_dir
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| stored.data_dir.as_ref().map(PathBuf::from))
            .unwrap_or_else(|| paths::data_dir_or_fallback(&env.host));
        let xray_executable = env
            .xray_executable
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| stored.xray_executable.as_ref().map(PathBuf::from))
            .unwrap_or_else(default_xray);

        let echo_url = resolve_echo_url(&stored, &env);

        let notice = config_error.as_ref().map(|error| {
            (
                format!("{error}; using defaults, and settings cannot be saved until it is fixed"),
                true,
            )
        });

        let settings = Self {
            config_path,
            stored,
            env,
            config_error,
            data_dir,
            xray_executable,
            echo_url,
        };
        (settings, notice)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn xray_executable(&self) -> &Path {
        &self.xray_executable
    }

    /// The endpoint the proxy test asks, which may see the address it is asked
    /// from: that is the whole point of asking it.
    pub fn echo_url(&self) -> &str {
        &self.echo_url
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.data_dir.join("runtime")
    }

    /// Every line of the settings page, in the order it is shown.
    pub fn rows(&self) -> Vec<SettingRow> {
        let path = |value: &Path| value.to_string_lossy().to_string();

        vec![
            SettingRow {
                key: SettingKey::DataDir,
                value: self.rendered(&self.data_dir),
                source: self.source_of(self.env.data_dir.is_some(), self.stored.data_dir.is_some()),
                env: self.env.data_dir.as_ref().map(|_| DATA_DIR_ENV),
                shadowed: shadowed(&self.stored.data_dir, self.env.data_dir.is_some()),
                note: None,
            },
            SettingRow {
                key: SettingKey::XrayExecutable,
                value: path(&self.xray_executable),
                source: self.source_of(
                    self.env.xray_executable.is_some(),
                    self.stored.xray_executable.is_some(),
                ),
                env: self.env.xray_executable.as_ref().map(|_| XRAY_BIN_ENV),
                shadowed: shadowed(&self.stored.xray_executable, self.env.xray_executable.is_some()),
                note: None,
            },
            SettingRow {
                key: SettingKey::EchoUrl,
                value: self.echo_url.clone(),
                source: self.source_of(self.env.echo_url.is_some(), self.stored.echo_url.is_some()),
                env: self.env.echo_url.as_ref().map(|_| ECHO_URL_ENV),
                shadowed: shadowed(&self.stored.echo_url, self.env.echo_url.is_some()),
                note: Some(
                    "asked to report the address a connection left from, so it sees that address; \
                     asked only when you run a proxy test"
                        .to_string(),
                ),
            },
            SettingRow {
                key: SettingKey::ChromiumBin,
                value: self
                    .env
                    .chromium_bin
                    .clone()
                    .unwrap_or_else(|| "not set; discovered on PATH".to_string()),
                source: if self.env.chromium_bin.is_some() {
                    Source::Environment
                } else {
                    Source::Default
                },
                env: self.env.chromium_bin.as_ref().map(|_| CHROMIUM_BIN_ENV),
                shadowed: None,
                note: Some(
                    "cores discovered from it are listed on the Browser Cores page".to_string(),
                ),
            },
            SettingRow {
                key: SettingKey::ChromiumMajor,
                value: self
                    .env
                    .chromium_major
                    .clone()
                    .unwrap_or_else(|| "not set; read from each binary".to_string()),
                source: if self.env.chromium_major.is_some() {
                    Source::Environment
                } else {
                    Source::Default
                },
                env: self.env.chromium_major.as_ref().map(|_| CHROMIUM_MAJOR_ENV),
                shadowed: None,
                note: Some("used only for binaries that answer nothing to --version".to_string()),
            },
            SettingRow {
                key: SettingKey::ConfigFile,
                value: path(&self.config_path),
                source: if self.env.config.is_some() {
                    Source::Environment
                } else {
                    Source::Default
                },
                env: self.env.config.as_ref().map(|_| CONFIG_ENV),
                shadowed: None,
                note: Some(
                    "the data directory and Xray executable are stored here, outside the data directory, so changing the data directory cannot lose them"
                        .to_string(),
                ),
            },
            SettingRow {
                key: SettingKey::RuntimeDir,
                value: path(&self.runtime_dir()),
                source: Source::Derived,
                env: None,
                shadowed: None,
                note: Some("temporary launch files; Xray configs are removed on shutdown".to_string()),
            },
        ]
    }

    /// A relative path is shown with the directory it resolves against, so the
    /// window never shows a path the user cannot find.
    fn rendered(&self, path: &Path) -> String {
        if path.is_absolute() || path.has_root() {
            return path.to_string_lossy().to_string();
        }
        match std::env::current_dir() {
            Ok(cwd) => format!("{} (relative to {})", path.display(), cwd.display()),
            Err(_) => path.to_string_lossy().to_string(),
        }
    }

    fn source_of(&self, from_env: bool, from_file: bool) -> Source {
        if from_env {
            Source::Environment
        } else if from_file {
            Source::ConfigFile
        } else {
            Source::Default
        }
    }

    /// Stores one editable setting in the config file.
    ///
    /// Refused while the config file is unreadable, because writing would
    /// replace whatever it still holds.
    pub fn set(&mut self, key: SettingKey, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{} cannot be empty", key.label().to_lowercase()));
        }
        if let Some(error) = &self.config_error {
            return Err(format!(
                "{} cannot be written while the config file is unreadable: {error}",
                self.config_path.display()
            ));
        }

        let mut stored = self.stored.clone();
        match key {
            SettingKey::DataDir => stored.data_dir = Some(value.to_string()),
            SettingKey::XrayExecutable => stored.xray_executable = Some(value.to_string()),
            SettingKey::EchoUrl => stored.echo_url = Some(value.to_string()),
            _ => return Err(format!("{} is not editable", key.label())),
        }

        write_config(&self.config_path, &stored)?;
        self.stored = stored;
        // Re-resolve, so the page shows the new value without a restart even
        // though the process keeps using the old one until it starts again.
        self.data_dir = self
            .env
            .data_dir
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| self.stored.data_dir.as_ref().map(PathBuf::from))
            .unwrap_or_else(|| paths::data_dir_or_fallback(&self.env.host));
        self.xray_executable = self
            .env
            .xray_executable
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| self.stored.xray_executable.as_ref().map(PathBuf::from))
            .unwrap_or_else(default_xray);
        // This one is read fresh on every test, so the new value is live now
        // rather than at the next start.
        self.echo_url = resolve_echo_url(&self.stored, &self.env);
        Ok(())
    }

    /// The value a setting would take on the next start.
    ///
    /// `None` means nothing is pending: either the setting is not stored, or the
    /// environment is winning and what was saved will not be used. A setting
    /// whose `effect` is "now" is never pending either - it is already in use, so
    /// there is no restart for it to wait on.
    #[cfg(test)]
    pub fn pending(&self, key: SettingKey) -> Option<String> {
        match key {
            SettingKey::DataDir if self.env.data_dir.is_some() => None,
            SettingKey::DataDir => self.stored.data_dir.clone(),
            SettingKey::XrayExecutable if self.env.xray_executable.is_some() => None,
            SettingKey::XrayExecutable => self.stored.xray_executable.clone(),
            _ => None,
        }
    }
}

/// The stored value the environment is overriding, if any.
fn shadowed(stored: &Option<String>, from_env: bool) -> Option<String> {
    if from_env { stored.clone() } else { None }
}

fn read_config(path: &Path) -> Result<Stored, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("could not read {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Stored::default()),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

fn write_config(path: &Path, stored: &Stored) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(stored)
        .map_err(|error| format!("could not encode the settings: {error}"))?;
    std::fs::write(path, format!("{text}\n"))
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::FALLBACK_DATA_DIR;

    /// A config file in its own directory, removed when the test ends.
    struct TempConfig {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempConfig {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-settings-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            let path = dir.join("config.json");
            Self { dir, path }
        }

        fn write(&self, text: &str) {
            std::fs::write(&self.path, text).expect("write config");
        }

        fn read(&self) -> String {
            std::fs::read_to_string(&self.path).expect("read config")
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn env(config: &TempConfig) -> Environment {
        Environment {
            config: Some(config.path.to_string_lossy().to_string()),
            ..Environment::default()
        }
    }

    /// The default is the platform's own place for application data; the
    /// relative `data` is only what is left for a host with no home directory.
    #[test]
    fn the_default_data_directory_follows_the_platform() {
        let config = TempConfig::new("platform");
        let with_home = |os: &'static str, home: &str| paths::Host {
            os,
            home: Some(PathBuf::from(home)),
            ..paths::Host::default()
        };
        let windows_base = PathBuf::from(r"C:\Users\me\AppData\Local");
        let cases = [
            (
                paths::Host {
                    local_app_data: Some(windows_base.clone()),
                    app_data: Some(PathBuf::from(r"C:\Users\me\AppData\Roaming")),
                    ..with_home("windows", r"C:\Users\me")
                },
                // Built with the same join rather than written out: `Path::join`
                // uses the separator of the host the test runs on.
                windows_base.join(paths::APP_DIR),
            ),
            (
                with_home("linux", "/home/me"),
                PathBuf::from("/home/me/.local/share/FpBrowser"),
            ),
            (
                with_home("macos", "/Users/me"),
                PathBuf::from("/Users/me/Library/Application Support/FpBrowser"),
            ),
            // No home directory to put it under: the fallback, unchanged from
            // what every host used to get.
            (
                paths::Host {
                    os: "linux",
                    ..paths::Host::default()
                },
                PathBuf::from(FALLBACK_DATA_DIR),
            ),
        ];

        for (host, expected) in cases {
            let (settings, _) = Settings::load(Environment {
                config: Some(config.path.to_string_lossy().to_string()),
                host,
                ..Environment::default()
            });
            assert_eq!(settings.data_dir(), expected);
        }
    }

    #[test]
    fn with_nothing_set_every_value_is_the_default() {
        let config = TempConfig::new("defaults");
        let (settings, notice) = Settings::load(env(&config));

        assert!(notice.is_none(), "a missing config file is not an error");
        assert_eq!(settings.data_dir(), Path::new(FALLBACK_DATA_DIR));
        assert_eq!(settings.xray_executable(), default_xray());
        let rows = settings.rows();
        assert_eq!(rows.len(), SettingKey::ALL.len());
        for key in [
            SettingKey::DataDir,
            SettingKey::XrayExecutable,
            SettingKey::EchoUrl,
            SettingKey::ChromiumBin,
            SettingKey::ChromiumMajor,
        ] {
            let row = rows.iter().find(|row| row.key == key).expect("the row");
            assert_ne!(
                row.source,
                Source::Environment,
                "{key:?} came from the environment"
            );
        }
        assert_eq!(
            rows.iter()
                .find(|row| row.key == SettingKey::ConfigFile)
                .expect("the row")
                .source,
            Source::Environment,
            "the test points the config path at a temp file through the environment"
        );
        assert_eq!(rows[0].source_label(), "from the default");
    }

    #[test]
    fn a_stored_value_is_used_and_says_where_it_came_from() {
        let config = TempConfig::new("stored");
        config.write(r#"{"data_dir": "/srv/fp", "xray_executable": "/opt/xray"}"#);
        let (settings, notice) = Settings::load(env(&config));

        assert!(notice.is_none());
        assert_eq!(settings.data_dir(), Path::new("/srv/fp"));
        assert_eq!(settings.xray_executable(), Path::new("/opt/xray"));
        let rows = settings.rows();
        assert_eq!(rows[0].source, Source::ConfigFile);
        assert_eq!(rows[0].source_label(), "from the config file");
        assert_eq!(rows[0].value, "/srv/fp");
        assert_eq!(rows[1].value, "/opt/xray");
    }

    /// A stored setting the environment overrides must be visible, not silently
    /// ignored: this is the "I set it and nothing happened" case.
    #[test]
    fn an_environment_override_shows_the_value_it_shadows() {
        let config = TempConfig::new("shadowed");
        config.write(r#"{"data_dir": "/srv/fp"}"#);
        let mut environment = env(&config);
        environment.data_dir = Some("/tmp/other".to_string());
        let (settings, _) = Settings::load(environment);

        assert_eq!(settings.data_dir(), Path::new("/tmp/other"));
        let row = &settings.rows()[0];
        assert_eq!(row.source, Source::Environment);
        assert_eq!(row.source_label(), "set by FP_BROWSER_DATA_DIR");
        assert_eq!(row.value, "/tmp/other");
        assert_eq!(
            row.shadowed_label().as_deref(),
            Some("the config file holds /srv/fp, which this overrides")
        );
    }

    /// The endpoint the proxy test asks is a standing choice about this machine,
    /// so it is stored and overridable rather than hard-coded.
    #[test]
    fn the_proxy_test_endpoint_is_stored_and_takes_effect_immediately() {
        let config = TempConfig::new("echo");
        let (mut settings, _) = Settings::load(env(&config));

        assert_eq!(settings.echo_url(), runtime::DEFAULT_ECHO_URL);
        assert!(
            settings.echo_url().starts_with("http://"),
            "the default must be plain http, so a certificate problem cannot be \
             read as a proxy problem: {}",
            settings.echo_url()
        );

        settings
            .set(SettingKey::EchoUrl, "  http://echo.example/ip  ")
            .expect("save the endpoint");

        assert_eq!(settings.echo_url(), "http://echo.example/ip", "trimmed");
        assert!(
            config.read().contains("http://echo.example/ip"),
            "it is stored, not just held in memory: {}",
            config.read()
        );

        let row = settings
            .rows()
            .into_iter()
            .find(|row| row.key == SettingKey::EchoUrl)
            .expect("a row for the endpoint");
        assert_eq!(row.source, Source::ConfigFile);
        assert_eq!(
            row.key.effect(),
            "now",
            "the next test asks this endpoint, not the next start"
        );
        assert_eq!(row.value, "http://echo.example/ip");
        assert_eq!(
            settings.pending(SettingKey::EchoUrl),
            None,
            "nothing is waiting on a restart: the endpoint is already in use"
        );
    }

    /// The row has to say that the endpoint learns the address it is asked from,
    /// because that is the one thing about this setting a user cannot infer.
    #[test]
    fn the_endpoint_row_says_what_the_endpoint_learns() {
        let config = TempConfig::new("echo-note");
        let (settings, _) = Settings::load(env(&config));
        let row = settings
            .rows()
            .into_iter()
            .find(|row| row.key == SettingKey::EchoUrl)
            .expect("a row for the endpoint");
        let note = row.note.expect("a note about what it sees");
        assert!(note.contains("sees that address"), "{note}");
        assert!(
            note.contains("only when you run a proxy test"),
            "it is asked on demand, not in the background: {note}"
        );
        assert!(row.key.editable(), "the endpoint is the user's to choose");
    }

    #[test]
    fn an_environment_endpoint_shadows_the_stored_one() {
        let config = TempConfig::new("echo-env");
        config.write(r#"{"echo_url": "http://stored.example/ip"}"#);
        let mut environment = env(&config);
        environment.echo_url = Some("http://env.example/ip".to_string());
        let (settings, _) = Settings::load(environment);

        assert_eq!(settings.echo_url(), "http://env.example/ip");
        let row = settings
            .rows()
            .into_iter()
            .find(|row| row.key == SettingKey::EchoUrl)
            .expect("a row for the endpoint");
        assert_eq!(row.source, Source::Environment);
        assert_eq!(row.source_label(), "set by FP_BROWSER_ECHO_URL");
        assert_eq!(
            row.value, "http://env.example/ip",
            "the page shows the endpoint a test would really ask"
        );
        assert_eq!(
            row.shadowed_label().as_deref(),
            Some("the config file holds http://stored.example/ip, which this overrides"),
            "a stored endpoint the environment wins must be shown, not hidden"
        );
    }

    #[test]
    fn a_broken_config_file_is_reported_and_does_not_lose_settings() {
        let config = TempConfig::new("broken");
        config.write("{ this is not json");
        let (mut settings, notice) = Settings::load(env(&config));

        let (message, is_error) = notice.expect("the problem is reported");
        assert!(is_error);
        assert!(message.contains("config.json"), "{message}");
        assert_eq!(
            settings.data_dir(),
            Path::new(FALLBACK_DATA_DIR),
            "the defaults are used so the app still starts"
        );

        let refusal = settings
            .set(SettingKey::DataDir, "/srv/fp")
            .expect_err("saving is refused while the file is broken");
        assert!(refusal.contains("unreadable"), "{refusal}");
        assert_eq!(
            config.read(),
            "{ this is not json",
            "the existing file was not overwritten"
        );
    }

    #[test]
    fn saving_keeps_the_other_setting() {
        let config = TempConfig::new("keep");
        let (mut settings, _) = Settings::load(env(&config));

        settings
            .set(SettingKey::DataDir, "/srv/fp")
            .expect("save data dir");
        settings
            .set(SettingKey::XrayExecutable, "/opt/xray")
            .expect("save xray");
        settings
            .set(SettingKey::EchoUrl, "http://echo.example/ip")
            .expect("save the endpoint");

        let text = config.read();
        assert!(text.contains("/srv/fp"), "{text}");
        assert!(text.contains("/opt/xray"), "{text}");
        assert!(
            text.contains("http://echo.example/ip"),
            "saving one setting rewrites the file, so every other value has to \
             survive the rewrite: {text}"
        );

        // And a fresh load agrees.
        let (reloaded, _) = Settings::load(env(&config));
        assert_eq!(reloaded.data_dir(), Path::new("/srv/fp"));
        assert_eq!(reloaded.xray_executable(), Path::new("/opt/xray"));
        assert_eq!(reloaded.echo_url(), "http://echo.example/ip");
    }

    #[test]
    fn a_saved_value_is_the_pending_one_even_while_the_process_uses_the_old() {
        let config = TempConfig::new("pending");
        let (mut settings, _) = Settings::load(env(&config));
        assert_eq!(settings.pending(SettingKey::DataDir), None);

        settings.set(SettingKey::DataDir, "/srv/fp").expect("save");

        assert_eq!(
            settings.rows()[0].value,
            "/srv/fp",
            "the page shows what the next start will use"
        );
        assert_eq!(
            settings.pending(SettingKey::DataDir).as_deref(),
            Some("/srv/fp")
        );
    }

    #[test]
    fn an_environment_override_has_no_pending_value() {
        let config = TempConfig::new("pending-env");
        let mut environment = env(&config);
        environment.data_dir = Some("/tmp/other".to_string());
        let (mut settings, _) = Settings::load(environment);

        // Stored, but overridden: it is not what the next start will use.
        settings.set(SettingKey::DataDir, "/srv/fp").expect("save");
        assert_eq!(settings.pending(SettingKey::DataDir), None);
        assert_eq!(settings.data_dir(), Path::new("/tmp/other"));
    }

    #[test]
    fn an_empty_value_is_refused() {
        let config = TempConfig::new("empty");
        let (mut settings, _) = Settings::load(env(&config));
        let error = settings
            .set(SettingKey::DataDir, "   ")
            .expect_err("empty is refused");
        assert!(error.contains("cannot be empty"), "{error}");
    }

    #[test]
    fn the_read_only_settings_cannot_be_saved() {
        let config = TempConfig::new("readonly");
        let (mut settings, _) = Settings::load(env(&config));
        for key in [
            SettingKey::ChromiumBin,
            SettingKey::ChromiumMajor,
            SettingKey::ConfigFile,
            SettingKey::RuntimeDir,
        ] {
            assert!(!key.editable(), "{key:?} is not editable");
            assert!(settings.set(key, "/tmp/x").is_err(), "{key:?}");
        }
    }

    #[test]
    fn the_runtime_directory_follows_the_data_directory() {
        let config = TempConfig::new("runtime");
        let (settings, _) = Settings::load(env(&config));
        assert_eq!(settings.runtime_dir(), Path::new("data/runtime"));
        let row = settings
            .rows()
            .into_iter()
            .find(|row| row.key == SettingKey::RuntimeDir)
            .expect("a runtime row");
        assert_eq!(row.key.effect(), "derived from the data directory");
        assert_eq!(row.source, Source::Derived);
        assert_eq!(
            row.source_label(),
            "derived from the data directory",
            "it is computed, not chosen anywhere"
        );
    }

    #[test]
    fn a_relative_data_directory_is_shown_against_the_working_directory() {
        let config = TempConfig::new("relative");
        let (settings, _) = Settings::load(env(&config));
        let value = &settings.rows()[0].value;
        assert!(value.starts_with(FALLBACK_DATA_DIR), "{value}");
        assert!(
            value.contains("relative to"),
            "a relative path is shown with what it is relative to: {value}"
        );
    }

    #[test]
    fn the_settings_page_names_every_source() {
        let config = TempConfig::new("sources");
        config.write(r#"{"data_dir": "/srv/fp"}"#);
        let mut environment = env(&config);
        environment.chromium_bin = Some("/opt/chrome".to_string());
        let (settings, _) = Settings::load(environment);

        let rows = settings.rows();
        let by_key = |key: SettingKey| {
            rows.iter()
                .find(|row| row.key == key)
                .expect("the row exists")
                .clone()
        };
        assert_eq!(by_key(SettingKey::DataDir).source, Source::ConfigFile);
        assert_eq!(
            by_key(SettingKey::XrayExecutable).source,
            Source::Default,
            "the xray path has no stored value"
        );
        assert_eq!(by_key(SettingKey::ChromiumBin).source, Source::Environment);
        assert_eq!(
            by_key(SettingKey::ChromiumMajor).value,
            "not set; read from each binary"
        );
        assert_eq!(
            by_key(SettingKey::ConfigFile).value,
            config.path.to_string_lossy()
        );
    }
}
