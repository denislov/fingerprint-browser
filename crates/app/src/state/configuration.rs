//! Configuration backups: exporting, importing, restoring and the browser-data
//! copies that sit beside them.
//!
//! Every one of these reads or writes the whole installation rather than one
//! profile, which is what makes them a group: they share the "one task at a time"
//! marker, the sentence that refuses a second task, and the worker the window
//! hands them to. See [`crate::maintenance`] for the half that runs off the
//! drawing thread.
//!
//! A child module of [`super`] rather than a separate type: these are `AppState`'s
//! own methods, and an `AppState` that had to publish its fields for them would be
//! a worse trade than a large `impl` block spread over a few files.

use super::*;

impl AppState {
    /// The export path field's contents. Empty means the default.
    ///
    /// Only the tests read this back. The window keeps the text in the field
    /// itself and hands it over when the button is pressed, so nothing in the
    /// program asks what was typed without also being the thing that typed it -
    /// and an ungated method would be dead code in the binary build, which
    /// `clippy --all-targets -D warnings` refuses.
    #[cfg(test)]
    pub fn export_path(&self) -> &str {
        &self.export_path
    }

    pub fn set_export_path(&mut self, path: impl Into<String>) {
        self.export_path = path.into();
    }

    /// Where an export would write now, typed or defaulted.
    pub fn export_destination(&self) -> PathBuf {
        let typed = self.export_path.trim();
        if typed.is_empty() {
            self.export_default_path()
        } else {
            PathBuf::from(typed)
        }
    }

    /// Whether the next export keeps the proxy credentials it could leave out.
    pub fn export_includes_credentials(&self) -> bool {
        self.export_includes_credentials
    }

    pub fn set_export_includes_credentials(&mut self, include: bool) {
        self.export_includes_credentials = include;
    }

    /// Writes a configuration backup and says what it did.
    ///
    /// Reading the three lists and writing one small file are both local and
    /// quick, so this runs on the calling thread - the same reason
    /// [`AppState::start`] does, and unlike a proxy test, which waits on a far
    /// end.
    ///
    /// Nothing here checks whether the destination is already taken. The default
    /// names a new file each second, so it cannot land on an older backup by
    /// accident; a path typed on purpose is meant, and refusing it would break
    /// updating a backup kept at one path.
    /// The export, on the calling thread.
    ///
    /// For tests, and only for tests: the window uses [`AppState::begin_export`]
    /// and applies the answer through [`AppState::finish_maintenance`], so the
    /// read and the write happen on a worker rather than on the thread that draws
    /// the window. This is the same three steps in a row, which is what makes the
    /// whole task testable without a window.
    #[cfg(test)]
    pub(crate) fn export_configuration(&mut self) -> Result<ExportReport, String> {
        let job = self.begin_export()?;
        let destination = job.destination.clone();
        let result = maintenance::run_export(job);
        self.finish_export(&destination, &result);
        result
    }

    /// The export as a value, for a caller that will run it somewhere else.
    ///
    /// Refused while another maintenance task is in flight: these read and write
    /// the same file and the same rows, and the window has one button rather than
    /// two. The marker is cleared by whichever `finish_` the answer goes to.
    pub fn begin_export(&mut self) -> Result<maintenance::ExportJob, String> {
        self.begin_maintenance(maintenance::Kind::Export)?;
        let credentials = if self.export_includes_credentials {
            Credentials::Included
        } else {
            Credentials::Excluded
        };
        Ok(maintenance::ExportJob {
            text: self.text(),
            profiles: Arc::clone(&self.profiles),
            cores: Arc::clone(&self.cores),
            proxies: Arc::clone(&self.proxies),
            credentials,
            origin: ExportOrigin {
                exported_at: crate::log_file::timestamp(SystemTime::now()),
                source_data_dir: self.settings.data_dir().display().to_string(),
            },
            destination: self.export_destination(),
        })
    }

    /// What an export did, said to the reader, and the marker let go.
    pub fn finish_export(
        &mut self,
        destination: &std::path::Path,
        result: &Result<ExportReport, String>,
    ) {
        self.maintenance = None;
        let t = self.text();
        match result {
            Ok(report) => self.set_notice(Notice::info(export_summary(report, t))),
            // The report names the file it wrote; a failure has no report, so the
            // sentence is the only place the destination appears.
            Err(message) => {
                let _ = destination;
                self.set_notice(Notice::error(message.clone()));
            }
        }
    }

    /// Records that a maintenance task has started, refusing a second one.
    pub(super) fn begin_maintenance(&mut self, kind: maintenance::Kind) -> Result<(), String> {
        if let Some(running) = self.maintenance {
            let message = self.text().maintenance_busy();
            self.set_notice(Notice::error(message.clone()));
            tracing::debug!("refused a {kind:?} while a {running:?} is running");
            return Err(message);
        }
        self.maintenance = Some(kind);
        Ok(())
    }

    /// Applies whatever a maintenance task reported.
    ///
    /// The one door the view's worker answers come through, so a new maintenance
    /// task cannot report its result anywhere the window does not hear it. Each
    /// `finish_` clears the marker, which is what lets the next task run: a task
    /// that never let go would refuse every task after it.
    pub fn finish_maintenance(&mut self, outcome: maintenance::Outcome) {
        match outcome {
            maintenance::Outcome::Exported {
                destination,
                result,
            } => self.finish_export(&destination, &result),
            maintenance::Outcome::Imported { source, result } => {
                self.finish_import(&source, &result)
            }
            maintenance::Outcome::Restored { source, result } => {
                self.finish_restore(&source, &result)
            }
            maintenance::Outcome::Redetected { id, result } => self.finish_redetect(id, &result),
        }
    }

    pub fn set_import_path(&mut self, path: impl Into<String>) {
        self.import_path = path.into();
    }

    /// Where a diagnostics report would write now.
    ///
    /// Recomputed like [`AppState::export_default_path`] and for the same
    /// reason: a window left open overnight still proposes the current second,
    /// and two reports in a row do not aim at the same file.
    pub fn diagnostics_destination(&self) -> PathBuf {
        crate::paths::default_diagnostics_file(self.settings.data_dir(), SystemTime::now())
    }

    /// Writes a report about this installation and says where it went.
    ///
    /// What it collects is in [`crate::diagnostics`]: the build, the settings in
    /// force with the source each came from, the state of every file this
    /// installation keeps, and the end of the activity log. It reads files and
    /// writes one, both locally and quickly, so it runs on the calling thread.
    ///
    /// The database is deliberately not among the files it reads: the proxy
    /// credentials are in it, and a report is meant to be sent to someone.
    pub fn write_diagnostics(&mut self) -> Result<PathBuf, String> {
        let t = self.text();
        let destination = self.diagnostics_destination();
        let result = crate::diagnostics::write_for(&self.settings, &destination, t);
        match &result {
            Ok(()) => self.set_notice(Notice::info(
                t.diag_written(&destination.display().to_string()),
            )),
            Err(message) => self.set_notice(Notice::error(message.clone())),
        }
        result.map(|()| destination)
    }

    /// Where an import would read from, or `None` when nothing was typed.
    pub fn import_source(&self) -> Option<PathBuf> {
        let typed = self.import_path.trim();
        (!typed.is_empty()).then(|| PathBuf::from(typed))
    }

    /// Reads a configuration backup into this installation and says what it did.
    ///
    /// Nothing is confirmed first, and that is the point of import being a
    /// separate verb from restore. Import never overwrites: the worst a mistaken
    /// one can do is add records the user did not want, which the sentence below
    /// says and which is undone one row at a time. Restore replaces, so restore
    /// is the one that will ask.
    ///
    /// Runs on the calling thread for the same reason an export does: one small
    /// file and a handful of rows are local and quick, and nothing here waits on
    /// a far end.
    ///
    /// The rows are reloaded afterwards, because a Profiles page still showing
    /// the list from before the import would contradict the sentence above it.
    /// The import, on the calling thread. See
    /// [`AppState::export_configuration`] for why this exists.
    #[cfg(test)]
    pub(crate) fn import_configuration(&mut self) -> Result<ImportReport, String> {
        let job = self.begin_import()?;
        let source = job.source.clone();
        let result = maintenance::run_import(job);
        self.finish_import(&source, &result);
        result
    }

    /// The import as a value, for a caller that will run it somewhere else.
    ///
    /// Every refusal that can be made before a thread is spawned is made here: no
    /// path typed, another maintenance task in flight.
    pub fn begin_import(&mut self) -> Result<maintenance::ImportJob, String> {
        let Some(source) = self.import_source() else {
            let message = "Type the path of a configuration backup to import.".to_string();
            self.set_notice(Notice::error(message.clone()));
            return Err(message);
        };
        self.begin_maintenance(maintenance::Kind::Import)?;
        Ok(maintenance::ImportJob {
            text: self.text(),
            source,
            data_dir: self.settings.data_dir().to_path_buf(),
            profiles: Arc::clone(&self.profiles),
            cores: Arc::clone(&self.cores),
            proxies: Arc::clone(&self.proxies),
        })
    }

    /// What an import did, said to the reader, with the rows reloaded.
    pub fn finish_import(
        &mut self,
        source: &std::path::Path,
        result: &Result<ImportReport, String>,
    ) {
        self.maintenance = None;
        let t = self.text();
        match result {
            Ok(report) => {
                let summary = import_summary(report, source, t);
                // An import that did less than the file asked for gets the
                // banner, which stays until it is dismissed; one that did
                // exactly what it asked gets a toast. That is what makes the
                // banner mean something when it appears.
                self.set_notice(if report.needs_attention() {
                    Notice::error(summary)
                } else {
                    Notice::info(summary)
                });
                let _ = self.load();
            }
            Err(message) => self.set_notice(Notice::error(message.clone())),
        }
    }

    pub fn set_restore_path(&mut self, path: impl Into<String>) {
        self.restore_path = path.into();
    }

    /// Where a restore would read from, or `None` when nothing was typed.
    pub fn restore_source(&self) -> Option<PathBuf> {
        let typed = self.restore_path.trim();
        (!typed.is_empty()).then(|| PathBuf::from(typed))
    }

    /// Whether this installation holds no cores, proxies or profiles.
    ///
    /// The question a restore's precondition is made of, asked before the
    /// window decides whether to confirm. A read failure is reported rather than
    /// treated as "empty", because guessing here would be the difference between
    /// asking and silently replacing.
    pub fn is_configuration_empty(&self) -> Result<bool, AppError> {
        Ok(
            application::read_configuration(&*self.profiles, &*self.cores, &*self.proxies)?
                .is_empty(),
        )
    }

    /// The names of the profiles whose browsers are active right now.
    ///
    /// Read from freshly reconciled snapshots rather than from a cached row, so
    /// the guard is against what is running at this moment. Restore and the
    /// browser-data copy both refuse while this is non-empty: the first would
    /// delete a row out from under a live process, the second would copy a
    /// directory Chromium is writing.
    fn active_profile_names(&mut self) -> Vec<String> {
        self.refresh_runtime();
        self.rows
            .iter()
            .filter(|row| row.state().is_active())
            .map(|row| row.profile.name.clone())
            .collect()
    }

    /// Makes this installation be the configuration backup at the typed path.
    ///
    /// The mode is the confirmation: [`RestoreMode::OnlyWhenEmpty`] refuses a
    /// populated installation, and [`RestoreMode::Replace`] carries out the
    /// replacement the window has already asked about. Running profiles block it
    /// either way, because deleting the rows of live processes would leave
    /// browsers this window can no longer stop.
    ///
    /// Runs on the calling thread for the same reason an import does - one small
    /// file and a handful of rows - and reloads the rows afterwards, because a
    /// list still showing what was replaced would contradict the sentence above
    /// it.
    /// The restore, on the calling thread. See
    /// [`AppState::export_configuration`] for why this exists.
    #[cfg(test)]
    pub(crate) fn restore_configuration(
        &mut self,
        mode: RestoreMode,
    ) -> Result<RestoreReport, String> {
        let job = self.begin_restore(mode)?;
        let source = job.source.clone();
        let result = maintenance::run_restore(job);
        self.finish_restore(&source, &result);
        result
    }

    /// The restore as a value, for a caller that will run it somewhere else.
    ///
    /// Every refusal the installation can make is made here, before a thread is
    /// spawned: nothing typed, a running browser, a profile another operation is
    /// holding, another maintenance task in flight. What is left for the worker is
    /// what only the file can be refused for.
    pub fn begin_restore(&mut self, mode: RestoreMode) -> Result<maintenance::RestoreJob, String> {
        let t = self.text();
        let Some(source) = self.restore_source() else {
            let message = t.restore_needs_path.to_string();
            self.set_notice(Notice::error(message.clone()));
            return Err(message);
        };

        let running = self.active_profile_names();
        if !running.is_empty() {
            let message = t.restore_running(&running.join(", "));
            self.set_notice(Notice::error(message.clone()));
            return Err(message);
        }

        // A profile that is starting, or that a copy is holding, is not in the
        // snapshot yet or is not in it at all - and the replacement is about to
        // delete the rows both of them are working from. Refused for the same
        // reason a running profile is, which is the case this generalises.
        if let Some((profile, operation)) = self.operations.any() {
            let message = t.install_busy(&self.profile_name(profile), self.busy_phrase(operation));
            self.set_notice(Notice::error(message.clone()));
            return Err(message);
        }

        let installation = self.installation.exclusive().map_err(|e| e.to_string())?;
        self.begin_maintenance(maintenance::Kind::Restore)?;
        Ok(maintenance::RestoreJob {
            _installation: installation,
            text: t,
            source,
            data_dir: self.settings.data_dir().to_path_buf(),
            mode,
            profiles: Arc::clone(&self.profiles),
            cores: Arc::clone(&self.cores),
            proxies: Arc::clone(&self.proxies),
            configuration: Arc::clone(&self.configuration),
        })
    }

    /// What a restore did, said to the reader, with the rows reloaded.
    pub fn finish_restore(
        &mut self,
        source: &std::path::Path,
        result: &Result<RestoreReport, String>,
    ) {
        self.maintenance = None;
        let t = self.text();
        match result {
            Ok(report) => {
                let summary = restore_summary(report, source, t);
                self.proxy_tasks.clear();
                self.verification_tasks.clear();
                self.proxy_tests.clear();
                self.verifications.clear();
                self.set_notice(if report.needs_attention() {
                    Notice::error(summary)
                } else {
                    Notice::info(summary)
                });
                let _ = self.load();
            }
            Err(message) => self.set_notice(Notice::error(message.clone())),
        }
    }

    pub fn set_browser_data_path(&mut self, path: impl Into<String>) {
        self.browser_data_path = path.into();
    }

    /// The browser-data backup directory, or `None` when nothing was typed.
    pub fn browser_data_directory(&self) -> Option<PathBuf> {
        let typed = self.browser_data_path.trim();
        (!typed.is_empty()).then(|| PathBuf::from(typed))
    }

    /// Builds one browser-data copy job, or says why it cannot be started.
    ///
    /// The run happens on a worker, so this only gathers: the profiles as
    /// storage holds them, the identifiers that are active, and the directory.
    /// Every refusal that can be made before a thread is spawned is made here -
    /// an empty path, an empty installation, a running profile, a profile
    /// something else is already doing something with - so the worker is never
    /// started for a copy that could not run.
    ///
    /// The second half of the answer is the lease on those profiles: it is taken
    /// here, and it is the worker that gives it back, because the worker is what
    /// knows the copy is over.
    pub fn browser_data_job(
        &mut self,
        direction: Direction,
    ) -> Result<(BrowserDataJob, Held), String> {
        let installation = self.installation.shared().map_err(|e| e.to_string())?;
        let t = self.text();
        let directory = self
            .browser_data_directory()
            .ok_or_else(|| "Type the directory to keep browser data in.".to_string())?;

        if self
            .profiles
            .list()
            .map_err(|error| error.to_string())?
            .is_empty()
        {
            return Err("There are no profiles whose browser data could be copied.".to_string());
        }

        let running = self.active_profile_names();
        if !running.is_empty() {
            return Err(t.copy_running(&running.join(", ")));
        }

        let profiles = self.profiles.list().map_err(|error| error.to_string())?;
        let active: Vec<ProfileId> = self
            .rows
            .iter()
            .filter(|row| row.state().is_active())
            .map(|row| row.profile.id)
            .collect();

        // Held from here, not from the worker's first line: the profiles a copy
        // will write are taken before the thread exists, so nothing can start one
        // of them in the moment between the click and the copy. Every profile is
        // in the job - a copy covers the installation - so a copy is what makes
        // the whole installation busy.
        let ids: Vec<ProfileId> = profiles.iter().map(|profile| profile.id).collect();
        let lease = self
            .operations
            .lease(&ids, Operation::Copying)
            .map_err(|busy| {
                t.profile_busy(
                    &self.profile_name(busy.profile),
                    self.busy_phrase(busy.operation),
                )
            })?;

        let job = BrowserDataJob {
            direction,
            profiles,
            running: active,
            directory,
        };
        Ok((job, lease.with_installation(installation)))
    }

    /// Reports the outcome of a browser-data copy the worker finished.
    ///
    /// A copy changes no stored row, so nothing is reloaded: the notice is the
    /// whole of what the window has to do.
    pub fn finish_browser_data(
        &mut self,
        direction: Direction,
        outcome: Result<BrowserDataReport, String>,
    ) {
        let t = self.text();
        let notice = match outcome {
            Ok(report) => Notice::info(browser_data_summary(direction, &report, t)),
            Err(reason) => Notice::error(reason),
        };
        self.set_notice(notice);
    }
}
