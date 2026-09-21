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
use crate::text::{Lang, Text, text};
use crate::theme::ThemeChoice;
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

    pub fn label(self, t: &Text) -> &'static str {
        match self {
            Self::DataDir => t.setting_data_dir,
            Self::XrayExecutable => t.setting_xray_executable,
            Self::EchoUrl => t.setting_echo_url,
            Self::ChromiumBin => t.setting_chromium_bin,
            Self::ChromiumMajor => t.setting_chromium_major,
            Self::ConfigFile => t.setting_config_file,
            Self::RuntimeDir => t.setting_runtime_dir,
        }
    }

    /// Whether the window may change it.
    pub fn editable(self) -> bool {
        matches!(self, Self::XrayExecutable | Self::EchoUrl)
    }

    /// When a change takes effect.
    ///
    /// An [`Effect`] rather than a phrase: the window compares this value (to
    /// decide what a row says, and what saving reports) and only then turns it
    /// into words. Comparing translated text would make the behaviour of the
    /// program depend on the language it is being read in.
    pub fn effect(self) -> Effect {
        match self {
            Self::DataDir => Effect::NextStart,
            Self::XrayExecutable => Effect::NextStart,
            Self::RuntimeDir => Effect::Derived,
            _ => Effect::Now,
        }
    }
}

/// When a setting's change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// The row and the dialog still describe a future start.
    NextStart,
    /// Read when the action happens, so the change is live.
    Now,
    /// Not chosen anywhere: computed from another setting.
    Derived,
}

impl Effect {
    /// The short form beside the row.
    pub fn label(self, t: &Text) -> &'static str {
        match self {
            Self::Now => t.effect_now,
            Self::NextStart => t.effect_next_start,
            Self::Derived => t.effect_derived,
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
    pub fn label(self, t: &Text) -> &'static str {
        match self {
            Self::Environment => t.source_environment,
            Self::ConfigFile => t.source_config_file,
            Self::Default => t.source_default,
            Self::Derived => t.source_derived,
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
    pub fn source_label(&self, t: &Text) -> String {
        match (self.source, self.env) {
            (Source::Environment, Some(env)) => t.source_set_by_env(env),
            (Source::Derived, _) => self.source.label(t).to_string(),
            _ => t.source_from(self.source.label(t)),
        }
    }

    /// What the window says about a stored value the environment is winning.
    pub fn shadowed_label(&self, t: &Text) -> Option<String> {
        self.shadowed.as_ref().map(|value| t.source_shadowed(value))
    }
}

/// The settings as they were stored, before the environment is applied.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Stored {
    /// Where the data directory used to be chosen. Read once, during the move
    /// of the config file into the data directory, and never written back: the
    /// file now lives inside the directory it used to name, so a value here
    /// could not decide where to look for it.
    #[serde(skip_serializing)]
    data_dir: Option<String>,
    xray_executable: Option<String>,
    echo_url: Option<String>,
    /// The chosen appearance, as a name. Absent means the default, which is why
    /// a config file written before the switch existed still reads.
    theme: Option<String>,
    /// The chosen language, the same way.
    lang: Option<String>,
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

/// Where the config file used to live: the platform's config directory.
///
/// Kept as a migration source. A file here is read once, folded into the data
/// directory, and removed; nothing writes here any more.
fn legacy_config_path(host: &paths::Host) -> PathBuf {
    paths::config_dir(host)
        .unwrap_or_else(|| PathBuf::from(".").join(paths::CONFIG_DIR))
        .join(paths::CONFIG_FILE)
}

/// The data directory this run uses.
///
/// The environment, or the platform's own directory, and nothing else. The
/// config file lives *inside* this directory, so a value read from that file
/// could not decide where to find it - and the one field that used to move it is
/// now a migration source instead. Moving the directory is done by setting
/// `FP_BROWSER_DATA_DIR`, which is also what makes it a decision about the whole
/// process rather than about the contents of a file inside it.
fn resolve_data_dir(env: &Environment) -> PathBuf {
    env.data_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::data_dir_or_fallback(&env.host))
}

/// Where the config file lives: the environment's path, or the data directory.
///
/// The environment override stays, because a test and a script both need to aim
/// the program at a file without inventing a data directory for it.
fn resolve_config_path(env: &Environment, data_dir: &Path) -> PathBuf {
    env.config
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join(paths::CONFIG_FILE))
}

/// What happened to a config file that was not already in the data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Migration {
    /// An old file named a data directory this run does not use.
    IgnoredDataDir(String),
    /// The settings were read, and could not be written to the new location.
    NotWritten(String),
}

/// Reads the stored settings, moving a config file out of the old location.
///
/// Returns the settings, an error to report, and what the move did - if it did
/// anything at all. The move has two endings that need a sentence: an old file
/// that named a data directory of its own, which cannot be honoured without
/// leaving the file that names it somewhere the next start does not look, and a
/// write that failed.
fn read_stored(
    config_path: &Path,
    env: &Environment,
    data_dir: &Path,
    t: &Text,
) -> (Stored, Option<String>, Option<Migration>) {
    if config_path.exists() {
        return match read_config(config_path, t) {
            Ok(stored) => (stored, None, None),
            Err(error) => (Stored::default(), Some(error), None),
        };
    }
    // An explicit path is not ours to migrate into: the caller said where it is.
    if env.config.is_some() {
        return (Stored::default(), None, None);
    }

    let legacy = legacy_config_path(&env.host);
    if !legacy.exists() {
        return (Stored::default(), None, None);
    }
    let stored = match read_config(&legacy, t) {
        Ok(stored) => stored,
        // Left where it is: a file that cannot be read is not one to move.
        Err(error) => return (Stored::default(), Some(error), None),
    };
    let ignored = stored
        .data_dir
        .clone()
        .filter(|old| Path::new(old) != data_dir);
    if let Some(old) = ignored {
        // Not moved either, and deliberately: the file is the only record of
        // where that directory is. Setting the environment variable to it makes
        // the next start resolve that directory and migrate the file there.
        return (stored, None, Some(Migration::IgnoredDataDir(old)));
    }

    let mut moved = stored.clone();
    // Read, and not carried over: the next start resolves the same directory
    // from the environment or the platform, so a stored copy would be a second
    // answer to a question that now has one.
    moved.data_dir = None;
    if let Err(error) = write_config(config_path, &moved, t) {
        // The values read are still the ones in force and the old file stays
        // where it is, so nothing is lost - but the next start will not find
        // them at the new location, and the banner has to say so rather than
        // leaving a settings page that looks like it worked.
        return (moved, None, Some(Migration::NotWritten(error)));
    }
    let _ = std::fs::remove_file(&legacy);
    (moved, None, None)
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
    theme: ThemeChoice,
    lang: Lang,
}

impl Settings {
    /// Reads the environment and the config file, without failing.
    ///
    /// A broken config file is reported and the defaults are used, so the app
    /// still starts; saving is refused until it is fixed.
    pub fn load(env: Environment) -> (Self, Option<Notice>) {
        let data_dir = resolve_data_dir(&env);
        let config_path = resolve_config_path(&env, &data_dir);
        // English here: the language is one of the things this read is trying to
        // find, so a file that fails to parse can only complain in the default.
        let (stored, config_error, migration) =
            read_stored(&config_path, &env, &data_dir, text(Lang::En));
        let xray_executable = env
            .xray_executable
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| stored.xray_executable.as_ref().map(PathBuf::from))
            .unwrap_or_else(default_xray);

        let echo_url = resolve_echo_url(&stored, &env);
        // No environment override: the appearance is a standing choice about
        // what the user is looking at, not about what this process does.
        let theme = ThemeChoice::from_code(stored.theme.as_deref().unwrap_or(""));
        // Resolved before the notice below, which is written in it: a config
        // file that failed to parse still says which language to complain in.
        let lang = Lang::from_code(stored.lang.as_deref().unwrap_or(""));

        // An error is worth more than the migration note, and only one of them
        // can be set: the move either read the old file or it did not.
        let notice = config_error
            .as_ref()
            .map(|error| (text(lang).settings_unreadable(error), true))
            .or_else(|| {
                migration.as_ref().map(|migration| match migration {
                    Migration::IgnoredDataDir(old) => (
                        text(lang).data_dir_no_longer_in_config(
                            old,
                            DATA_DIR_ENV,
                            &data_dir.display().to_string(),
                        ),
                        true,
                    ),
                    Migration::NotWritten(error) => (
                        text(lang).settings_not_moved(
                            &legacy_config_path(&env.host).display().to_string(),
                            &config_path.display().to_string(),
                            error,
                        ),
                        true,
                    ),
                })
            });

        let settings = Self {
            config_path,
            stored,
            env,
            config_error,
            data_dir,
            xray_executable,
            echo_url,
            theme,
            lang,
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

    /// The appearance the window is shown in.
    pub fn theme(&self) -> ThemeChoice {
        self.theme
    }

    /// Stores the chosen appearance.
    ///
    /// Refused while the config file is unreadable, like the path settings and
    /// for the same reason: saving rewrites the whole file, so a broken one
    /// would lose the values it still holds in exchange for a colour.
    pub fn set_theme(&mut self, choice: ThemeChoice) -> Result<(), String> {
        if let Some(error) = &self.config_error {
            return Err(self
                .text()
                .settings_cannot_write(&self.config_path.display().to_string(), error));
        }
        let mut stored = self.stored.clone();
        stored.theme = Some(choice.code().to_string());
        write_config(&self.config_path, &stored, self.text())?;
        self.stored = stored;
        self.theme = choice;
        Ok(())
    }

    /// The language the window is shown in.
    pub fn language(&self) -> Lang {
        self.lang
    }

    /// The table for the language in force.
    pub fn text(&self) -> &'static Text {
        text(self.lang)
    }

    /// Stores the chosen language.
    ///
    /// Refused while the config file is unreadable, exactly like the appearance
    /// and for the same reason: saving rewrites the whole file.
    pub fn set_language(&mut self, lang: Lang) -> Result<(), String> {
        if let Some(error) = &self.config_error {
            return Err(self
                .text()
                .settings_cannot_write(&self.config_path.display().to_string(), error));
        }
        let mut stored = self.stored.clone();
        stored.lang = Some(lang.code().to_string());
        write_config(&self.config_path, &stored, self.text())?;
        self.stored = stored;
        self.lang = lang;
        Ok(())
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.data_dir.join("runtime")
    }

    /// Every line of the settings page, in the order it is shown.
    pub fn rows(&self, t: &Text) -> Vec<SettingRow> {
        let path = |value: &Path| value.to_string_lossy().to_string();

        vec![
            SettingRow {
                key: SettingKey::DataDir,
                value: self.rendered(&self.data_dir, t),
                source: self.source_of(self.env.data_dir.is_some(), false),
                env: self.env.data_dir.as_ref().map(|_| DATA_DIR_ENV),
                // An old config file may still name a directory this build does
                // not read; the row says so with the sentence used for any value
                // the environment is overriding, because that is what happened.
                shadowed: self.stored.data_dir.clone(),
                note: Some(t.help_data_dir.to_string()),
            },
            SettingRow {
                key: SettingKey::XrayExecutable,
                value: path(&self.xray_executable),
                source: self.source_of(
                    self.env.xray_executable.is_some(),
                    self.stored.xray_executable.is_some(),
                ),
                env: self.env.xray_executable.as_ref().map(|_| XRAY_BIN_ENV),
                shadowed: shadowed(
                    &self.stored.xray_executable,
                    self.env.xray_executable.is_some(),
                ),
                note: None,
            },
            SettingRow {
                key: SettingKey::EchoUrl,
                value: self.echo_url.clone(),
                source: self.source_of(self.env.echo_url.is_some(), self.stored.echo_url.is_some()),
                env: self.env.echo_url.as_ref().map(|_| ECHO_URL_ENV),
                shadowed: shadowed(&self.stored.echo_url, self.env.echo_url.is_some()),
                note: Some(t.help_echo_url.to_string()),
            },
            SettingRow {
                key: SettingKey::ChromiumBin,
                value: self
                    .env
                    .chromium_bin
                    .clone()
                    .unwrap_or_else(|| t.value_not_set_path.to_string()),
                source: if self.env.chromium_bin.is_some() {
                    Source::Environment
                } else {
                    Source::Default
                },
                env: self.env.chromium_bin.as_ref().map(|_| CHROMIUM_BIN_ENV),
                shadowed: None,
                note: Some(t.help_chromium_bin.to_string()),
            },
            SettingRow {
                key: SettingKey::ChromiumMajor,
                value: self
                    .env
                    .chromium_major
                    .clone()
                    .unwrap_or_else(|| t.value_not_set_versions.to_string()),
                source: if self.env.chromium_major.is_some() {
                    Source::Environment
                } else {
                    Source::Default
                },
                env: self.env.chromium_major.as_ref().map(|_| CHROMIUM_MAJOR_ENV),
                shadowed: None,
                note: Some(t.help_chromium_major.to_string()),
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
                note: Some(t.help_config_file.to_string()),
            },
            SettingRow {
                key: SettingKey::RuntimeDir,
                value: path(&self.runtime_dir()),
                source: Source::Derived,
                env: None,
                shadowed: None,
                note: Some(t.help_runtime_dir.to_string()),
            },
        ]
    }

    /// A relative path is shown with the directory it resolves against, so the
    /// window never shows a path the user cannot find.
    fn rendered(&self, path: &Path, t: &Text) -> String {
        if path.is_absolute() || path.has_root() {
            return path.to_string_lossy().to_string();
        }
        match std::env::current_dir() {
            Ok(cwd) => {
                t.setting_relative_to(&path.display().to_string(), &cwd.display().to_string())
            }
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
        let t = self.text();
        if value.is_empty() {
            return Err(t.setting_cannot_be_empty(&key.label(t).to_lowercase()));
        }
        if let Some(error) = &self.config_error {
            return Err(t.settings_cannot_write(&self.config_path.display().to_string(), error));
        }

        let mut stored = self.stored.clone();
        match key {
            SettingKey::XrayExecutable => stored.xray_executable = Some(value.to_string()),
            SettingKey::EchoUrl => stored.echo_url = Some(value.to_string()),
            _ => return Err(t.setting_not_editable(key.label(t))),
        }

        write_config(&self.config_path, &stored, self.text())?;
        self.stored = stored;
        // Re-resolve, so the page shows the new value without a restart even
        // though the process keeps using the old one until it starts again.
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
    /// whose `effect` is `Effect::Now` is never pending either - it is already in use, so
    /// there is no restart for it to wait on.
    #[cfg(test)]
    pub fn pending(&self, key: SettingKey) -> Option<String> {
        match key {
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

fn read_config(path: &Path, t: &Text) -> Result<Stored, String> {
    let display = path.display().to_string();
    let failed = |error: &dyn std::fmt::Display| t.config_read_failed(&display, &error.to_string());
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|error| failed(&error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Stored::default()),
        Err(error) => Err(failed(&error)),
    }
}

fn write_config(path: &Path, stored: &Stored, t: &Text) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            t.config_create_failed(&parent.display().to_string(), &error.to_string())
        })?;
    }
    let text = serde_json::to_string_pretty(stored)
        .map_err(|error| t.config_encode_failed(&error.to_string()))?;
    std::fs::write(path, format!("{text}\n")).map_err(|error| {
        t.config_file_write_failed(&path.display().to_string(), &error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use crate::text::en;

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
        let rows = settings.rows(en());
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
        assert_eq!(rows[0].source_label(en()), "from the default");
    }

    #[test]
    fn a_stored_value_is_used_and_says_where_it_came_from() {
        let config = TempConfig::new("stored");
        config.write(r#"{"xray_executable": "/opt/xray"}"#);
        let (settings, notice) = Settings::load(env(&config));

        assert!(notice.is_none());
        assert_eq!(settings.xray_executable(), Path::new("/opt/xray"));
        let row = settings.rows(en())[1].clone();
        assert_eq!(row.key, SettingKey::XrayExecutable);
        assert_eq!(row.source, Source::ConfigFile);
        assert_eq!(row.source_label(en()), "from the config file");
        assert_eq!(row.value, "/opt/xray");
    }

    /// The config file lives in the data directory, so everything a run needs is
    /// under one path - and a file left in the old location is folded in once,
    /// then removed, rather than being read for ever from two places.
    #[test]
    fn a_config_file_in_the_old_location_is_moved_into_the_data_directory() {
        let home =
            std::env::temp_dir().join(format!("fp-settings-home-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let host = paths::Host {
            home: Some(home.clone()),
            ..paths::Host::default()
        };
        // Taken from the platform rule rather than spelled out, so a test cannot
        // pass while the directory the program uses has moved somewhere else.
        let data = paths::data_dir(&host).expect("a data directory");
        let legacy = home.join(".config/fp-browser/config.json");
        std::fs::create_dir_all(legacy.parent().expect("parent")).expect("legacy dir");
        std::fs::write(
            &legacy,
            r#"{"xray_executable": "/opt/xray", "theme": "light", "lang": "zh"}"#,
        )
        .expect("legacy config");

        let environment = Environment {
            host: host.clone(),
            ..Environment::default()
        };
        let (settings, notice) = Settings::load(environment);

        assert!(notice.is_none(), "a plain move needs no explanation");
        assert_eq!(settings.data_dir(), data, "the platform's own directory");
        assert_eq!(settings.xray_executable(), Path::new("/opt/xray"));
        assert_eq!(settings.theme(), ThemeChoice::Light);
        assert_eq!(settings.language(), Lang::Zh);
        let moved = data.join(paths::CONFIG_FILE);
        assert!(moved.exists(), "the file is where the next start looks");
        assert!(!legacy.exists(), "and it is not in two places");
        let written = std::fs::read_to_string(&moved).expect("read moved file");
        assert!(!written.contains("data_dir"), "{written}");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A move that cannot be written says so, and keeps reading the old file:
    /// the values are in force either way, and the alternative is a settings
    /// page that looks like it saved.
    #[test]
    fn a_move_that_cannot_be_written_keeps_reading_the_old_file() {
        let home =
            std::env::temp_dir().join(format!("fp-settings-home-blocked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("temp dir");
        let legacy = home.join(".config/fp-browser/config.json");
        std::fs::create_dir_all(legacy.parent().expect("parent")).expect("legacy dir");
        std::fs::write(&legacy, r#"{"theme": "light"}"#).expect("legacy config");
        // A file where the data directory would have to go: creating it fails,
        // which is what a full disk or a read-only home looks like from here.
        let blocked = home.join("blocked");
        std::fs::write(&blocked, b"not a directory").expect("a file in the way");

        let environment = Environment {
            data_dir: Some(blocked.join("data").to_string_lossy().to_string()),
            host: paths::Host {
                home: Some(home.clone()),
                ..paths::Host::default()
            },
            ..Environment::default()
        };
        let (settings, notice) = Settings::load(environment);

        let (message, is_error) = notice.expect("a notice");
        assert!(is_error, "{message}");
        assert!(message.contains("could not be moved"), "{message}");
        assert!(
            legacy.exists(),
            "the settings are still only here, so the file has to stay"
        );
        assert_eq!(
            settings.theme(),
            ThemeChoice::Light,
            "and they are in force"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The one case the move cannot decide: an old file that named a data
    /// directory of its own. It is left alone - it is the only record of where
    /// that directory is - and the banner says how to get back to it.
    #[test]
    fn a_config_file_naming_another_data_directory_is_left_where_it_is() {
        let home =
            std::env::temp_dir().join(format!("fp-settings-home-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let host = paths::Host {
            home: Some(home.clone()),
            ..paths::Host::default()
        };
        let data = paths::data_dir(&host).expect("a data directory");
        let elsewhere = home.join("elsewhere");
        let legacy = home.join(".config/fp-browser/config.json");
        std::fs::create_dir_all(legacy.parent().expect("parent")).expect("legacy dir");
        std::fs::write(
            &legacy,
            format!(
                r#"{{"data_dir": "{}", "theme": "light"}}"#,
                elsewhere.display()
            ),
        )
        .expect("legacy config");

        let environment = Environment {
            host,
            ..Environment::default()
        };
        let (settings, notice) = Settings::load(environment);

        let (message, is_error) = notice.expect("a notice");
        assert!(is_error, "the profile list is about to look empty");
        assert!(
            message.contains(&elsewhere.display().to_string()),
            "{message}"
        );
        assert!(message.contains(DATA_DIR_ENV), "{message}");
        assert!(
            legacy.exists(),
            "the file that knows where that directory is was not moved"
        );
        assert_eq!(
            settings.data_dir(),
            data,
            "this run uses the directory it resolved, not the one in the file"
        );
        assert_eq!(
            settings.rows(en())[0].shadowed_label(en()).as_deref(),
            Some(
                format!(
                    "the config file holds {}, which this overrides",
                    elsewhere.display()
                )
                .as_str()
            ),
            "the row says what was ignored"
        );
        assert_eq!(
            settings.theme(),
            ThemeChoice::Light,
            "the rest still applies"
        );
        let _ = std::fs::remove_dir_all(&home);
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
        let row = &settings.rows(en())[0];
        assert_eq!(row.source, Source::Environment);
        assert_eq!(row.source_label(en()), "set by FP_BROWSER_DATA_DIR");
        assert_eq!(row.value, "/tmp/other");
        assert_eq!(
            row.shadowed_label(en()).as_deref(),
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
            .rows(en())
            .into_iter()
            .find(|row| row.key == SettingKey::EchoUrl)
            .expect("a row for the endpoint");
        assert_eq!(row.source, Source::ConfigFile);
        assert_eq!(
            row.key.effect().label(en()),
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
            .rows(en())
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
            .rows(en())
            .into_iter()
            .find(|row| row.key == SettingKey::EchoUrl)
            .expect("a row for the endpoint");
        assert_eq!(row.source, Source::Environment);
        assert_eq!(row.source_label(en()), "set by FP_BROWSER_ECHO_URL");
        assert_eq!(
            row.value, "http://env.example/ip",
            "the page shows the endpoint a test would really ask"
        );
        assert_eq!(
            row.shadowed_label(en()).as_deref(),
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
            .set(SettingKey::XrayExecutable, "/opt/xray")
            .expect("save xray");
        settings
            .set(SettingKey::EchoUrl, "http://echo.example/ip")
            .expect("save the endpoint");

        let text = config.read();
        assert!(text.contains("/opt/xray"), "{text}");
        assert!(
            text.contains("http://echo.example/ip"),
            "saving one setting rewrites the file, so every other value has to \
             survive the rewrite: {text}"
        );

        // And a fresh load agrees.
        let (reloaded, _) = Settings::load(env(&config));
        assert_eq!(reloaded.xray_executable(), Path::new("/opt/xray"));
        assert_eq!(reloaded.echo_url(), "http://echo.example/ip");
    }

    /// The appearance survives a restart, and a config file written before the
    /// switch existed still starts - on the palette the program has always
    /// painted, not on the component library's default.
    #[test]
    fn the_appearance_is_stored_and_an_absent_one_means_dark() {
        let config = TempConfig::new("theme");
        config.write(r#"{"xray_executable": "/srv/xray"}"#);
        let (mut settings, _) = Settings::load(env(&config));
        assert_eq!(settings.theme(), ThemeChoice::Dark, "absent means dark");

        settings.set_theme(ThemeChoice::Light).expect("save");
        assert_eq!(settings.theme(), ThemeChoice::Light);
        assert_eq!(
            settings.xray_executable(),
            Path::new("/srv/xray"),
            "kept across the rewrite"
        );

        let (reloaded, _) = Settings::load(env(&config));
        assert_eq!(reloaded.theme(), ThemeChoice::Light);
    }

    /// The language survives a restart, a region is not a different language,
    /// and a name this build does not know starts in English rather than
    /// refusing to start at all.
    #[test]
    fn the_language_is_stored_and_an_unknown_one_means_english() {
        let config = TempConfig::new("language");
        config.write(r#"{"xray_executable": "/srv/xray"}"#);
        let (mut settings, _) = Settings::load(env(&config));
        assert_eq!(settings.language(), Lang::En, "absent means English");

        settings.set_language(Lang::Zh).expect("save");
        assert_eq!(settings.language(), Lang::Zh);
        assert_eq!(
            settings.xray_executable(),
            Path::new("/srv/xray"),
            "kept across the rewrite"
        );

        let (reloaded, _) = Settings::load(env(&config));
        assert_eq!(reloaded.language(), Lang::Zh, "the choice outlives the run");

        config.write(r#"{"lang": "zh-Hans"}"#);
        let (regional, _) = Settings::load(env(&config));
        assert_eq!(
            regional.language(),
            Lang::Zh,
            "a region is the same language"
        );

        config.write(r#"{"lang": "kl"}"#);
        let (unknown, _) = Settings::load(env(&config));
        assert_eq!(unknown.language(), Lang::En, "not knowing is not a refusal");
        assert!(
            !unknown.rows(en()).is_empty(),
            "and the window still renders"
        );
    }

    /// A config file this build cannot write refuses the change rather than
    /// accepting one that would be gone by the next start.
    #[test]
    fn the_appearance_is_refused_while_the_config_file_is_unreadable() {
        let config = TempConfig::new("theme-broken");
        config.write("{ not json");
        let (mut settings, _) = Settings::load(env(&config));

        let error = settings
            .set_theme(ThemeChoice::Light)
            .expect_err("a broken file cannot be rewritten");

        assert!(error.contains("cannot be written"), "{error}");
        assert_eq!(settings.theme(), ThemeChoice::Dark, "nothing changed");
    }

    #[test]
    fn a_saved_value_is_the_pending_one_even_while_the_process_uses_the_old() {
        let config = TempConfig::new("pending");
        let (mut settings, _) = Settings::load(env(&config));
        assert_eq!(settings.pending(SettingKey::XrayExecutable), None);

        settings
            .set(SettingKey::XrayExecutable, "/opt/xray")
            .expect("save");

        assert!(
            settings
                .rows(en())
                .iter()
                .any(|row| row.key == SettingKey::XrayExecutable && row.value == "/opt/xray"),
            "the page shows what the next start will use"
        );
        assert_eq!(
            settings.pending(SettingKey::XrayExecutable).as_deref(),
            Some("/opt/xray")
        );
    }

    #[test]
    fn an_environment_override_has_no_pending_value() {
        let config = TempConfig::new("pending-env");
        let mut environment = env(&config);
        environment.xray_executable = Some("/tmp/other".to_string());
        let (mut settings, _) = Settings::load(environment);

        // Stored, but overridden: it is not what the next start will use.
        settings
            .set(SettingKey::XrayExecutable, "/opt/xray")
            .expect("save");
        assert_eq!(settings.pending(SettingKey::XrayExecutable), None);
        assert_eq!(settings.xray_executable(), Path::new("/tmp/other"));
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
            .rows(en())
            .into_iter()
            .find(|row| row.key == SettingKey::RuntimeDir)
            .expect("a runtime row");
        assert_eq!(
            row.key.effect().label(en()),
            "derived from the data directory"
        );
        assert_eq!(row.source, Source::Derived);
        assert_eq!(
            row.source_label(en()),
            "derived from the data directory",
            "it is computed, not chosen anywhere"
        );
    }

    #[test]
    fn a_relative_data_directory_is_shown_against_the_working_directory() {
        let config = TempConfig::new("relative");
        let (settings, _) = Settings::load(env(&config));
        let value = &settings.rows(en())[0].value;
        assert!(value.starts_with(FALLBACK_DATA_DIR), "{value}");
        assert!(
            value.contains("relative to"),
            "a relative path is shown with what it is relative to: {value}"
        );
    }

    #[test]
    fn the_settings_page_names_every_source() {
        let config = TempConfig::new("sources");
        config.write(r#"{"xray_executable": "/opt/xray"}"#);
        let mut environment = env(&config);
        environment.chromium_bin = Some("/opt/chrome".to_string());
        let (settings, _) = Settings::load(environment);

        let rows = settings.rows(en());
        let by_key = |key: SettingKey| {
            rows.iter()
                .find(|row| row.key == key)
                .expect("the row exists")
                .clone()
        };
        assert_eq!(
            by_key(SettingKey::DataDir).source,
            Source::Default,
            "nothing but the environment can move the data directory now"
        );
        assert_eq!(
            by_key(SettingKey::XrayExecutable).source,
            Source::ConfigFile
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
