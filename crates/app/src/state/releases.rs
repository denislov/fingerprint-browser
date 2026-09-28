//! The cores published on GitHub: what the list says, and the one download in
//! flight.
//!
//! This is the window's side of [`crate::core_releases`]. That module knows how
//! to ask GitHub and how to turn an asset into a directory with a browser in it;
//! this is what the dialog reads and what a worker's answers are applied to.
//! Nothing here dials anything - the catalog and the downloader are injected into
//! the view - so the whole flow is testable with two fakes and a temporary
//! directory.
//!
//! The version a downloaded core reports is read by
//! [`AppState::add_core`], the same probe the Add Core form goes through, for the
//! same reason: the major decides which fingerprint switches a profile on it may
//! claim, and a release tag is what the file is called rather than what the
//! binary inside answers.

use super::*;
use crate::core_releases::{self, CoreAsset, CoreRelease, Downloaded};

/// What the window knows about the published cores.
#[derive(Debug, Clone)]
pub enum ReleaseStatus {
    /// Nothing has been asked for yet. The dialog asks when it is opened.
    Unasked,
    /// A list is on its way.
    Loading,
    /// What the last fetch returned, newest first.
    Listed(Vec<CoreRelease>),
    /// Why the last fetch failed, already written for the reader.
    Failed(String),
}

/// The download in flight.
#[derive(Debug, Clone)]
pub struct ReleaseDownload {
    /// Which asset of which release: the row the progress belongs to.
    pub key: String,
    pub release: String,
    /// The asset's file name, for the sentences about it.
    pub asset: String,
    pub received: u64,
    pub total: u64,
}

impl ReleaseDownload {
    /// How far it has got, when the server said how big the file is.
    ///
    /// A fraction of the total rather than a flag, and `None` rather than a
    /// made-up number when there is no total: a bar that invents its own length
    /// is a bar that has to be unlearned when the real one arrives.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.received as f32 / self.total as f32).clamp(0.0, 1.0))
    }
}

impl AppState {
    pub fn releases(&self) -> &ReleaseStatus {
        &self.releases
    }

    pub fn download(&self) -> Option<&ReleaseDownload> {
        self.download.as_ref()
    }

    /// Where downloaded cores are kept: one directory per release, under the
    /// data directory, so two versions never share a tree.
    pub fn cores_dir(&self) -> PathBuf {
        self.settings.data_dir().join(crate::paths::CORES_DIR)
    }

    /// The directory one release's file and unpacked tree go in.
    ///
    /// The tag names the directory, sanitized on the way: it arrives in a JSON
    /// document, and a tag is not allowed to describe a path either.
    pub fn release_dir(&self, tag: &str) -> PathBuf {
        self.cores_dir().join(core_releases::safe_file_name(tag))
    }

    /// Marks a fetch as started.
    ///
    /// The previous list is dropped rather than kept behind a spinner: it is a
    /// list of what was published, and a refresh that failed would otherwise
    /// leave the reader choosing from a list that may no longer be true.
    pub fn begin_release_fetch(&mut self) {
        self.releases = ReleaseStatus::Loading;
    }

    /// Takes what a fetch found, or why it failed.
    pub fn finish_release_fetch(&mut self, result: Result<Vec<CoreRelease>, String>) {
        self.releases = match result {
            Ok(releases) => ReleaseStatus::Listed(releases),
            Err(error) => {
                // The banner and the dialog both say it: the banner is what the
                // reader sees if the dialog was closed while the fetch was out,
                // and the list is what the dialog shows in place of rows.
                let message = self.text().core_download_failed(&error);
                self.set_notice(Notice::error(message.clone()));
                ReleaseStatus::Failed(message)
            }
        };
    }

    /// Records a download starting.
    ///
    /// One at a time, and refused rather than queued: two cores is a third of a
    /// gigabyte, the row has one progress bar, and a queue behind a two-minute
    /// download is a click that appears to have been ignored. The refusal names
    /// what is already running, so the reader knows to wait rather than to try
    /// again.
    pub fn begin_core_download(
        &mut self,
        release: &str,
        asset: &CoreAsset,
    ) -> Result<ReleaseDownload, String> {
        if let Some(running) = &self.download {
            return Err(self.text().core_release_busy(&running.asset));
        }
        self.download = Some(ReleaseDownload {
            key: core_releases::asset_key(release, &asset.name),
            release: release.to_string(),
            asset: asset.name.clone(),
            received: 0,
            total: asset.size,
        });
        Ok(self.download.clone().expect("just recorded"))
    }

    /// Records how far the download in flight has got.
    ///
    /// An answer for an asset that is no longer the one in flight is dropped:
    /// the reader may have cancelled, and a stale report must not move a bar
    /// that belongs to something else.
    pub fn update_download(&mut self, key: &str, received: u64, total: u64) {
        if let Some(download) = &mut self.download
            && download.key == key
        {
            download.received = received;
            if total > 0 {
                download.total = total;
            }
        }
    }

    /// Applies what a finished download did.
    ///
    /// A core the download produced is registered through the same probe the Add
    /// Core form uses, and announced the same way, so a downloaded core is not a
    /// second kind of core. A file that is not a core - an installer, a disk
    /// image, an archive this could not find a browser in - is reported as what
    /// it is and where it is, because the reader can still use it by hand.
    pub fn finish_core_download(&mut self, release: &str, result: Result<Downloaded, String>) {
        self.download = None;
        let t = self.text();
        let downloaded = match result {
            Ok(downloaded) => downloaded,
            Err(message) => {
                self.set_notice(Notice::error(message));
                return;
            }
        };

        let Some(executable) = downloaded.executable else {
            let message = match &downloaded.unpacked {
                Some(directory) => {
                    t.core_release_no_binary(release, &directory.display().to_string())
                }
                None => t.core_release_saved(&downloaded.file.display().to_string()),
            };
            // An error rather than a success toast: the reader asked for a core
            // and has a file they still have to do something with, and a toast
            // would be gone before they had read the second half.
            self.set_notice(Notice::error(message));
            return;
        };

        if let Err(error) = self.add_core(None, executable.clone()) {
            // `add_core` has already said what the probe found; this replaces
            // that with a sentence that also names the file, because the reader
            // now has one on disk and needs to know which.
            let message = t.core_release_unusable(
                release,
                &executable.display().to_string(),
                &error.to_string(),
            );
            self.set_notice(Notice::error(message));
        }
    }
}
