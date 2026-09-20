//! The activity log, written down.
//!
//! The Log page is a session view; this is the part of it that outlives the
//! window. Every line the page shows is appended to `activity.log` under the
//! data directory, and when that file passes [`MAX_LOG_BYTES`] it is rotated to
//! `activity.log.1`, which is replaced on the next rotation. Two files, bounded,
//! no per-run litter and nothing that grows for as long as the tool is used.
//!
//! Writing is best effort by design: a log that cannot be written must never
//! stop the window, but it must not be dropped in silence either, so the first
//! failure is kept and reported rather than repeated per line.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Directory the log lives in, inside the data directory.
pub const LOG_DIR: &str = "logs";
/// The file the current session appends to.
pub const LOG_FILE: &str = "activity.log";
/// Rotate once the current file passes this size: at most two files, 1 MiB.
pub const MAX_LOG_BYTES: u64 = 512 * 1024;

/// The window's activity log on disk.
#[derive(Debug)]
pub struct LogFile {
    path: PathBuf,
    written: u64,
    limit: u64,
}

impl LogFile {
    /// Opens the log in `dir`, creating the directory and rotating a full file.
    ///
    /// A directory that cannot be created or written is an error here, before
    /// the first line, so the window can say the log is not being kept instead
    /// of discovering it line by line.
    pub fn open(dir: &Path) -> Result<Self, String> {
        Self::open_with_limit(dir, MAX_LOG_BYTES)
    }

    pub fn open_with_limit(dir: &Path, limit: u64) -> Result<Self, String> {
        fs::create_dir_all(dir)
            .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
        let path = dir.join(LOG_FILE);
        let existing = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        if existing >= limit {
            rotate(&path)?;
        }
        let written = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        // Touch it now: an unwritable directory is reported at startup, not at
        // the first line someone needed.
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("could not open {}: {error}", path.display()))?;
        Ok(Self {
            path,
            written,
            limit,
        })
    }

    /// A sink at an exact path, for a test that drives the failure path.
    #[cfg(test)]
    pub(crate) fn at(path: PathBuf) -> Self {
        Self {
            path,
            written: 0,
            limit: MAX_LOG_BYTES,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one line, rotating first when the file is full.
    pub fn append(&mut self, line: &str) -> Result<(), String> {
        if self.written >= self.limit {
            rotate(&self.path)?;
            self.written = 0;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| format!("could not open {}: {error}", self.path.display()))?;
        file.write_all(line.as_bytes())
            .map_err(|error| format!("could not write {}: {error}", self.path.display()))?;
        self.written += line.len() as u64;
        Ok(())
    }
}

fn rotate(path: &Path) -> Result<(), String> {
    let rotated = path.with_extension("log.1");
    if rotated.exists() {
        fs::remove_file(&rotated)
            .map_err(|error| format!("could not remove {}: {error}", rotated.display()))?;
    }
    fs::rename(path, &rotated)
        .map_err(|error| format!("could not rotate {}: {error}", path.display()))
}

/// A line's timestamp as `2026-09-20T08:14:57.123Z`.
///
/// The page shows how long ago a line was written; a file that outlives the
/// session needs an absolute time, and computing UTC here keeps the crate free
/// of a date library.
pub fn timestamp(at: SystemTime) -> String {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = civil_from_unix(millis / 1000);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        millis % 1000
    )
}

/// Days since the Unix epoch to a civil date, and seconds to a clock time.
///
/// Howard Hinnant's `civil_from_days`, which is exact for every date this is
/// ever asked about.
fn civil_from_unix(seconds: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (hour, minute, second) = (
        (rest / 3_600) as u32,
        ((rest % 3_600) / 60) as u32,
        (rest % 60) as u32,
    );

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if month <= 2 { year + 1 } else { year };

    (year, month, day, hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fp-log-file-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_line_is_appended_and_readable_back() {
        let dir = temp_dir("append");
        let mut log = LogFile::open(&dir).expect("open");

        log.append("first line\n").expect("append");
        log.append("second line\n").expect("append");

        let text = fs::read_to_string(log.path()).expect("read back");
        assert_eq!(text, "first line\nsecond line\n");
        assert_eq!(log.path(), dir.join(LOG_FILE));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_file_is_appended_to_rather_than_replaced() {
        let dir = temp_dir("existing");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join(LOG_FILE), "from a previous run\n").expect("seed");

        let mut log = LogFile::open_with_limit(&dir, 1024).expect("open");
        log.append("this run\n").expect("append");

        let text = fs::read_to_string(log.path()).expect("read back");
        assert_eq!(text, "from a previous run\nthis run\n");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A long session must not grow a file without bound, and rotation must not
    /// lose the line that triggered it.
    #[test]
    fn a_full_file_is_rotated_and_the_new_line_survives() {
        let dir = temp_dir("rotate");
        let mut log = LogFile::open_with_limit(&dir, 20).expect("open");

        for index in 0..5 {
            log.append(&format!("line {index}\n")).expect("append");
        }

        let current = fs::read_to_string(log.path()).expect("current");
        let rotated = fs::read_to_string(log.path().with_extension("log.1")).expect("rotated");
        assert!(current.contains("line 4"), "{current}");
        assert!(rotated.contains("line 0"), "{rotated}");
        assert!(
            current.len() + rotated.len() < 64,
            "both files stay bounded"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_file_is_rotated_at_startup_too() {
        let dir = temp_dir("rotate-open");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join(LOG_FILE), "x".repeat(64)).expect("seed");

        LogFile::open_with_limit(&dir, 32).expect("open");

        assert!(dir.join(LOG_FILE).exists());
        assert!(dir.join(LOG_FILE).with_extension("log.1").exists());
        assert_eq!(
            fs::metadata(dir.join(LOG_FILE)).expect("stat").len(),
            0,
            "the new file starts empty"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_that_cannot_be_created_is_an_error() {
        let dir = temp_dir("unwritable");
        fs::create_dir_all(&dir).expect("dir");
        // A file where the directory should be.
        let blocked = dir.join("logs");
        fs::write(&blocked, "not a directory").expect("seed");

        let error = LogFile::open(&blocked).expect_err("must fail");
        assert!(error.contains("could not create"), "{error}");

        // And a sink pointed at a path that cannot be written reports the first
        // append rather than panicking.
        let mut sink = LogFile::at(blocked.join("activity.log"));
        let error = sink.append("line\n").expect_err("must fail");
        assert!(error.contains("could not open"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_timestamp_is_utc_and_reads_the_same_everywhere() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20.000Z"
        );
        // A leap day, which is where a hand-rolled calendar usually breaks.
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00.000Z"
        );
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_millis(1_700_000_000_123)),
            "2023-11-14T22:13:20.123Z"
        );
    }
}
