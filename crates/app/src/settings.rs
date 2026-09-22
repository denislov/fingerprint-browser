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

use crate::exit::ExitMode;
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
    /// What closing the window does, the same way. Absent means "ask", which is
    /// also what a file written before the exit modes existed means.
    exit_mode: Option<String>,
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
mod resolution;
pub use resolution::language_hint;
use resolution::*;

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
    /// What closing the window does. Resolved once, like the appearance and the
    /// language: it is read when the window is closed, which is a decision made
    /// long after this read.
    exit_mode: ExitMode,
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
        // Like the appearance: a standing choice about this installation, not
        // about what this process does, so no environment override.
        let exit_mode = ExitMode::from_code(stored.exit_mode.as_deref().unwrap_or(""));

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
            exit_mode,
        };
        (settings, notice)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn xray_executable(&self) -> &Path {
        &self.xray_executable
    }

    /// The config file this run read, and the one it writes to.
    ///
    /// Exposed because a report has to name the file it is talking about, and
    /// because the path is a fact about the installation rather than about the
    /// page that happens to show it.
    pub fn config_path(&self) -> &Path {
        &self.config_path
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

    /// What closing the window does.
    pub fn exit_mode(&self) -> ExitMode {
        self.exit_mode
    }

    /// Stores what closing the window does.
    ///
    /// Written before it is used, like the appearance and the language: a config
    /// file that cannot be written refuses the choice rather than showing one that
    /// would be gone by the next start. This one is read when the window is
    /// closed rather than when it is drawn, so "before it is used" means before
    /// any window is closed with it.
    pub fn set_exit_mode(&mut self, mode: ExitMode) -> Result<(), String> {
        if let Some(error) = &self.config_error {
            return Err(self
                .text()
                .settings_cannot_write(&self.config_path.display().to_string(), error));
        }
        let mut stored = self.stored.clone();
        stored.exit_mode = Some(mode.code().to_string());
        write_config(&self.config_path, &stored, self.text())?;
        self.stored = stored;
        self.exit_mode = mode;
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

mod persistence;
use persistence::*;

#[cfg(test)]
mod tests;
