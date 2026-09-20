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
    /// Probes the executable for its version.
    ///
    /// On Windows, Chromium binaries are GUI subsystem applications that do not
    /// implement `--version` and would open a blank browser window if executed
    /// with it. We therefore check local manifests (`manifest.json`, `*.manifest`)
    /// and PE version information first, which is instantaneous and process-free.
    pub fn probe(executable: &Path, timeout: Duration) -> Self {
        if let Some(banner) = read_from_manifest_or_directory(executable) {
            return Self::from_banner(Some(banner));
        }

        #[cfg(windows)]
        if let Some(banner) = read_pe_version_windows(executable, timeout) {
            return Self::from_banner(Some(banner));
        }

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

fn read_from_manifest_or_directory(executable: &Path) -> Option<String> {
    let parent = executable.parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }

    // 1. Check manifest.json
    let manifest_json = parent.join("manifest.json");
    if manifest_json.is_file()
        && let Ok(content) = std::fs::read_to_string(&manifest_json)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&content)
        && let Some(ver) = value.get("version").and_then(|v| v.as_str())
    {
        let ver = ver.trim();
        if parse_major(ver).is_some() {
            return Some(format!("Chromium {ver}"));
        }
    }

    // 2. Scan for *.manifest files (e.g. "148.0.7778.215.manifest")
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && let Some(extension) = path.extension()
                && extension.eq_ignore_ascii_case("manifest")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                let stem = stem.trim();
                if stem.starts_with(|c: char| c.is_ascii_digit()) && parse_major(stem).is_some() {
                    return Some(format!("Chromium {stem}"));
                }
            }
        }
    }

    // 3. Scan for version subdirectories (e.g. "148.0.7778.215/")
    if let Ok(entries) = std::fs::read_dir(parent) {
        let mut best_version: Option<(u32, String)> = None;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                let name = name.trim();
                if name.starts_with(|c: char| c.is_ascii_digit())
                    && let Some(major) = parse_major(name)
                    && best_version
                        .as_ref()
                        .is_none_or(|(best_major, _)| major > *best_major)
                {
                    best_version = Some((major, name.to_string()));
                }
            }
        }
        if let Some((_, ver)) = best_version {
            return Some(format!("Chromium {ver}"));
        }
    }

    None
}

#[cfg(windows)]
fn read_pe_version_windows(executable: &Path, timeout: Duration) -> Option<String> {
    if !executable.is_file() {
        return None;
    }

    let script = format!(
        "(Get-Item -LiteralPath '{}').VersionInfo.ProductVersion",
        executable.display()
    );
    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + timeout.min(Duration::from_secs(3));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut output = String::new();
                child.stdout.take()?.read_to_string(&mut output).ok()?;
                let output = output.trim();
                if parse_major(output).is_some() {
                    return Some(format!("Chromium {output}"));
                }
                return None;
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

    #[test]
    fn probes_version_from_manifest_file() {
        let dir = std::env::temp_dir().join(format!("fp-manifest-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let fake_exe = dir.join("chrome.exe");
        std::fs::write(&fake_exe, b"").expect("write fake exe");
        let manifest = dir.join("148.0.7778.215.manifest");
        std::fs::write(&manifest, b"<assembly></assembly>").expect("write manifest");

        let report = VersionReport::probe(&fake_exe, Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(report.is_detected());
        assert_eq!(report.major, Some(148));
        assert_eq!(report.banner.as_deref(), Some("Chromium 148.0.7778.215"));
    }

    #[test]
    fn probes_version_from_manifest_json() {
        let dir = std::env::temp_dir().join(format!("fp-json-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let fake_exe = dir.join("chrome.exe");
        std::fs::write(&fake_exe, b"").expect("write fake exe");
        let manifest = dir.join("manifest.json");
        std::fs::write(&manifest, r#"{"version": "144.0.7559.132"}"#).expect("write manifest");

        let report = VersionReport::probe(&fake_exe, Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(report.is_detected());
        assert_eq!(report.major, Some(144));
        assert_eq!(report.banner.as_deref(), Some("Chromium 144.0.7559.132"));
    }

    #[test]
    fn probes_version_from_version_subdirectory() {
        let dir = std::env::temp_dir().join(format!("fp-dir-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("153.0.4234.32")).expect("temp dir");

        let fake_exe = dir.join("msedge.exe");
        std::fs::write(&fake_exe, b"").expect("write fake exe");

        let report = VersionReport::probe(&fake_exe, Duration::from_millis(200));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(report.is_detected());
        assert_eq!(report.major, Some(153));
        assert_eq!(report.banner.as_deref(), Some("Chromium 153.0.4234.32"));
    }

    #[test]
    fn probes_real_chromium_if_present() {
        let path = Path::new(r"D:\TEMP\chromium_148\chrome.exe");
        if path.is_file() {
            let report = VersionReport::probe(path, Duration::from_millis(500));
            assert!(report.is_detected());
            assert_eq!(report.major, Some(148));
            assert_eq!(report.banner.as_deref(), Some("Chromium 148.0.7778.215"));
        }
    }
}
