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
//! - **Written whole, then put in place.** The copy is built beside its
//!   destination and renamed onto it only once it is complete, so a copy that
//!   fails halfway - a file that cannot be read, a disk that fills - leaves the
//!   directory that was there exactly as it was. [`commit`] is the two renames
//!   and [`recover`] is what a run killed between them leaves behind.
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

/// Puts back what an interrupted restore left in these profiles' own directories,
/// and names the profiles it did that for.
///
/// A restore's destination is the profile's own data directory, which the program
/// knows at startup, so this is the half of the recovery that can happen then. The
/// other half - a copy whose destination is inside the backup directory - cannot:
/// that directory is chosen for one operation and not recorded anywhere, so it is
/// recovered when it is next used instead.
///
/// Called before the window opens, because its answer is a sentence the user
/// should read: a restore that was killed between its two renames left the profile
/// with no data directory at all, and one that was not put back looks like data
/// that was lost.
pub fn recover_user_data_dirs(profiles: &[BrowserProfile]) -> Vec<String> {
    let mut put_back = Vec::new();
    for profile in profiles {
        let to = resolve(&profile.user_data_dir);
        // Recovery may run before a copy plan exists. Protect every other
        // profile's directory, including profiles with no source to copy.
        if profiles.iter().any(|other| {
            other.id != profile.id
                && footprint(&to)
                    .iter()
                    .any(|path| overlaps(path, &resolve(&other.user_data_dir)))
        }) {
            tracing::warn!("refused overlapping recovery for {}", profile.name);
            continue;
        }
        match recover(&profile.name, &to) {
            Ok(true) => put_back.push(profile.name.clone()),
            Ok(false) => {}
            Err(error) => tracing::warn!("{error}"),
        }
    }
    put_back
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
        // What a previous run left in the middle of a swap, before anything else
        // looks at the destination.
        recover(&copy.profile, &copy.to)?;
        claim(&copy.profile, &copy.to)?;

        // Built beside the destination rather than in it, so the destination is
        // untouched until there is a complete copy to put there: a failure
        // halfway through - a file that cannot be read, a disk that fills - has
        // written nothing anyone reads, and the old directory is still the one in
        // place. Beside rather than in the system temporary directory, because the
        // swap that follows is a rename, and a rename is only a move within one
        // filesystem.
        let bytes = copy_tree(&copy.from, &staged(&copy.to)).map_err(|source| {
            // The unfinished copy is not the destination's replacement and never
            // becomes one; leaving it would only be confusing.
            let _ = recover(&copy.profile, &copy.to);
            BrowserDataError::Copy {
                profile: copy.profile.clone(),
                from: copy.from.clone(),
                to: copy.to.clone(),
                source,
            }
        })?;

        commit(copy)?;
        recover(&copy.profile, &copy.to)?;

        report.bytes += bytes;
        report.copied.push(copy.profile.clone());
    }

    Ok(report)
}

/// One copy that will be made: where the data is now, and where it goes.
struct Copy {
    id: ProfileId,
    profile: String,
    from: PathBuf,
    to: PathBuf,
}

/// Reserved sibling names are used only while an ownership record exists.
/// Unmarked leftovers from older versions are preserved for manual recovery.
const STAGING: &str = "partial";
const DISPLACED: &str = "old";

/// Where a finished copy of `to` is built.
fn staged(to: &Path) -> PathBuf {
    beside(to, STAGING)
}

/// Where the directory `to` currently names is kept while the copy takes its name.
fn displaced(to: &Path) -> PathBuf {
    beside(to, DISPLACED)
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

        let to = resolve(&to);
        copies.push(Copy {
            id: profile.id,
            profile: profile.name.clone(),
            from: resolve(&from),
            to,
        });
    }

    for copy in &copies {
        if copy.from == copy.to {
            return Err(BrowserDataError::SameDirectory {
                profile: copy.profile.clone(),
                path: copy.from.clone(),
            });
        }
        for path in footprint(&copy.to) {
            if overlaps(&copy.from, &path) {
                return Err(BrowserDataError::Overlap {
                    profile: copy.profile.clone(),
                    from: copy.from.clone(),
                    to: path,
                });
            }
            for other in ordered {
                let source = resolve(&other.user_data_dir);
                if other.id != copy.id && overlaps(&path, &source) {
                    return Err(BrowserDataError::ProfilesOverlap {
                        first_profile: copy.profile.clone(),
                        second_profile: other.name.clone(),
                        first: path.clone(),
                        second: source,
                    });
                }
            }
        }
    }
    for (index, first) in copies.iter().enumerate() {
        let first_paths = std::iter::once(first.from.clone()).chain(footprint(&first.to));
        for a in first_paths {
            for second in &copies[index + 1..] {
                for b in std::iter::once(second.from.clone()).chain(footprint(&second.to)) {
                    if overlaps(&a, &b) {
                        return Err(BrowserDataError::ProfilesOverlap {
                            first_profile: first.profile.clone(),
                            second_profile: second.profile.clone(),
                            first: a.clone(),
                            second: b,
                        });
                    }
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

/// Puts the replacement in place of whatever is there.
///
/// Two renames rather than a delete and a copy: the old directory is renamed
/// aside first, so a failure while the copy takes its name can put it back, and
/// the only window in which the destination does not exist is the two renames
/// themselves. Everything before this happened in the staging directory, which
/// nothing else reads.
fn commit(copy: &Copy) -> Result<(), BrowserDataError> {
    let (staging, old) = (staged(&copy.to), displaced(&copy.to));

    if !exists(&copy.to) {
        return rename(&staging, &copy.to, &copy.profile);
    }

    rename(&copy.to, &old, &copy.profile)?;
    if let Err(source) = std::fs::rename(&staging, &copy.to) {
        // The destination is missing and the old directory is the only copy of
        // it. Put that back before reporting, so a failed copy is a copy that did
        // not happen rather than one that lost what was there.
        if let Err(restoring) = std::fs::rename(&old, &copy.to) {
            tracing::error!(
                "could not put {} back after a failed copy of {}'s browser data: {restoring}",
                copy.to.display(),
                copy.profile
            );
        }
        return Err(BrowserDataError::Replace {
            profile: copy.profile.clone(),
            path: copy.to.clone(),
            source,
        });
    }

    // Ownership remains recorded until recovery has removed all leftovers.
    Ok(())
}

/// Recovers only a swap with a valid ownership record. A suffix alone is
/// never evidence that a directory belongs to us.
fn recover(profile: &str, to: &Path) -> Result<bool, BrowserDataError> {
    let (old, staging) = (displaced(to), staged(to));
    let marker = ownership(to);
    if !exists(&marker) {
        if exists(&old) || exists(&staging) {
            return Err(BrowserDataError::Unowned {
                path: to.to_path_buf(),
            });
        }
        return Ok(false);
    }
    let record: CopyOwner = std::fs::read(&marker)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .filter(|record: &CopyOwner| {
            record.destination == to && record.format == "fp-browser-copy-v1"
        })
        .ok_or_else(|| BrowserDataError::Unowned {
            path: marker.clone(),
        })?;
    let _operation = record.operation;
    let mut put_back = false;

    if !exists(to) && exists(&old) {
        tracing::warn!(
            "{}'s earlier copy was interrupted with {} renamed aside; putting it back",
            profile,
            old.display()
        );
        rename(&old, to, profile)?;
        put_back = true;
    }
    for path in [&old, &staging, &marker] {
        remove_any(path).map_err(|source| BrowserDataError::Replace {
            profile: profile.to_string(),
            path: path.clone(),
            source,
        })?;
    }
    Ok(put_back)
}

/// Renames one of a copy's directories, reporting the failure as that copy's.
fn rename(from: &Path, to: &Path, profile: &str) -> Result<(), BrowserDataError> {
    std::fs::rename(from, to).map_err(|source| BrowserDataError::Replace {
        profile: profile.to_string(),
        path: to.to_path_buf(),
        source,
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CopyOwner {
    format: String,
    operation: uuid::Uuid,
    destination: PathBuf,
}

fn ownership(to: &Path) -> PathBuf {
    beside(to, "copy-owner.json")
}

fn footprint(to: &Path) -> Vec<PathBuf> {
    [to.to_path_buf(), staged(to), displaced(to), ownership(to)]
        .into_iter()
        .map(|path| resolve(&path))
        .collect()
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a == b || contains(a, b) || contains(b, a)
}

fn claim(profile: &str, to: &Path) -> Result<(), BrowserDataError> {
    let path = ownership(to);
    let write = || -> io::Result<()> {
        use std::io::Write;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let record = CopyOwner {
            format: "fp-browser-copy-v1".into(),
            operation: uuid::Uuid::new_v4(),
            destination: to.to_path_buf(),
        };
        file.write_all(&serde_json::to_vec(&record)?)?;
        file.sync_all()
    };
    write().map_err(|source| BrowserDataError::Replace {
        profile: profile.to_string(),
        path,
        source,
    })
}

fn remove_any(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Whether anything at all is at this path, including a link that points nowhere.
fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// A path beside `path`, named after it so that a leftover is recognizable.
fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    path.with_file_name(name)
}

/// Why a browser-data copy could not be taken.
#[derive(Debug, Error)]
pub enum BrowserDataError {
    #[error(
        "copy leftovers at {path} have no valid ownership record; preserved for manual recovery"
    )]
    Unowned { path: PathBuf },
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

    #[error("could not copy {profile}'s browser data from {from} to {to}: {source}")]
    Copy {
        profile: String,
        from: PathBuf,
        to: PathBuf,
        source: io::Error,
    },

    /// The finished copy could not be put in place of what was there.
    ///
    /// The old directory is put back before this is reported, so the destination
    /// is either the copy or what it was - never neither.
    #[error("could not put the copied browser data in place at {path} for {profile}: {source}")]
    Replace {
        profile: String,
        path: PathBuf,
        source: io::Error,
    },
}

#[cfg(test)]
mod tests;
