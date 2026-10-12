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
use crate::core_releases::{CoreAsset, CoreRelease, Downloaded};
use crate::releases;

/// What the window knows about the published cores.
///
/// The shape is [`crate::releases::ReleaseStatus`]'s; what makes this the cores'
/// is the release type inside.
pub type ReleaseStatus = releases::ReleaseStatus<CoreRelease>;

/// The core download in flight; the shape is [`crate::releases::Download`]'s.
pub use crate::releases::Download as ReleaseDownload;

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
        self.cores_dir().join(releases::safe_file_name(tag))
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
    ///
    /// The banner says it as well as the dialog: the banner is what the reader
    /// sees if the dialog was closed while the fetch was out, and the list is
    /// what the dialog shows in place of rows.
    pub fn finish_release_fetch(&mut self, result: Result<Vec<CoreRelease>, String>) {
        let t = self.text();
        if let Some(message) = self
            .releases
            .finish_fetch(result, |error| t.core_download_failed(error))
        {
            self.set_notice(Notice::error(message));
        }
    }

    /// Records a download starting.
    ///
    /// The one-at-a-time rule and its reason live with the machinery:
    /// [`crate::releases::Download::begin`].
    pub fn begin_core_download(
        &mut self,
        release: &str,
        asset: &CoreAsset,
    ) -> Result<ReleaseDownload, String> {
        let t = self.text();
        ReleaseDownload::begin(&mut self.download, t, release, asset)
    }

    /// Records how far the download in flight has got.
    ///
    /// Whether a report still belongs to the download in flight is the
    /// download's own judgement - the rule is [`crate::releases::Download::update`]'s.
    pub fn update_download(&mut self, key: &str, received: u64, total: u64) {
        if let Some(download) = &mut self.download {
            download.update(key, received, total);
        }
    }

    /// Applies what a finished download did.
    ///
    /// A core the download produced is registered through the same probe the Add
    /// Core form uses, and announced the same way, so a downloaded core is not a
    /// second kind of core. The probe is a job: the file is on disk and the row
    /// is stored when the answer arrives, so the window's worker reads the
    /// version rather than the tick that applied this.
    ///
    /// A file that is not a core - an installer, a disk image, an archive this
    /// could not find a browser in - is reported as what it is and where it is,
    /// because the reader can still use it by hand.
    pub fn finish_core_download(
        &mut self,
        release: &str,
        result: Result<Downloaded, String>,
    ) -> Option<maintenance::AddJob> {
        self.download = None;
        let t = self.text();
        let downloaded = match result {
            Ok(downloaded) => downloaded,
            Err(message) => {
                self.set_notice(Notice::error(message));
                return None;
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
            return None;
        };

        Some(self.begin_add_core(
            None,
            executable.clone(),
            maintenance::CoreSaveOrigin::Download {
                release: release.to_string(),
                executable,
            },
        ))
    }
}
