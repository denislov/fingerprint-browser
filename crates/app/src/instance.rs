//! One window per data directory.
//!
//! Two copies of the window would share the database, the runtime directory and
//! the profiles, and the second one's startup reclaim would read the first one's
//! session records, find its live children and stop them - a browser the user is
//! looking at, killed by a second launch. Packaging is what turned that from a
//! theoretical problem into an easy one to hit: a menu entry and a desktop
//! shortcut are two ways to launch a program that is already running.
//!
//! So a run takes a lock on the data directory before it opens the database, and
//! a run that cannot take it stops and says so.
//!
//! **The lock is the operating system's, not a file this program interprets.**
//! `flock` on Unix, `LockFileEx` on Windows; both are released when the process
//! ends, however it ends - a clean exit, a panic, a `SIGKILL`, a crash the
//! debugger catches. That is the whole reason this is not a pid file: there is no
//! stale lock to age out, no pid to trust, and no identity check to get wrong.
//!
//! What the holder writes into the locked file is for the run it refuses, and for
//! whoever reads a diagnostics report: nothing here decides anything from it. A
//! refusal that cannot read the line still refuses - being unable to say *who*
//! holds the directory is not a reason to hand it to a second run.

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The file a run locks, in the data directory it locks.
pub const LOCK_FILE: &str = "instance.lock";

/// Who holds the lock, as the run being refused reads it back.
///
/// Written for a person, not for a decision: the pid is what a task manager can
/// be searched for, and the build is what a bug report is asked for first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    /// The build, in the spelling `--version` prints.
    pub build: String,
    /// Milliseconds since the Unix epoch, kept as a number like the session
    /// records do, so the file reads the same everywhere.
    pub started_at: u64,
}

/// Why a run could not take the lock.
#[derive(Debug)]
pub enum Busy {
    /// Another run holds it, with what that run wrote about itself where the line
    /// could be read.
    Held(Option<Holder>),
    /// The lock file could not be opened, which is a fact about the data
    /// directory rather than about another run.
    Unavailable(String),
}

/// A held lock. Dropping it releases the directory, and so does the process
/// ending without dropping it.
#[derive(Debug)]
pub struct InstanceLock {
    file: File,
}

impl InstanceLock {
    /// Takes the lock on `data_dir`, or reports who holds it.
    ///
    /// The directory is created if it is not there, because this runs before
    /// storage opens and a first run has no data directory yet.
    pub fn acquire(data_dir: &Path) -> Result<Self, Busy> {
        std::fs::create_dir_all(data_dir).map_err(unavailable)?;

        let path = data_dir.join(LOCK_FILE);
        let mut file = open(&path).map_err(unavailable)?;

        match take(&file) {
            // Held by another run. Read the line it wrote before giving up, so
            // the refusal can name it where that is possible.
            Ok(false) => return Err(Busy::Held(holder(&path))),
            Ok(true) => {}
            Err(error) => return Err(unavailable(error)),
        }

        // Only the holder writes here, so this replaces a previous run's line
        // rather than racing it. Failing to describe ourselves is not a reason to
        // refuse a run that has already won the directory.
        let me = Holder {
            pid: std::process::id(),
            build: crate::version::line(),
            started_at: now_millis(),
        };
        if let Err(error) = describe(&mut file, &me) {
            tracing::warn!("could not write {path:?}: {error}");
        }

        Ok(Self { file })
    }

    /// The file this lock is held on, for the diagnostics report.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(LOCK_FILE)
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Closing the file would release it too. Saying so is the point: the
        // release is the kernel's on every path, not this destructor's.
        let _ = release(&self.file);
    }
}

fn open(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // The same mode as the rest of what this program keeps in the data
        // directory. The line holds no secret, and a file that is not the user's
        // alone is still the wrong default.
        options.mode(0o600);
    }
    options.open(path)
}

fn describe(file: &mut File, holder: &Holder) -> std::io::Result<()> {
    let line =
        serde_json::to_vec(holder).map_err(|error| std::io::Error::other(error.to_string()))?;
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(&line)?;
    file.flush()
}

/// The holder's line, or `None` when it is absent, empty or not what this
/// program writes. An unreadable line is not a reason to refuse less: the lock
/// has already said the directory is taken.
fn holder(path: &Path) -> Option<Holder> {
    let mut bytes = Vec::new();
    File::open(path).ok()?.read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn unavailable(error: std::io::Error) -> Busy {
    Busy::Unavailable(error.to_string())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Takes the lock without waiting for it. `Ok(false)` is "another run has it",
/// which is a different answer from an error and is reported differently.
#[cfg(unix)]
fn take(file: &File) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;

    // SAFETY: the descriptor belongs to `file` and is open for the length of the
    // call; `flock` reads no memory.
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if taken == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    // Both names are the same number on Linux; `EWOULDBLOCK` is the one the
    // other Unixes spell it, and this asks for a non-blocking lock either way.
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn release(file: &File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;

    // SAFETY: as in `take`.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// The byte the Windows lock is taken on.
///
/// A Windows byte-range lock is mandatory - it stops *reads* as well as writes -
/// so locking byte 0 would stop the refused run reading the holder's line, and
/// the refusal is more useful when it can say who holds the directory. The range
/// is far past anything a JSON line reaches, and it is the same range in both
/// processes because it is named once, here.
#[cfg(windows)]
const LOCK_OFFSET: i64 = 1 << 20;

#[cfg(windows)]
fn overlapped() -> windows_sys::Win32::System::IO::OVERLAPPED {
    let mut overlapped = windows_sys::Win32::System::IO::OVERLAPPED::default();
    // SAFETY: the union's other arm is a pointer this call does not read; the
    // offset is written as the two halves the API takes.
    unsafe {
        overlapped.Anonymous.Anonymous.Offset = LOCK_OFFSET as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (LOCK_OFFSET >> 32) as u32;
    }
    overlapped
}

#[cfg(windows)]
fn take(file: &File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };

    let mut overlapped = overlapped();
    // SAFETY: the handle belongs to `file` and is open for the length of the
    // call; `overlapped` outlives it and is not read after.
    let taken = unsafe {
        LockFileEx(
            file.as_raw_handle() as HANDLE,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if taken != 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(windows)]
fn release(file: &File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;

    let mut overlapped = overlapped();
    // SAFETY: as in `take`.
    let released =
        unsafe { UnlockFileEx(file.as_raw_handle() as HANDLE, 0, 1, 0, &mut overlapped) };
    if released != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// The platforms this program ships on are both covered above. Any other would
/// have no lock rather than a wrong one, and is named here so that is a decision
/// rather than a compile error in a module nobody expected to reach.
#[cfg(not(any(unix, windows)))]
fn take(_file: &File) -> std::io::Result<bool> {
    Ok(true)
}

#[cfg(not(any(unix, windows)))]
fn release(_file: &File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A data directory of its own, removed when the test ends.
    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            // The tests in one binary run in parallel, and two of them taking the
            // same path would be testing each other's lock.
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "fp-instance-{name}-{}-{sequence}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self { dir }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn the_first_run_takes_the_directory() {
        let fixture = Fixture::new("first");
        let lock = InstanceLock::acquire(&fixture.dir).expect("nothing holds it yet");
        assert!(InstanceLock::path(&fixture.dir).is_file());
        // And it says who it is, so a refusal can name it.
        let holder = holder(&InstanceLock::path(&fixture.dir)).expect("the holder's line");
        assert_eq!(holder.pid, std::process::id());
        assert!(holder.build.contains(crate::version::VERSION), "{holder:?}");
        drop(lock);
    }

    /// The case the module exists for.
    #[test]
    fn a_second_run_is_refused_and_told_who_holds_the_directory() {
        let fixture = Fixture::new("second");
        let _held = InstanceLock::acquire(&fixture.dir).expect("the first run");

        match InstanceLock::acquire(&fixture.dir) {
            Err(Busy::Held(Some(holder))) => {
                assert_eq!(holder.pid, std::process::id(), "the line names the holder");
            }
            other => panic!("a second run must be refused: {other:?}"),
        }
    }

    /// Releasing has to make the directory available again, or closing and
    /// reopening the window would be impossible.
    #[test]
    fn dropping_the_lock_lets_the_next_run_in() {
        let fixture = Fixture::new("release");
        let first = InstanceLock::acquire(&fixture.dir).expect("the first run");
        drop(first);

        let second = InstanceLock::acquire(&fixture.dir).expect("the lock was released");
        // And the line now describes the second run rather than the first.
        let holder = holder(&InstanceLock::path(&fixture.dir)).expect("the holder's line");
        assert_eq!(holder.pid, std::process::id());
        drop(second);
    }

    /// A line that cannot be read still refuses. The lock is the answer to "is
    /// the directory taken"; the line is only the answer to "by whom".
    #[test]
    fn a_refusal_survives_a_line_that_cannot_be_read() {
        let fixture = Fixture::new("unreadable");
        let held = InstanceLock::acquire(&fixture.dir).expect("the first run");
        let path = InstanceLock::path(&fixture.dir);

        // Overwritten without truncating: what matters is that the bytes are not
        // the document this module writes, and a shorter write over a longer one
        // is that as surely as a truncation is.
        let mut file = OpenOptions::new().write(true).open(&path).expect("open");
        file.write_all(b"not json").expect("clobber the line");
        file.flush().expect("flush");
        drop(file);

        match InstanceLock::acquire(&fixture.dir) {
            Err(Busy::Held(None)) => {}
            other => panic!("an unreadable line must still refuse: {other:?}"),
        }
        drop(held);
    }

    /// The lock is the kernel's, so the process that holds it ending is what
    /// releases it - including when nothing gets to run afterwards. A lock file
    /// this program had to clean up would be one a crash could strand.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_holder_that_is_killed_releases_the_lock() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        /// Holds the same `flock(2)` lock this module takes, and says so, so the
        /// refusal below is about the lock rather than about a race with an
        /// interpreter's startup.
        const HOLDER: &str = "import fcntl, sys, time\n\
             f = open(sys.argv[1], 'a+')\n\
             fcntl.flock(f, fcntl.LOCK_EX)\n\
             print('locked', flush=True)\n\
             time.sleep(60)\n";

        let fixture = Fixture::new("killed");
        std::fs::create_dir_all(&fixture.dir).expect("data dir");
        let path = InstanceLock::path(&fixture.dir);

        // Skipped rather than failed where python3 is missing: it is this test's
        // tool, not the program's, and the runtime lifecycle tests already ask
        // the same of the machine.
        let Ok(mut holder) = Command::new("python3")
            .arg("-c")
            .arg(HOLDER)
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            eprintln!("python3 is not installed; skipping");
            return;
        };

        let mut handshake = String::new();
        BufReader::new(holder.stdout.take().expect("piped"))
            .read_line(&mut handshake)
            .expect("the holder says it holds the lock");
        assert_eq!(handshake.trim(), "locked", "the holder never took the lock");

        assert!(
            matches!(InstanceLock::acquire(&fixture.dir), Err(Busy::Held(_))),
            "another process's lock must refuse this one"
        );

        holder.kill().expect("kill the holder");
        holder.wait().expect("reap the holder");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(lock) = InstanceLock::acquire(&fixture.dir) {
                drop(lock);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the kernel releases the lock when the holder dies"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The lock file is the user's, like everything else in the data directory.
    #[cfg(unix)]
    #[test]
    fn the_lock_file_is_not_readable_by_everyone() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new("mode");
        let lock = InstanceLock::acquire(&fixture.dir).expect("take it");
        let mode = std::fs::metadata(InstanceLock::path(&fixture.dir))
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
        drop(lock);
    }
}
