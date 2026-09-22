//! Copying the browser data a configuration backup deliberately leaves out.
//!
//! A browser-data backup is a copy of one or more `profiles/<id>` directories:
//! cookies, Local Storage, IndexedDB, the sessions a login left behind. It is a
//! separate artifact from the configuration backup because it is hundreds of
//! megabytes per profile where that is kilobytes, and the common case - move my
//! configuration to a new machine - must not cost gigabytes to do it.
//!
//! Three rules make the copy safe and predictable:
//!
//! - **Only stopped profiles.** A running Chromium holds its singleton lock and
//!   writes while the copy runs, so the copy would be of an unspecified moment.
//!   The caller says which profiles are running; this refuses and names them,
//!   before anything is written.
//! - **No path contains another.** A copy clears its destination, so a
//!   destination that contains its own source would delete the data it is about
//!   to read, and a destination inside its source would be copied into itself as
//!   it grew. Every path in the run is resolved and compared before the first
//!   write - see [`plan`] - because a backup directory is a path the user picked,
//!   and every one of those shapes is reachable by picking the wrong one.
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

    // Every path this run will use, resolved and compared before the first byte:
    // a refusal here leaves every directory exactly as it was.
    let (copies, skipped) = plan(&ordered, directory, direction)?;

    let mut report = BrowserDataReport {
        directory: directory.to_path_buf(),
        skipped,
        ..Default::default()
    };

    for copy in &copies {
        // Replaced whole rather than merged: a backup with last week's stale
        // files left in it is not a copy of the directory it claims to be. The
        // source exists, so the destination is safe to clear.
        if copy.to.is_dir() {
            std::fs::remove_dir_all(&copy.to).map_err(|source| BrowserDataError::Remove {
                profile: copy.profile.clone(),
                path: copy.to.clone(),
                source,
            })?;
        }

        let bytes = copy_tree(&copy.from, &copy.to).map_err(|source| BrowserDataError::Copy {
            profile: copy.profile.clone(),
            from: copy.from.clone(),
            to: copy.to.clone(),
            source,
        })?;
        report.bytes += bytes;
        report.copied.push(copy.profile.clone());
    }

    Ok(report)
}

/// One copy that will be made: where the data is now, and where it goes.
struct Copy {
    profile: String,
    from: PathBuf,
    to: PathBuf,
}

/// The copies a run will make, in name order, and the profiles with no data to
/// copy.
///
/// Both paths of every copy, and every pair of them, are resolved and compared
/// here, before the caller writes anything. The comparison is by *directory*
/// rather than by spelling: an existing path is resolved through symlinks and
/// Windows reparse points, and a path that does not exist yet is resolved
/// through its deepest existing ancestor with the rest applied lexically, so
/// "the destination is the source, reached another way" and "the destination is
/// inside the source, which does not exist yet" are both caught.
///
/// The comparison is across profiles as well as within one, because a run
/// copies several at once: one profile's destination may be another's source,
/// and a copy that clears it would be reading data the other is still to write.
fn plan(
    ordered: &[&BrowserProfile],
    directory: &Path,
    direction: Direction,
) -> Result<(Vec<Copy>, Vec<String>), BrowserDataError> {
    let mut copies: Vec<Copy> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    for profile in ordered {
        let held = directory.join("profiles").join(profile.id.to_string());
        let (from, to) = match direction {
            Direction::ToBackup => (profile.user_data_dir.clone(), held),
            Direction::FromBackup => (held, profile.user_data_dir.clone()),
        };

        // A profile that has never been started has no browser data to copy,
        // which is worth saying rather than failing the whole copy over. It has
        // no destination to check either, because nothing will be written.
        if !from.is_dir() {
            skipped.push(profile.name.clone());
            continue;
        }

        copies.push(Copy {
            profile: profile.name.clone(),
            from: resolve(&from),
            to: resolve(&to),
        });
    }

    for copy in &copies {
        if copy.from == copy.to {
            return Err(BrowserDataError::SameDirectory {
                profile: copy.profile.clone(),
                path: copy.from.clone(),
            });
        }
        if contains(&copy.from, &copy.to) || contains(&copy.to, &copy.from) {
            return Err(BrowserDataError::Overlap {
                profile: copy.profile.clone(),
                from: copy.from.clone(),
                to: copy.to.clone(),
            });
        }
    }

    for (index, first) in copies.iter().enumerate() {
        for second in &copies[index + 1..] {
            for (a, b) in [
                (&first.from, &second.from),
                (&first.from, &second.to),
                (&first.to, &second.from),
                (&first.to, &second.to),
            ] {
                if a == b || contains(a, b) || contains(b, a) {
                    return Err(BrowserDataError::ProfilesOverlap {
                        first_profile: first.profile.clone(),
                        second_profile: second.profile.clone(),
                        first: a.clone(),
                        second: b.clone(),
                    });
                }
            }
        }
    }

    Ok((copies, skipped))
}

/// Whether `outer` is a directory that contains `inner`.
///
/// Component-wise, so `/a/b` contains `/a/b/c` and not `/a/bc`.
fn contains(outer: &Path, inner: &Path) -> bool {
    inner != outer && inner.starts_with(outer)
}

/// Resolves a path for comparison with another, existing or not.
///
/// The deepest ancestor that exists is canonicalised - which is what resolves a
/// symlink in the way, and on Windows a junction or another reparse point - and
/// the components after it are applied lexically, because there is nothing there
/// to resolve and the canonical prefix has no links left in it to mislead them.
fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        // A relative path is relative to the process, and the two paths this is
        // asked about are compared with each other: without this, a relative
        // destination that does not exist yet could never be seen to be inside
        // an absolute source.
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };

    let mut missing = PathBuf::new();
    let mut current = absolute.as_path();
    loop {
        if let Ok(real) = std::fs::canonicalize(current) {
            return normalize(&real.join(&missing));
        }
        let Some(name) = current.file_name() else {
            // A root, or a `..` with nothing to resolve it against: there is no
            // existing ancestor left, so the path can only be read as written.
            return normalize(&absolute);
        };
        missing = Path::new(name).join(&missing);
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => return normalize(&absolute),
        }
    }
}

/// Folds `.` and `..` out of a path that no longer has a filesystem to resolve
/// them against.
///
/// `..` pops, which is right after [`resolve`]'s canonical prefix: a canonical
/// path has no links left, so its parent really is the component before it.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
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

    /// One profile's source and destination contain each other.
    ///
    /// The same deletion as [`Self::SameDirectory`] one level up: a destination
    /// that contains the source would be cleared before the source is read, and
    /// a destination inside the source would be copied into itself.
    #[error("{profile}'s browser data and its copy overlap: {from} and {to}")]
    Overlap {
        profile: String,
        from: PathBuf,
        to: PathBuf,
    },

    /// Two profiles' copies would use directories that contain each other.
    ///
    /// Refused before either is written: one copy would be clearing a directory
    /// the other is reading from, or writing to.
    #[error(
        "the browser-data copies for {first_profile} and {second_profile} overlap: {first} and {second}"
    )]
    ProfilesOverlap {
        first_profile: String,
        second_profile: String,
        first: PathBuf,
        second: PathBuf,
    },

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

    /// A backup directory *inside* the profile it copies. The copy creates the
    /// destination under the source and then walks into it, so the tree it is
    /// reading grows while it reads - the shape that runs until the path limit or
    /// the disk does. Refused before anything is created.
    #[test]
    fn a_backup_directory_inside_the_profile_directory_is_refused() {
        let scratch = Scratch::new("inside");
        let profile = profile(&scratch.join("data/profiles/one"), "Work laptop");
        seed_profile(&profile.user_data_dir, "state");
        let inside = profile.user_data_dir.join("backup");

        let error = copy_browser_data(std::slice::from_ref(&profile), &HashSet::new(), &inside)
            .expect_err("the copy would descend into its own source");

        match error {
            BrowserDataError::Overlap { profile: named, .. } => assert_eq!(named, "Work laptop"),
            other => panic!("expected Overlap, got {other:?}"),
        }
        assert!(!inside.exists(), "the refused copy created nothing");
        assert_eq!(
            std::fs::read(profile.user_data_dir.join("Local State")).expect("read"),
            b"state",
            "the source is exactly as it was"
        );
    }

    /// A restore whose destination is an ancestor of its source: the backup
    /// lives inside the profile directory it would be restored to. Clearing the
    /// destination first - which is what a restore does - would delete the backup
    /// before reading it, and the restore would then fail with the data gone.
    #[test]
    fn a_restore_whose_destination_contains_its_source_is_refused() {
        let scratch = Scratch::new("ancestor");
        let profile = profile(&scratch.join("data/profiles/one"), "Work laptop");
        seed_profile(&profile.user_data_dir, "state");
        let backup = profile.user_data_dir.join("backup");
        let held = backup.join("profiles").join(profile.id.to_string());
        seed_profile(&held, "from-backup");

        let error = restore_browser_data(std::slice::from_ref(&profile), &HashSet::new(), &backup)
            .expect_err("the destination contains the source");

        match error {
            BrowserDataError::Overlap { profile: named, .. } => assert_eq!(named, "Work laptop"),
            other => panic!("expected Overlap, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(held.join("Local State")).expect("read"),
            b"from-backup",
            "the backup is still there to be restored by hand"
        );
        assert_eq!(
            std::fs::read(profile.user_data_dir.join("Local State")).expect("read"),
            b"state",
            "and the profile's own data was not cleared either"
        );
    }

    /// Two profiles pointed at directories that contain one another. The second
    /// profile's data directory *is* where the first one's backup goes, so
    /// copying the first would clear the second's source. The run is refused
    /// before the first profile has written anything.
    #[test]
    fn two_profiles_whose_directories_overlap_are_refused() {
        let scratch = Scratch::new("cross");
        let backup = scratch.join("backup");
        let alice = profile(&scratch.join("data/profiles/a"), "Alice");
        seed_profile(&alice.user_data_dir, "alice");
        let mut zoe = profile(&scratch.join("data/profiles/z"), "Zoe");
        seed_profile(&zoe.user_data_dir, "zoe");
        zoe.user_data_dir = backup.join("profiles").join(alice.id.to_string());
        seed_profile(&zoe.user_data_dir, "zoe");
        let zoe_dir = zoe.user_data_dir.clone();

        let error = copy_browser_data(&[alice, zoe], &HashSet::new(), &backup)
            .expect_err("one profile's destination is the other's source");

        match error {
            BrowserDataError::ProfilesOverlap {
                first_profile,
                second_profile,
                ..
            } => {
                assert_eq!(first_profile, "Alice");
                assert_eq!(second_profile, "Zoe");
            }
            other => panic!("expected ProfilesOverlap, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(zoe_dir.join("Local State")).expect("read"),
            b"zoe",
            "Alice was not copied over Zoe's data"
        );
    }

    /// A destination that does not exist yet, reached through a symlinked
    /// ancestor, inside the source. Only resolving the destination's deepest
    /// existing ancestor catches this; comparing the paths as written does not,
    /// and the copy would then descend into its own source.
    #[cfg(unix)]
    #[test]
    fn a_destination_inside_a_symlinked_source_is_refused() {
        let scratch = Scratch::new("symlink");
        let profile = profile(&scratch.join("data/profiles/one"), "Work laptop");
        seed_profile(&profile.user_data_dir, "state");
        let link = scratch.join("link");
        std::os::unix::fs::symlink(&profile.user_data_dir, &link).expect("symlink");

        // `link/profiles/<id>` does not exist, and `link` is the source.
        let error = copy_browser_data(std::slice::from_ref(&profile), &HashSet::new(), &link)
            .expect_err("the copy would descend into its own source");

        match error {
            BrowserDataError::Overlap { profile: named, .. } => assert_eq!(named, "Work laptop"),
            other => panic!("expected Overlap, got {other:?}"),
        }
        assert!(!link.join("profiles").exists(), "nothing was created");
        assert_eq!(
            std::fs::read(profile.user_data_dir.join("Local State")).expect("read"),
            b"state",
            "the source is exactly as it was"
        );
    }
}
