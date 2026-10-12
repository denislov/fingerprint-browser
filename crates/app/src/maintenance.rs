//! The things the window asks for that are about the whole installation.
//!
//! An export reads every core, proxy and profile and writes a file; an import
//! reads one file and then writes every record in it; a restore replaces the whole
//! configuration in one transaction; a version probe starts a process and waits
//! for it to answer. None of them is local and quick in the way a page switch is -
//! an import of a large file is thousands of database writes, and a probe waits on
//! a program this window did not write - and all of them used to run inside the
//! click handler, which is the thread that draws the window. The window stopped
//! redrawing for as long as they took.
//!
//! Saving a core is two of a probe and one write: the Add Core form and a core
//! pointed at another binary both re-read the version, and a finished download
//! registers what it unpacked through the same read. All three go through the
//! worker now ([`AddJob`], [`UpdateJob`]); the probe is the part that waits on
//! somebody else's program, so it is the part that must not be in the click.
//!
//! Each is now a value: a job holds everything the work needs - the services, the
//! paths, the text for its own failure sentences - and the `run_` function beside
//! it performs it somewhere else, reporting one [`Outcome`]. The view builds a job
//! on the click, hands it to a thread, and applies the outcome when it arrives
//! through the channel the view drains; [`crate::state::AppState`]'s synchronous
//! entry points are the same three steps in a row, which is what the tests drive.
//!
//! The services travel as `Arc`s because they are shared with the state that
//! built the task, and the job is the only thing the worker owns.

use crate::text::Text;
use application::{
    Credentials, ExportOrigin, ExportReport, ImportReport, RestoreError, RestoreMode, RestoreReport,
};
use domain::{BrowserCore, CoreId};
use std::path::PathBuf;
use std::sync::Arc;
use storage::ConfigurationRepository;

use application::{CoreService, ProfileService, ProxyService};

/// Which maintenance task is in flight, as far as the window is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Export,
    Import,
    Restore,
    Redetect,
}

/// What a task did, with everything the state needs to answer for it.
pub enum Outcome {
    Exported {
        destination: PathBuf,
        result: Result<ExportReport, String>,
    },
    Imported {
        source: PathBuf,
        result: Result<ImportReport, String>,
    },
    Restored {
        source: PathBuf,
        result: Result<RestoreReport, String>,
    },
    Redetected {
        id: CoreId,
        result: Result<BrowserCore, String>,
    },
    CoreAdded {
        origin: CoreSaveOrigin,
        result: Result<BrowserCore, String>,
    },
    CoreUpdated {
        origin: CoreSaveOrigin,
        result: Result<BrowserCore, String>,
    },
}

/// Who asked for a core to be saved: an answer is said in a different place
/// for each, and only one of them is still on screen when it lands.
///
/// The form shows the reason on itself, where the path that was refused is
/// still on screen. A finished download answers in the banner, and names the
/// file, because the reader now has one on disk and needs to know which - the
/// probe's own sentence would not say.
#[derive(Debug, Clone)]
pub enum CoreSaveOrigin {
    /// The Add Core form, or an edit of a stored core, numbered by the form
    /// that asked.
    ///
    /// The number is what keeps a cancelled form's answer - which still lands,
    /// in the banner - from being applied to a form opened later: it is only
    /// taken by the form still waiting for that exact ask.
    Form { token: u64 },
    /// The browser a finished download unpacked.
    Download {
        release: String,
        executable: PathBuf,
    },
}

/// Everything an export needs: the three lists, the file, and what goes in it.
pub struct ExportJob {
    pub text: &'static Text,
    pub profiles: Arc<dyn ProfileService>,
    pub cores: Arc<dyn CoreService>,
    pub proxies: Arc<dyn ProxyService>,
    pub credentials: Credentials,
    pub origin: ExportOrigin,
    pub destination: PathBuf,
}

/// Everything an import needs. Import never overwrites a profile, so there is
/// nothing here about how to treat one that is already stored.
pub struct ImportJob {
    pub text: &'static Text,
    pub source: PathBuf,
    /// Where a profile whose recorded directory is not on this machine is
    /// pointed instead.
    pub data_dir: PathBuf,
    pub profiles: Arc<dyn ProfileService>,
    pub cores: Arc<dyn CoreService>,
    pub proxies: Arc<dyn ProxyService>,
}

/// Everything a restore needs, including the repository that replaces the whole
/// configuration as one thing.
pub struct RestoreJob {
    pub _installation: application::coordination::InstallationLease,
    pub text: &'static Text,
    pub source: PathBuf,
    pub data_dir: PathBuf,
    pub mode: RestoreMode,
    pub profiles: Arc<dyn ProfileService>,
    pub cores: Arc<dyn CoreService>,
    pub proxies: Arc<dyn ProxyService>,
    pub configuration: Arc<dyn ConfigurationRepository>,
}

/// Everything a version probe needs. It is the one task with no file in it: what
/// it waits on is the binary reporting its own version.
pub struct RedetectJob {
    pub id: CoreId,
    pub cores: Arc<dyn CoreService>,
}

/// Registering a binary: the same probe, one row stored.
pub struct AddJob {
    pub name: Option<String>,
    pub path: PathBuf,
    pub cores: Arc<dyn CoreService>,
    /// Who asked, carried to the outcome so the answer is said in the right
    /// place.
    pub origin: CoreSaveOrigin,
}

/// Saving a core, re-reading its version when it was pointed at another binary.
pub struct UpdateJob {
    pub core: BrowserCore,
    pub cores: Arc<dyn CoreService>,
    /// Who asked; see [`CoreSaveOrigin`].
    pub origin: CoreSaveOrigin,
}

/// Reads the whole configuration and writes one file.
pub fn run_export(job: ExportJob) -> Result<ExportReport, String> {
    let t = job.text;
    application::read_configuration(&*job.profiles, &*job.cores, &*job.proxies)
        .map_err(|error| t.export_read_failed(&error.to_string()))
        .and_then(|snapshot| {
            application::write_config_backup(
                snapshot,
                job.credentials,
                job.origin,
                &job.destination,
            )
            .map_err(|error| t.export_write_failed(&error.to_string()))
        })
}

/// Reads one file and writes every record in it, one at a time.
pub fn run_import(job: ImportJob) -> Result<ImportReport, String> {
    let t = job.text;
    application::read_config_backup(&job.source)
        .map_err(|error| t.file_read_failed(&error.to_string()))
        .and_then(|document| {
            application::read_configuration(&*job.profiles, &*job.cores, &*job.proxies)
                .map_err(|error| t.export_read_failed(&error.to_string()))
                .and_then(|present| {
                    let plan = application::plan_import(&document, &present, &job.data_dir);
                    application::apply_import(plan, &*job.cores, &*job.proxies, &*job.profiles)
                        .map_err(|error| t.config_write_failed(&error.to_string()))
                })
        })
}

/// Replaces the whole configuration with the file's, in one transaction.
pub fn run_restore(job: RestoreJob) -> Result<RestoreReport, String> {
    let t = job.text;
    application::read_config_backup(&job.source)
        .map_err(|error| t.file_read_failed(&error.to_string()))
        .and_then(|document| {
            application::read_configuration(&*job.profiles, &*job.cores, &*job.proxies)
                .map_err(|error| t.export_read_failed(&error.to_string()))
                .and_then(|present| {
                    let plan =
                        application::plan_restore(&document, &present, &job.data_dir, job.mode)
                            .map_err(|error| match error {
                                RestoreError::NotEmpty { present } => {
                                    t.restore_not_empty(&t.counts_phrase(
                                        present.cores,
                                        present.proxies,
                                        present.profiles,
                                    ))
                                }
                                // The file's own faults. Each says what is wrong with
                                // it, because there is nothing the reader could do to
                                // the installation to make it fit.
                                other => t.restore_refused(&other.to_string()),
                            })?;
                    application::apply_restore(plan, &present, job.configuration.as_ref())
                        .map_err(|error| t.config_write_failed(&error.to_string()))
                })
        })
}

/// Reads a stored core's binary version again.
pub fn run_redetect(job: RedetectJob) -> Result<BrowserCore, String> {
    job.cores
        .redetect(job.id)
        .map_err(|error| error.to_string())
}

/// Registers a binary: reads its version and stores what it answered.
pub fn run_add(job: AddJob) -> Result<BrowserCore, String> {
    job.cores
        .add(job.name, job.path)
        .map_err(|error| error.to_string())
}

/// Saves a core, probing its binary when the path changed.
pub fn run_update(job: UpdateJob) -> Result<BrowserCore, String> {
    job.cores
        .update(job.core)
        .map_err(|error| error.to_string())
}
