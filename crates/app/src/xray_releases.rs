//! The Xray builds published for this program, and fetching one.
//!
//! Xray-core publishes a Windows, a Linux and a macOS asset per release, as
//! [`crate::core_releases`] knows fingerprint-chromium to - and then does three
//! things differently, which is why this is a module of its own rather than a
//! parameterisation of that one:
//!
//! - **The architecture is in the file name.** `Xray-linux-64.zip` and
//!   `Xray-linux-arm64-v8a.zip` are one platform. A release carries twenty-odd
//!   assets across every system and every architecture upstream supports, so
//!   picking "the Linux one" is not a question with one answer - and picking
//!   wrong is not a slower download, it is a binary this machine cannot run.
//! - **The archive carries what this program does not use.** `geoip.dat` and
//!   `geosite.dat` are twenty-nine megabytes of routing data, and the
//!   configuration written by [`runtime::XrayConfigBuilder`] has no routing rules
//!   at all. They are not extracted, rather than downloaded and deleted.
//! - **What arrives is a setting, not a catalogue row.** A core is registered in
//!   the database and probed for its version; the engine is a path in the
//!   configuration, read at the start of a run, and one an environment variable
//!   can outrank.
//!
//! What the two do share is called rather than copied: the name sanitiser, the
//! size label, the `.part` rename, the runnable bit and the `User-Agent`.
//!
//! Nothing here dials at test time. The catalog and the downloader are traits so
//! the window can be handed fakes, the same way [`crate::proxy_tester`] and
//! [`crate::open_dir`] are.

use crate::core_releases::{Platform, make_runnable, partial_path, safe_file_name, user_agent};
use crate::text::Text;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The repository the published engines come from.
///
/// One constant rather than `XTLS/Xray-core` written into the request, the
/// dialog's caption and a test: a mirror is then a change in one place.
pub const REPOSITORY: &str = "XTLS/Xray-core";

/// The API root one release list is read from.
const API: &str = "https://api.github.com";

/// How many releases one fetch asks for.
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
/// written as and what the HTTP client cannot express: `timeout_recv_body` is the
/// total time for the body, and its budget is not restarted by a read that
/// arrives. So it is set generously - twenty megabytes, or the hundred-odd of a
/// core, finishes on any line faster than about 75 KB/s - and what it still
/// catches is a transfer that has effectively stopped. The failure it exists to
/// prevent is the one a reader on a slow line used to get instead: a download
/// reported as failed while it was still arriving.
const DOWNLOAD_BODY_BUDGET: Duration = Duration::from_secs(30 * 60);

/// The buffer one download is copied through.
const COPY_BUFFER: usize = 64 * 1024;

/// How many bytes arrive between two progress reports.
const REPORT_EVERY: u64 = 1024 * 1024;

/// The name of the file one download's provenance is written to.
///
/// Recorded beside the binary so a report can say which upstream build is on
/// disk without reading the configuration, and so a reader can check the archive
/// against upstream's own `.dgst` with the hash recorded here.
pub const PROVENANCE_FILE: &str = "PROVENANCE.txt";

/// The names in a published archive that this program writes.
///
/// The engine and the licence it is distributed under. Deliberately not
/// `geoip.dat` and `geosite.dat`: this program writes no routing rules, so the
/// two would be twenty-nine megabytes on a user's disk that nothing ever reads.
/// Upstream's geo data is also assembled from third-party sources rather than
/// from Xray-core itself, and leaving it in the archive is one licence question
/// this feature does not have to answer.
///
/// Matched on the entry's own base name, so an entry at any depth is judged by
/// what it is called rather than by where it sits.
fn keeps(name: &str) -> bool {
    matches!(
        Path::new(name).file_name().and_then(|n| n.to_str()),
        Some("xray" | "xray.exe" | "LICENSE")
    )
}

/// Which architecture a published asset was built for.
///
/// Read out of the file name, which is the only place it is written: GitHub's
/// asset metadata carries a size and nothing about what the file runs on. The
/// variants are upstream's own set rather than a guess at the machine, so an
/// asset this program has no name for is skipped rather than offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    X86_32,
    Arm64,
    /// 32-bit ARM, newest first: `cfg!(target_arch = "arm")` does not say which
    /// of the three a machine is, so they are ranked rather than assumed.
    Arm32V7,
    Arm32V6,
    Arm32V5,
    Riscv64,
    Loong64,
    S390x,
    Ppc64,
    Ppc64Le,
    Mips32Le,
    Mips32,
    Mips64Le,
    Mips64,
}

impl Architecture {
    /// The token the file name carries, after the platform's own.
    ///
    /// Matched against what follows the platform token rather than against the
    /// whole name, so `Xray-linux-mips32.zip` cannot be read as the `-32` build:
    /// the remainder is `-mips32` and not `-32`.
    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::X86_64 => "-64",
            Self::X86_32 => "-32",
            Self::Arm64 => "-arm64-v8a",
            Self::Arm32V7 => "-arm32-v7a",
            Self::Arm32V6 => "-arm32-v6",
            Self::Arm32V5 => "-arm32-v5",
            Self::Riscv64 => "-riscv64",
            Self::Loong64 => "-loong64",
            Self::S390x => "-s390x",
            Self::Ppc64 => "-ppc64",
            Self::Ppc64Le => "-ppc64le",
            Self::Mips32Le => "-mips32le",
            Self::Mips32 => "-mips32",
            Self::Mips64Le => "-mips64le",
            Self::Mips64 => "-mips64",
        }
    }

    /// Every architecture, for the reader to be told which one it got.
    pub fn label(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::X86_32 => "32-bit x86",
            Self::Arm64 => "arm64",
            Self::Arm32V7 => "arm32 v7a",
            Self::Arm32V6 => "arm32 v6",
            Self::Arm32V5 => "arm32 v5",
            Self::Riscv64 => "riscv64",
            Self::Loong64 => "loong64",
            Self::S390x => "s390x",
            Self::Ppc64 => "ppc64",
            Self::Ppc64Le => "ppc64le",
            Self::Mips32Le => "mips32le",
            Self::Mips32 => "mips32",
            Self::Mips64Le => "mips64le",
            Self::Mips64 => "mips64",
        }
    }

    /// The architecture this build runs on, or `None` on one it has no name for.
    ///
    /// A `const fn` over `cfg!` rather than `std::env::consts::ARCH`, for the
    /// reason [`Platform::host`] is one: the rows a test sees must not depend on
    /// the machine running it.
    pub const fn host() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X86_64)
        } else if cfg!(target_arch = "x86") {
            Some(Self::X86_32)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Arm64)
        } else if cfg!(target_arch = "arm") {
            // The three are not distinguished by `cfg!`: v7 is what every system
            // this program ships on uses for a 32-bit ARM build, and it is the
            // rank below that decides among the three if upstream published more
            // than one.
            Some(Self::Arm32V7)
        } else if cfg!(target_arch = "riscv64") {
            Some(Self::Riscv64)
        } else if cfg!(target_arch = "loongarch64") {
            Some(Self::Loong64)
        } else if cfg!(target_arch = "s390x") {
            Some(Self::S390x)
        } else if cfg!(target_arch = "powerpc64") {
            // Endianness is the one thing `cfg!` does say here, and the two are
            // not interchangeable.
            if cfg!(target_endian = "little") {
                Some(Self::Ppc64Le)
            } else {
                Some(Self::Ppc64)
            }
        } else {
            None
        }
    }

    /// How well a build for this architecture runs on `host`; lower is better.
    ///
    /// `None` means it would not run at all. Equality is the answer everywhere
    /// except the 32-bit ARM family, where the machine's own sub-architecture is
    /// not knowable from this build's own `cfg` and the newest is preferred.
    /// MIPS is deliberately not fanned out the same way: the byte order of a
    /// mips host is knowable, the CPU level is not, and picking one would be a
    /// guess dressed as a preference.
    fn fits(self, host: Self) -> Option<u8> {
        if self == host {
            return Some(0);
        }
        match (host, self) {
            (Self::Arm32V7, Self::Arm32V6) => Some(1),
            (Self::Arm32V7, Self::Arm32V5) => Some(2),
            (Self::Arm32V6, Self::Arm32V5) => Some(1),
            _ => None,
        }
    }
}

/// One downloadable file in one release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrayAsset {
    pub name: String,
    pub url: String,
    /// The size GitHub reports, or `0` when it reported none.
    pub size: u64,
    pub platform: Platform,
    pub architecture: Architecture,
}

impl XrayAsset {
    /// How the build is named to a reader: the system and the architecture.
    ///
    /// Both, and in that order, because the architecture is the half a reader
    /// cannot check any other way: the file's own name is the only place it is
    /// written, and the download dialog's whole difference from the core dialog
    /// above it is that it says so before the click rather than after.
    pub fn label(&self) -> String {
        format!("{} · {}", self.platform.label(), self.architecture.label())
    }

    /// Where upstream publishes the digest of this file.
    ///
    /// Beside the asset and named after it: the release carries
    /// `Xray-linux-64.zip.dgst` next to `Xray-linux-64.zip`. Recorded rather
    /// than fetched - what a reader can check against upstream is a command, and
    /// this program does not need to guess at a digest file's format to write
    /// down which file it has.
    pub fn digest_url(&self) -> String {
        format!("{}.dgst", self.url)
    }
}

/// One published version, with the assets it offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrayRelease {
    pub tag: String,
    /// The day it was published, as `YYYY-MM-DD`, or empty when GitHub gave no
    /// date.
    pub published: String,
    pub prerelease: bool,
    pub assets: Vec<XrayAsset>,
}

impl XrayRelease {
    /// The asset this machine would run, or `None` when the release has none.
    ///
    /// Host platform and host architecture only, which is the one place this
    /// parts company with the core dialog: there the reader may be downloading
    /// for another machine and the three system buttons are all useful, and here
    /// the architecture is part of the name. Offering a platform without knowing
    /// the architecture of the machine at the far end would be offering a
    /// fifty-fifty guess, so the dialog offers the build this program can run and
    /// names the architecture it is.
    pub fn for_host(&self) -> Option<&XrayAsset> {
        let host = Architecture::host()?;
        let platform = Platform::host();
        self.assets
            .iter()
            .filter(|asset| asset.platform == platform)
            .filter_map(|asset| asset.architecture.fits(host).map(|rank| (rank, asset)))
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, asset)| asset)
    }
}

/// How one asset of one release is named to the window.
pub fn asset_key(release: &str, asset: &str) -> String {
    format!("{release}/{asset}")
}

/// Reads what a repository has published.
pub trait XrayCatalog: Send + Sync {
    /// Every release with at least one asset this program understands, newest
    /// first, as GitHub orders them.
    fn list(&self) -> Result<Vec<XrayRelease>, String>;
}

/// The real catalog: one request to the GitHub API.
pub struct GithubXrayCatalog {
    agent: ureq::Agent,
}

impl GithubXrayCatalog {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent())
            .timeout_global(Some(CATALOG_TIMEOUT))
            .build()
            .into();
        Self { agent }
    }
}

impl Default for GithubXrayCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl XrayCatalog for GithubXrayCatalog {
    fn list(&self) -> Result<Vec<XrayRelease>, String> {
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
    pub asset: XrayAsset,
    /// The release's directory. The archive lands in it and the engine is
    /// written beside it.
    pub directory: PathBuf,
}

/// What a finished download left on disk.
pub struct Downloaded {
    /// The engine binary, with its runnable bit set.
    pub executable: PathBuf,
    /// The SHA-256 of the archive as it arrived, lower-case hex.
    ///
    /// Recorded so the file on disk is identifiable later, and so a reader can
    /// compare it against the `.dgst` upstream publishes beside the archive.
    pub archive_sha256: String,
    /// Where that comparison is made.
    pub digest_url: String,
    /// The bytes the archive held that this program did not write, so the report
    /// can say what was left out rather than leaving the reader to wonder why
    /// twenty megabytes did not arrive.
    pub skipped: u64,
}

/// Fetches one asset and unpacks the engine out of it.
pub trait XrayDownloader: Send + Sync {
    /// Downloads `job.asset` into `job.directory`, writes the engine and the
    /// licence out of the archive, and reports what is there now.
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

/// The real downloader: an HTTPS fetch, then the engine out of the zip.
pub struct HttpXrayDownloader {
    agent: ureq::Agent,
}

impl HttpXrayDownloader {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent())
            // No whole-request deadline, for the reason the core downloader has
            // none: a slow line is not a failure. Each step that could hang is
            // bounded instead.
            .timeout_connect(Some(DOWNLOAD_CONNECT_TIMEOUT))
            .timeout_recv_response(Some(DOWNLOAD_RESPONSE_TIMEOUT))
            .timeout_recv_body(Some(DOWNLOAD_BODY_BUDGET))
            .build()
            .into();
        Self { agent }
    }

    /// Streams one URL into `partial`, reporting as it goes, and hashes it.
    ///
    /// Returns the bytes written and their SHA-256. The file is complete when
    /// this returns `Ok`; the caller renames it, so a run that is interrupted
    /// leaves a `.part` rather than a half-written file that looks like an
    /// engine.
    fn fetch(
        &self,
        asset: &XrayAsset,
        partial: &Path,
        progress: &mut dyn FnMut(u64, u64),
        t: &Text,
    ) -> Result<(u64, String), String> {
        let response = self
            .agent
            .get(&asset.url)
            .call()
            .map_err(|error| t.xray_release_request_failed(&asset.name, &error.to_string()))?;
        let total = response.body().content_length().unwrap_or(asset.size);
        let mut reader = response.into_body().into_reader();
        let mut writer = BufWriter::new(File::create(partial).map_err(|error| {
            t.xray_release_disk_failed(&partial.display().to_string(), &error.to_string())
        })?);

        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; COPY_BUFFER];
        let mut received: u64 = 0;
        let mut reported: u64 = 0;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| t.xray_release_request_failed(&asset.name, &error.to_string()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            writer.write_all(&buffer[..read]).map_err(|error| {
                t.xray_release_disk_failed(&partial.display().to_string(), &error.to_string())
            })?;
            received += read as u64;
            if received - reported >= REPORT_EVERY {
                reported = received;
                progress(received, total);
            }
        }
        writer.flush().map_err(|error| {
            t.xray_release_disk_failed(&partial.display().to_string(), &error.to_string())
        })?;
        drop(writer);
        // The last report is sent whatever the size: a progress bar that stops
        // one report short of the end is a bar that looks stuck.
        progress(received, total);
        Ok((received, format!("{:x}", hasher.finalize())))
    }

    /// Writes the entries of `archive` that [`keeps`] names into `destination`.
    ///
    /// Every path written is built from a name this program chose - the matched
    /// entry's own base name, over a fixed directory - and never from the entry's
    /// path. An entry called `../../xray` therefore lands where `xray` lands,
    /// which is the property the `zip` crate's own extraction would have to
    /// validate instead.
    ///
    /// Returns the engine's path, the licence's, and how many bytes were left in
    /// the archive.
    fn extract(&self, archive: &Path, destination: &Path, t: &Text) -> Result<Extracted, String> {
        let file = File::open(archive).map_err(|error| {
            t.xray_release_disk_failed(&archive.display().to_string(), &error.to_string())
        })?;
        let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|error| {
            t.xray_release_unpack_failed(&archive.display().to_string(), &error.to_string())
        })?;

        let mut extracted = Extracted::default();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|error| {
                t.xray_release_unpack_failed(&archive.display().to_string(), &error.to_string())
            })?;
            if !entry.is_file() || !keeps(entry.name()) {
                extracted.skipped += entry.size();
                continue;
            }
            let Some(name) = Path::new(entry.name())
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                extracted.skipped += entry.size();
                continue;
            };
            let target = destination.join(&name);
            let mut writer = BufWriter::new(File::create(&target).map_err(|error| {
                t.xray_release_disk_failed(&target.display().to_string(), &error.to_string())
            })?);
            std::io::copy(&mut entry, &mut writer).map_err(|error| {
                t.xray_release_disk_failed(&target.display().to_string(), &error.to_string())
            })?;
            writer.flush().map_err(|error| {
                t.xray_release_disk_failed(&target.display().to_string(), &error.to_string())
            })?;
            drop(writer);
            match name.as_str() {
                "xray" | "xray.exe" => extracted.engine = Some(target),
                _ => extracted.licence = Some(target),
            }
        }
        Ok(extracted)
    }
}

impl Default for HttpXrayDownloader {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`HttpXrayDownloader::extract`] found in one archive.
#[derive(Default)]
struct Extracted {
    engine: Option<PathBuf>,
    licence: Option<PathBuf>,
    skipped: u64,
}

impl XrayDownloader for HttpXrayDownloader {
    fn download(
        &self,
        job: &DownloadJob,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Downloaded, String> {
        let t = job.text;
        fs::create_dir_all(&job.directory).map_err(|error| {
            t.xray_release_disk_failed(&job.directory.display().to_string(), &error.to_string())
        })?;

        let file = job.directory.join(safe_file_name(&job.asset.name));
        let partial = partial_path(&file);
        let (_, archive_sha256) = self.fetch(&job.asset, &partial, progress, t)?;
        // Windows refuses a rename onto a name that is taken, and a reader who
        // downloads the same release twice is not making a mistake.
        let _ = fs::remove_file(&file);
        fs::rename(&partial, &file).map_err(|error| {
            t.xray_release_disk_failed(&file.display().to_string(), &error.to_string())
        })?;

        let extracted = self.extract(&file, &job.directory, t)?;
        let Some(executable) = extracted.engine else {
            // The licence is not an engine: an archive without one is reported
            // as what it is, and the directory is named because the files that
            // did arrive are still there.
            return Err(
                t.xray_release_no_binary(&job.release, &job.directory.display().to_string())
            );
        };
        make_runnable(&executable);
        if let Some(licence) = &extracted.licence {
            make_runnable_off(licence);
        }

        let digest_url = job.asset.digest_url();
        write_provenance(
            &job.directory,
            &Provenance {
                release: &job.release,
                asset: &job.asset.name,
                url: &job.asset.url,
                sha256: &archive_sha256,
                digest_url: &digest_url,
            },
            t,
        )?;

        Ok(Downloaded {
            executable,
            archive_sha256,
            digest_url,
            skipped: extracted.skipped,
        })
    }
}

/// What one download leaves written down beside the binary.
struct Provenance<'a> {
    release: &'a str,
    asset: &'a str,
    url: &'a str,
    sha256: &'a str,
    digest_url: &'a str,
}

/// Records where a binary on disk came from.
///
/// The file is written for the same reason the release scripts write a
/// `sha256` beside every artifact: a binary whose origin is not written down is
/// one nobody can check and nobody can replace with confidence. The digest is
/// the local one, and the line above it names where upstream publishes the one
/// to compare it against.
fn write_provenance(directory: &Path, provenance: &Provenance<'_>, t: &Text) -> Result<(), String> {
    let path = directory.join(PROVENANCE_FILE);
    let body = format!(
        "repository {REPOSITORY}\nrelease {}\nasset {}\nurl {}\nsha256 {}\ndigest {}\n",
        provenance.release,
        provenance.asset,
        provenance.url,
        provenance.sha256,
        provenance.digest_url,
    );
    fs::write(&path, body).map_err(|error| {
        t.xray_release_disk_failed(&path.display().to_string(), &error.to_string())
    })
}

/// Clears the runnable bit on the licence, on the systems that have one.
///
/// A licence is not a program. The archive carries it without the bit and a
/// downloader that set `0o755` on everything it wrote would mark it runnable,
/// which is a small lie about a file whose whole purpose is to be read.
fn make_runnable_off(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() & !0o111);
            let _ = fs::set_permissions(path, permissions);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Which platform and which architecture an asset's name describes.
///
/// The name is the part the publisher controls and the part the releases are
/// consistent about: `Xray-windows-64.zip`, `Xray-linux-arm64-v8a.zip`,
/// `Xray-macos-arm64-v8a.zip`. A name this does not recognise is not guessed at -
/// the asset is skipped, and a release left with nothing is skipped with it.
///
/// The architecture is read from what follows the platform's own token, not from
/// the name as a whole: `Xray-linux-mips32.zip` ends in `32.zip` and is not the
/// 32-bit x86 build, and only comparing the remainder to a token exactly is what
/// tells the two apart.
fn classify(name: &str) -> Option<(Platform, Architecture)> {
    let lower = name.to_ascii_lowercase();
    // Only a zip is an archive here. The `.dgst` published beside every archive
    // does not end in `.zip` and is refused by this line, which is the only place
    // that has to be said.
    let stem = lower.strip_suffix(".zip")?;
    // `xray-freebsd-64` and `xray-android-arm64-v8a` are published too, and
    // neither is a system this window runs on: a name whose prefix is not in
    // this table is not one to offer, and neither is one whose platform prefix
    // is followed by nothing the architecture table knows.
    let (platform, rest) = PLATFORM_PREFIXES
        .into_iter()
        .find_map(|(prefix, platform)| Some((platform, stem.strip_prefix(prefix)?)))?;
    let architecture = ARCHITECTURES
        .into_iter()
        .find(|arch| arch.token() == rest)?;
    Some((platform, architecture))
}

/// The platform half of an asset name, longest first where one is a prefix of
/// another - none is today, and the order is what keeps that from mattering.
const PLATFORM_PREFIXES: [(&str, Platform); 3] = [
    ("xray-windows", Platform::Windows),
    ("xray-linux", Platform::Linux),
    ("xray-macos", Platform::MacOs),
];

/// Every architecture, so a name can be looked up by its token.
const ARCHITECTURES: [Architecture; 15] = [
    Architecture::X86_64,
    Architecture::X86_32,
    Architecture::Arm64,
    Architecture::Arm32V7,
    Architecture::Arm32V6,
    Architecture::Arm32V5,
    Architecture::Riscv64,
    Architecture::Loong64,
    Architecture::S390x,
    Architecture::Ppc64,
    Architecture::Ppc64Le,
    Architecture::Mips32Le,
    Architecture::Mips32,
    Architecture::Mips64Le,
    Architecture::Mips64,
];

/// What a worker reported back to the window.
pub enum ReleaseEvent {
    /// The list came back, newest first.
    Listed(Result<Vec<XrayRelease>, String>),
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
fn parse_releases(json: &str) -> Result<Vec<XrayRelease>, String> {
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
            let assets: Vec<XrayAsset> = assets
                .into_iter()
                .filter_map(|asset| {
                    let (platform, architecture) = classify(&asset.name)?;
                    Some(XrayAsset {
                        name: asset.name,
                        url: asset.browser_download_url,
                        size: asset.size,
                        platform,
                        architecture,
                    })
                })
                .collect();
            // A release this program has nothing to run from is not a choice;
            // listing it would be a row of buttons that are all absent.
            if assets.is_empty() {
                return None;
            }
            Some(XrayRelease {
                tag: tag_name,
                published: published_day(published_at.as_deref()),
                prerelease,
                assets,
            })
        })
        .collect())
}

/// The publication date, as `YYYY-MM-DD`.
fn published_day(at: Option<&str>) -> String {
    at.map(|at| at.chars().take(10).collect())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A catalog a test drives: it answers with whatever it was given.
    pub struct FakeCatalog {
        answer: Result<Vec<XrayRelease>, String>,
        calls: AtomicUsize,
    }

    impl FakeCatalog {
        pub fn listing(releases: Vec<XrayRelease>) -> Self {
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

    impl XrayCatalog for FakeCatalog {
        fn list(&self) -> Result<Vec<XrayRelease>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }
    }

    /// What a [`FakeDownloader`] was told to answer with.
    enum Answer {
        /// Writes an engine with this body and answers with it.
        Engine(String),
        /// Refuses with this message.
        Refusal(String),
    }

    /// A downloader a test drives: it writes the engine a probe can read.
    ///
    /// The file really is written, because the state reads what came back and
    /// records it as a setting - a fake that answered with a path that is not on
    /// disk would test the fake rather than the path.
    pub struct FakeDownloader {
        answer: Answer,
        jobs: Mutex<Vec<(String, String)>>,
        progress: Mutex<Vec<(u64, u64)>>,
    }

    impl FakeDownloader {
        pub fn unpacking(body: &str) -> Self {
            Self {
                answer: Answer::Engine(body.to_string()),
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

        pub fn progress(&self) -> Vec<(u64, u64)> {
            self.progress.lock().expect("progress lock").clone()
        }
    }

    impl XrayDownloader for FakeDownloader {
        fn download(
            &self,
            job: &DownloadJob,
            progress: &mut dyn FnMut(u64, u64),
        ) -> Result<Downloaded, String> {
            self.jobs
                .lock()
                .expect("jobs lock")
                .push((job.release.clone(), job.asset.name.clone()));
            let body = match &self.answer {
                Answer::Engine(body) => body.clone(),
                Answer::Refusal(message) => return Err(message.clone()),
            };

            fs::create_dir_all(&job.directory).expect("fake release dir");
            let executable = job
                .directory
                .join(if cfg!(windows) { "xray.exe" } else { "xray" });
            fs::write(&executable, body.as_bytes()).expect("fake engine");

            self.progress
                .lock()
                .expect("progress lock")
                .push((job.asset.size, job.asset.size));
            progress(job.asset.size, job.asset.size);
            Ok(Downloaded {
                executable,
                archive_sha256: "0".repeat(64),
                digest_url: job.asset.digest_url(),
                skipped: 0,
            })
        }
    }

    /// A release to list, with one asset per platform this build runs on.
    pub fn release(tag: &str) -> XrayRelease {
        let platform = Platform::host();
        let architecture = Architecture::host().expect("a host architecture");
        XrayRelease {
            tag: tag.to_string(),
            published: "2026-03-27".to_string(),
            prerelease: false,
            assets: vec![asset(tag, platform, architecture)],
        }
    }

    /// One asset, named the way upstream names them.
    pub fn asset(tag: &str, platform: Platform, architecture: Architecture) -> XrayAsset {
        let system = match platform {
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::MacOs => "macos",
        };
        XrayAsset {
            name: format!("Xray-{system}{}.zip", architecture.token()),
            url: format!("https://example.invalid/{tag}/Xray-{system}.zip"),
            size: 1024,
            platform,
            architecture,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names the releases actually carry, one per platform and architecture.
    #[test]
    fn an_asset_name_says_which_system_and_which_architecture_it_is() {
        let cases = [
            (
                "Xray-windows-64.zip",
                Platform::Windows,
                Architecture::X86_64,
            ),
            (
                "Xray-windows-32.zip",
                Platform::Windows,
                Architecture::X86_32,
            ),
            (
                "Xray-windows-arm64-v8a.zip",
                Platform::Windows,
                Architecture::Arm64,
            ),
            ("Xray-linux-64.zip", Platform::Linux, Architecture::X86_64),
            (
                "Xray-linux-arm64-v8a.zip",
                Platform::Linux,
                Architecture::Arm64,
            ),
            (
                "Xray-linux-arm32-v7a.zip",
                Platform::Linux,
                Architecture::Arm32V7,
            ),
            ("Xray-linux-s390x.zip", Platform::Linux, Architecture::S390x),
            ("Xray-macos-64.zip", Platform::MacOs, Architecture::X86_64),
            (
                "Xray-macos-arm64-v8a.zip",
                Platform::MacOs,
                Architecture::Arm64,
            ),
        ];
        for (name, platform, architecture) in cases {
            assert_eq!(classify(name), Some((platform, architecture)), "{name}");
        }
    }

    /// The one that a suffix comparison would get wrong: `-mips32` ends in
    /// `32.zip` and is not the 32-bit x86 build.
    #[test]
    fn a_mips_build_is_not_read_as_the_x86_build() {
        assert_eq!(
            classify("Xray-linux-mips32.zip"),
            Some((Platform::Linux, Architecture::Mips32))
        );
        assert_eq!(
            classify("Xray-linux-mips32le.zip"),
            Some((Platform::Linux, Architecture::Mips32Le))
        );
        assert_eq!(
            classify("Xray-linux-mips64.zip"),
            Some((Platform::Linux, Architecture::Mips64))
        );
        assert_ne!(
            classify("Xray-linux-mips32.zip").map(|(_, arch)| arch),
            Some(Architecture::X86_32)
        );
    }

    /// A system this window does not run on, and a digest beside an archive.
    #[test]
    fn a_name_this_does_not_know_is_skipped_rather_than_guessed_at() {
        for name in [
            "Xray-freebsd-64.zip",
            "Xray-android-arm64-v8a.zip",
            "Xray-linux-64.zip.dgst",
            "Xray-source.zip",
            "notes.txt",
            "Xray-linux.zip",
        ] {
            assert_eq!(classify(name), None, "{name}");
        }
    }

    /// The architecture this build runs on wins, and one that would not run here
    /// is not offered.
    #[test]
    fn the_asset_chosen_is_the_one_this_machine_runs() {
        let release = XrayRelease {
            tag: "v26.3.27".to_string(),
            published: "2026-03-27".to_string(),
            prerelease: false,
            assets: vec![
                testing::asset("v26.3.27", Platform::host(), Architecture::Mips64Le),
                testing::asset("v26.3.27", Platform::host(), Architecture::host().unwrap()),
            ],
        };

        let chosen = release.for_host().expect("an asset for this machine");

        assert_eq!(
            chosen.architecture,
            Architecture::host().expect("a host architecture")
        );
        assert!(release.for_host().is_some());
    }

    /// A release that published nothing for this machine is a row with no
    /// button, not a row with the wrong one.
    #[test]
    fn a_release_with_no_build_for_this_machine_offers_none() {
        let foreign = match Architecture::host().expect("a host architecture") {
            Architecture::X86_64 => Architecture::Mips64,
            _ => Architecture::X86_64,
        };
        let release = XrayRelease {
            tag: "v26.3.27".to_string(),
            published: "2026-03-27".to_string(),
            prerelease: false,
            assets: vec![testing::asset("v26.3.27", Platform::host(), foreign)],
        };

        assert!(release.for_host().is_none());
        assert!(release.for_host().is_none());
    }

    /// The three 32-bit ARM builds are ranked rather than guessed at, and
    /// nothing else is interchangeable.
    #[test]
    fn only_the_arm32_family_is_interchangeable() {
        assert_eq!(Architecture::Arm32V7.fits(Architecture::Arm32V7), Some(0));
        assert_eq!(Architecture::Arm32V6.fits(Architecture::Arm32V7), Some(1));
        assert_eq!(Architecture::Arm32V5.fits(Architecture::Arm32V7), Some(2));
        assert_eq!(Architecture::Arm32V5.fits(Architecture::Arm32V6), Some(1));
        assert_eq!(Architecture::X86_32.fits(Architecture::X86_64), None);
        assert_eq!(Architecture::Mips64Le.fits(Architecture::Mips64), None);
        assert_eq!(Architecture::Ppc64.fits(Architecture::Ppc64Le), None);
        assert_eq!(Architecture::Ppc64Le.fits(Architecture::Ppc64Le), Some(0));
    }

    /// The engine and the licence are written; the geo data is not.
    #[test]
    fn only_the_engine_and_the_licence_are_kept() {
        for kept in ["xray", "xray.exe", "LICENSE", "some/dir/xray"] {
            assert!(keeps(kept), "{kept}");
        }
        for skipped in ["geoip.dat", "geosite.dat", "README.md", "xray.dgst", ""] {
            assert!(!keeps(skipped), "{skipped}");
        }
    }

    /// A release with nothing this can run is not a row of buttons.
    #[test]
    fn a_release_with_no_usable_asset_is_not_listed() {
        let json = r#"[
            {"tag_name": "v26.3.27", "draft": false, "assets": [
                {"name": "Xray-linux-64.zip.dgst", "size": 10, "browser_download_url": "https://x/1"}
            ]},
            {"tag_name": "v26.2.6", "draft": false, "assets": [
                {"name": "Xray-windows-64.zip", "size": 20,
                 "browser_download_url": "https://x/2"}
            ]}
        ]"#;

        let releases = parse_releases(json).expect("parses");

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag, "v26.2.6");
        assert_eq!(releases[0].assets.len(), 1);
        assert_eq!(releases[0].assets[0].architecture, Architecture::X86_64);
    }

    /// A draft is not published; a pre-release is, and says so.
    #[test]
    fn a_draft_is_left_out_and_a_pre_release_is_marked() {
        let json = r#"[
            {"tag_name": "draft", "draft": true, "assets": [
                {"name": "Xray-linux-64.zip", "size": 1, "browser_download_url": "https://x/draft"}
            ]},
            {"tag_name": "v27.0.0", "draft": false, "prerelease": true,
             "published_at": "2026-07-01T12:34:56Z", "assets": [
                {"name": "Xray-linux-64.zip", "size": 1, "browser_download_url": "https://x/pre"}
            ]}
        ]"#;

        let releases = parse_releases(json).expect("parses");

        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag, "v27.0.0");
        assert!(releases[0].prerelease);
        assert_eq!(releases[0].published, "2026-07-01");
    }

    #[test]
    fn a_reply_that_is_not_the_expected_document_is_an_error() {
        let error = parse_releases("{\"message\": \"API rate limit exceeded\"}")
            .expect_err("a rate-limit answer is not a release list");
        assert!(error.contains("not the release list"), "{error}");
    }

    /// The offered build is named by the system and by the architecture, which
    /// is the difference a reader has to be able to see before clicking.
    #[test]
    fn a_build_is_named_by_its_system_and_its_architecture() {
        let asset = XrayAsset {
            name: "Xray-linux-64.zip".to_string(),
            url: "https://example.invalid/1".to_string(),
            size: 1,
            platform: Platform::Linux,
            architecture: Architecture::X86_64,
        };

        assert_eq!(asset.label(), "Linux · x86_64");
    }

    /// Where upstream publishes the digest is beside the archive it describes,
    /// which is what the provenance file records.
    #[test]
    fn the_digest_is_looked_for_beside_the_archive() {
        let asset = XrayAsset {
            name: "Xray-linux-64.zip".to_string(),
            url: "https://github.com/XTLS/Xray-core/releases/download/v26.3.27/Xray-linux-64.zip"
                .to_string(),
            size: 1,
            platform: Platform::Linux,
            architecture: Architecture::X86_64,
        };

        assert_eq!(
            asset.digest_url(),
            "https://github.com/XTLS/Xray-core/releases/download/v26.3.27/Xray-linux-64.zip.dgst"
        );
    }

    /// A tree that removes itself, so a run leaves no directory behind.
    struct TempTree {
        dir: PathBuf,
    }

    impl TempTree {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-xray-releases-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            Self { dir }
        }

        fn path(&self) -> PathBuf {
            self.dir.clone()
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A real zip, made and read back with the libraries the download uses: the
    /// engine and the licence land, and the geo data does not.
    #[test]
    fn an_archive_gives_up_its_engine_and_nothing_else() {
        let root = TempTree::new("extract");
        let archive = root.path().join("Xray-linux-64.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&archive).expect("create"));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, body) in [
                ("xray", "an engine"),
                ("LICENSE", "MPL-2.0"),
                ("geoip.dat", "29 MB of it"),
                ("geosite.dat", "and 10 more"),
                ("README.md", "how to run it"),
            ] {
                zip.start_file(name, options).expect("start entry");
                zip.write_all(body.as_bytes()).expect("write entry");
            }
            zip.finish().expect("finish");
        }

        let destination = root.path().join("out");
        fs::create_dir_all(&destination).expect("out dir");
        let extracted = HttpXrayDownloader::new()
            .extract(&archive, &destination, crate::text::en())
            .expect("extracts");

        assert_eq!(
            extracted.engine.as_deref(),
            Some(destination.join("xray").as_path())
        );
        assert_eq!(
            extracted.licence.as_deref(),
            Some(destination.join("LICENSE").as_path())
        );
        assert_eq!(
            fs::read_to_string(destination.join("xray")).expect("read"),
            "an engine"
        );
        assert!(
            !destination.join("geoip.dat").exists(),
            "geo data is not written"
        );
        assert!(
            !destination.join("geosite.dat").exists(),
            "geo data is not written"
        );
        assert!(
            !destination.join("README.md").exists(),
            "only two names are kept"
        );
        assert!(extracted.skipped > 0, "the rest is counted as left behind");
    }

    /// An entry that tries to describe a path lands where its base name says,
    /// which is the property the whole extraction rests on.
    #[test]
    fn an_entry_cannot_write_outside_the_directory() {
        let root = TempTree::new("escape");
        let archive = root.path().join("Xray-linux-64.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&archive).expect("create"));
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("../../xray", options).expect("start entry");
            zip.write_all(b"an engine").expect("write entry");
            zip.finish().expect("finish");
        }

        let destination = root.path().join("out");
        fs::create_dir_all(&destination).expect("out dir");
        let extracted = HttpXrayDownloader::new()
            .extract(&archive, &destination, crate::text::en())
            .expect("extracts");

        assert_eq!(
            extracted.engine.as_deref(),
            Some(destination.join("xray").as_path())
        );
        assert!(
            !root.path().join("xray").exists(),
            "nothing above the directory"
        );
    }

    /// An archive with no engine in it is an error rather than an empty success.
    #[test]
    fn an_archive_with_no_engine_says_so() {
        let root = TempTree::new("no-engine");
        let archive = root.path().join("Xray-linux-64.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&archive).expect("create"));
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("geoip.dat", options).expect("start entry");
            zip.write_all(b"not an engine").expect("write entry");
            zip.finish().expect("finish");
        }

        let destination = root.path().join("out");
        fs::create_dir_all(&destination).expect("out dir");
        let extracted = HttpXrayDownloader::new()
            .extract(&archive, &destination, crate::text::en())
            .expect("extracts");

        assert!(extracted.engine.is_none());
    }

    /// The real catalog against the real repository.
    ///
    /// Ignored by default, like the suites that need a real browser or a real
    /// Xray: it dials GitHub, so it is an acceptance run and not part of the
    /// gate. Run it with `--ignored` when the request or the parsing changes -
    /// and, in particular, when a new upstream release is expected to carry an
    /// asset name this does not know, which is the one thing the fakes cannot
    /// notice.
    #[test]
    #[ignore = "dials the GitHub API; run with --ignored"]
    fn the_real_repository_answers_with_releases_this_can_read() {
        let releases = GithubXrayCatalog::new()
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
            assert!(
                !release.assets.is_empty(),
                "{} was listed with no asset this could use",
                release.tag
            );
            for asset in &release.assets {
                assert!(
                    asset.url.starts_with("https://"),
                    "{} is not an https URL",
                    asset.url
                );
                assert!(
                    !asset.name.is_empty(),
                    "an asset with no file name cannot be offered"
                );
            }
        }
        // The build this program would offer, found by name in a real release:
        // the arch-aware match, against the names upstream really uses.
        if let Some(host) = Architecture::host() {
            assert!(
                releases.iter().any(|release| release.for_host().is_some()),
                "no published release carries a build for {} {}",
                Platform::host().label(),
                host.label()
            );
        }
    }

    /// A whole real engine, from the request to the binary on disk.
    ///
    /// The only test that exercises the parts of a download the fakes stand in
    /// for: the redirect from the asset URL to the object store, the streaming
    /// reader, the `.part` rename, and the extractor over a real release archive.
    /// Upstream's own layout - rather than one this repository made up - is what
    /// that last step reads the names out of.
    #[test]
    #[ignore = "downloads a real engine over the network; run with --ignored"]
    fn a_real_release_downloads_and_unpacks_into_an_engine() {
        let releases = GithubXrayCatalog::new().list().expect("the list");
        let release = releases
            .iter()
            .find(|release| release.for_host().is_some())
            .expect("a release for this machine");
        let asset = release.for_host().expect("the asset just found").clone();

        let root = TempTree::new("real-download");
        let directory = root.path().join("release");
        fs::create_dir_all(&directory).expect("release dir");
        let job = DownloadJob {
            text: crate::text::en(),
            release: release.tag.clone(),
            asset,
            directory: directory.clone(),
        };

        let progress = std::cell::RefCell::new(Vec::new());
        let downloaded = HttpXrayDownloader::new()
            .download(&job, &mut |received, total| {
                progress.borrow_mut().push((received, total))
            })
            .expect("the real download");

        assert!(
            downloaded.executable.is_file(),
            "the engine is on disk at {}",
            downloaded.executable.display()
        );
        assert_eq!(
            downloaded.executable.parent(),
            Some(directory.as_path()),
            "the engine lands in the release directory itself"
        );
        assert_eq!(
            downloaded.archive_sha256.len(),
            64,
            "the archive was hashed as it arrived"
        );
        assert!(
            downloaded.skipped > 0,
            "the geo data in the archive was left out"
        );
        let progress = progress.into_inner();
        let (last, total) = *progress.last().expect("progress was reported");
        assert!(
            progress.len() > 1,
            "a twenty megabyte archive is reported as it arrives, not once at the end"
        );
        assert_eq!(
            last, total,
            "the last report is the whole file, so the bar reaches its end"
        );
        let provenance = fs::read_to_string(directory.join(PROVENANCE_FILE)).expect("a record");
        assert!(
            provenance.contains(&downloaded.archive_sha256),
            "{provenance}"
        );
        assert!(provenance.contains(&release.tag), "{provenance}");
    }

    /// The provenance file names the release, the asset, the hash and where to
    /// check it - which is the whole of what makes a binary on disk traceable.
    #[test]
    fn the_provenance_file_records_where_the_binary_came_from() {
        let root = TempTree::new("provenance");
        let provenance = Provenance {
            release: "v26.3.27",
            asset: "Xray-linux-64.zip",
            url: "https://example.invalid/Xray-linux-64.zip",
            sha256: &"a".repeat(64),
            digest_url: "https://example.invalid/Xray-linux-64.zip.dgst",
        };

        write_provenance(&root.path(), &provenance, crate::text::en()).expect("writes");

        let body = fs::read_to_string(root.path().join(PROVENANCE_FILE)).expect("read");
        assert!(body.contains("repository XTLS/Xray-core"), "{body}");
        assert!(body.contains("release v26.3.27"), "{body}");
        assert!(body.contains("asset Xray-linux-64.zip"), "{body}");
        assert!(
            body.contains(&format!("sha256 {}", "a".repeat(64))),
            "{body}"
        );
        assert!(
            body.contains("digest https://example.invalid/Xray-linux-64.zip.dgst"),
            "{body}"
        );
    }
}
