//! Environment precedence and legacy settings migration.
use super::*;

pub(super) fn legacy_config_path(host: &paths::Host) -> PathBuf {
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
pub(super) fn resolve_data_dir(env: &Environment) -> PathBuf {
    env.data_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::data_dir_or_fallback(&env.host))
}

/// Where the config file lives: the environment's path, or the data directory.
///
/// The environment override stays, because a test and a script both need to aim
/// the program at a file without inventing a data directory for it.
pub(super) fn resolve_config_path(env: &Environment, data_dir: &Path) -> PathBuf {
    env.config
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join(paths::CONFIG_FILE))
}

/// What happened to a config file that was not already in the data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Migration {
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
pub(super) fn read_stored(
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

/// The language this run would speak, read the cheap way.
///
/// `--help` is answered before the window exists and has to be answered in a
/// real language. Loading the settings to find that language would fold an
/// older installation's config file into the data directory as a side effect of
/// asking how the program is used, which is a strange thing for `--help` to do
/// to someone. So this reads the stored `lang` and nothing else: no migration,
/// no writes, no notices. Anything it cannot read - no file, a broken one, a
/// name this build does not know - is English, which is the answer
/// [`Settings::load`] would have given too.
pub fn language_hint(env: &Environment) -> Lang {
    let data_dir = resolve_data_dir(env);
    let config_path = resolve_config_path(env, &data_dir);
    let stored = if config_path.exists() {
        stored_language(&config_path)
    } else if env.config.is_none() {
        // The file that has not been moved yet still says which language to
        // move it in.
        stored_language(&legacy_config_path(&env.host))
    } else {
        None
    };
    Lang::from_code(stored.as_deref().unwrap_or(""))
}

/// Just the `lang` field, read leniently.
///
/// Deliberately not [`read_config`]: this is a hint for a message that is about
/// to be printed, and a file with one unknown field in it should not cost the
/// reader their own language.
pub(super) fn stored_language(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("lang")?.as_str().map(str::to_string)
}

/// The endpoint the proxy diagnostic asks what address it left from.
///
/// The default is plain HTTP on purpose. The first question a proxy test asks is
/// whether traffic left at all; over TLS a certificate that does not match the
/// endpoint would look exactly like a proxy that does not forward, and the two
/// need opposite fixes. An `https://` value is not downgraded: the diagnostic
/// refuses it with a configuration fault rather than quietly asking a different
/// question than the one the address appears to ask.
pub(super) fn resolve_echo_url(stored: &Stored, env: &Environment) -> String {
    env.echo_url
        .clone()
        .or_else(|| stored.echo_url.clone())
        .unwrap_or_else(|| runtime::DEFAULT_ECHO_URL.to_string())
}
