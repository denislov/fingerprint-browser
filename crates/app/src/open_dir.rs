//! Opening a profile's browser data directory in the desktop's file manager.
//!
//! There is no portable "reveal this directory" API, so this runs the desktop's
//! own opener (`xdg-open`, `open`, `explorer`). The command is built by a pure
//! function and the spawning sits behind a trait, for the same reason the
//! verifier does: a test must be able to click the button without a file manager
//! appearing on the machine running it.

use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// What this build should run to open a directory.
pub const CURRENT_OS: &str = std::env::consts::OS;

/// How long the opener is given to hand the request off.
///
/// An opener that exits 0 handed it to something; one that exits non-zero found
/// nothing to hand it to. One that is still running after this is a file manager
/// that stays in the foreground, which is an open too - so the wait has to end
/// somewhere, and this is where.
pub const OPENER_GRACE: Duration = Duration::from_secs(5);

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
    /// Opens the directory and reports whether the desktop accepted it.
    ///
    /// Blocks until the opener has handed the request off, or until it is clear
    /// that it will not; callers run it off the UI thread. Returns an error
    /// message to show when nothing was opened. The directory is not created: a
    /// profile that has never run has no browser data yet, and saying so is more
    /// useful than an empty folder.
    fn open(&self, path: &Path) -> Result<(), String>;
}

/// The real opener: runs the platform command and waits, so the window can say
/// whether anything happened instead of assuming the spawn was the open.
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
        let mut child = Command::new(&program)
            .args(&args)
            .spawn()
            .map_err(|error| format!("could not run {program}: {error}"))?;
        wait_for_opener(&program, &mut child, &path, OPENER_GRACE)
    }
}

/// Waits for an opener to finish handing the request off.
///
/// Three outcomes, and they are different things: it succeeded, it failed (a
/// desktop with no handler for a directory is what `xdg-open` exits non-zero
/// for), or it is still running, which is a file manager that stays in the
/// foreground and therefore did open something.
fn wait_for_opener(
    program: &str,
    child: &mut Child,
    path: &Path,
    grace: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!(
                    "{program} exited with {status}; nothing may have opened {}",
                    path.display()
                ));
            }
            Ok(None) if Instant::now() >= deadline => return Ok(()),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(format!("could not wait for {program}: {error}")),
        }
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

    /// A spawn is not an open. These three drive the wait without a file
    /// manager, which is the part a test must not run.
    #[cfg(unix)]
    mod opener_wait {
        use super::*;

        fn spawn(program: &str, args: &[&str]) -> Child {
            Command::new(program)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn the stand-in opener")
        }

        #[test]
        fn an_opener_that_exits_successfully_is_an_open() {
            let mut child = spawn("/bin/true", &[]);
            wait_for_opener("/bin/true", &mut child, Path::new("/tmp"), OPENER_GRACE)
                .expect("exit 0 handed the request off");
        }

        /// The case the spawn cannot see: a desktop with no handler exits
        /// non-zero, and the window has to say so rather than claim success.
        #[test]
        fn an_opener_that_fails_is_reported_with_what_it_ran() {
            let mut child = spawn("/bin/false", &[]);
            let error = wait_for_opener(
                "/bin/false",
                &mut child,
                Path::new("/tmp/profiles/1"),
                OPENER_GRACE,
            )
            .expect_err("exit 1 opened nothing");
            assert!(error.contains("/bin/false"), "{error}");
            assert!(error.contains("/tmp/profiles/1"), "{error}");
            assert!(error.contains("nothing may have opened"), "{error}");
        }

        /// A file manager that stays in the foreground is an open, so the wait
        /// has to end without treating it as a failure.
        #[test]
        fn an_opener_that_keeps_running_is_an_open() {
            let mut child = spawn("/bin/sleep", &["30"]);
            wait_for_opener(
                "/bin/sleep",
                &mut child,
                Path::new("/tmp"),
                Duration::from_millis(50),
            )
            .expect("still running is still an open");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
