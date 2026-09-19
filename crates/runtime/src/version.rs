//! Browser-core version detection.
//!
//! The core's major decides which fingerprint switches it can honour, so it has
//! to come from the binary itself rather than from a hand-typed field. Probing
//! is bounded: the executable is not part of this project and a wrong or hung
//! binary must never block startup.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long `<executable> --version` may take before it is killed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// What a core answered when asked for its version.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VersionReport {
    /// Banner exactly as printed, when the executable answered successfully.
    pub banner: Option<String>,
    /// Major parsed out of the banner, when it contained one.
    pub major: Option<u32>,
}

impl VersionReport {
    /// Asks `<executable> --version` and parses the answer.
    pub fn probe(executable: &Path, timeout: Duration) -> Self {
        Self::from_banner(read_banner(executable, timeout))
    }

    pub fn from_banner(banner: Option<String>) -> Self {
        let major = banner.as_deref().and_then(parse_major);
        Self { banner, major }
    }

    /// Whether the executable reported a usable version at all.
    pub fn is_detected(&self) -> bool {
        self.major.is_some()
    }
}

/// First run of ASCII digits in the banner: `Chromium 148.0.7778.215` is 148.
pub fn parse_major(banner: &str) -> Option<u32> {
    let digits: String = banner
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn read_banner(executable: &Path, timeout: Duration) -> Option<String> {
    let mut child = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + timeout;
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
                std::thread::sleep(POLL_INTERVAL);
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
        assert_eq!(
            VersionReport::from_banner(Some("Chromium".into())).major,
            None
        );
    }

    #[test]
    fn a_report_keeps_the_raw_banner_for_the_core_record() {
        let report = VersionReport::from_banner(Some("Chromium 148.0.7778.215".into()));

        assert!(report.is_detected());
        assert_eq!(report.major, Some(148));
        assert_eq!(report.banner.as_deref(), Some("Chromium 148.0.7778.215"));
    }

    #[test]
    fn a_missing_executable_reports_nothing() {
        let report = VersionReport::probe(
            Path::new("definitely-not-a-real-binary-xyz"),
            Duration::from_millis(200),
        );

        assert!(!report.is_detected());
        assert_eq!(report.banner, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_hanging_executable_is_killed_at_the_deadline() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("fp-version-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let script = dir.join("hangs-on-version");
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").expect("write script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).expect("chmod");

        let started = Instant::now();
        let report = VersionReport::probe(&script, Duration::from_millis(300));
        let elapsed = started.elapsed();

        let _ = std::fs::remove_dir_all(&dir);
        assert!(!report.is_detected());
        assert!(
            elapsed < Duration::from_secs(5),
            "probe must respect its deadline, took {elapsed:?}"
        );
    }
}
