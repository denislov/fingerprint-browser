//! Profiles: what a profile is, what can be done to it, and the two readings the
//! window can take of a running one.
//!
//! This is the largest topic in the state and the one the rest of it is about: the
//! rows draw a profile, the runtime commands act on one, and a verification reads a
//! fingerprint out of the browser of one. It is also where the proxy-gated start
//! lives - the start that waits for an answer through the profile's own proxy
//! before it queues the launch - which is a rule about a profile rather than about
//! the runtime underneath it.
//!
//! See [`super`] for the state these methods belong to, [`super::configuration`]
//! for the tasks that act on the whole installation, and [`super::proxies`] for the
//! proxy rules a profile's egress answers to.

use super::*;

impl AppState {
    /// The address question for a profile, when it has one to be asked.
    ///
    /// A profile with no proxy claims nothing about an address, so it is not
    /// asked: the endpoint learns the address it was asked from, and there is
    /// no reason to spend that on a browser that was never meant to leave by
    /// anywhere else.
    ///
    /// The expectation is the address the proxy was *tested* at, and it is
    /// absent until a test has run. An untested proxy leaves nothing to
    /// disagree with, and a reading is still worth having: it says where the
    /// traffic went, which is the first thing anyone wants to know.
    pub(super) fn egress_job(&self, proxy_id: Option<ProxyId>) -> Option<EgressJob> {
        let proxy_id = proxy_id?;
        Some(EgressJob {
            echo_url: self.settings.echo_url().to_string(),
            expected: self
                .proxy_tests
                .get(&proxy_id)
                .and_then(ProxyTest::exit_ip)
                .map(str::to_string),
        })
    }

    pub(super) fn verification_job(&mut self, id: ProfileId) -> Result<VerificationJob, AppError> {
        let t = self.text();
        let row = self
            .rows
            .iter()
            .find(|row| row.profile.id == id)
            .ok_or_else(|| AppError::Other(t.profile_not_found(&id.to_string())))?;
        let port = row
            .cdp_port()
            .ok_or_else(|| AppError::Other(t.verify_needs_browser()))?;
        let core = self
            .cores
            .get(row.profile.core_id)?
            .ok_or_else(|| AppError::Other(t.core_not_found(&row.profile.core_id.to_string())))?;
        // A core whose version was never read has no capability table, and
        // asking for one would be asking what the engine may claim. This is the
        // same refusal the launch path makes.
        let capabilities = core
            .capabilities()
            .ok_or_else(|| AppError::Conflict(t.core_has_no_version(&core.name)))?;
        Ok(VerificationJob {
            task: crate::task::TaskId::new(),
            session: row.snapshot.as_ref().map_or(0, |s| s.acknowledged_start),
            profile_id: id,
            port,
            profile: row.profile.fingerprint.clone(),
            capabilities,
            egress: self.egress_job(row.profile.proxy_id),
        })
    }

    pub fn verification(&self, id: ProfileId) -> Option<&Verification> {
        self.verifications.get(&id)
    }

    /// Clears a verification result, e.g. after the profile restarted: a new
    /// browser has a new fingerprint.
    pub fn forget_verification(&mut self, id: ProfileId) {
        self.verification_tasks.remove(&id);
        self.verifications.remove(&id);
    }

    pub fn complete_verification(
        &mut self,
        job: &VerificationJob,
        outcome: Result<VerificationReport, String>,
    ) {
        if self.verification_tasks.get(&job.profile_id) != Some(&job.task) {
            return;
        }
        let current = self.runtime.snapshot(job.profile_id);
        if !current.is_some_and(|s| {
            s.state.is_running()
                && s.acknowledged_start == job.session
                && s.cdp_port == Some(job.port)
        }) {
            self.forget_verification(job.profile_id);
            return;
        }
        self.verification_tasks.remove(&job.profile_id);
        self.finish_verification(job.profile_id, outcome);
    }

    /// Records the outcome of a verification the view ran on a worker.
    pub fn finish_verification(
        &mut self,
        id: ProfileId,
        outcome: Result<VerificationReport, String>,
    ) {
        let t = self.text();
        let verification = match outcome {
            Ok(report) if report.discrepancies.is_empty() => Verification::Confirmed(report),
            Ok(report) => Verification::Disagreements(report),
            Err(reason) => Verification::Unreadable(reason),
        };
        // A reading is the answer to a question the user asked, so it belongs
        // in the history as well as on the row.
        match &verification {
            Verification::Confirmed(report) => {
                self.append_log(LogLevel::Info, Some(id), confirmed_line(report, t));
                self.toast(ToastKind::Success, t.fingerprint_confirmed_toast);
            }
            Verification::Disagreements(report) => {
                let message = t.verification_claims_toast(report.discrepancies.len());
                self.toast(
                    ToastKind::Warning,
                    t.verification_claims_details(report.discrepancies.len()),
                );
                self.append_log(LogLevel::Warning, Some(id), message);
            }
            Verification::Unreadable(reason) => {
                self.toast(ToastKind::Error, t.fingerprint_read_failed(reason));
                self.append_log(LogLevel::Error, Some(id), t.fingerprint_read_failed(reason));
            }
            Verification::Running => {}
        }
        self.verifications.insert(id, verification);
    }

    /// Claims the verification slot for a profile and assembles the job.
    ///
    /// Refuses when the profile is not running, because a fingerprint can only
    /// be read out of a live browser, and while another verification is in
    /// flight for the same profile.
    pub fn begin_verification(&mut self, id: ProfileId) -> Result<VerificationJob, AppError> {
        let t = self.text();
        let existing = self.verifications.get(&id);
        if existing.is_some_and(Verification::is_running) {
            return Err(AppError::Other(t.verification_busy(&id.to_string())));
        }
        let job = self.verification_job(id)?;
        self.verification_tasks.insert(id, job.task);
        self.verifications.insert(id, Verification::Running);
        Ok(job)
    }

    /// The stored profile behind a row, for the editor to start from.
    pub fn profile(&self, id: ProfileId) -> Option<BrowserProfile> {
        self.rows
            .iter()
            .find(|row| row.profile.id == id)
            .map(|row| row.profile.clone())
    }

    /// Removes a profile. Its browser data is kept on disk: deleting a profile
    /// should not be the same decision as destroying its sessions.
    pub fn delete_profile(&mut self, id: ProfileId) -> Result<(), AppError> {
        if self.queued_starts.contains_key(&id) {
            return Err(AppError::Conflict(
                "stop the queued start before deleting this profile".into(),
            ));
        }
        // A profile that is being checked has no browser yet; leaving the check
        // pending would start a profile that no longer exists, so it is called
        // off with the record.
        if self.pending_starts.remove(&id).is_some() {
            self.operations.free(id, Operation::Starting);
        }
        self.record(self.profiles.delete(id))?;
        self.forget_verification(id);
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.load_rows()?;
        self.refresh_runtime();
        Ok(())
    }

    /// Copies a profile under a new name, id, seed and data directory.
    pub fn duplicate_profile(&mut self, id: ProfileId) -> Result<ProfileId, AppError> {
        let t = self.text();
        let name = t.profile_copy_name(&self.next_profile_name());
        let duplicated = self.record(self.profiles.duplicate(id, name))?;
        self.load_rows()?;
        self.refresh_runtime();
        self.selected = Some(duplicated.id);
        Ok(duplicated.id)
    }

    /// Writes an edited profile back, keeping the row list in step.
    pub fn update_profile(&mut self, profile: BrowserProfile) -> Result<(), AppError> {
        let id = profile.id;
        self.record(self.profiles.update(profile))?;
        self.forget_verification(id);
        self.load_rows()?;
        self.refresh_runtime();
        Ok(())
    }

    /// Takes the profile for a start that is about to be queued.
    ///
    /// The lease outlives this call on purpose: `RuntimeService::start` returns as
    /// soon as the command is queued, so releasing here would leave exactly the
    /// window this is for - a copy that could begin while the browser is coming
    /// up. [`AppState::refresh_runtime`] gives it back only when the snapshot
    /// acknowledges this particular request, including failed or cancelled starts.
    pub(super) fn begin_starting(&self, id: ProfileId) -> Result<(), AppError> {
        let _installation = self.installation.shared()?;
        let t = self.text();
        self.operations
            .take(id, Operation::Starting)
            .map_err(|busy| {
                AppError::Other(t.profile_busy(
                    &self.profile_name(busy.profile),
                    self.busy_phrase(busy.operation),
                ))
            })
    }

    /// Calls off every start that is waiting on one proxy.
    ///
    /// A proxy that was edited or removed cannot answer the question that was
    /// asked about it - the old reading is a claim about the old upstream - so the
    /// checks waiting on it are called off rather than left waiting for an answer
    /// that will never be matched to them.
    pub(super) fn cancel_pending_starts(&mut self, proxy: ProxyId, reason: &str) {
        let waiting: Vec<ProfileId> = self
            .pending_starts
            .iter()
            .filter(|(_, pending)| pending.proxy == proxy)
            .map(|(profile, _)| *profile)
            .collect();
        for profile in waiting {
            self.cancel_pending_start(profile, reason);
        }
    }

    /// Calls off a start that is waiting for a proxy, and says why.
    ///
    /// The lease goes back first: whatever went wrong with the check, the profile
    /// is free again, and a wait that nobody is going to answer must not be what
    /// keeps it that way.
    pub(super) fn cancel_pending_start(&mut self, profile: ProfileId, reason: &str) {
        self.pending_starts.remove(&profile);
        self.operations.free(profile, Operation::Starting);
        self.set_checking_proxy(profile, false);
        let message = {
            let t = self.text();
            t.start_not_checked(&self.profile_name(profile), reason)
        };
        self.set_notice(Notice::error(message.clone()));
        self.toast(ToastKind::Error, message);
    }

    /// Marks a row as waiting for its proxy, now rather than at the next tick.
    pub(super) fn set_checking_proxy(&mut self, id: ProfileId, checking: bool) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.profile.id == id) {
            row.checking_proxy = checking;
        }
    }

    pub fn stop(&mut self, id: ProfileId) -> Result<(), AppError> {
        // A stop pressed while the proxy is still being asked calls the start
        // off rather than stopping a browser that was never launched: the command
        // would reach a runtime with nothing to stop, and the answer to the check
        // would then start the profile the reader just called off.
        if self.pending_starts.remove(&id).is_some() {
            self.operations.free(id, Operation::Starting);
            self.set_checking_proxy(id, false);
            self.refresh_runtime();
            return Ok(());
        }

        self.record(self.runtime.stop(id))?;
        // The reading described a browser that no longer exists.
        self.forget_verification(id);
        self.refresh_runtime();
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn restart(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.begin_starting(id)?;
        if let Err(error) = self.queue_opening(id, Opening::Restart) {
            self.operations.free(id, Operation::Starting);
            return Err(error);
        }
        Ok(())
    }

    /// Starts a profile without asking its proxy first.
    ///
    /// The command half of [`AppState::begin_opening`], for a caller that has
    /// already decided: a test, or the acceptance harness that drives the state
    /// directly. The window never uses it - `begin_opening` is what makes a start
    /// wait for the proxy, and a second way in would be a way around that.
    #[cfg(test)]
    pub(crate) fn start(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.begin_starting(id)?;
        if let Err(error) = self.queue_opening(id, Opening::Start) {
            self.operations.free(id, Operation::Starting);
            return Err(error);
        }
        Ok(())
    }

    /// Queues the command an opening was holding, for a profile that holds its
    /// lease.
    ///
    /// The second half of [`AppState::begin_opening`]: either nothing had to be
    /// asked, or the proxy has just answered. Fails only when the command itself
    /// does, and the caller is what gives the lease back in that case.
    pub(super) fn queue_opening(&mut self, id: ProfileId, how: Opening) -> Result<(), AppError> {
        let result = match how {
            Opening::Start => self.runtime.start(id),
            Opening::Restart => self.runtime.restart(id),
        };
        let request = self.record(result)?;
        self.queued_starts.insert(id, request);
        if how == Opening::Restart {
            // A restarted browser is a new browser: the old reading is stale.
            self.forget_verification(id);
        }
        self.refresh_runtime();
        Ok(())
    }

    /// Opens a profile, once its proxy has said it can carry the traffic.
    ///
    /// The gate is the whole point of this method. A profile that leaves through a
    /// proxy is only useful while that proxy carries traffic, and a launch that
    /// goes ahead without it produces the worst of both outcomes: a browser that
    /// looks healthy while its traffic leaks, or one whose first page never
    /// loads. So the command is not queued until one request has come back
    /// through the same engine the profile would use - [`crate::proxy_tester`],
    /// the same question the Proxies page asks by hand.
    ///
    /// The lease is taken here, before the check, and that is what makes the two
    /// halves one operation: while the proxy is being asked, the profile is
    /// already busy - a second start, a restart and a browser-data copy are all
    /// refused - and a check that fails gives the lease back, so the profile can
    /// be tried again as soon as the proxy is fixed.
    ///
    /// A profile with no proxy has nothing to ask, so it is queued here and the
    /// answer is [`StartGate::Queued`].
    pub fn begin_opening(&mut self, id: ProfileId, how: Opening) -> Result<StartGate, AppError> {
        self.begin_starting(id)?;

        let Some(proxy_id) = self.profile(id).and_then(|profile| profile.proxy_id) else {
            // Nothing to ask: the request a check would send is the one the
            // browser makes for its own first page anyway.
            return match self.queue_opening(id, how) {
                Ok(()) => Ok(StartGate::Queued),
                Err(error) => {
                    // The command never reached the runtime, so no snapshot will
                    // ever answer for it: give the profile back here rather than
                    // at the tick.
                    self.operations.free(id, Operation::Starting);
                    Err(error)
                }
            };
        };

        // A test of this proxy is already in flight - the Proxies page's own
        // button, or another profile's start. Its answer is the answer this
        // opening needs, so it waits for it instead of refusing and making the
        // reader press Start again a few seconds later.
        let pending = PendingStart {
            proxy: proxy_id,
            how,
            asked: Instant::now(),
        };
        if self
            .proxy_tests
            .get(&proxy_id)
            .is_some_and(ProxyTest::is_running)
        {
            self.pending_starts.insert(id, pending);
            self.set_checking_proxy(id, true);
            return Ok(StartGate::Awaiting);
        }

        match self.begin_proxy_test(proxy_id) {
            Ok(job) => {
                self.pending_starts.insert(id, pending);
                self.set_checking_proxy(id, true);
                Ok(StartGate::Checking(Box::new(job)))
            }
            Err(error) => {
                // No such proxy, or one that cannot be tested at all. Nothing is
                // going to answer for this profile, so it is refused now rather
                // than left waiting for an answer that was never asked for.
                self.operations.free(id, Operation::Starting);
                let message = {
                    let t = self.text();
                    t.start_not_checked(&self.profile_name(id), &error.to_string())
                };
                self.set_notice(Notice::error(message));
                Err(error)
            }
        }
    }

    /// Creates the profile a filled-in form asked for.
    ///
    /// The draft carries the whole profile the user configured, so nothing is
    /// defaulted behind their back: the seed, the core and the fingerprint are
    /// the ones on the form. Only what the form does not ask about (id, data
    /// directory, start target) is left to the service.
    pub fn create_profile_from(&mut self, draft: NewProfile) -> Result<ProfileId, AppError> {
        let t = self.text();
        let profile = self.record(self.profiles.create(draft))?;
        let id = profile.id;
        self.load()?;
        self.selected = Some(id);
        self.set_notice(Notice::info(t.profile_created(&profile.name)));
        Ok(id)
    }

    /// Creates a profile with the default fingerprint on the first core.
    ///
    /// The window's New Profile form goes through [`Self::create_profile_from`]
    /// instead, so this is only the shortcut a caller without a form in front of
    /// it uses - the acceptance harness that needs a working row, and the tests.
    #[cfg(test)]
    pub fn create_profile(&mut self, name: &str) -> Result<ProfileId, AppError> {
        let core_id = self.core_id()?;
        self.create_profile_from(NewProfile {
            name: name.trim().to_string(),
            core_id,
            user_data_dir: None,
            fingerprint: None,
            proxy_id: None,
            window: None,
            start_target: None,
        })
    }
}
