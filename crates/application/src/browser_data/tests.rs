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

    let report =
        copy_browser_data(&[bob, alice], &HashSet::new(), &scratch.join("backup")).expect("copy");

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

/// The copy is built whole beside its destination and only then renamed over
/// it. A source that cannot be read partway through - here a link to nothing -
/// must therefore leave the directory that was already there exactly as it
/// was, rather than a destination that was cleared and refilled by halves.
#[cfg(unix)]
#[test]
fn a_copy_that_fails_leaves_the_directory_that_was_there() {
    let scratch = Scratch::new("failed");
    let profile = profile(&scratch.join("data/profiles/one"), "Work laptop");
    seed_profile(&profile.user_data_dir, "current");
    std::os::unix::fs::symlink("nowhere", profile.user_data_dir.join("broken"))
        .expect("a link that resolves to nothing");
    let backup = scratch.join("backup");
    let held = backup.join("profiles").join(profile.id.to_string());
    std::fs::create_dir_all(&held).expect("previous backup");
    std::fs::write(held.join("Local State"), b"last week").expect("write");

    let error = copy_browser_data(std::slice::from_ref(&profile), &HashSet::new(), &backup)
        .expect_err("the link cannot be read");

    assert!(matches!(error, BrowserDataError::Copy { .. }), "{error:?}");
    assert_eq!(
        std::fs::read(held.join("Local State")).expect("read"),
        b"last week",
        "the previous backup is exactly what it was"
    );
    assert!(
        !staged(&held).exists(),
        "the unfinished copy was not left behind"
    );
}

/// The other half of that: a good copy leaves no staging directory and no
/// displaced one, so the next run has nothing to interpret.
#[test]
fn a_copy_that_succeeds_leaves_nothing_beside_the_destination() {
    let scratch = Scratch::new("clean");
    let profile = profile(&scratch.join("data/profiles/one"), "Work laptop");
    seed_profile(&profile.user_data_dir, "current");
    let backup = scratch.join("backup");
    let held = backup.join("profiles").join(profile.id.to_string());
    std::fs::create_dir_all(&held).expect("previous backup");
    std::fs::write(held.join("Stale File"), b"old").expect("write");

    copy_browser_data(&[profile], &HashSet::new(), &backup).expect("copy");

    assert!(!staged(&held).exists(), "no staging directory is left");
    assert!(!displaced(&held).exists(), "no displaced directory is left");
}

/// A run killed between the two renames of a swap leaves the destination
/// missing and the only copy of its data under `.old`. The next run puts that
/// back before it does anything else - which is what makes a failed copy after
/// it harmless rather than fatal.
#[cfg(unix)]
#[test]
fn a_destination_an_interrupted_swap_left_aside_is_put_back() {
    let scratch = Scratch::new("interrupted");
    let profile = profile(&scratch.join("data/profiles/one"), "Work laptop");
    seed_profile(&profile.user_data_dir, "current");
    // The copy about to run cannot finish, so what the destination holds
    // afterwards is exactly what the recovery put there.
    std::os::unix::fs::symlink("nowhere", profile.user_data_dir.join("broken"))
        .expect("a link that resolves to nothing");
    let backup = scratch.join("backup");
    let held = backup.join("profiles").join(profile.id.to_string());
    std::fs::create_dir_all(&held).expect("previous backup");
    std::fs::write(held.join("Local State"), b"last week").expect("write");
    claim(&profile.name, &held).expect("ownership");
    // The window a kill can land in: renamed aside, not yet renamed back.
    std::fs::rename(&held, displaced(&held)).expect("rename aside");
    std::fs::create_dir_all(staged(&held)).expect("unfinished copy");

    let error = copy_browser_data(std::slice::from_ref(&profile), &HashSet::new(), &backup)
        .expect_err("the copy cannot read the link");

    assert!(matches!(error, BrowserDataError::Copy { .. }), "{error:?}");
    assert_eq!(
        std::fs::read(held.join("Local State")).expect("read"),
        b"last week",
        "the directory the interrupted swap left aside was put back"
    );
    assert!(!displaced(&held).exists(), "and it is not a leftover now");
    assert!(!staged(&held).exists(), "nor is the unfinished copy");
}

/// The restart half: a profile whose own directory is missing while its `.old`
/// is beside it - a restore killed between the two renames - is put back and
/// named, before anything else can mistake the missing directory for data that
/// was never there.
#[test]
fn a_restore_left_aside_is_put_back_at_startup_and_named() {
    let scratch = Scratch::new("startup");
    let laptop = profile(&scratch.join("data/profiles/one"), "Work laptop");
    let directory = laptop.user_data_dir.clone();
    std::fs::create_dir_all(directory.parent().expect("profiles directory")).expect("create");
    seed_profile(&directory, "restored");
    claim(&laptop.name, &directory).expect("ownership");
    std::fs::rename(&directory, displaced(&directory)).expect("rename aside");

    let put_back = recover_user_data_dirs(std::slice::from_ref(&laptop));

    assert_eq!(put_back, vec!["Work laptop".to_string()]);
    assert_eq!(
        std::fs::read(directory.join("Local State")).expect("read"),
        b"restored",
        "the profile's data is back where it belongs"
    );
    assert!(!displaced(&directory).exists());

    // And a profile with nothing to put back is not named.
    let quiet = profile(&scratch.join("data/profiles/two"), "Fresh");
    assert!(recover_user_data_dirs(std::slice::from_ref(&quiet)).is_empty());
}

#[test]
fn reserved_siblings_cannot_be_a_backup_source() {
    for suffix in [STAGING, DISPLACED] {
        let scratch = Scratch::new(&format!("reserved-{suffix}"));
        let mut profile = profile(&scratch.join("placeholder"), "Work");
        let backup = scratch.join("backup");
        let target = backup.join("profiles").join(profile.id.to_string());
        profile.user_data_dir = beside(&target, suffix);
        seed_profile(&profile.user_data_dir, "irreplaceable");
        assert!(
            copy_browser_data(std::slice::from_ref(&profile), &HashSet::new(), &backup).is_err()
        );
        assert_eq!(
            std::fs::read(profile.user_data_dir.join("Local State")).unwrap(),
            b"irreplaceable"
        );
        assert!(!target.exists());
    }
}

#[test]
fn unowned_siblings_are_preserved_during_copy_and_startup() {
    let scratch = Scratch::new("unowned");
    let profile = profile(&scratch.join("profile"), "Work");
    seed_profile(&profile.user_data_dir, "source");
    let backup = scratch.join("backup");
    let target = backup.join("profiles").join(profile.id.to_string());
    seed_profile(&staged(&target), "unrelated");
    assert!(matches!(
        copy_browser_data(std::slice::from_ref(&profile), &HashSet::new(), &backup),
        Err(BrowserDataError::Unowned { .. })
    ));
    assert_eq!(
        std::fs::read(staged(&target).join("Local State")).unwrap(),
        b"unrelated"
    );
    seed_profile(&displaced(&profile.user_data_dir), "other user's directory");
    assert!(recover_user_data_dirs(std::slice::from_ref(&profile)).is_empty());
    assert_eq!(
        std::fs::read(displaced(&profile.user_data_dir).join("Local State")).unwrap(),
        b"other user's directory"
    );
}

#[test]
fn recovery_does_not_delete_another_profiles_directory() {
    let scratch = Scratch::new("recovery-overlap");
    let first = profile(&scratch.join("first"), "First");
    seed_profile(&first.user_data_dir, "first");
    claim(&first.name, &first.user_data_dir).unwrap();
    let second = profile(&staged(&first.user_data_dir), "Second");
    seed_profile(&second.user_data_dir, "second");
    recover_user_data_dirs(&[first, second.clone()]);
    assert_eq!(
        std::fs::read(second.user_data_dir.join("Local State")).unwrap(),
        b"second"
    );
}
