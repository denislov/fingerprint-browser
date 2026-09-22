//! The four things the window asks for that are about the whole installation.
//!
//! An export reads every core, proxy and profile and writes a file; an import
//! reads one file and then writes every record in it; a restore replaces the whole
//! configuration in one transaction; a version probe starts a process and waits
//! for it to answer. None of them is local and quick in the way a page switch is -
//! an import of a large file is thousands of database writes, and a probe waits on
//! a program this window did not write - and all four used to run inside the click
//! handler, which is the thread that draws the window. The window stopped
//! redrawing for as long as they took.
//!
//! Each is now a value: a job holds everything the work needs - the services, the
//! paths, the text for its own failure sentences - and the `run_` function beside
//! it performs it somewhere else, reporting one [`Outcome`]. The view builds a job
//! on the click, hands it to a thread, and applies the outcome when it arrives
//! through the channel the view drains; [`crate::state::AppState`]'s synchronous
//! entry points are the same three steps in a row, which is what the tests drive.
//!
//! The services travel as `Arc`s because three of the four are shared with the
//! state that built the task, and the job is the only thing the worker owns.

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
