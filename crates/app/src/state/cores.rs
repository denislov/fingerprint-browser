//! Browser cores: which ones are registered, where their binaries are, and what
//! version they report.
//!
//! A core is what a profile launches with, so most of what lives here is either a
//! reading of the filesystem (is the binary still there) or the version probe -
//! the one maintenance task that is about a core rather than the configuration.
//! See [`super`] for the state these methods belong to, and
//! [`crate::maintenance`] for the worker the probe runs on.

use super::*;

impl AppState {
    /// Every core with the profiles that use it, for the Browser Cores page.
    pub fn core_rows(&self) -> Result<Vec<CoreRow>, AppError> {
        let usage = self.cores.usage()?;
        Ok(self
            .cores
            .list()?
            .into_iter()
            .map(|core| CoreRow {
                present: core.executable.is_file(),
                used_by: usage.get(&core.id).cloned().unwrap_or_default(),
                core,
            })
            .collect())
    }

    pub fn core(&self, id: CoreId) -> Option<BrowserCore> {
        self.cores.get(id).ok().flatten()
    }

    /// Registers a browser binary, reading its version rather than trusting one.
    pub fn add_core(&mut self, name: Option<String>, path: PathBuf) -> Result<CoreId, AppError> {
        let t = self.text();
        let core = self.record(self.cores.add(name, path))?;
        let id = core.id;
        self.set_notice(Notice::info(t.core_added(
            &core.name,
            &core.version,
            core.major,
        )));
        Ok(id)
    }

    /// Saves a core, re-reading the version when its executable changed.
    pub fn update_core(&mut self, core: BrowserCore) -> Result<(), AppError> {
        let t = self.text();
        let saved = self.record(self.cores.update(core))?;
        self.set_notice(Notice::info(t.core_saved(
            &saved.name,
            &saved.version,
            saved.major,
        )));
        // The rows carry display names that came from this core.
        self.load_rows()?;
        Ok(())
    }

    /// Re-reads a core's version, for a binary that was replaced in place.
    ///
    /// The synchronous half of this is for tests, for the same reason the other
    /// three are: the window runs the probe on a worker.
    #[cfg(test)]
    pub(crate) fn redetect_core(&mut self, id: CoreId) -> Result<(), AppError> {
        // Reading a version starts the core's binary and waits for it to answer,
        // which is why the window runs this on a worker; a caller that wants the
        // answer before returning gets the same three steps in a row.
        let job = self.begin_redetect(id).map_err(AppError::Other)?;
        let result = maintenance::run_redetect(job);
        self.finish_redetect(id, &result);
        Ok(())
    }

    /// The version probe as a value, for a caller that will run it somewhere else.
    pub fn begin_redetect(&mut self, id: CoreId) -> Result<maintenance::RedetectJob, String> {
        self.begin_maintenance(maintenance::Kind::Redetect)?;
        Ok(maintenance::RedetectJob {
            id,
            cores: Arc::clone(&self.cores),
        })
    }

    /// What a version probe found, said to the reader, with the rows reloaded.
    pub fn finish_redetect(&mut self, id: CoreId, result: &Result<BrowserCore, String>) {
        self.maintenance = None;
        let t = self.text();
        match result {
            Ok(refreshed) => {
                self.set_notice(Notice::info(t.core_refreshed(
                    &refreshed.name,
                    &refreshed.version,
                    refreshed.major,
                )));
                let _ = self.load_rows();
            }
            // The probe is the failure: a missing binary, a version that cannot be
            // read, a program that never answers.
            Err(message) => {
                let _ = id;
                self.set_notice(Notice::error(message.clone()));
            }
        }
    }

    /// Removes a core. Refused while a profile still launches with it.
    pub fn delete_core(&mut self, id: CoreId) -> Result<(), AppError> {
        let t = self.text();
        let name = self
            .core(id)
            .map(|core| core.name)
            .unwrap_or_else(|| id.to_string());
        self.record(self.cores.delete(id))?;
        self.set_notice(Notice::info(t.core_deleted(&name)));
        Ok(())
    }

    pub fn has_core(&self) -> bool {
        self.cores
            .list()
            .map(|cores| !cores.is_empty())
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub(super) fn default_core_id(&self) -> Result<CoreId, AppError> {
        let t = self.text();
        self.cores
            .list()?
            .first()
            .map(|core| core.id)
            .ok_or_else(|| AppError::Conflict(t.no_core_registered.to_string()))
    }

    /// The cores a profile can be put on, for the profile form.
    ///
    /// A core with no detected version is listed too: it is a real choice that
    /// will be refused at launch, and hiding it would leave the user with a
    /// form that cannot explain where their core went.
    pub fn core_choices(&self) -> Vec<CoreChoice> {
        self.cores
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|core| {
                let capabilities = core.capabilities();
                CoreChoice {
                    generation: generation_line(&core),
                    exclusions_honoured: capabilities
                        .as_ref()
                        .is_some_and(|capabilities| capabilities.supports_disable_spoofing),
                    major: core.major,
                    name: core.name,
                    id: core.id,
                }
            })
            .collect()
    }
}
