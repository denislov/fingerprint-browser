//! Browser-core discovery and catalogue maintenance for the bootstrap.
//!
//! Version detection itself lives in the runtime crate
//! ([`runtime::version::VersionReport`]) because it feeds the capability table.
//! This module owns the product decisions around it: where to look for a core,
//! what to name it, and how a replaced binary is noticed.

use domain::{BrowserCore, CoreId, suggested_name};
use runtime::version::{DEFAULT_TIMEOUT as VERSION_TIMEOUT, VersionReport};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use storage::CoreRepository;

/// Explicit core executable. Wins over every other source.
pub use crate::settings::CHROMIUM_BIN_ENV as BIN_ENV;
/// Major override for binaries that do not answer `--version` usefully.
pub use crate::settings::CHROMIUM_MAJOR_ENV as MAJOR_ENV;

/// A banner message and whether it is an error, as the window banner shows it.
pub type Notice = (String, bool);

/// Version probe, injected so maintenance can be tested without spawning.
pub type Probe<'a> = &'a dyn Fn(&Path) -> VersionReport;

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

/// Reconciles the core catalogue with what is actually on disk.
///
/// Registers one core when the catalogue is empty, and otherwise re-probes the
/// registered cores so a replaced binary cannot keep a stale major (and with it
/// a stale capability table).
pub fn maintain(cores: &dyn CoreRepository) -> Option<Notice> {
    maintain_with(cores, discover(), env_major(), &|path| {
        VersionReport::probe(path, VERSION_TIMEOUT)
    })
}

fn maintain_with(
    cores: &dyn CoreRepository,
    discovered: Option<PathBuf>,
    major_override: Option<u32>,
    probe: Probe<'_>,
) -> Option<Notice> {
    let existing = match cores.list() {
        Ok(existing) => existing,
        Err(error) => return Some((format!("could not read browser cores: {error}"), true)),
    };

    if existing.is_empty() {
        let Some(executable) = discovered else {
            return Some((
                format!(
                    "no browser core found; set {} to a fingerprint-chromium executable and restart",
                    BIN_ENV
                ),
                true,
            ));
        };
        return register(cores, &executable, major_override, probe);
    }

    refresh(cores, &existing, probe)
}

fn register(
    cores: &dyn CoreRepository,
    executable: &Path,
    major_override: Option<u32>,
    probe: Probe<'_>,
) -> Option<Notice> {
    let report = probe(executable);
    let major = report.major.or(major_override).unwrap_or(0);
    let version = report
        .banner
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let core = BrowserCore {
        id: CoreId::new(),
        name: suggested_name(executable, major),
        executable: executable.to_path_buf(),
        version,
        major,
    };

    if let Err(error) = cores.save(&core) {
        return Some((format!("could not store browser core: {error}"), true));
    }

    tracing::info!(
        "registered browser core {} (major {})",
        core.name,
        core.major
    );

    (major == 0).then(|| {
        (
            format!(
                "{} did not report a usable version; set {} so fingerprint switches can be checked",
                core.executable.display(),
                MAJOR_ENV
            ),
            false,
        )
    })
}

/// Re-probes every registered core and rewrites the ones whose binary changed.
///
/// A core that cannot be re-read is left alone: its stored major was either
/// detected earlier or set by `FP_BROWSER_CHROMIUM_MAJOR`, and dropping it would
/// break a working configuration. A missing executable is reported instead.
fn refresh(
    cores: &dyn CoreRepository,
    existing: &[BrowserCore],
    probe: Probe<'_>,
) -> Option<Notice> {
    let mut updated: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    for core in existing {
        if !core.executable.is_file() {
            missing.push(format!("{} ({})", core.name, core.executable.display()));
            continue;
        }

        let report = probe(&core.executable);
        let Some(major) = report.major else {
            continue;
        };
        if major == core.major && report.banner.as_deref() == Some(core.version.as_str()) {
            continue;
        }

        let refreshed = BrowserCore {
            name: refreshed_name(core, major),
            version: report
                .banner
                .clone()
                .unwrap_or_else(|| core.version.clone()),
            major,
            ..core.clone()
        };
        if let Err(error) = cores.save(&refreshed) {
            return Some((format!("could not update browser core: {error}"), true));
        }

        tracing::info!(
            "browser core {} changed: major {} -> {}",
            core.name,
            core.major,
            major
        );
        updated.push(format!(
            "{} is now {} (major {})",
            core.name, refreshed.version, refreshed.major
        ));
    }

    if !missing.is_empty() {
        return Some((
            format!(
                "browser core executable missing: {}; set {} and restart",
                missing.join(", "),
                BIN_ENV
            ),
            true,
        ));
    }

    (!updated.is_empty()).then(|| {
        (
            format!("browser core updated: {}", updated.join("; ")),
            false,
        )
    })
}

/// Keeps an auto-generated name in step with the detected major, but never
/// rewrites a name the user chose.
fn refreshed_name(core: &BrowserCore, major: u32) -> String {
    if core.name == suggested_name(&core.executable, core.major) {
        suggested_name(&core.executable, major)
    } else {
        core.name.clone()
    }
}

fn existing_file(path: &Path) -> Option<PathBuf> {
    path.is_file().then(|| path.to_path_buf())
}

fn env_major() -> Option<u32> {
    std::env::var(MAJOR_ENV).ok()?.trim().parse().ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use storage::MemCoreRepository;

    fn touching(path: &Path) {
        std::fs::write(path, b"#!/bin/sh\n").expect("write executable");
    }

    fn probe_returning(banner: Option<&'static str>) -> impl Fn(&Path) -> VersionReport {
        move |_| VersionReport::from_banner(banner.map(str::to_string))
    }

    /// A stand-in browser binary in its own directory.
    ///
    /// The directory goes away with the guard: a test run used to leave one
    /// behind per test.
    struct TempExecutable {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempExecutable {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-core-detect-{}", CoreId::new()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            let path = dir.join(name);
            touching(&path);
            Self { dir, path }
        }

        fn path(&self) -> PathBuf {
            self.path.clone()
        }

        /// Removes the executable, as if the binary had been uninstalled.
        fn uninstall(self) {
            std::fs::remove_file(&self.path).expect("remove executable");
        }
    }

    impl Drop for TempExecutable {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn an_empty_catalogue_registers_the_discovered_core() {
        let cores: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let executable = TempExecutable::new("chrome");

        let notice = maintain_with(
            cores.as_ref(),
            Some(executable.path()),
            None,
            &probe_returning(Some("Chromium 148.0.7778.215")),
        );

        assert_eq!(notice, None);
        let stored = cores.list().expect("list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].major, 148);
        assert_eq!(stored[0].version, "Chromium 148.0.7778.215");
        assert_eq!(stored[0].name, "chrome 148");
        assert_eq!(stored[0].executable, executable.path());
    }

    #[test]
    fn the_major_override_covers_a_silent_binary() {
        let cores: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let executable = TempExecutable::new("chrome");

        let notice = maintain_with(
            cores.as_ref(),
            Some(executable.path()),
            Some(144),
            &probe_returning(None),
        );

        assert_eq!(notice, None);
        assert_eq!(cores.list().expect("list")[0].major, 144);
        assert_eq!(cores.list().expect("list")[0].version, "unknown");
    }

    #[test]
    fn an_unreadable_version_is_reported_without_a_major() {
        let cores: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let executable = TempExecutable::new("chrome");

        let notice = maintain_with(
            cores.as_ref(),
            Some(executable.path()),
            None,
            &probe_returning(None),
        );

        let (message, is_error) = notice.expect("notice");
        assert!(!is_error, "a registered but unclassified core is a warning");
        assert!(message.contains(MAJOR_ENV), "{message}");
        assert_eq!(cores.list().expect("list")[0].major, 0);
    }

    #[test]
    fn no_discovered_core_is_an_error_not_an_empty_catalogue() {
        let cores: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());

        let notice = maintain_with(cores.as_ref(), None, None, &probe_returning(None));

        let (message, is_error) = notice.expect("notice");
        assert!(is_error);
        assert!(message.contains("no browser core found"), "{message}");
        assert!(message.contains(BIN_ENV), "{message}");
        assert!(cores.list().expect("list").is_empty());
    }

    #[test]
    fn a_replaced_binary_updates_the_stored_major_and_name() {
        let cores: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let executable = TempExecutable::new("chrome");
        maintain_with(
            cores.as_ref(),
            Some(executable.path()),
            None,
            &probe_returning(Some("Chromium 148.0.7778.215")),
        );

        let notice = maintain_with(
            cores.as_ref(),
            None,
            None,
            &probe_returning(Some("Chromium 150.0.1234.5")),
        );

        let (message, is_error) = notice.expect("notice");
        assert!(!is_error);
        assert!(message.contains("150.0.1234.5"), "{message}");
        let stored = cores.list().expect("list");
        assert_eq!(stored.len(), 1, "refresh must not add a second core");
        assert_eq!(stored[0].major, 150);
        assert_eq!(stored[0].name, "chrome 150");
    }

    #[test]
    fn a_custom_core_name_survives_a_version_change() {
        let cores: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let executable = TempExecutable::new("chrome");
        maintain_with(
            cores.as_ref(),
            Some(executable.path()),
            None,
            &probe_returning(Some("Chromium 148.0.7778.215")),
        );
        let mut renamed = cores.list().expect("list")[0].clone();
        renamed.name = "Work Browser".to_string();
        cores.save(&renamed).expect("rename");

        maintain_with(
            cores.as_ref(),
            None,
            None,
            &probe_returning(Some("Chromium 150.0.1234.5")),
        );

        let stored = cores.list().expect("list");
        assert_eq!(stored[0].name, "Work Browser");
        assert_eq!(stored[0].major, 150);
    }

    #[test]
    fn an_unreadable_probe_keeps_the_stored_core() {
        let cores: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let executable = TempExecutable::new("chrome");
        maintain_with(
            cores.as_ref(),
            Some(executable.path()),
            None,
            &probe_returning(Some("Chromium 148.0.7778.215")),
        );

        let notice = maintain_with(cores.as_ref(), None, None, &probe_returning(None));

        assert_eq!(notice, None, "an unreadable probe is not news");
        assert_eq!(cores.list().expect("list")[0].major, 148);
    }

    #[test]
    fn a_missing_executable_is_an_error_that_names_the_core() {
        let cores: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let executable = TempExecutable::new("chrome");
        maintain_with(
            cores.as_ref(),
            Some(executable.path()),
            None,
            &probe_returning(Some("Chromium 148.0.7778.215")),
        );
        executable.uninstall();

        let notice = maintain_with(cores.as_ref(), None, None, &probe_returning(None));

        let (message, is_error) = notice.expect("notice");
        assert!(is_error);
        assert!(message.contains("chrome 148"), "{message}");
        assert!(message.contains(BIN_ENV), "{message}");
    }

    #[test]
    fn names_the_core_from_the_executable_and_major() {
        assert_eq!(
            suggested_name(Path::new("/tmp/tools/chrome"), 148),
            "chrome 148"
        );
        assert_eq!(
            suggested_name(Path::new("/tmp/tools/chrome"), 0),
            "chrome (version unknown)"
        );
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
