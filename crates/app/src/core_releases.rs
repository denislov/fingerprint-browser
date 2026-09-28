//! The browser cores published for this program, and fetching one.
//!
//! A core is a fingerprint-chromium build, and the builds this program is meant
//! to launch are published as GitHub releases by `adryfish/fingerprint-chromium`:
//! one release per Chromium version, each holding a Windows, a Linux and a macOS
//! asset. This module is the narrow half of that - asking GitHub what has been
//! published, and turning one asset into a directory with a browser binary in it.
//!
//! It sits below the window and above the disk, and knows about neither
//! [`crate::state`] nor GPUI. That is what makes the classification and the
//! unpacking testable without a network: the two traits exist so the window can
//! be handed a catalog and a downloader that never dial, the same way
//! [`crate::proxy_tester`] and [`crate::open_dir`] are handed theirs.
//!
//! Registering what came down is deliberately *not* here. The major a core
//! reports decides which fingerprint switches it may be asked to spoof, so the
//! reading has to come from the one place that probes a binary
//! ([`application::CoreService`]) rather than from the release tag - `148.0.7778.215`
//! is what the file is called, not necessarily what the binary inside answers.

use crate::text::Text;
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The repository the published cores come from.
///
/// One constant rather than the same `adryfish/fingerprint-chromium` written into
/// the request, the dialog's caption and a test: a fork or a mirror is then a
/// change in one place.
pub const REPOSITORY: &str = "adryfish/fingerprint-chromium";

/// The API root one release list is read from.
const API: &str = "https://api.github.com";

/// How many releases one fetch asks for.
///
/// The page lists versions to choose between, and a reader picking a core wants
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
/// Deliberately generous: a core is a hundred megabytes and a slow line is not a
/// failure. It is a whole-body budget and not the bound on a stalled connection
/// this was written as - `timeout_recv_body` is the total time for the body, and
/// its budget is not restarted by a read that arrives - so at two minutes a
/// hundred megabytes needed a line better than a megabyte a second, and a reader
/// below that was told a download had failed while it was still arriving. What
/// this still catches is a transfer that has effectively stopped, which is the
/// failure that would otherwise sit on the row forever.
const DOWNLOAD_BODY_BUDGET: Duration = Duration::from_secs(30 * 60);

/// The buffer one download is copied through.
const COPY_BUFFER: usize = 64 * 1024;

/// How many bytes arrive between two progress reports.
///
/// A report is a channel message and a repaint; one per read would be three
/// thousand of them for one core. This is small enough that the bar moves
/// steadily and large enough that the window is not redrawn for every packet.
const REPORT_EVERY: u64 = 1024 * 1024;

/// How deep the search for the browser binary goes.
///
/// A released archive is a directory or two with the browser at the top; this is
/// the bound that keeps a malformed archive with a symlink loop from turning the
/// search into a walk of the filesystem.
const SEARCH_DEPTH: usize = 8;

/// The directory an unpacked archive lands in, under the release's own.
pub const UNPACK_DIR: &str = "unpacked";

/// How the client names itself to GitHub.
///
/// The API refuses a request that carries no `User-Agent`, and one that says
/// what it is makes this program's requests legible in whatever the reader has
/// to look at if the repository ever wonders who is calling it.
pub(crate) fn user_agent() -> String {
    format!("{}/{}", crate::version::BRAND, crate::version::VERSION)
}

/// Which system a published asset is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

impl Platform {
    /// Every platform, in the order the dialog lists its buttons.
    pub const ALL: [Platform; 3] = [Platform::Windows, Platform::Linux, Platform::MacOs];

    /// The system this build is running on.
    ///
    /// `const` so it can be used where the table is built, and pure so the rows
    /// a test sees do not depend on the machine running it.
    pub const fn host() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// The name, as the release assets spell it.
    ///
    /// Not translated: `Windows`, `Linux` and `macOS` are what the systems are
    /// called in every language this window speaks, like `SOCKS5` and `Chrome`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
        }
    }

    /// The stable id a row's button is named by, so a test can find it without
    /// knowing which release it belongs to.
    pub fn id(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::MacOs => "macos",
        }
    }
}

/// What kind of file a published asset is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// A portable build: an archive that unpacks into a directory with a
    /// runnable browser in it.
    Portable,
    /// A Windows installer, which installs a browser rather than being one.
    Installer,
    /// A Linux AppImage: one file that runs where it stands, once it may.
    AppImage,
    /// A macOS disk image.
    DiskImage,
}

impl AssetKind {
    /// Whether downloading this is the whole of adding a core.
    ///
    /// True for the two kinds this program can put a browser binary on disk
    /// from by itself. An installer and a disk image are files the user runs;
    /// what they leave behind is somewhere this program was not told about.
    pub fn becomes_core(self) -> bool {
        matches!(self, Self::Portable | Self::AppImage)
    }

    /// Whether the file has to be unpacked before a browser is in it.
    pub fn unpacks(self) -> bool {
        matches!(self, Self::Portable)
    }
}

/// One downloadable file in one release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreAsset {
    pub name: String,
    pub url: String,
    /// The size GitHub reports, or `0` when it reported none.
    pub size: u64,
    pub platform: Platform,
    pub kind: AssetKind,
}

/// One published version, with the assets it offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreRelease {
    /// The tag, which is the version the build reports: `148.0.7778.215`.
    pub tag: String,
    /// The day it was published, as `YYYY-MM-DD`, or empty when GitHub gave no
    /// date.
    pub published: String,
    pub prerelease: bool,
    pub assets: Vec<CoreAsset>,
}

impl CoreRelease {
    /// The asset this platform would run, or `None` when the release has none.
    ///
    /// A portable build wins over the alternatives: it is the one this program
    /// can unpack and register by itself, and it is the one that does not
    /// install anything into the machine it runs on.
    pub fn for_platform(&self, platform: Platform) -> Option<&CoreAsset> {
        self.assets
            .iter()
            .filter(|asset| asset.platform == platform)
            .min_by_key(|asset| match asset.kind {
                AssetKind::Portable => 0,
                AssetKind::AppImage => 1,
                AssetKind::Installer => 2,
                AssetKind::DiskImage => 3,
            })
    }
}

/// How one asset of one release is named to the window.
///
/// The key a progress report belongs to: a row is a version and a file, and two
/// releases can carry assets with the same name.
pub fn asset_key(release: &str, asset: &str) -> String {
    format!("{release}/{asset}")
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

/// Reads what a repository has published.
pub trait ReleaseCatalog: Send + Sync {
    /// Every release with at least one asset this program understands, newest
    /// first, as GitHub orders them.
    fn list(&self) -> Result<Vec<CoreRelease>, String>;
}

/// The real catalog: one request to the GitHub API.
pub struct GithubReleaseCatalog {
    agent: ureq::Agent,
}

impl GithubReleaseCatalog {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent())
            // One whole-request deadline, because a list that has not arrived in
            // half a minute is a list the reader has stopped waiting for. A
            // download cannot use this: see [`HttpCoreDownloader`].
            .timeout_global(Some(CATALOG_TIMEOUT))
            .build()
            .into();
        Self { agent }
    }
}

impl Default for GithubReleaseCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ReleaseCatalog for GithubReleaseCatalog {
    fn list(&self) -> Result<Vec<CoreRelease>, String> {
        let url = format!("{API}/repos/{REPOSITORY}/releases?per_page={RELEASES_PER_PAGE}");
        let response = self
            .agent
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|error| format!("GitHub did not answer: {error}"))?;
        let json = response
            .into_body()
            .read_to_string()
            .map_err(|error| format!("the reply could not be read: {error}"))?;
        parse_releases(&json)
    }
}

/// One asset, and where its release is being put.
pub struct DownloadJob {
    /// The table the job's own failure sentences come from; see
    /// [`crate::maintenance`], which is the same arrangement for the same
    /// reason - a sentence built on a worker thread must already be in the
    /// reader's language.
    pub text: &'static Text,
    pub release: String,
    pub asset: CoreAsset,
    /// The release's directory. The file lands in it and an archive unpacks
    /// into [`UNPACK_DIR`] under it.
    pub directory: PathBuf,
}

/// What a finished download left on disk.
pub struct Downloaded {
    /// The file that was written.
    pub file: PathBuf,
    /// The directory an archive was unpacked into, when it was one.
    pub unpacked: Option<PathBuf>,
    /// The browser binary, when one was found. `None` means the download
    /// succeeded and this program could not find a browser in it - a disk
    /// image, an installer, or an archive whose layout is not the one this
    /// knows.
    pub executable: Option<PathBuf>,
}

/// Fetches one asset and unpacks it.
pub trait CoreDownloader: Send + Sync {
    /// Downloads `job.asset` into `job.directory`, unpacks it when it is an
    /// archive, and reports what is there now.
    ///
    /// `progress` is called with the bytes received and the total when the
    /// server gave one, `0` when it did not. It blocks, so callers run it off
    /// the thread that draws the window.
    fn download(
        &self,
        job: &DownloadJob,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Downloaded, String>;
}

/// The real downloader: an HTTPS fetch, then whichever unpacker the file needs.
pub struct HttpCoreDownloader {
    agent: ureq::Agent,
}

impl HttpCoreDownloader {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent())
            // No whole-request deadline: a core is a hundred megabytes, and a
            // deadline that a slow line trips would report a working download as
            // a failure. What is bounded instead is each step that could hang -
            // connecting, waiting for the headers, and waiting for bytes that
            // never come.
            .timeout_connect(Some(DOWNLOAD_CONNECT_TIMEOUT))
            .timeout_recv_response(Some(DOWNLOAD_RESPONSE_TIMEOUT))
            .timeout_recv_body(Some(DOWNLOAD_BODY_BUDGET))
            .build()
            .into();
        Self { agent }
    }

    /// Streams one URL into `partial`, reporting as it goes.
    ///
    /// Returns the bytes written. The file is complete when this returns `Ok`;
    /// the caller renames it, so a run that is interrupted leaves a `.part`
    /// rather than a half-written file that looks like a core.
    fn fetch(
        &self,
        asset: &CoreAsset,
        partial: &Path,
        progress: &mut dyn FnMut(u64, u64),
        t: &Text,
    ) -> Result<u64, String> {
        let response = self
            .agent
            .get(&asset.url)
            .call()
            .map_err(|error| t.core_release_request_failed(&asset.name, &error.to_string()))?;
        // The API's own number is the fallback: the asset list carries a size
        // for every file, and a redirect or a proxy may not repeat it.
        let total = response.body().content_length().unwrap_or(asset.size);
        let mut reader = response.into_body().into_reader();
        let mut writer = BufWriter::new(File::create(partial).map_err(|error| {
            t.core_release_disk_failed(&partial.display().to_string(), &error.to_string())
        })?);

        let mut buffer = vec![0u8; COPY_BUFFER];
        let mut received: u64 = 0;
        let mut reported: u64 = 0;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| t.core_release_request_failed(&asset.name, &error.to_string()))?;
            if read == 0 {
                break;
            }
            writer.write_all(&buffer[..read]).map_err(|error| {
                t.core_release_disk_failed(&partial.display().to_string(), &error.to_string())
            })?;
            received += read as u64;
            if received - reported >= REPORT_EVERY {
                reported = received;
                progress(received, total);
            }
        }
        writer.flush().map_err(|error| {
            t.core_release_disk_failed(&partial.display().to_string(), &error.to_string())
        })?;
        drop(writer);
        // The last report is sent whatever the size: a progress bar that stops
        // one report short of the end is a bar that looks stuck.
        progress(received, total);
        Ok(received)
    }
}

impl Default for HttpCoreDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreDownloader for HttpCoreDownloader {
    fn download(
        &self,
        job: &DownloadJob,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Downloaded, String> {
        let t = job.text;
        fs::create_dir_all(&job.directory).map_err(|error| {
            t.core_release_disk_failed(&job.directory.display().to_string(), &error.to_string())
        })?;

        let file = job.directory.join(safe_file_name(&job.asset.name));
        let partial = partial_path(&file);
        self.fetch(&job.asset, &partial, progress, t)?;
        // Windows refuses a rename onto a name that is taken, and a reader who
        // downloads the same release twice is not making a mistake.
        let _ = fs::remove_file(&file);
        fs::rename(&partial, &file).map_err(|error| {
            t.core_release_disk_failed(&file.display().to_string(), &error.to_string())
        })?;

        if job.asset.kind.unpacks() {
            let unpacked = job.directory.join(UNPACK_DIR);
            fs::create_dir_all(&unpacked).map_err(|error| {
                t.core_release_disk_failed(&unpacked.display().to_string(), &error.to_string())
            })?;
            unpack(&file, &unpacked)
                .map_err(|error| t.core_release_unpack_failed(&job.asset.name, &error))?;
            let executable = find_browser(&unpacked, job.asset.platform);
            if let Some(path) = &executable {
                make_runnable(path);
            }
            return Ok(Downloaded {
                file,
                unpacked: Some(unpacked),
                executable,
            });
        }

        // An AppImage is the browser: it needs no unpacking and one bit set.
        // The condition is `becomes_core` rather than a comparison with
        // `AppImage` because the two questions are different: this branch is
        // about the file being runnable where it stands, and the kinds that are
        // not - an installer, a disk image - are the same ones the download
        // cannot register.
        if job.asset.kind.becomes_core() {
            make_runnable(&file);
            return Ok(Downloaded {
                executable: Some(file.clone()),
                file,
                unpacked: None,
            });
        }

        Ok(Downloaded {
            file,
            unpacked: None,
            executable: None,
        })
    }
}

/// What a worker reported back to the window.
///
/// One enum for the three answers, so a fourth cannot report somewhere the
/// window does not hear it - the same reason [`crate::maintenance::Outcome`] is
/// one type.
pub enum ReleaseEvent {
    /// The list came back, newest first.
    Listed(Result<Vec<CoreRelease>, String>),
    /// How much of one asset has arrived.
    Progress {
        key: String,
        received: u64,
        total: u64,
    },
    /// One asset finished, with what unpacking left behind.
    Downloaded {
        release: String,
        result: Result<Downloaded, String>,
    },
}

/// Reads one release list out of the API's JSON.
///
/// Split from the request so the shape it depends on is tested against a
/// document rather than against the network - which is also what makes a change
/// in GitHub's answer a failing test instead of a dialog that lists nothing.
fn parse_releases(json: &str) -> Result<Vec<CoreRelease>, String> {
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

    #[derive(Deserialize)]
    struct RawAsset {
        name: String,
        #[serde(default)]
        size: u64,
        browser_download_url: String,
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
            let assets: Vec<CoreAsset> = assets
                .into_iter()
                .filter_map(|asset| {
                    let (platform, kind) = classify(&asset.name)?;
                    Some(CoreAsset {
                        name: asset.name,
                        url: asset.browser_download_url,
                        size: asset.size,
                        platform,
                        kind,
                    })
                })
                .collect();
            // A release this program has nothing to run from is not a choice;
            // listing it would be a row of buttons that are all absent.
            if assets.is_empty() {
                return None;
            }
            Some(CoreRelease {
                tag: tag_name,
                published: published_day(published_at.as_deref()),
                prerelease,
                assets,
            })
        })
        .collect())
}

/// Which platform and which kind of file an asset's name describes.
///
/// Read from the name rather than from GitHub's asset metadata, because the
/// name is the part the publisher controls and the part the releases are
/// consistent about: `..._windows_x64.zip`, `...-x86_64_linux.tar.xz`,
/// `..._macos.dmg`. A name this does not recognise is not guessed at - the
/// asset is skipped, and a release left with nothing is skipped with it.
fn classify(name: &str) -> Option<(Platform, AssetKind)> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".dmg") {
        return Some((Platform::MacOs, AssetKind::DiskImage));
    }
    if lower.ends_with(".appimage") {
        return Some((Platform::Linux, AssetKind::AppImage));
    }
    if lower.ends_with(".exe") {
        return Some((Platform::Windows, AssetKind::Installer));
    }
    if lower.ends_with(".zip") || lower.ends_with(".tar.xz") {
        if lower.contains("windows") {
            return Some((Platform::Windows, AssetKind::Portable));
        }
        if lower.contains("linux") {
            return Some((Platform::Linux, AssetKind::Portable));
        }
        if lower.contains("macos") || lower.contains("osx") {
            return Some((Platform::MacOs, AssetKind::Portable));
        }
    }
    None
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

/// Unpacks `archive` into `destination`, choosing the reader by its extension.
///
/// Both readers refuse to write outside `destination`: `tar::Archive::unpack`
/// validates every entry's path, and `zip`'s extraction does the same with the
/// names it reads out of the index. That is the whole reason the release archive
/// is unpacked with a library rather than by running the platform's `tar`.
fn unpack(archive: &Path, destination: &Path) -> Result<(), String> {
    let name = archive
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".zip") {
        let file = File::open(archive).map_err(|error| error.to_string())?;
        let mut zip =
            zip::ZipArchive::new(BufReader::new(file)).map_err(|error| error.to_string())?;
        zip.extract(destination).map_err(|error| error.to_string())
    } else if name.ends_with(".tar.xz") {
        let file = File::open(archive).map_err(|error| error.to_string())?;
        let mut tar = tar::Archive::new(xz2::read::XzDecoder::new(BufReader::new(file)));
        tar.unpack(destination).map_err(|error| error.to_string())
    } else {
        Err(format!("there is no unpacker for {name}"))
    }
}

/// The browser binary inside an unpacked release, if one is there.
///
/// The shallowest match wins, because a released archive has the browser at its
/// top and anything deeper is a copy shipped alongside it.
fn find_browser(root: &Path, platform: Platform) -> Option<PathBuf> {
    let mut found = Vec::new();
    collect_browsers(root, 0, platform, &mut found);
    found.sort_by_key(|path| path.components().count());
    found.into_iter().next()
}

fn collect_browsers(root: &Path, depth: usize, platform: Platform, found: &mut Vec<PathBuf>) {
    if depth > SEARCH_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_browsers(&path, depth + 1, platform, found);
        } else if is_browser(&path, platform) {
            found.push(path);
        }
    }
}

/// Whether a file in an unpacked release is the browser.
///
/// The name has to match exactly, because everything else in the directory is
/// named around it: `chrome_sandbox`, `chrome-wrapper`, `chromedriver` and
/// `chrome_crashpad_handler` all sit beside `chrome` and none of them is the
/// browser.
fn is_browser(path: &Path, platform: Platform) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match platform {
        Platform::Linux => name == "chrome" || name == "chromium",
        Platform::Windows => name == "chrome.exe" || name == "chromium.exe",
        // A macOS build is an application bundle and its binary is named after
        // the bundle rather than `chrome`: what identifies it is the one
        // directory a bundle keeps its executable in.
        Platform::MacOs => {
            name == "chrome"
                || name == "chromium"
                || path
                    .parent()
                    .is_some_and(|parent| parent.ends_with("Contents/MacOS"))
        }
    }
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

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A catalog a test drives: it answers with whatever it was given.
    pub struct FakeCatalog {
        answer: Result<Vec<CoreRelease>, String>,
        calls: AtomicUsize,
    }

    impl FakeCatalog {
        pub fn listing(releases: Vec<CoreRelease>) -> Self {
            Self {
                answer: Ok(releases),
                calls: AtomicUsize::new(0),
            }
        }

        pub fn failing(message: &str) -> Self {
            Self {
                answer: Err(message.to_string()),
                calls: AtomicUsize::new(0),
            }
        }

        pub fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl ReleaseCatalog for FakeCatalog {
        fn list(&self) -> Result<Vec<CoreRelease>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }
    }

    /// What a [`FakeDownloader`] was told to answer with.
    enum Answer {
        /// Writes a browser reporting this banner and answers with it.
        Browser(String),
        /// Refuses with this message.
        Refusal(String),
    }

    /// A downloader a test drives: it writes the browser a core probe can read
    /// and answers with it, or refuses with what it was given.
    ///
    /// The file really is written, because the state registers what comes back
    /// through the same core service the rest of the window uses - a fake that
    /// answered with a path that is not on disk would test the fake rather than
    /// the path.
    pub struct FakeDownloader {
        answer: Answer,
        jobs: Mutex<Vec<(String, String)>>,
        progress: Mutex<Vec<(u64, u64)>>,
    }

    impl FakeDownloader {
        /// Answers with a browser reporting `banner`, written into the job's
        /// directory.
        pub fn unpacking(banner: &str) -> Self {
            Self {
                answer: Answer::Browser(banner.to_string()),
                jobs: Mutex::new(Vec::new()),
                progress: Mutex::new(Vec::new()),
            }
        }

        pub fn refusing(message: &str) -> Self {
            Self {
                answer: Answer::Refusal(message.to_string()),
                jobs: Mutex::new(Vec::new()),
                progress: Mutex::new(Vec::new()),
            }
        }

        /// Which release and asset each call was asked for, oldest first.
        pub fn jobs(&self) -> Vec<(String, String)> {
            self.jobs.lock().expect("jobs lock").clone()
        }

        /// Every progress report the fake sent, oldest first.
        pub fn progress(&self) -> Vec<(u64, u64)> {
            self.progress.lock().expect("progress lock").clone()
        }
    }

    impl CoreDownloader for FakeDownloader {
        fn download(
            &self,
            job: &DownloadJob,
            progress: &mut dyn FnMut(u64, u64),
        ) -> Result<Downloaded, String> {
            self.jobs
                .lock()
                .expect("jobs lock")
                .push((job.release.clone(), job.asset.name.clone()));
            let banner = match &self.answer {
                Answer::Browser(banner) => banner.clone(),
                Answer::Refusal(message) => return Err(message.clone()),
            };

            let directory = job.directory.join(UNPACK_DIR);
            fs::create_dir_all(&directory).expect("fake unpack dir");
            let executable = directory.join(if cfg!(windows) {
                "chrome.exe"
            } else {
                "chrome"
            });
            fs::write(&executable, banner.as_bytes()).expect("fake browser");
            let file = job.directory.join(safe_file_name(&job.asset.name));
            fs::write(&file, b"fake archive").expect("fake archive");

            self.progress
                .lock()
                .expect("progress lock")
                .push((job.asset.size, job.asset.size));
            progress(job.asset.size, job.asset.size);
            Ok(Downloaded {
                file,
                unpacked: Some(directory),
                executable: Some(executable),
            })
        }
    }

    /// A release to list, with one asset per platform.
    pub fn release(tag: &str) -> CoreRelease {
        CoreRelease {
            tag: tag.to_string(),
            published: "2026-06-21".to_string(),
            prerelease: false,
            assets: vec![
                asset(
                    tag,
                    Platform::Windows,
                    "windows_x64.zip",
                    AssetKind::Portable,
                ),
                asset(
                    tag,
                    Platform::Linux,
                    "x86_64_linux.tar.xz",
                    AssetKind::Portable,
                ),
                asset(tag, Platform::MacOs, "macos.dmg", AssetKind::DiskImage),
            ],
        }
    }

    fn asset(tag: &str, platform: Platform, suffix: &str, kind: AssetKind) -> CoreAsset {
        CoreAsset {
            name: format!("ungoogled-chromium_{tag}_{suffix}"),
            url: format!("https://example.invalid/{tag}/{suffix}"),
            size: 1024,
            platform,
            kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// The names the releases actually carry, one per platform and kind.
    #[test]
    fn an_asset_name_says_which_system_and_which_kind_it_is() {
        let cases = [
            (
                "ungoogled-chromium_148.0.7778.215-1.1_windows_x64.zip",
                Platform::Windows,
                AssetKind::Portable,
            ),
            (
                "ungoogled-chromium_148.0.7778.215-1.1_installer_x64.exe",
                Platform::Windows,
                AssetKind::Installer,
            ),
            (
                "ungoogled-chromium-148.0.7778.215-1-x86_64_linux.tar.xz",
                Platform::Linux,
                AssetKind::Portable,
            ),
            (
                "ungoogled-chromium-148.0.7778.215-1-x86_64.AppImage",
                Platform::Linux,
                AssetKind::AppImage,
            ),
            (
                "ungoogled-chromium_148.0.7778.215-1.1_macos.dmg",
                Platform::MacOs,
                AssetKind::DiskImage,
            ),
        ];
        for (name, platform, kind) in cases {
            assert_eq!(classify(name), Some((platform, kind)), "{name}");
        }
    }

    #[test]
    fn a_name_this_does_not_know_is_skipped_rather_than_guessed_at() {
        for name in [
            "ungoogled-chromium-148.0.7778.215-1-x86_64_linux.tar.bz2",
            "notes.txt",
            "ungoogled-chromium-148.0.7778.215-1.tar.xz",
            "SHA256SUMS",
        ] {
            assert_eq!(classify(name), None, "{name}");
        }
    }

    /// A release with nothing this can run is not a row of buttons.
    #[test]
    fn a_release_with_no_usable_asset_is_not_listed() {
        let json = r#"[
            {"tag_name": "148.0.7778.215", "draft": false, "assets": [
                {"name": "SHA256SUMS", "size": 10, "browser_download_url": "https://x/1"}
            ]},
            {"tag_name": "147.0.0.0", "draft": false, "assets": [
                {"name": "ungoogled-chromium_147.0.0.0-1.1_windows_x64.zip", "size": 20,
                 "browser_download_url": "https://x/2"}
            ]}
        ]"#;

        let releases = parse_releases(json).expect("parses");

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag, "147.0.0.0");
        assert_eq!(releases[0].assets.len(), 1);
    }

    /// A draft is not published; a pre-release is, and says so.
    #[test]
    fn a_draft_is_left_out_and_a_pre_release_is_marked() {
        let json = r#"[
            {"tag_name": "draft", "draft": true, "assets": [
                {"name": "ungoogled-chromium_draft_windows_x64.zip", "size": 1,
                 "browser_download_url": "https://x/draft"}
            ]},
            {"tag_name": "149.0.0.0", "draft": false, "prerelease": true,
             "published_at": "2026-07-01T12:34:56Z", "assets": [
                {"name": "ungoogled-chromium_149.0.0.0-1.1_windows_x64.zip", "size": 1,
                 "browser_download_url": "https://x/pre"}
            ]}
        ]"#;

        let releases = parse_releases(json).expect("parses");

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag, "149.0.0.0");
        assert!(releases[0].prerelease);
        assert_eq!(releases[0].published, "2026-07-01");
    }

    #[test]
    fn a_reply_that_is_not_the_expected_document_is_an_error() {
        let error = parse_releases("{\"message\": \"API rate limit exceeded\"}")
            .expect_err("a rate-limit answer is not a release list");
        assert!(error.contains("not the release list"), "{error}");
    }

    /// The portable build wins: it is the one this program unpacks itself.
    #[test]
    fn the_preferred_asset_is_the_one_that_can_become_a_core() {
        let release = CoreRelease {
            tag: "148.0.7778.215".to_string(),
            published: "2026-06-21".to_string(),
            prerelease: false,
            assets: vec![
                CoreAsset {
                    name: "installer.exe".to_string(),
                    url: "https://x/installer".to_string(),
                    size: 1,
                    platform: Platform::Windows,
                    kind: AssetKind::Installer,
                },
                CoreAsset {
                    name: "portable.zip".to_string(),
                    url: "https://x/portable".to_string(),
                    size: 1,
                    platform: Platform::Windows,
                    kind: AssetKind::Portable,
                },
            ],
        };

        let chosen = release
            .for_platform(Platform::Windows)
            .expect("a Windows asset");
        assert_eq!(chosen.name, "portable.zip");
        assert!(chosen.kind.becomes_core());
        assert!(
            release.for_platform(Platform::Linux).is_none(),
            "a platform the release does not cover has no asset"
        );
    }

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

    /// The shallowest browser wins, and the files named around it are not it.
    #[test]
    fn the_browser_is_found_by_its_exact_name() {
        let root = TempTree::new("find-browser");
        root.write("bundle/chrome", "the browser");
        root.write("bundle/chrome_sandbox", "not the browser");
        root.write("bundle/chrome-wrapper", "not the browser");
        root.write("bundle/chromedriver", "not the browser");
        root.write("deeper/bundle/chrome", "a copy");

        let found = find_browser(&root.path(), Platform::Linux).expect("a browser");

        assert_eq!(found, root.path().join("bundle/chrome"));
    }

    /// A macOS build keeps its executable in the one directory a bundle has.
    #[test]
    fn a_macos_bundle_binary_is_found_inside_it() {
        let root = TempTree::new("find-bundle");
        root.write("Chromium.app/Contents/MacOS/Chromium", "the browser");

        let found = find_browser(&root.path(), Platform::MacOs).expect("a browser");

        assert!(found.ends_with("Chromium.app/Contents/MacOS/Chromium"));
    }

    /// A zip is unpacked for real, and the browser inside it is found: this is
    /// the Windows half of the feature, exercised without a network.
    #[test]
    fn a_zip_release_unpacks_into_a_tree_with_a_browser_in_it() {
        let root = TempTree::new("unpack-zip");
        let archive = root.path().join("portable.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&archive).expect("create"));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("bundle/chrome.exe", options)
                .expect("start entry");
            zip.write_all(b"Chromium 148.0.7778.215")
                .expect("write entry");
            zip.finish().expect("finish");
        }

        let destination = root.path().join("unpacked");
        unpack(&archive, &destination).expect("unpacks");

        let found = find_browser(&destination, Platform::Windows).expect("a browser");
        assert!(found.ends_with("bundle/chrome.exe"));
        assert_eq!(
            fs::read_to_string(&found).expect("read"),
            "Chromium 148.0.7778.215"
        );
    }

    /// The Linux half: a real `tar.xz`, made and read back with the same
    /// libraries the download uses.
    #[test]
    fn a_tar_xz_release_unpacks_into_a_tree_with_a_browser_in_it() {
        let root = TempTree::new("unpack-tar-xz");
        let archive = root.path().join("linux.tar.xz");
        let payload = b"Chromium 148.0.7778.215";
        {
            let file = File::create(&archive).expect("create");
            let encoder = xz2::write::XzEncoder::new(BufWriter::new(file), 1);
            let mut tar = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(
                &mut header,
                "bundle/chrome",
                Cursor::new(payload.as_slice()),
            )
            .expect("append");
            tar.into_inner()
                .expect("finish tar")
                .finish()
                .expect("finish xz");
        }

        let destination = root.path().join("unpacked");
        unpack(&archive, &destination).expect("unpacks");

        let found = find_browser(&destination, Platform::Linux).expect("a browser");
        assert!(found.ends_with("bundle/chrome"));
        assert_eq!(
            fs::read_to_string(&found).expect("read"),
            "Chromium 148.0.7778.215"
        );
    }

    #[test]
    fn a_file_with_no_unpacker_says_so() {
        let root = TempTree::new("unpack-none");
        let archive = root.path().join("core.7z");
        fs::write(&archive, b"not an archive").expect("write");

        let error = unpack(&archive, &root.path().join("out")).expect_err("no unpacker");
        assert!(error.contains("no unpacker"), "{error}");
    }

    /// The real catalog against the real repository.
    ///
    /// Ignored by default, like the suites that need a real browser or a real
    /// Xray: it dials GitHub, so it is an acceptance run and not part of the
    /// gate. Run it with `--ignored` when the request or the parsing changes -
    /// it is the one test that would notice GitHub answering in a shape this
    /// does not read.
    #[test]
    #[ignore = "dials the GitHub API; run with --ignored"]
    fn the_real_repository_answers_with_releases_this_can_read() {
        let releases = GithubReleaseCatalog::new()
            .list()
            .expect("the repository answers");

        assert!(
            !releases.is_empty(),
            "the repository has published releases"
        );
        for release in &releases {
            assert!(!release.tag.is_empty());
            assert!(
                !release.published.is_empty(),
                "{} has no publication date",
                release.tag
            );
            for asset in &release.assets {
                assert!(!asset.name.is_empty());
                assert!(
                    asset.url.starts_with("https://"),
                    "{} is not an https URL",
                    asset.url
                );
            }
        }
        // Every system this program runs on is covered by something, which is
        // what makes the dialog useful on all three.
        for platform in Platform::ALL {
            assert!(
                releases
                    .iter()
                    .any(|release| release.for_platform(platform).is_some()),
                "no release covers {}",
                platform.label()
            );
        }
    }

    /// A whole real core, from the request to the browser binary on disk.
    ///
    /// The only test that exercises the parts of a download the fakes stand in
    /// for: the redirect from the asset URL to the object store, the streaming
    /// reader, the `.part` rename, and the unpacker over a real release archive.
    /// Ignored because it moves a hundred megabytes over the network, and
    /// skipped on a system whose build is not an archive this can unpack - a
    /// macOS disk image is a file to run, not a tree to search.
    #[test]
    #[ignore = "downloads a real core over the network; run with --ignored"]
    fn a_real_release_downloads_and_unpacks_into_a_browser() {
        let releases = GithubReleaseCatalog::new().list().expect("the list");
        let platform = Platform::host();
        let asset = releases
            .iter()
            .find_map(|release| release.for_platform(platform))
            .cloned()
            .expect("a release for this system");
        if !asset.kind.unpacks() {
            return;
        }

        let directory = std::env::temp_dir().join("fp-core-releases-real-download");
        let _ = fs::remove_dir_all(&directory);
        let job = DownloadJob {
            text: crate::text::en(),
            release: releases[0].tag.clone(),
            asset: asset.clone(),
            directory: directory.clone(),
        };

        let mut last = (0u64, 0u64);
        let downloaded = HttpCoreDownloader::new()
            .download(&job, &mut |received, total| last = (received, total))
            .expect("the download");

        assert!(downloaded.file.is_file(), "the archive is on disk");
        assert!(last.0 > 0, "progress was reported");
        assert_eq!(last.0, last.1, "the last report is the whole file");
        if asset.kind.unpacks() {
            let executable = downloaded.executable.expect("a browser in the archive");
            assert!(executable.is_file(), "{} is a file", executable.display());
            assert!(
                executable.starts_with(&directory),
                "{} is inside {}",
                executable.display(),
                directory.display()
            );
        }

        let _ = fs::remove_dir_all(&directory);
    }

    /// A tree that removes itself, so a run leaves no directory behind - the
    /// same guard the core-service tests use.
    struct TempTree {
        dir: PathBuf,
    }

    impl TempTree {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-core-releases-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            Self { dir }
        }

        fn path(&self) -> PathBuf {
            self.dir.clone()
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.dir.join(relative);
            fs::create_dir_all(path.parent().expect("a parent")).expect("parent dir");
            fs::write(path, contents).expect("write");
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}
