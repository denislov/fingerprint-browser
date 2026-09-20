//! Copying the browser data a configuration backup deliberately leaves out.
//!
//! A browser-data backup is a copy of one or more `profiles/<id>` directories:
//! cookies, Local Storage, IndexedDB, the sessions a login left behind. It is a
//! separate artifact from the configuration backup because it is hundreds of
//! megabytes per profile where that is kilobytes, and the common case - move my
//! configuration to a new machine - must not cost gigabytes to do it.
//!
//! Two rules make the copy safe and predictable:
//!
//! - **Only stopped profiles.** A running Chromium holds its singleton lock and
//!   writes while the copy runs, so the copy would be of an unspecified moment.
//!   The caller says which profiles are running; this refuses and names them,
//!   before anything is written.
//! - **The directory is copied whole.** Excluding caches would mean tracking
//!   Chromium's internal layout across versions, which changes; the cost of
//!   copying whole is size, and the cost of a filter is a backup that is quietly
//!   incomplete after a Chromium update. Size is the cheaper of the two, and the
//!   size is reported before the copy is used.
//!
//! The layout is the data directory's own - `profiles/<id>/` under whatever
//! directory the caller names - so a backup can be pointed back at, and a copy of
//! a whole data directory taken with the program closed is the same shape.

use domain::{BrowserProfile, ProfileId};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Which way a copy goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From each profile's own directory into the backup directory.
    ToBackup,
    /// From the backup directory back into each profile's own directory.
    FromBackup,
}

/// What one copy did, in the terms a user has to hear it in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrowserDataReport {
    /// The backup directory that was written to or read from.
    pub directory: PathBuf,
    /// The profiles whose data was copied, by name.
    pub copied: Vec<String>,
    /// The profiles that were asked for but have no directory yet, by name.
    ///
    /// A profile that has never been started has no browser data to copy, which
    /// is worth saying rather than failing the whole copy over.
    pub skipped: Vec<String>,
    /// How many bytes were written.
    pub bytes: u64,
}

impl BrowserDataReport {
    /// Whether anything moved at all.
    pub fn is_empty(&self) -> bool {
        self.copied.is_empty()
    }

    /// Every profile the copy touched or passed over, for a summary sentence.
    pub fn considered(&self) -> usize {
        self.copied.len() + self.skipped.len()
    }
}

/// Copies each profile's browser data into `directory`.
///
/// `running` is the set of profiles that are active in the runtime; any of them
/// among `profiles` refuses the whole copy, because a partial copy of a running
/// profile is worse than none. The copy is refused before the first byte is
/// written.
pub fn copy_browser_data(
    profiles: &[BrowserProfile],
    running: &HashSet<ProfileId>,
    directory: &Path,
) -> Result<BrowserDataReport, BrowserDataError> {
    transfer(profiles, running, directory, Direction::ToBackup)
}

/// Copies each profile's browser data back out of `directory`.
///
/// The mirror of [`copy_browser_data`], with the same rule about running
/// profiles: a running Chromium is writing the directory being replaced.
pub fn restore_browser_data(
    profiles: &[BrowserProfile],
    running: &HashSet<ProfileId>,
    directory: &Path,
) -> Result<BrowserDataReport, BrowserDataError> {
    transfer(profiles, running, directory, Direction::FromBackup)
}

/// The one copy routine, parameterised by direction.
fn transfer(
    profiles: &[BrowserProfile],
    running: &HashSet<ProfileId>,
    directory: &Path,
    direction: Direction,
) -> Result<BrowserDataReport, BrowserDataError> {
    let blocked: Vec<String> = profiles
        .iter()
        .filter(|profile| running.contains(&profile.id))
        .map(|profile| profile.name.clone())
        .collect();
    if !blocked.is_empty() {
        return Err(BrowserDataError::Running { names: blocked });
    }

    // Ordered by name so a report reads the same way twice, and so the copy does
    // not depend on the order storage happens to list in.
    let mut ordered: Vec<&BrowserProfile> = profiles.iter().collect();
    ordered.sort_by_key(|profile| profile.name.to_lowercase());

    let mut report = BrowserDataReport {
        directory: directory.to_path_buf(),
        ..Default::default()
    };

    for profile in ordered {
        let held = directory.join("profiles").join(profile.id.to_string());
        let (from, to) = match direction {
            Direction::ToBackup => (profile.user_data_dir.clone(), held),
            Direction::FromBackup => (held, profile.user_data_dir.clone()),
        };

        if !from.is_dir() {
            report.skipped.push(profile.name.clone());
            continue;
        }
        if same_directory(&from, &to) {
            return Err(BrowserDataError::SameDirectory {
                profile: profile.name.clone(),
                path: from,
            });
        }

        // Replaced whole rather than merged: a backup with last week's stale
        // files left in it is not a copy of the directory it claims to be. The
        // source exists, so the destination is safe to clear.
        if to.is_dir() {
            std::fs::remove_dir_all(&to).map_err(|source| BrowserDataError::Remove {
                profile: profile.name.clone(),
                path: to.clone(),
                source,
            })?;
        }

        let bytes = copy_tree(&from, &to).map_err(|source| BrowserDataError::Copy {
            profile: profile.name.clone(),
            from: from.clone(),
            to: to.clone(),
            source,
        })?;
        report.bytes += bytes;
        report.copied.push(profile.name.clone());
    }

    Ok(report)
}

/// Copies a directory tree, returning how many bytes of files it wrote.
///
/// Symlinks are followed by `std::fs::copy`: a Chromium profile that links a
/// cache elsewhere gets the file's contents, which is portable and needs no
/// platform-specific link handling on a path whose only job is to be a copy.
fn copy_tree(from: &Path, to: &Path) -> io::Result<u64> {
    std::fs::create_dir_all(to)?;
    let mut bytes = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            bytes += copy_tree(&entry.path(), &target)?;
        } else {
            bytes += std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(bytes)
}

/// Whether two paths name the same directory.
///
/// Canonicalised when both exist, which catches a symlink or a `.` in the way;
/// lexical equality otherwise, which reaches the case that matters most before
/// anything has been written: a backup pointed at the very data directory it
/// would copy.
fn same_directory(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Why a browser-data copy could not be taken.
#[derive(Debug, Error)]
pub enum BrowserDataError {
    /// Profiles that are running, by name. Nothing was copied.
    #[error("a browser-data copy needs its profiles stopped")]
    Running { names: Vec<String> },

    /// The backup directory and the profile's own directory are the same place.
    ///
    /// Refused rather than attempted: clearing the destination first, as a copy
    /// does, would delete the very data about to be read.
    #[error("{profile}'s browser data and the backup directory are the same directory")]
    SameDirectory { profile: String, path: PathBuf },

    #[error("could not clear {path} before copying {profile}'s browser data: {source}")]
    Remove {
        profile: String,
        path: PathBuf,
        source: io::Error,
    },

    #[error("could not copy {profile}'s browser data from {from} to {to}: {source}")]
    Copy {
        profile: String,
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{CoreId, FingerprintProfile, StartTarget, WindowProfile};

    /// A directory of its own under the system temporary directory, removed when
    /// the test ends.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-browser-data-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self { dir }
        }

        fn join(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn profile(dir: &Path, name: &str) -> BrowserProfile {
        BrowserProfile {
            id: ProfileId::new(),
            name: name.to_string(),
            core_id: CoreId::new(),
            user_data_dir: dir.to_path_buf(),
            fingerprint: FingerprintProfile::new_random(7),
            proxy_id: None,
            window: WindowProfile::new(1280, 800),
            start_target: StartTarget::Blank,
        }
    }

    /// A profile directory with a little data in it, including a nested file.
    fn seed_profile(dir: &Path, cookie: &str) {
        std::fs::create_dir_all(dir.join("Default")).expect("create profile dir");
        std::fs::write(dir.join("Default").join("Cookies"), b"cookie").expect("write");
        std::fs::write(dir.join("Local State"), cookie.as_bytes()).expect("write");
    }

    #[test]
    fn a_backup_copies_the_whole_directory_and_reports_the_bytes() {
        let scratch = Scratch::new("copy");
        let data = scratch.join("data");
        let profile = profile(&data.join("profiles/one"), "Work laptop");
        seed_profile(&profile.user_data_dir, "state");

        let report = copy_browser_data(
            std::slice::from_ref(&profile),
            &HashSet::new(),
            &scratch.join("backup"),
        )
        .expect("copy");

        assert_eq!(report.copied, vec!["Work laptop".to_string()]);
        assert!(report.skipped.is_empty());
        let held = scratch
            .join("backup")
            .join("profiles")
            .join(profile.id.to_string());
        assert_eq!(
            std::fs::read(held.join("Default").join("Cookies")).expect("read"),
            b"cookie",
            "a nested file is copied"
        );
        assert_eq!(
            std::fs::read(held.join("Local State")).expect("read"),
            b"state"
        );
        assert_eq!(report.bytes, (b"cookie".len() + b"state".len()) as u64);
    }

    /// The rule the design states plainly: a running profile's data is not
    /// copied, and the refusal names the profiles so the reader knows what to
    /// stop.
    #[test]
    fn a_running_profile_is_refused_by_name_before_anything_is_copied() {
        let scratch = Scratch::new("running");
        let running = profile(&scratch.join("data/profiles/one"), "Busy");
        let idle = profile(&scratch.join("data/profiles/two"), "Idle");
        seed_profile(&running.user_data_dir, "a");
        seed_profile(&idle.user_data_dir, "b");
        let active: HashSet<ProfileId> = [running.id].into_iter().collect();

        let error = copy_browser_data(&[running, idle], &active, &scratch.join("backup"))
            .expect_err("a running profile blocks the copy");

        match error {
            BrowserDataError::Running { names } => assert_eq!(names, vec!["Busy".to_string()]),
            other => panic!("expected Running, got {other:?}"),
        }
        assert!(
            !scratch.join("backup").exists(),
            "nothing is written when the copy is refused"
        );
    }

    /// A profile that has never run has no browser data; that is a skip, not a
    /// failure of the whole backup.
    #[test]
    fn a_profile_with_no_directory_yet_is_skipped_not_refused() {
        let scratch = Scratch::new("absent");
        let fresh = profile(&scratch.join("data/profiles/fresh"), "Fresh");

        let report =
            copy_browser_data(&[fresh], &HashSet::new(), &scratch.join("backup")).expect("copy");

        assert!(report.copied.is_empty());
        assert_eq!(report.skipped, vec!["Fresh".to_string()]);
        assert!(report.is_empty());
    }

    #[test]
    fn a_restore_copies_back_into_the_profiles_own_directory() {
        let scratch = Scratch::new("restore");
        let backup = scratch.join("backup");
        let profile = profile(&scratch.join("data/profiles/one"), "Work laptop");
        let held = backup.join("profiles").join(profile.id.to_string());
        seed_profile(&held, "from-backup");

        let report = restore_browser_data(std::slice::from_ref(&profile), &HashSet::new(), &backup)
            .expect("restore");

        assert_eq!(report.copied, vec!["Work laptop".to_string()]);
        assert_eq!(
            std::fs::read(profile.user_data_dir.join("Local State")).expect("read"),
            b"from-backup"
        );
    }

    /// A backup is a copy, not a merge: a file that was in an older backup and is
    /// not in the source must not survive into the new one.
    #[test]
    fn a_backup_replaces_a_stale_directory_rather_than_merging_into_it() {
        let scratch = Scratch::new("stale");
        let profile = profile(&scratch.join("data/profiles/one"), "Work laptop");
        seed_profile(&profile.user_data_dir, "current");
        let backup = scratch.join("backup");
        let held = backup.join("profiles").join(profile.id.to_string());
        std::fs::create_dir_all(&held).expect("stale backup dir");
        std::fs::write(held.join("Stale File"), b"old").expect("write stale");

        copy_browser_data(&[profile], &HashSet::new(), &backup).expect("copy");

        assert!(
            !held.join("Stale File").exists(),
            "the stale file was carried over"
        );
        assert_eq!(
            std::fs::read(held.join("Local State")).expect("read"),
            b"current"
        );
    }

    /// Pointing the backup at the data directory itself would have the copy
    /// clear the very directory it is reading; it is refused by name.
    #[test]
    fn a_backup_directory_that_is_the_profile_directory_is_refused() {
        let scratch = Scratch::new("same");
        let data = scratch.join("data");
        // The dangerous shape: a profile on its default path and a backup aimed
        // at the data directory itself, so the copy would clear its own source.
        let mut profile = profile(&data.join("profiles/one"), "Work laptop");
        profile.user_data_dir = data.join("profiles").join(profile.id.to_string());
        seed_profile(&profile.user_data_dir, "state");

        let error = copy_browser_data(std::slice::from_ref(&profile), &HashSet::new(), &data)
            .expect_err("the backup would overwrite its own source");

        match error {
            BrowserDataError::SameDirectory { profile: named, .. } => {
                assert_eq!(named, "Work laptop")
            }
            other => panic!("expected SameDirectory, got {other:?}"),
        }
    }

    #[test]
    fn profiles_are_copied_in_name_order() {
        let scratch = Scratch::new("order");
        let alice = profile(&scratch.join("data/profiles/a"), "alice");
        let bob = profile(&scratch.join("data/profiles/b"), "Bob");
        seed_profile(&alice.user_data_dir, "a");
        seed_profile(&bob.user_data_dir, "b");

        let report = copy_browser_data(&[bob, alice], &HashSet::new(), &scratch.join("backup"))
            .expect("copy");

        assert_eq!(report.copied, vec!["alice".to_string(), "Bob".to_string()]);
    }
}
