//! What this installation is, written down for whoever has to debug it.
//!
//! The report answers the questions a bug report otherwise spends three
//! messages on - which build, which platform, where the files are, whether they
//! are there, and what the program last said - and it answers them from the
//! outside, the way a maintainer would ask for them.
//!
//! Two things it deliberately does **not** do:
//!
//! - It never opens the database. Proxy credentials live in it, and a report
//!   that is meant to be pasted into a bug report is the last place they should
//!   turn up; the file's size and mode are reported instead.
//! - It never copies the browser data. The directories are counted, not read.
//!
//! The one part that is not a fact about the machine is the end of the activity
//! log, which is the app's own account of what it did - and is therefore the
//! most useful part of the file. `diag_intro` says so, so nobody has to guess
//! before sharing it.

use crate::log_file;
use crate::settings::Settings;
use crate::text::Text;
use crate::version;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How much of the activity log a report carries.
///
/// Enough to hold the run that went wrong, short enough to read: the log is
/// capped at 500 lines in memory and two files on disk, so this is a slice of
/// one of them rather than everything.
pub const LOG_TAIL: usize = 40;

/// A section of the report: a heading and the lines under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    /// Complete lines, already rendered: a line is either `- label: value` or a
    /// line copied out of the log, and the difference is not worth a type.
    pub body: Vec<String>,
}

/// The report, rendered in the language that was in force when it was collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub title: String,
    pub intro: String,
    pub sections: Vec<Section>,
}

impl Report {
    /// The whole file, as markdown that also reads as plain text.
    pub fn render(&self) -> String {
        let mut out = format!("# {}\n\n{}\n", self.title, self.intro);
        for section in &self.sections {
            out.push_str(&format!("\n## {}\n\n", section.title));
            for line in &section.body {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}

/// Everything the report says about this installation.
pub fn collect(settings: &Settings, at: SystemTime, t: &Text) -> Report {
    Report {
        title: t.diag_title.to_string(),
        intro: t.diag_intro.to_string(),
        sections: vec![
            build_section(at, t),
            settings_section(settings, t),
            files_section(settings, t),
            log_section(settings, t),
        ],
    }
}

/// Collects the report and writes it to `destination`.
pub fn write_for(settings: &Settings, destination: &Path, t: &Text) -> Result<(), String> {
    write(&collect(settings, SystemTime::now(), t), destination, t)
}

/// Writes a rendered report, creating the directory it goes in.
pub fn write(report: &Report, destination: &Path, t: &Text) -> Result<(), String> {
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            t.diag_create_failed(&parent.display().to_string(), &error.to_string())
        })?;
    }
    fs::write(destination, report.render()).map_err(|error| {
        t.diag_write_failed(&destination.display().to_string(), &error.to_string())
    })
}

fn build_section(at: SystemTime, t: &Text) -> Section {
    Section {
        title: t.diag_build.to_string(),
        body: vec![
            line(t.diag_version, version::VERSION),
            line(t.diag_commit, version::COMMIT),
            line(t.diag_platform, &version::platform()),
            line(t.diag_report_time, &log_file::timestamp(at)),
        ],
    }
}

/// The Settings page's rows, which is the point: the report says what is in
/// force and where each value came from, so "I set it and nothing happened" is
/// answerable without a second round of questions.
fn settings_section(settings: &Settings, t: &Text) -> Section {
    let mut body = Vec::new();
    for row in settings.rows(t) {
        let mut value = format!("{} ({})", row.value, row.source_label(t));
        if let Some(shadowed) = row.shadowed_label(t) {
            value.push_str("; ");
            value.push_str(&shadowed);
        }
        body.push(line(row.key.label(t), &value));
    }
    Section {
        title: t.diag_settings.to_string(),
        body,
    }
}

fn files_section(settings: &Settings, t: &Text) -> Section {
    let data = settings.data_dir();
    // The same `<data dir>/profiles` the profile service creates
    // (`application::DefaultProfileService`): the report names the directory it
    // is about, and counts it rather than listing what is inside.
    let described: [(String, PathBuf); 9] = [
        (t.setting_data_dir.to_string(), data.to_path_buf()),
        (
            t.setting_config_file.to_string(),
            settings.config_path().to_path_buf(),
        ),
        (t.diag_database.to_string(), data.join("app.db")),
        // The file the lock is taken on. It outlives the run that made it - the
        // lock is the kernel's and goes with the process, while the file stays -
        // so its presence in a report says where the lock is, not that one is
        // held. See `instance`.
        (
            t.diag_instance_lock.to_string(),
            crate::instance::InstanceLock::path(data),
        ),
        (
            t.diag_activity_log.to_string(),
            data.join(log_file::LOG_DIR).join(log_file::LOG_FILE),
        ),
        (
            t.diag_activity_log_rotated.to_string(),
            data.join(log_file::LOG_DIR)
                .join(log_file::LOG_FILE)
                .with_extension("log.1"),
        ),
        (t.setting_runtime_dir.to_string(), settings.runtime_dir()),
        (t.browser_data_title.to_string(), data.join("profiles")),
        // Not a path of its own, but the answer to "is the engine even there" -
        // and the one file whose mode decides whether a launch can work at all.
        (
            t.setting_xray_executable.to_string(),
            settings.xray_executable().to_path_buf(),
        ),
    ];
    Section {
        title: t.diag_files.to_string(),
        body: described
            .into_iter()
            .map(|(label, path)| line(&label, &describe(&path, t)))
            .collect(),
    }
}

fn log_section(settings: &Settings, t: &Text) -> Section {
    let path = settings
        .data_dir()
        .join(log_file::LOG_DIR)
        .join(log_file::LOG_FILE);
    let title = t.diag_log.to_string();
    let body = match fs::read_to_string(&path) {
        Ok(text) => {
            let all: Vec<&str> = text.lines().collect();
            let shown = all.len().min(LOG_TAIL);
            let mut body = vec![t.diag_log_lines(shown, all.len())];
            body.extend(all[all.len() - shown..].iter().map(|line| line.to_string()));
            if shown == 0 {
                body.push(t.diag_log_empty.to_string());
            }
            body
        }
        // Unreadable and absent are the same answer here, and it is an answer:
        // a report that silently left the section out would look like a log
        // with nothing in it.
        Err(_) => vec![describe(&path, t)],
    };
    Section { title, body }
}

/// `- label: value`, the report's one line shape.
fn line(label: &str, value: &str) -> String {
    format!("- {label}: {value}")
}

/// What is at a path: a directory and how many entries, a file and how big, or
/// that it is not there.
///
/// A size rather than a content: the report is about the installation, and the
/// database is the one file in it that must not be quoted from.
fn describe(path: &Path, t: &Text) -> String {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            // A directory that cannot be listed is still a directory; the count
            // is the least important word in the line.
            let entries = fs::read_dir(path)
                .map(|entries| entries.count())
                .unwrap_or(0);
            with_mode(t.diag_directory(entries), &metadata, t)
        }
        Ok(metadata) => with_mode(t.diag_bytes(metadata.len()), &metadata, t),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => t.diag_missing.to_string(),
        Err(error) => t.diag_unreadable(&error.to_string()),
    }
}

fn with_mode(size: String, metadata: &fs::Metadata, t: &Text) -> String {
    match mode(metadata) {
        Some(mode) => format!("{size} ({})", t.diag_mode(&mode)),
        None => size,
    }
}

/// The permission bits, where the platform has them.
///
/// The four digits include set-uid and the sticky bit, because whether an
/// executable can be executed is exactly the question this line is asked.
#[cfg(unix)]
fn mode(metadata: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    Some(format!("{:04o}", metadata.permissions().mode() & 0o7777))
}

/// Windows has no mode bits; the ACL that would answer this properly is not a
/// line in a report.
#[cfg(not(unix))]
fn mode(_metadata: &fs::Metadata) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Environment, Settings};
    use crate::text::en;
    use std::time::{Duration, UNIX_EPOCH};

    /// An installation in a directory of its own, removed when the test ends.
    struct Fixture {
        dir: PathBuf,
        settings: Settings,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("fp-diagnostics-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            let (settings, notice) = Settings::load(Environment {
                data_dir: Some(dir.to_string_lossy().to_string()),
                config: Some(dir.join("config.json").to_string_lossy().to_string()),
                ..Environment::default()
            });
            assert!(notice.is_none(), "a fresh installation has nothing to say");
            Self { dir, settings }
        }

        fn render(&self) -> String {
            // One fixed instant, so the report's own timestamp is assertable.
            collect(
                &self.settings,
                UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                en(),
            )
            .render()
        }

        fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let path = self.dir.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent");
            }
            fs::write(&path, bytes).expect("write");
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_report_names_the_build_the_place_and_every_setting() {
        let fixture = Fixture::new("build");
        fixture.write(
            "config.json",
            br#"{"xray_executable": "/opt/xray", "theme": "light"}"#,
        );
        // Reloaded so the report describes the config file that is really there.
        let (settings, _) = Settings::load(Environment {
            data_dir: Some(fixture.dir.to_string_lossy().to_string()),
            config: Some(
                fixture
                    .dir
                    .join("config.json")
                    .to_string_lossy()
                    .to_string(),
            ),
            ..Environment::default()
        });
        let report = collect(&settings, UNIX_EPOCH, en()).render();

        let mut expected: Vec<String> = vec![
            version::VERSION.to_string(),
            version::COMMIT.to_string(),
            std::env::consts::OS.to_string(),
            "1970-01-01T00:00:00.000Z".to_string(),
            fixture.dir.display().to_string(),
            "/opt/xray".to_string(),
        ];
        // Every row the Settings page shows is in the file, with its source.
        expected.extend(
            settings
                .rows(en())
                .iter()
                .map(|row| row.key.label(en()).to_string()),
        );
        expected.push("from the config file".to_string());

        for expected in expected {
            assert!(
                report.contains(&expected),
                "{expected} is missing:\n{report}"
            );
        }
    }

    /// The question the report exists to answer: is the file actually there.
    #[test]
    fn a_file_that_is_not_there_is_reported_as_missing_and_then_as_a_size() {
        let fixture = Fixture::new("files");
        let database = format!("- {}: missing", en().diag_database);
        assert!(fixture.render().contains(&database), "{}", fixture.render());

        fixture.write("app.db", &[0u8; 100]);

        let report = fixture.render();
        assert!(
            report.contains(&format!("- {}: 100 bytes", en().diag_database)),
            "{report}"
        );
        assert!(
            !report.contains(&database),
            "the directory that now has a database is no longer reported missing:\n{report}"
        );
    }

    /// The end of the log is the app's own account of the failure, and the
    /// reason someone is asked for a report at all.
    #[test]
    fn a_report_carries_the_end_of_the_log_and_not_the_beginning() {
        let fixture = Fixture::new("log");
        let lines: String = (0..100)
            .map(|index| format!("entry-{index:03}\n"))
            .collect();
        fixture.write("logs/activity.log", lines.as_bytes());

        let report = fixture.render();
        assert!(report.contains("entry-099"), "{report}");
        assert!(
            !report.contains("entry-000"),
            "only the tail is carried:\n{report}"
        );
        assert!(report.contains("of 100 lines"), "{report}");
    }

    /// A report is meant to be pasted in public. The database is where the
    /// proxy credentials are, so it is measured and never read.
    #[test]
    fn a_report_never_carries_what_is_in_the_database() {
        let fixture = Fixture::new("secrets");
        let credentials = b"outbound password=hunter2";
        fixture.write("app.db", credentials);

        let report = fixture.render();
        assert!(!report.contains("hunter2"), "{report}");
        assert!(!report.contains("password"), "{report}");
        // What is reported is how big the file is, which is the whole of what a
        // maintainer can use from it without the credentials coming along.
        assert!(
            report.contains(&format!(
                "- {}: {} bytes",
                en().diag_database,
                credentials.len()
            )),
            "{report}"
        );
    }

    #[test]
    fn the_report_is_written_where_it_was_asked() {
        let fixture = Fixture::new("write");
        let destination = fixture.dir.join("reports/one.md");
        let report = collect(&fixture.settings, UNIX_EPOCH, en());

        write(&report, &destination, en()).expect("write");

        let written = fs::read_to_string(&destination).expect("read back");
        assert_eq!(written, report.render());
        // A directory that did not exist was created rather than refused.
        assert!(destination.parent().expect("parent").is_dir());
    }

    #[test]
    fn a_report_that_cannot_be_written_says_which_path() {
        let fixture = Fixture::new("blocked");
        // A file where the directory would have to go.
        let blocked = fixture.write("blocked", b"not a directory");
        let destination = blocked.join("report.md");

        let error = write(
            &collect(&fixture.settings, UNIX_EPOCH, en()),
            &destination,
            en(),
        )
        .expect_err("must fail");

        assert!(error.contains("blocked"), "{error}");
    }

    /// The permission bits are in the report because they are the difference
    /// between a config file only its owner can read and one the whole machine
    /// can - and because no other page says.
    #[cfg(unix)]
    #[test]
    fn the_mode_of_a_file_is_part_of_what_the_report_says() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new("mode");
        let path = fixture.write("config.json", br#"{"theme": "light"}"#);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");

        let report = fixture.render();
        assert!(report.contains("0600"), "{report}");
    }
}
