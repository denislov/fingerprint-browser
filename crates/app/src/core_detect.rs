//! Browser-core discovery for the application bootstrap.
//!
//! Phase 4 owns real version detection in the runtime crate. Until that lands
//! this module only has to produce one launchable core row so the window can
//! start a browser: it resolves an executable, asks it for `--version`, and
//! parses the major out of the answer.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Explicit core executable. Wins over every other source.
pub const BIN_ENV: &str = "FP_BROWSER_CHROMIUM_BIN";
/// Major override for binaries that do not answer `--version` usefully.
pub const MAJOR_ENV: &str = "FP_BROWSER_CHROMIUM_MAJOR";

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const VERSION_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedCore {
    pub name: String,
    pub executable: PathBuf,
    pub version: String,
    /// Detected major, or the `FP_BROWSER_CHROMIUM_MAJOR` override.
    /// `0` means "unknown"; the capability layer rejects it at launch.
    pub major: u32,
}

/// Environment-first discovery, then conventional relative paths, then `PATH`.
///
/// An explicit `FP_BROWSER_CHROMIUM_BIN` is authoritative: when it is set but
/// does not point at a file, discovery stops instead of silently picking a
/// different browser than the user asked for.
pub fn discover() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(BIN_ENV) {
        return existing_file(Path::new(&explicit));
    }

    if let Some(found) = relative_candidates()
        .into_iter()
        .find_map(|path| existing_file(&path))
    {
        return Some(found);
    }

    let path_var = std::env::var_os("PATH")?;
    let names = executable_names();
    std::env::split_paths(&path_var)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find_map(|candidate| existing_file(&candidate))
}

fn existing_file(path: &Path) -> Option<PathBuf> {
    path.is_file().then(|| path.to_path_buf())
}

pub fn detect(executable: &Path) -> DetectedCore {
    let version = read_version(executable);
    let major = version
        .as_deref()
        .and_then(parse_major)
        .or_else(env_major)
        .unwrap_or(0);

    DetectedCore {
        name: display_name(executable, major),
        executable: executable.to_path_buf(),
        version: version.unwrap_or_else(|| "unknown".to_string()),
        major,
    }
}

/// First run of ASCII digits in the version banner, e.g. `Chromium 148.0.7778.215`.
pub fn parse_major(version_output: &str) -> Option<u32> {
    let digits: String = version_output
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn env_major() -> Option<u32> {
    std::env::var(MAJOR_ENV).ok()?.trim().parse().ok()
}

fn display_name(executable: &Path, major: u32) -> String {
    let stem = executable
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "browser".to_string());
    if major == 0 {
        format!("{stem} (version unknown)")
    } else {
        format!("{stem} {major}")
    }
}

fn relative_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = ["bin/chromium", "bin/chrome", "chrome"]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if cfg!(windows) {
        candidates.extend(
            ["bin/chrome.exe", "chrome.exe"]
                .into_iter()
                .map(PathBuf::from),
        );
    }
    candidates
}

fn executable_names() -> Vec<OsString> {
    let names: &[&str] = if cfg!(windows) {
        &["chrome.exe", "chromium.exe"]
    } else if cfg!(target_os = "macos") {
        &["Chromium", "Google Chrome"]
    } else {
        &["chromium", "chrome", "google-chrome", "ungoogled-chromium"]
    };
    names.iter().map(OsString::from).collect()
}

/// Run `<executable> --version` with a bounded wait.
///
/// The binary is not part of this project, so it can never be awaited without a
/// deadline: a wrong or hung executable must not block the window.
fn read_version(executable: &Path) -> Option<String> {
    let mut child = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut output = String::new();
                child.stdout.take()?.read_to_string(&mut output).ok()?;
                let output = output.trim().to_string();
                return (!output.is_empty()).then_some(output);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(VERSION_POLL_INTERVAL);
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_major_from_a_chromium_banner() {
        assert_eq!(parse_major("Chromium 148.0.7778.215"), Some(148));
        assert_eq!(parse_major("Google Chrome 143.0.7499.169"), Some(143));
        assert_eq!(parse_major("  Chromium 110.0.5481.177\n"), Some(110));
    }

    #[test]
    fn rejects_banners_without_digits() {
        assert_eq!(parse_major(""), None);
        assert_eq!(parse_major("Chromium"), None);
    }

    #[test]
    fn names_the_core_from_the_executable_and_major() {
        assert_eq!(
            display_name(Path::new("/tmp/tools/chrome"), 148),
            "chrome 148"
        );
        assert_eq!(
            display_name(Path::new("/tmp/tools/chrome"), 0),
            "chrome (version unknown)"
        );
    }

    #[test]
    fn detection_falls_back_to_a_named_version_unknown_core() {
        let detected = detect(Path::new("definitely-not-a-real-binary-xyz"));

        assert_eq!(detected.major, 0);
        assert_eq!(detected.version, "unknown");
        assert!(detected.name.contains("version unknown"));
    }

    #[test]
    fn existing_file_rejects_paths_that_do_not_resolve() {
        assert_eq!(
            existing_file(Path::new("definitely-not-a-real-binary-xyz")),
            None
        );

        let running = std::env::current_exe().expect("current exe");
        assert_eq!(existing_file(&running), Some(running));
    }
}
