//! Where this program keeps its files.
//!
//! One rule per platform, and every one of them is a pure function of the
//! environment: the environment is read once into a [`Host`], and the mapping
//! from a `Host` to a directory is what the tests check - so the Windows answer
//! is tested on whatever machine the tests run on, the way `open_dir::opener` is.
//!
//! The data directory used to be the relative `data`, next to wherever the
//! program happened to be started. That is now only the fallback for a host with
//! no home directory at all; a host that has one gets the place its platform
//! keeps application data in.

use crate::text::Text;
use std::path::{Path, PathBuf};

/// The directory this program's data lives in, on every platform.
pub const APP_DIR: &str = "FpBrowser";

/// The directory the config file lives in.
///
/// This is where the config file *used* to live, outside the data directory, so
/// that it could remember which data directory to use. It lives inside the data
/// directory now (see [`crate::settings`]) and this is read once, to carry an
/// older installation's settings across; nothing writes here any more. The name
/// is the one this program has always used rather than [`APP_DIR`], because a
/// file that is being migrated still has to be found under the name it was
/// written with.
pub const CONFIG_DIR: &str = "fp-browser";

/// The data directory used when the host has no home directory to put it under.
pub const FALLBACK_DATA_DIR: &str = "data";

/// The parts of the environment that decide where files go.
///
/// A value rather than a set of reads, so a test can say "this is Windows" or
/// "this host has no home directory" without one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Host {
    /// As `std::env::consts::OS` spells it: `linux`, `windows`, `macos`, ...
    pub os: &'static str,
    pub home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
}

impl Host {
    /// The environment this process is running in.
    pub fn from_process() -> Self {
        let read = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| !path.as_os_str().is_empty())
        };
        Self {
            os: std::env::consts::OS,
            home: read("HOME"),
            xdg_data_home: read("XDG_DATA_HOME"),
            xdg_config_home: read("XDG_CONFIG_HOME"),
            local_app_data: read("LOCALAPPDATA"),
            app_data: read("APPDATA"),
        }
    }
}

/// Where this host keeps application data, or `None` when it cannot be told.
///
/// Linux and macOS: `$XDG_DATA_HOME` (the spec says a relative one is ignored)
/// or `~/.local/share`, then `~/Library/Application Support`. Windows:
/// `%LOCALAPPDATA%`, which is `%APPDATA%\Local` on a default profile - the
/// roaming variable is only the fallback, because browser profiles and a
/// database should not follow a roaming profile between machines.
pub fn data_dir(host: &Host) -> Option<PathBuf> {
    let base = match host.os {
        "windows" => host.local_app_data.clone().or_else(|| {
            host.app_data
                .as_ref()
                .map(|app_data| app_data.join("Local"))
        }),
        "macos" => host
            .home
            .as_ref()
            .map(|home| home.join("Library/Application Support")),
        _ => absolute(&host.xdg_data_home)
            .or_else(|| host.home.as_ref().map(|home| home.join(".local/share"))),
    }?;
    Some(base.join(APP_DIR))
}

/// The config file's name, inside whichever directory holds it.
pub const CONFIG_FILE: &str = "config.json";

/// Where this host keeps the config file, or `None` when it cannot be told.
///
/// The config file lives in the data directory now; this is where it used to
/// live, and is read once so an installation that predates the move keeps its
/// settings.
pub fn config_dir(host: &Host) -> Option<PathBuf> {
    let base = match host.os {
        "windows" => host.app_data.clone(),
        "macos" => host
            .home
            .as_ref()
            .map(|home| home.join("Library/Application Support")),
        _ => absolute(&host.xdg_config_home)
            .or_else(|| host.home.as_ref().map(|home| home.join(".config"))),
    }?;
    Some(base.join(CONFIG_DIR))
}

/// [`data_dir`], or the relative fallback for a host that has no home directory.
pub fn data_dir_or_fallback(host: &Host) -> PathBuf {
    data_dir(host).unwrap_or_else(|| PathBuf::from(FALLBACK_DATA_DIR))
}

/// The directory a configuration export writes into when the user has not said
/// where.
pub const EXPORT_DIR: &str = "exports";

/// Where a configuration export writes with no path typed:
/// `<data dir>/exports/fp-browser-config-<stamp>.json`.
///
/// The stamp is what makes the default destination safe to write over. An export
/// replaces whatever is at its path, so a default that named a fixed file would
/// destroy the previous backup every time it was used; naming a new one each
/// second means reaching an existing path is something the user did on purpose.
pub fn default_export_file(data_dir: &Path, at: std::time::SystemTime) -> PathBuf {
    data_dir.join(EXPORT_DIR).join(format!(
        "fp-browser-config-{}.json",
        crate::log_file::file_stamp(at)
    ))
}

/// A notice for a run that follows the move of the default data directory.
///
/// The default used to be the relative `data`, next to wherever the program was
/// started from. Someone who has one there would otherwise open the window to an
/// empty list and no explanation; the old directory is one field away on the
/// Settings page, so all this does is name it. Said once: the next start creates
/// a database in the new place, and then the first check below holds.
pub fn moved_data_dir_notice(data_dir: &Path, legacy: &Path, t: &Text) -> Option<(String, bool)> {
    if data_dir.join("app.db").exists() || !legacy.join("app.db").exists() {
        return None;
    }
    Some((
        t.data_dir_moved_notice(
            &data_dir.display().to_string(),
            &legacy.display().to_string(),
        ),
        // Not an error: nothing is broken, and nothing was moved.
        false,
    ))
}

/// The XDG spec says a relative `$XDG_*_HOME` is to be ignored, not resolved.
fn absolute(value: &Option<PathBuf>) -> Option<PathBuf> {
    value
        .clone()
        .filter(|path| path.is_absolute() || path.has_root())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::en;

    fn windows_with_local_app_data() -> Host {
        Host {
            os: "windows",
            home: Some(PathBuf::from(r"C:\Users\me")),
            local_app_data: Some(PathBuf::from(r"C:\Users\me\AppData\Local")),
            app_data: Some(PathBuf::from(r"C:\Users\me\AppData\Roaming")),
            ..Host::default()
        }
    }

    fn linux_with_home() -> Host {
        Host {
            os: "linux",
            home: Some(PathBuf::from("/home/me")),
            ..Host::default()
        }
    }

    fn macos_with_home() -> Host {
        Host {
            os: "macos",
            home: Some(PathBuf::from("/Users/me")),
            ..Host::default()
        }
    }

    /// The decision this module makes, as two platform-independent pieces: the
    /// base directory it chose, and the name under it.
    ///
    /// An expected path *string* would only be right on the platform the test
    /// runs on, because `Path::join` spells the separator of the host it runs on:
    /// joining `C:\\Users\\me` on Linux produces `C:\\Users\\me/FpBrowser`.
    fn placement(directory: Option<PathBuf>) -> (Option<PathBuf>, Option<String>) {
        match directory {
            Some(path) => (
                path.parent().map(Path::to_path_buf),
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string()),
            ),
            None => (None, None),
        }
    }

    #[test]
    fn the_data_directory_is_the_platforms_own_place() {
        let cases = [
            (
                windows_with_local_app_data(),
                PathBuf::from(r"C:\Users\me\AppData\Local"),
            ),
            (linux_with_home(), PathBuf::from("/home/me/.local/share")),
            (
                macos_with_home(),
                PathBuf::from("/Users/me/Library/Application Support"),
            ),
        ];
        for (host, base) in cases {
            let (parent, name) = placement(data_dir(&host));
            assert_eq!(parent, Some(base), "the base for {}", host.os);
            assert_eq!(name.as_deref(), Some(APP_DIR), "the name for {}", host.os);
        }
    }

    #[test]
    fn xdg_data_home_wins_over_the_home_directory() {
        let host = Host {
            xdg_data_home: Some(PathBuf::from("/srv/data")),
            ..linux_with_home()
        };
        assert_eq!(
            placement(data_dir(&host)).0,
            Some(PathBuf::from("/srv/data"))
        );

        // The spec: a relative one is ignored rather than resolved.
        let relative = Host {
            xdg_data_home: Some(PathBuf::from("data")),
            ..linux_with_home()
        };
        assert_eq!(
            placement(data_dir(&relative)).0,
            Some(PathBuf::from("/home/me/.local/share"))
        );
    }

    #[test]
    fn windows_falls_back_to_appdata_local_when_local_appdata_is_unset() {
        let host = Host {
            local_app_data: None,
            ..windows_with_local_app_data()
        };
        // Built with the same join the code uses: on a Linux test host that
        // separator is a forward slash, and an expectation written out by hand
        // would only be right on Windows.
        assert_eq!(
            placement(data_dir(&host)).0,
            Some(PathBuf::from(r"C:\Users\me\AppData\Roaming").join("Local"))
        );
    }

    #[test]
    fn a_host_with_no_home_directory_keeps_the_relative_directory() {
        // Nothing to put it under: the old behaviour is the only answer left,
        // and it is not silent - the Settings page shows the value it resolved to.
        assert_eq!(data_dir(&Host::default()), None);
        assert_eq!(
            data_dir_or_fallback(&Host::default()),
            PathBuf::from(FALLBACK_DATA_DIR)
        );
        assert_eq!(
            data_dir_or_fallback(&Host {
                os: "windows",
                ..Host::default()
            }),
            PathBuf::from(FALLBACK_DATA_DIR)
        );
    }

    #[test]
    fn the_config_directory_is_platform_aware_and_unchanged_on_linux() {
        let cases = [
            (
                linux_with_home(),
                PathBuf::from("/home/me/.config"),
                "linux",
            ),
            (
                Host {
                    xdg_config_home: Some(PathBuf::from("/etc/xdg")),
                    ..linux_with_home()
                },
                PathBuf::from("/etc/xdg"),
                "linux with XDG_CONFIG_HOME",
            ),
            (
                windows_with_local_app_data(),
                // Roaming is what config means on Windows; the data directory
                // is the local one.
                PathBuf::from(r"C:\Users\me\AppData\Roaming"),
                "windows",
            ),
            (
                macos_with_home(),
                PathBuf::from("/Users/me/Library/Application Support"),
                "macos",
            ),
        ];
        for (host, base, label) in cases {
            let (parent, name) = placement(config_dir(&host));
            assert_eq!(parent, Some(base), "the base for {label}");
            assert_eq!(name.as_deref(), Some(CONFIG_DIR), "the name for {label}");
        }
    }

    #[test]
    fn this_machine_has_a_platform_place_to_put_its_data() {
        // The pure tests above say what each platform gets; this one exercises
        // the real lookup, so a variable read the wrong way round is caught on
        // the machine that runs the suite rather than by a user who opens the
        // window to an empty list.
        let host = Host::from_process();
        assert_eq!(host.os, std::env::consts::OS);
        if host.home.is_none() && host.local_app_data.is_none() {
            // A container with no home directory at all: the fallback is the
            // answer, and the test above covers it.
            return;
        }
        let directory = data_dir(&host).expect("a host with a home directory has a data directory");
        assert!(directory.is_absolute(), "{directory:?}");
        assert_eq!(directory.file_name().unwrap(), APP_DIR);
    }

    #[test]
    fn the_move_is_reported_once_and_only_when_there_is_something_to_report() {
        let dir = std::env::temp_dir().join(format!("fp-paths-{}", std::process::id()));
        let data = dir.join("share/FpBrowser");
        let legacy = dir.join("checkout/data");
        std::fs::create_dir_all(&legacy).expect("legacy");
        std::fs::write(legacy.join("app.db"), b"").expect("write");

        // An old database and a new directory with none: say where it is.
        let (message, error) = moved_data_dir_notice(&data, &legacy, en()).expect("a notice");
        assert!(!error, "nothing is broken");
        assert!(message.contains("data"), "{message}");
        assert!(message.contains("Settings"), "{message}");

        // Once the new place has a database, there is nothing to say.
        std::fs::create_dir_all(&data).expect("data");
        std::fs::write(data.join("app.db"), b"").expect("write");
        assert_eq!(moved_data_dir_notice(&data, &legacy, en()), None);

        // And a checkout with nothing in it is not worth a word either.
        std::fs::remove_file(data.join("app.db")).expect("remove");
        std::fs::remove_file(legacy.join("app.db")).expect("remove");
        assert_eq!(moved_data_dir_notice(&data, &legacy, en()), None);

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn the_default_export_lands_under_the_data_directory_in_its_own_folder() {
        let data = Path::new("/home/alice/.local/share/FpBrowser");
        let file = default_export_file(data, std::time::UNIX_EPOCH);

        assert_eq!(file.parent().unwrap(), data.join(EXPORT_DIR));
        assert_eq!(
            file.file_name().unwrap(),
            "fp-browser-config-19700101-000000.json"
        );
    }

    #[test]
    fn the_default_export_names_a_new_file_each_second() {
        // This is what makes writing over an existing path safe: the default can
        // only collide with itself within the same second, so an export that
        // replaces a file is one the user pointed at deliberately.
        let data = Path::new("/data");
        let first = default_export_file(data, std::time::UNIX_EPOCH);
        let second = default_export_file(
            data,
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1),
        );

        assert_ne!(first, second);
        assert_eq!(
            second.file_name().unwrap(),
            "fp-browser-config-19700101-000001.json"
        );
    }

    #[test]
    fn a_default_export_name_holds_no_character_a_windows_path_cannot() {
        let file = default_export_file(Path::new("/data"), std::time::UNIX_EPOCH);
        let name = file.file_name().unwrap().to_string_lossy();

        // The colon a clock time carries is the one that would have broken this,
        // and it is the reason `log_file::file_stamp` exists at all.
        for forbidden in [':', '*', '?', '"', '<', '>', '|', '/', '\\'] {
            assert!(!name.contains(forbidden), "{name} holds {forbidden:?}");
        }
    }
}
