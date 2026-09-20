//! Opening a profile's browser data directory in the desktop's file manager.
//!
//! There is no portable "reveal this directory" API, so this runs the desktop's
//! own opener (`xdg-open`, `open`, `explorer`). The command is built by a pure
//! function and the spawning sits behind a trait, for the same reason the
//! verifier does: a test must be able to click the button without a file manager
//! appearing on the machine running it.

use std::path::Path;
use std::process::Command;

/// What this build should run to open a directory.
pub const CURRENT_OS: &str = std::env::consts::OS;

/// The opener for a platform, as `std::env::consts::OS` spells it.
///
/// Kept pure so the mapping is tested on every platform, not only the one the
/// test happens to run on.
pub fn opener(path: &Path, os: &str) -> (String, Vec<String>) {
    let directory = path.to_string_lossy().to_string();
    match os {
        "macos" => ("open".to_string(), vec![directory]),
        "windows" => ("explorer".to_string(), vec![directory]),
        // xdg-open is the desktop's own choice, on every Linux desktop that
        // has one; it is what the rest of this crate's tooling assumes.
        _ => ("xdg-open".to_string(), vec![directory]),
    }
}

/// Opens a directory in the desktop's file manager.
pub trait DirectoryOpener: Send + Sync {
    /// Returns an error message to show when nothing could be opened. The
    /// directory is not created: a profile that has never run has no browser
    /// data yet, and saying so is more useful than an empty folder.
    fn open(&self, path: &Path) -> Result<(), String>;
}

/// The real opener: spawn the platform command and reap it on a worker, so a
/// slow file manager cannot leave a zombie behind or block the window.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemDirectoryOpener;

impl DirectoryOpener for SystemDirectoryOpener {
    fn open(&self, path: &Path) -> Result<(), String> {
        // The opener is a foreign process and may not share this one's working
        // directory, so a relative data directory is resolved here, the same
        // way the Settings page shows it.
        let path = match path.is_absolute() {
            true => path.to_path_buf(),
            false => std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf()),
        };
        if !path.is_dir() {
            return Err(format!(
                "{} is not a directory yet; start the profile once and the browser will create it",
                path.display()
            ));
        }
        let (program, args) = opener(&path, CURRENT_OS);
        let child = Command::new(&program)
            .args(&args)
            .spawn()
            .map_err(|error| format!("could not run {program}: {error}"))?;
        // Reaped on a worker so a slow file manager cannot leave a zombie or
        // block the window. The window has already reported the open as done:
        // an opener that ran but found no handler is a desktop problem this
        // process cannot see from a spawn, so it is at least written down.
        std::thread::spawn(move || {
            let mut child = child;
            match child.wait() {
                Ok(status) if !status.success() => tracing::warn!(
                    "{program} exited with {status}; nothing may have opened for {}",
                    path.display()
                ),
                Ok(_) => {}
                Err(error) => tracing::warn!("could not wait for {program}: {error}"),
            }
        });
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;

    /// An opener a test drives: it answers with what it was given and records
    /// the paths it was asked to open.
    #[derive(Default)]
    pub struct FakeOpener {
        outcome: Option<Result<(), String>>,
        opened: Mutex<Vec<std::path::PathBuf>>,
    }

    impl FakeOpener {
        pub fn working() -> Self {
            Self {
                outcome: Some(Ok(())),
                opened: Mutex::new(Vec::new()),
            }
        }

        /// An opener that fails the way a machine without a file manager does.
        pub fn failing(reason: &str) -> Self {
            Self {
                outcome: Some(Err(reason.to_string())),
                opened: Mutex::new(Vec::new()),
            }
        }

        pub fn opened(&self) -> Vec<std::path::PathBuf> {
            self.opened.lock().expect("opened lock").clone()
        }
    }

    impl DirectoryOpener for FakeOpener {
        fn open(&self, path: &Path) -> Result<(), String> {
            self.opened
                .lock()
                .expect("opened lock")
                .push(path.to_path_buf());
            self.outcome.clone().unwrap_or(Ok(()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_opener_matches_the_platform() {
        let path = PathBuf::from("/home/me/data/profiles/1");
        assert_eq!(opener(&path, "linux").0, "xdg-open");
        assert_eq!(opener(&path, "macos").0, "open");
        assert_eq!(opener(&path, "windows").0, "explorer");
        for os in ["linux", "macos", "windows"] {
            let (_, args) = opener(&path, os);
            assert_eq!(
                args,
                vec![path.to_string_lossy().to_string()],
                "the directory is passed as one argument on {os}"
            );
        }
    }

    /// The real opener is the one piece a test must not run: a file manager
    /// window would open on the machine running the suite.
    #[test]
    fn a_directory_that_does_not_exist_is_refused_with_its_path() {
        let missing = std::env::temp_dir().join(format!("fp-open-{}", std::process::id()));
        let error = SystemDirectoryOpener
            .open(&missing)
            .expect_err("nothing to open");
        assert!(error.contains(&missing.display().to_string()), "{error}");
        assert!(error.contains("not a directory"), "{error}");
    }

    /// The opener is a foreign process, so a relative data directory must not
    /// be handed to it as-is.
    #[test]
    fn a_relative_directory_is_named_against_the_working_directory() {
        let relative = PathBuf::from(format!("definitely-not-a-fp-dir-{}", std::process::id()));
        let error = SystemDirectoryOpener
            .open(&relative)
            .expect_err("nothing to open");
        let cwd = std::env::current_dir().expect("a working directory");
        assert!(
            error.contains(&cwd.join(&relative).display().to_string()),
            "{error}"
        );
    }
}
