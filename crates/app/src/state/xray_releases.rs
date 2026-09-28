//! The Xray builds published on GitHub: what the list says, and the one download
//! in flight.
//!
//! This is the window's side of [`crate::xray_releases`]. That module knows how
//! to ask GitHub, how to pick the build for this machine and how to take an
//! engine out of an archive; this is what the dialog reads and what a worker's
//! answers are applied to. Nothing here dials anything - the catalog and the
//! downloader are injected into the view - so the whole flow is testable with two
//! fakes and a temporary directory.
//!
//! The one place this parts company with the core download beside it is what a
//! finished download *becomes*. A core is registered in the database; an engine
//! is a path in the configuration, so finishing here means writing a setting -
//! and then checking whether that setting is the one in force, because an
//! environment variable outranks it and a download reported as installed when it
//! will never run is worse than one reported as failed.

use super::*;
use crate::core_releases::size_label;
use crate::settings::{SettingKey, XRAY_BIN_ENV};
use crate::xray_releases::{Downloaded, XrayAsset, XrayRelease};

/// What the window knows about the published engines.
#[derive(Debug, Clone)]
pub enum XrayReleaseStatus {
    /// Nothing has been asked for yet. The dialog asks when it is opened.
    Unasked,
    /// A list is on its way.
    Loading,
    /// What the last fetch returned, newest first.
    Listed(Vec<XrayRelease>),
    /// Why the last fetch failed, already written for the reader.
    Failed(String),
}

/// The engine download in flight.
#[derive(Debug, Clone)]
pub struct XrayDownload {
    /// Which asset of which release: the row the progress belongs to.
    pub key: String,
    pub release: String,
    /// The asset's file name, for the sentences about it.
    pub asset: String,
    pub received: u64,
    pub total: u64,
}

impl XrayDownload {
    /// How far it has got, when the server said how big the file is.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.received as f32 / self.total as f32).clamp(0.0, 1.0))
    }
}

impl AppState {
    pub fn xray_releases(&self) -> &XrayReleaseStatus {
        &self.xray_releases
    }

    pub fn xray_download(&self) -> Option<&XrayDownload> {
        self.xray_download.as_ref()
    }

    /// Where the engines this window downloaded are kept: one directory per
    /// release, under the data directory, so two versions never share a tree.
    pub fn xray_dir(&self) -> PathBuf {
        self.settings.data_dir().join(crate::paths::XRAY_DIR)
    }

    /// The directory one release's archive and engine go in.
    ///
    /// The tag names the directory, sanitized on the way: it arrives in a JSON
    /// document, and a tag is not allowed to describe a path either.
    pub fn xray_release_dir(&self, tag: &str) -> PathBuf {
        self.xray_dir()
            .join(crate::core_releases::safe_file_name(tag))
    }

    /// The release this window downloaded and pointed the setting at, if any.
    ///
    /// Read back out of the path rather than remembered: the setting is the one
    /// record of which engine is in force, and a second copy here would be a
    /// copy that survives the reader pointing the row at their own binary.
    pub fn downloaded_xray_tag(&self) -> Option<String> {
        let executable = self.settings.xray_executable();
        let release = executable.parent()?;
        // `xray-releases/<tag>/xray`: the parent of the file's directory is the tree
        // program writes, and a path anywhere else was not put there by it.
        if release.parent()? != self.xray_dir() {
            return None;
        }
        release.file_name()?.to_str().map(str::to_string)
    }

    /// The engine path in force, as the settings row shows it.
    ///
    /// The same value the card's state line is about: the row and the card are
    /// two readings of one path, and a second resolution of it here is where the
    /// two would drift apart.
    pub fn xray_executable(&self) -> &std::path::Path {
        self.settings.xray_executable()
    }

    /// Whether there is an executable at the configured path.
    pub fn xray_engine_present(&self) -> bool {
        self.settings.xray_executable().is_file()
    }

    /// Marks a fetch as started.
    pub fn begin_xray_release_fetch(&mut self) {
        self.xray_releases = XrayReleaseStatus::Loading;
    }

    /// Takes what a fetch found, or why it failed.
    pub fn finish_xray_release_fetch(&mut self, result: Result<Vec<XrayRelease>, String>) {
        self.xray_releases = match result {
            Ok(releases) => XrayReleaseStatus::Listed(releases),
            Err(error) => {
                // The banner and the dialog both say it: the banner is what the
                // reader sees if the dialog was closed while the fetch was out,
                // and the list is what the dialog shows in place of rows.
                let message = self.text().xray_download_failed(&error);
                self.set_notice(Notice::error(message.clone()));
                XrayReleaseStatus::Failed(message)
            }
        };
    }

    /// Records a download starting.
    ///
    /// One at a time, and refused rather than queued, for the reason the core
    /// download is: the row has one progress bar, and a queue behind a download
    /// is a click that appears to have been ignored.
    pub fn begin_xray_download(
        &mut self,
        release: &str,
        asset: &XrayAsset,
    ) -> Result<XrayDownload, String> {
        if let Some(running) = &self.xray_download {
            return Err(self.text().xray_release_busy(&running.asset));
        }
        self.xray_download = Some(XrayDownload {
            key: crate::xray_releases::asset_key(release, &asset.name),
            release: release.to_string(),
            asset: asset.name.clone(),
            received: 0,
            total: asset.size,
        });
        Ok(self.xray_download.clone().expect("just recorded"))
    }

    /// Records how far the download in flight has got.
    ///
    /// An answer for an asset that is no longer the one in flight is dropped:
    /// the reader may have closed the dialog, and a stale report must not move a
    /// bar that belongs to something else.
    pub fn update_xray_download(&mut self, key: &str, received: u64, total: u64) {
        if let Some(download) = &mut self.xray_download
            && download.key == key
        {
            download.received = received;
            if total > 0 {
                download.total = total;
            }
        }
    }

    /// Applies what a finished download did: an engine on disk, and the setting
    /// that makes it the one this program runs.
    pub fn finish_xray_download(&mut self, release: &str, result: Result<Downloaded, String>) {
        self.xray_download = None;
        let t = self.text();
        let downloaded = match result {
            Ok(downloaded) => downloaded,
            Err(message) => {
                self.set_notice(Notice::error(message));
                return;
            }
        };

        let path = downloaded.executable.display().to_string();
        // What was left in the archive is said before what was written, and as a
        // toast rather than the banner: the reader who asked for an engine gets
        // the banner telling them it is there, and the twenty-nine megabytes of
        // routing data that did not arrive is the footnote to it.
        let kept = size_label(
            std::fs::metadata(&downloaded.executable)
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        );
        let skipped = size_label(downloaded.skipped);
        self.push_notice(t.xray_release_trimmed(&kept, &skipped), false);

        // The provenance line, in the log rather than the banner: which hash is
        // on disk is what a report is asked for weeks later, and what a reader
        // compares against upstream's own `.dgst`. The banner has the sentence
        // that needs acting on.
        self.append_log(
            LogLevel::Info,
            None,
            t.xray_release_recorded(
                release,
                &path,
                &downloaded.archive_sha256,
                &downloaded.digest_url,
            ),
        );

        if let Err(error) = self.settings.set(SettingKey::XrayExecutable, &path) {
            self.set_notice(Notice::error(
                t.xray_release_unrecorded(release, &path, &error),
            ));
            return;
        }

        // The setting is written; whether it is the one that will be read is a
        // different question, and the one answer that must not be dressed up as
        // success.
        if self.settings.xray_from_env() {
            self.set_notice(Notice::error(t.xray_release_shadowed(
                release,
                &path,
                XRAY_BIN_ENV,
            )));
        } else {
            self.set_notice(Notice::info(t.xray_release_saved(release, &path)));
        }
    }
}
