//! The release machinery the two GitHub downloads share.
//!
//! Two features offer a list of published builds and fetch one of them: the
//! browser cores ([`crate::core_releases`]) and the Xray engine
//! ([`crate::xray_releases`]). They differ in what an asset means - a core is
//! chosen by system and kind of file, an engine by system and architecture - and
//! in what a finished download becomes, a row in the database against a path in
//! the configuration. Everything else they agree on: one request to the GitHub
//! API, a JSON document read into releases, one asset streamed to disk under a
//! `.part` name, and the small things a file name or a byte count needs on the
//! way. That agreement is this module, called rather than copied.
//!
//! What is deliberately not here is each feature's taxonomy: which assets a
//! release offers is [`Asset::read`]'s, and it is the two features that
//! implement it. The shapes at the end are shared for the same reason the
//! requests are - a job handed to a worker, the answer that comes back and the
//! two things a window keeps while a list is in flight are the same three things
//! on both sides, and only the asset type inside them differs.
//!
//! Nothing here dials at test time. The two agents a real request needs are
//! built by [`catalog_agent`] and [`download_agent`], and each feature's own
//! traits are what the window is handed fakes for.

use crate::text::Text;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The API root one release list is read from.
const API: &str = "https://api.github.com";

/// How many releases one fetch asks for.
///
/// The page lists versions to choose between, and a reader picking a build wants
/// the recent ones. Thirty is more than a Chromium major has releases in a year,
/// so the one page is the whole choice rather than the first page of one.
const RELEASES_PER_PAGE: usize = 30;

/// How long one release list may take.
const CATALOG_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a download may take to connect.
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a download may wait for the response headers before giving up.
const DOWNLOAD_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the whole body of a download may take.
///
/// A budget rather than a bound on a stalled connection, which is what this was
/// written as and what the HTTP client cannot express: `timeout_recv_body` is
/// the total time for the body, and its budget is not restarted by a read that
/// arrives. So it is deliberately generous - a core is a hundred megabytes and
/// an engine twenty, and both finish on a line faster than about 75 KB/s - and
/// what it still catches is a transfer that has effectively stopped. The failure
/// it exists to prevent is the one a reader on a slow line used to get instead:
/// a download reported as failed while it was still arriving.
const DOWNLOAD_BODY_BUDGET: Duration = Duration::from_secs(30 * 60);

/// The buffer one download is copied through.
const COPY_BUFFER: usize = 64 * 1024;

/// How many bytes arrive between two progress reports.
///
/// A report is a channel message and a repaint; one per read would be three
/// thousand of them for one core. This is small enough that the bar moves
/// steadily and large enough that the window is not redrawn for every packet.
const REPORT_EVERY: u64 = 1024 * 1024;

/// How the client names itself to GitHub.
///
/// The API refuses a request that carries no `User-Agent`, and one that says
/// what it is makes this program's requests legible in whatever the reader has
/// to look at if the repository ever wonders who is calling it.
fn user_agent() -> String {
    format!("{}/{}", crate::version::NAME, crate::version::VERSION)
}

/// An agent for the one request a release list needs.
pub(crate) fn catalog_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(user_agent())
        // One whole-request deadline, because a list that has not arrived in
        // half a minute is a list the reader has stopped waiting for. A
        // download cannot use this: see [`download_agent`].
        .timeout_global(Some(CATALOG_TIMEOUT))
        .build()
        .into()
}

/// An agent for a download.
pub(crate) fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(user_agent())
        // No whole-request deadline: a core is a hundred megabytes, and a
        // deadline that a slow line trips would report a working download as a
        // failure. What is bounded instead is each step that could hang -
        // connecting, waiting for the headers, and waiting for bytes that never
        // come.
        .timeout_connect(Some(DOWNLOAD_CONNECT_TIMEOUT))
        .timeout_recv_response(Some(DOWNLOAD_RESPONSE_TIMEOUT))
        .timeout_recv_body(Some(DOWNLOAD_BODY_BUDGET))
        .build()
        .into()
}

/// Asks a repository what it has published, as the API's own document.
///
/// Split from the reading so the shape the reading depends on is tested against
/// a document rather than against the network - which is also what makes a
/// change in GitHub's answer a failing test instead of a dialog that lists
/// nothing.
pub(crate) fn fetch_release_list(agent: &ureq::Agent, repository: &str) -> Result<String, String> {
    let url = format!("{API}/repos/{repository}/releases?per_page={RELEASES_PER_PAGE}");
    let response = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| format!("GitHub did not answer: {error}"))?;
    response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("the reply could not be read: {error}"))
}

/// A byte count as the window writes it: `181 MB`.
///
/// Binary units with the decimal spelling, which is what the file managers on
/// the three systems this runs on show, so a size here matches the one in the
/// folder it lands in. Language-neutral, and therefore not in the text table.
pub fn size_label(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.0} {}", UNITS[unit])
}

/// One asset as the API writes it.
///
/// The API's own field names, and its own idea of a size (absent means zero), in
/// one place: a change in GitHub's answer is then a change here rather than in
/// each feature's reader.
#[derive(Deserialize)]
pub(crate) struct RawAsset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(rename = "browser_download_url")]
    pub url: String,
}

/// What the two published-asset kinds have in common.
///
/// Each is read out of the API's fields - which is where the two taxonomies
/// part, and why [`read`](Asset::read) is the one method a feature has to
/// think about - and each carries the three facts a fetch needs. Everything
/// else about an asset is the feature's own.
pub(crate) trait Asset: Sized {
    /// Reads one asset, or `None` for a file name this program has nothing to
    /// run from.
    fn read(raw: RawAsset) -> Option<Self>;

    /// The file name the release gave it, which is what it is downloaded as.
    fn name(&self) -> &str;

    /// Where the asset is fetched from.
    fn url(&self) -> &str;

    /// The size GitHub reported, or `0` when it reported none.
    fn size(&self) -> u64;
}

/// One published version, with the assets it offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release<A> {
    /// The tag the release is published under. For a core it is the version the
    /// build reports: `148.0.7778.215`.
    pub tag: String,
    /// The day it was published, as `YYYY-MM-DD`, or empty when GitHub gave no
    /// date.
    pub published: String,
    pub prerelease: bool,
    pub assets: Vec<A>,
}

/// One asset, and where its release is being put.
pub struct DownloadJob<A> {
    /// The table the job's own failure sentences come from; see
    /// [`crate::maintenance`], which is the same arrangement for the same
    /// reason - a sentence built on a worker thread must already be in the
    /// reader's language.
    pub text: &'static Text,
    pub release: String,
    pub asset: A,
    /// The release's own directory. The file lands in it, and whatever unpacking
    /// produces goes under it too.
    pub directory: PathBuf,
}

/// What a fetch left: the file where it will stay, and the digest of what
/// arrived when one was asked for.
pub(crate) struct Fetched {
    /// The file, named as the release named it.
    pub file: PathBuf,
    /// The SHA-256 of the file as it arrived, lower-case hex. `None` when the
    /// caller did not ask for one.
    pub sha256: Option<String>,
}

/// Reads one release list out of the API's JSON.
///
/// The whole walk, so a change in the document is one change: the draft filter,
/// the day the publication date is cut down to, and the rule that a release this
/// program has nothing to run from is not a choice - listing it would be a row
/// of buttons that are all absent.
pub(crate) fn read_releases<A: Asset>(json: &str) -> Result<Vec<Release<A>>, String> {
    #[derive(Deserialize)]
    struct RawRelease {
        tag_name: String,
        #[serde(default)]
        published_at: Option<String>,
        #[serde(default)]
        prerelease: bool,
        #[serde(default)]
        draft: bool,
        #[serde(default)]
        assets: Vec<RawAsset>,
    }

    let raw: Vec<RawRelease> = serde_json::from_str(json)
        .map_err(|error| format!("the reply was not the release list this expects: {error}"))?;

    Ok(raw
        .into_iter()
        // A draft is not published, and the API can hand one to a caller that is
        // allowed to see it; a pre-release is listed, because the tag says so.
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let RawRelease {
                tag_name,
                published_at,
                prerelease,
                draft: _,
                assets,
            } = release;
            let assets: Vec<A> = assets.into_iter().filter_map(A::read).collect();
            if assets.is_empty() {
                return None;
            }
            Some(Release {
                tag: tag_name,
                published: published_day(published_at.as_deref()),
                prerelease,
                assets,
            })
        })
        .collect())
}

/// Fetches one asset into its release's directory and renames it into place.
///
/// The file is written under a `.part` name and renamed at the end, so a run
/// that is interrupted leaves a `.part` rather than a half-written file that
/// looks like what it was meant to become. `hash` asks for the digest of what
/// arrived as it is read: the engine records it beside the binary and the core
/// has nowhere to put it.
pub(crate) fn fetch_asset<A: Asset>(
    agent: &ureq::Agent,
    job: &DownloadJob<A>,
    progress: &mut dyn FnMut(u64, u64),
    hash: bool,
) -> Result<Fetched, String> {
    let t = job.text;
    fs::create_dir_all(&job.directory).map_err(|error| {
        t.release_disk_failed(&job.directory.display().to_string(), &error.to_string())
    })?;

    let file = job.directory.join(safe_file_name(job.asset.name()));
    let partial = partial_path(&file);
    let sha256 = stream_into(agent, job, &partial, progress, hash, t)?;
    // Windows refuses a rename onto a name that is taken, and a reader who
    // downloads the same release twice is not making a mistake.
    let _ = fs::remove_file(&file);
    fs::rename(&partial, &file)
        .map_err(|error| t.release_disk_failed(&file.display().to_string(), &error.to_string()))?;
    Ok(Fetched { file, sha256 })
}

/// Streams one asset into `partial`, reporting as it goes.
///
/// Returns the digest of what arrived when one was asked for. The file is
/// complete when this returns `Ok`; the caller renames it.
fn stream_into<A: Asset>(
    agent: &ureq::Agent,
    job: &DownloadJob<A>,
    partial: &Path,
    progress: &mut dyn FnMut(u64, u64),
    hash: bool,
    t: &Text,
) -> Result<Option<String>, String> {
    let asset = &job.asset;
    let response = agent
        .get(asset.url())
        .call()
        .map_err(|error| t.release_request_failed(asset.name(), &error.to_string()))?;
    // The API's own number is the fallback: the asset list carries a size
    // for every file, and a redirect or a proxy may not repeat it.
    let total = response.body().content_length().unwrap_or(asset.size());
    let mut reader = response.into_body().into_reader();
    let mut writer = BufWriter::new(File::create(partial).map_err(|error| {
        t.release_disk_failed(&partial.display().to_string(), &error.to_string())
    })?);

    let mut hasher = hash.then(Sha256::new);
    let mut buffer = vec![0u8; COPY_BUFFER];
    let mut received: u64 = 0;
    let mut reported: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| t.release_request_failed(asset.name(), &error.to_string()))?;
        if read == 0 {
            break;
        }
        if let Some(hasher) = &mut hasher {
            hasher.update(&buffer[..read]);
        }
        writer.write_all(&buffer[..read]).map_err(|error| {
            t.release_disk_failed(&partial.display().to_string(), &error.to_string())
        })?;
        received += read as u64;
        if received - reported >= REPORT_EVERY {
            reported = received;
            progress(received, total);
        }
    }
    writer.flush().map_err(|error| {
        t.release_disk_failed(&partial.display().to_string(), &error.to_string())
    })?;
    drop(writer);
    // The last report is sent whatever the size: a progress bar that stops
    // one report short of the end is a bar that looks stuck.
    progress(received, total);
    Ok(hasher.map(|hasher| format!("{:x}", hasher.finalize())))
}

/// How one asset of one release is named to the window.
///
/// The key a progress report belongs to: a row is a version and a file, and two
/// releases can carry assets with the same name.
pub fn asset_key(release: &str, asset: &str) -> String {
    format!("{release}/{asset}")
}

/// A name that is safe to create in the download's own directory.
///
/// The asset name comes out of a JSON document, so it is data rather than a
/// path this program chose: `../../.ssh/authorized_keys` is a legal asset name
/// and must not become a path here. Everything that is not a plain name
/// character becomes `_`, and a name that is left empty becomes `core`. The
/// release tag goes through the same rule where it names a directory.
pub(crate) fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect();
    // `.` and `..` survive the mapping and are not file names: one is this
    // directory and the other is its parent. Everything else keeps the name the
    // release gave it, so what is on disk is what the release called it.
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "core".to_string()
    } else {
        cleaned
    }
}

/// The `.part` name a download is written under until it is complete.
pub(crate) fn partial_path(file: &Path) -> PathBuf {
    let mut name = file
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    file.with_file_name(name)
}

/// Marks a file as runnable, on the systems that have the bit.
///
/// Best effort: an archive that carried the bit keeps it, and one that did not
/// is reported by the version probe that follows rather than here - which is
/// where the reader can still be told which file to run by hand.
pub(crate) fn make_runnable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o755);
            let _ = fs::set_permissions(path, permissions);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// The publication date, as `YYYY-MM-DD`.
///
/// The API sends an RFC 3339 instant and the window shows a day. The hour is
/// deliberately dropped: it is in UTC, the reader is not, and a time that has to
/// be converted before it means anything is worse than the date that does not.
fn published_day(at: Option<&str>) -> String {
    at.map(|at| at.chars().take(10).collect())
        .unwrap_or_default()
}

/// What a worker reported back to the window.
///
/// One enum for the three answers, so a fourth cannot report somewhere the
/// window does not hear it - the same reason [`crate::maintenance::Outcome`] is
/// one type. `A` is the release as the list carries it and `D` is what the
/// download leaves on disk, which is the one part the two features do not share.
pub enum ReleaseEvent<A, D> {
    /// The list came back, newest first.
    Listed(Result<Vec<A>, String>),
    /// How much of one asset has arrived.
    Progress {
        key: String,
        received: u64,
        total: u64,
    },
    /// One asset finished, with what unpacking left behind.
    Downloaded {
        release: String,
        result: Result<D, String>,
    },
}

/// What the window knows about the published releases of one repository.
#[derive(Debug, Clone)]
pub enum ReleaseStatus<A> {
    /// Nothing has been asked for yet. The dialog asks when it is opened.
    Unasked,
    /// A list is on its way.
    Loading,
    /// What the last fetch returned, newest first.
    Listed(Vec<A>),
    /// Why the last fetch failed, already written for the reader.
    Failed(String),
}

impl<A> ReleaseStatus<A> {
    /// Takes what a fetch found, or why it failed.
    ///
    /// A failure is written by `failed` - each list has its own words for what
    /// could not be read - and the sentence is also handed back, because the
    /// caller shows it in the banner as well: the banner is what the reader sees
    /// if the dialog was closed while the fetch was out.
    pub fn finish_fetch(
        &mut self,
        result: Result<Vec<A>, String>,
        failed: impl FnOnce(&str) -> String,
    ) -> Option<String> {
        match result {
            Ok(releases) => {
                *self = Self::Listed(releases);
                None
            }
            Err(error) => {
                let message = failed(&error);
                *self = Self::Failed(message.clone());
                Some(message)
            }
        }
    }
}

/// The download in flight.
#[derive(Debug, Clone)]
pub struct Download {
    /// Which asset of which release: the row the progress belongs to.
    pub key: String,
    pub release: String,
    /// The asset's file name, for the sentences about it.
    pub asset: String,
    pub received: u64,
    pub total: u64,
}

impl Download {
    /// Records a download starting, or says why it cannot.
    ///
    /// One at a time, and refused rather than queued: a core is a third of a
    /// gigabyte, a row has one progress bar, and a queue behind a two-minute
    /// download is a click that appears to have been ignored. The refusal names
    /// what is already running, so the reader knows to wait rather than to try
    /// again.
    pub fn begin<A: Asset>(
        slot: &mut Option<Download>,
        t: &Text,
        release: &str,
        asset: &A,
    ) -> Result<Download, String> {
        if let Some(running) = slot {
            return Err(t.release_busy(&running.asset));
        }
        *slot = Some(Download {
            key: asset_key(release, asset.name()),
            release: release.to_string(),
            asset: asset.name().to_string(),
            received: 0,
            total: asset.size(),
        });
        Ok(slot.clone().expect("just recorded"))
    }

    /// Records how far it has got.
    ///
    /// An answer for an asset that is no longer the one in flight is dropped:
    /// the reader may have cancelled, and a stale report must not move a bar
    /// that belongs to something else. The total is only taken when the server
    /// gave one - the asset list's own size is not overwritten with a zero.
    pub fn update(&mut self, key: &str, received: u64, total: u64) {
        if self.key == key {
            self.received = received;
            if total > 0 {
                self.total = total;
            }
        }
    }

    /// How far it has got, when the server said how big the file is.
    ///
    /// A fraction of the total rather than a flag, and `None` rather than a
    /// made-up number when there is no total: a bar that invents its own length
    /// is a bar that has to be unlearned when the real one arrives.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.received as f32 / self.total as f32).clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_is_written_in_the_units_a_file_manager_uses() {
        assert_eq!(size_label(0), "0 B");
        assert_eq!(size_label(1023), "1023 B");
        assert_eq!(size_label(1024), "1 KB");
        assert_eq!(size_label(189_767_686), "181 MB");
        assert_eq!(size_label(2 * 1024 * 1024 * 1024), "2 GB");
    }

    /// The asset name is data out of a JSON document, not a path this program
    /// chose, and it must not be able to name a file outside the download's own
    /// directory.
    #[test]
    fn an_asset_name_cannot_describe_a_path() {
        assert_eq!(safe_file_name("core.zip"), "core.zip");
        assert_eq!(safe_file_name("a/b.zip"), "a_b.zip");
        assert_eq!(safe_file_name("..\\..\\x.zip"), ".._.._x.zip");
        assert_eq!(safe_file_name("../../x.zip"), ".._.._x.zip");
        assert_eq!(safe_file_name(".."), "core");
        assert_eq!(safe_file_name("."), "core");
        assert_eq!(safe_file_name(""), "core");
    }

    /// A `.part` file sits beside the file it will become, so the rename at the
    /// end of a download stays within one directory.
    #[test]
    fn a_partial_download_is_named_beside_its_file() {
        let file = PathBuf::from("/cores/148/x.tar.xz");
        assert_eq!(
            partial_path(&file),
            PathBuf::from("/cores/148/x.tar.xz.part")
        );
    }
}
