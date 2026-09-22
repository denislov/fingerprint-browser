//! Proxies: the rows the page shows, the rules an edit answers to, and the tests
//! that go out through one.
//!
//! The two halves are here together because they are one topic: a proxy is only
//! as good as the last request that came back through it, which is why a stored
//! proxy carries a reading and why a test is what keeps that reading honest.
//! See [`super`] for the state these methods belong to and [`crate::proxy_tester`]
//! for the worker a test runs on.

use super::*;

impl AppState {
    pub(super) fn proxy_names(&self) -> Result<HashMap<ProxyId, String>, AppError> {
        Ok(self
            .proxies
            .list()?
            .into_iter()
            .map(|proxy| (proxy.id, proxy.name))
            .collect())
    }

    /// Every proxy with the profiles assigned to it, for the Proxies page.
    pub fn proxy_rows(&self) -> Result<Vec<ProxyRow>, AppError> {
        let usage = self.proxies.usage()?;
        Ok(self
            .proxies
            .list()?
            .into_iter()
            .map(|proxy| ProxyRow {
                used_by: usage.get(&proxy.id).cloned().unwrap_or_default(),
                proxy,
            })
            .collect())
    }

    /// `(id, name)` for every proxy, for the profile editor's picker.
    pub fn proxy_choices(&self) -> Vec<(ProxyId, String)> {
        self.proxies
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|proxy| (proxy.id, proxy.name))
            .collect()
    }

    pub fn proxy(&self, id: ProxyId) -> Option<ProxyProfile> {
        self.proxies.get(id).ok().flatten()
    }

    pub fn create_proxy(
        &mut self,
        name: &str,
        outbound: ProxyOutbound,
    ) -> Result<ProxyId, AppError> {
        let t = self.text();
        let created = self.record(self.proxies.create(NewProxy {
            name: name.trim().to_string(),
            outbound,
        }))?;
        let id = created.id;
        self.set_notice(Notice::info(t.proxy_created(&created.name)));
        Ok(id)
    }

    pub fn update_proxy(&mut self, proxy: ProxyProfile) -> Result<(), AppError> {
        let t = self.text();
        self.record(self.proxies.update(proxy.clone()))?;
        self.set_notice(Notice::info(t.proxy_saved(&proxy.name)));
        // The old result was about the old upstream, and would read as a claim
        // about the new one. Last, so that a start this edit called off is what
        // the banner says: a profile left waiting would otherwise read as one
        // that was never asked to start.
        self.forget_proxy_test(proxy.id);
        Ok(())
    }

    /// Removes a proxy. Refused while a profile still points at it, because the
    /// alternative is that profile quietly going direct.
    pub fn delete_proxy(&mut self, id: ProxyId) -> Result<(), AppError> {
        let t = self.text();
        let name = self
            .proxy(id)
            .map(|proxy| proxy.name)
            .unwrap_or_else(|| t.a_removed_profile.to_string());
        self.record(self.proxies.delete(id))?;
        self.set_notice(Notice::info(t.proxy_deleted(&name)));
        // Nothing points at this id any more, so a result kept for it could
        // only ever be shown against a different proxy.
        self.forget_proxy_test(id);
        Ok(())
    }

    /// Claims the test slot for a proxy and assembles the job.
    ///
    /// A proxy can be tested whether or not anything is running, which is the
    /// point: a proxy has to be judged before a profile is launched with it.
    /// Refused only while another test of the same proxy is in flight, which
    /// would start a second engine for an answer already on its way.
    pub fn begin_proxy_test(&mut self, id: ProxyId) -> Result<ProxyTestJob, AppError> {
        let t = self.text();
        if self.proxy_tests.get(&id).is_some_and(ProxyTest::is_running) {
            return Err(AppError::Other(t.proxy_test_busy(&id.to_string())));
        }
        let proxy = self
            .proxies
            .get(id)?
            .ok_or_else(|| AppError::NotFound(t.proxy_not_found(&id.to_string())))?;
        let job = ProxyTestJob {
            task: crate::task::TaskId::new(),
            proxy_id: id,
            proxy,
            echo_url: self.settings.echo_url().to_string(),
            live_port: self.live_port_for(id),
        };
        self.proxy_tasks.insert(id, job.task);
        self.proxy_tests.insert(id, ProxyTest::Running);
        Ok(job)
    }

    pub fn complete_proxy_test(&mut self, job: &ProxyTestJob, outcome: Result<Diagnosis, Fault>) {
        if self.proxy_tasks.get(&job.proxy_id) != Some(&job.task) {
            return;
        }
        self.proxy_tasks.remove(&job.proxy_id);
        self.finish_proxy_test(job.proxy_id, job.is_live(), outcome);
    }

    /// Records the outcome of a test the view ran on a worker.
    ///
    /// `live` says whether the request went through an engine that was already
    /// up, because the two answers mean different things: one is a rehearsal,
    /// the other is what is happening to a profile's traffic right now.
    pub fn finish_proxy_test(
        &mut self,
        id: ProxyId,
        live: bool,
        outcome: Result<Diagnosis, Fault>,
    ) {
        let t = self.text();
        let test = match outcome {
            Ok(diagnosis) => ProxyTest::Passed(ProxyReading {
                exit_ip: diagnosis.exit_ip,
                elapsed: diagnosis.elapsed,
                live,
            }),
            Err(fault) => ProxyTest::Failed(fault),
        };
        let name = self
            .proxy(id)
            .map(|proxy| proxy.name)
            .unwrap_or_else(|| t.a_removed_profile.to_string());

        // The openings that were waiting for this answer, taken out first: the
        // answer is what decides them, and a test that is only a reading has
        // none.
        let waiting: Vec<(ProfileId, Opening)> = self
            .pending_starts
            .iter()
            .filter(|(_, pending)| pending.proxy == id)
            .map(|(profile, pending)| (*profile, pending.how))
            .collect();
        for (profile, _) in &waiting {
            self.pending_starts.remove(profile);
            self.set_checking_proxy(*profile, false);
        }

        match &test {
            ProxyTest::Passed(reading) => {
                if waiting.is_empty() {
                    self.toast(
                        ToastKind::Success,
                        t.proxy_traffic_from(&name, &reading.exit_ip),
                    );
                }
                // A start that waited for this reading is the reason the reading
                // was taken, so its sentence is the one the reader is waiting
                // for. The profile's lease is already held, and queuing is what
                // the answer buys.
                for (profile, how) in waiting {
                    let profile_name = self.profile_name(profile);
                    match self.queue_opening(profile, how) {
                        Ok(()) => self.toast(
                            ToastKind::Success,
                            t.started_through_proxy(&profile_name, &name, &reading.exit_ip),
                        ),
                        Err(_) => {
                            // `queue_opening` recorded the reason; nothing may be
                            // left holding the profile.
                            self.operations.free(profile, Operation::Starting);
                        }
                    }
                }
            }
            ProxyTest::Failed(fault) => {
                if waiting.is_empty() {
                    // The class is what to act on, so it goes first; the evidence
                    // follows, because a dashboard line is too short to hold it
                    // and this is the one moment the user is thinking about this
                    // proxy.
                    self.toast(
                        ToastKind::Error,
                        t.proxy_no_traffic_reached(&name, &fault.to_string()),
                    );
                }
                for (profile, _) in waiting {
                    // The refusal, and the profile is free again: a proxy that is
                    // down now may be up in a minute, and the reader has to be
                    // able to press Start then.
                    self.operations.free(profile, Operation::Starting);
                    let message = t.proxy_blocks_start(
                        &self.profile_name(profile),
                        &name,
                        &fault.to_string(),
                    );
                    self.set_notice(Notice::error(message.clone()));
                    self.toast(ToastKind::Error, message);
                }
            }
            ProxyTest::Running => {}
        }
        // The path is the half of the answer the row cannot show, and the class
        // is what to act on. Both go in the record, which outlives the row.
        if let Some(detail) = test.detail(self.text()) {
            let level = if test.fault().is_some() {
                LogLevel::Error
            } else {
                LogLevel::Info
            };
            self.append_log(level, None, t.proxy_test_log(&name, &detail));
        }
        self.proxy_tests.insert(id, test);
    }

    /// Clears a test result. Editing a proxy is enough: a different upstream is
    /// a different answer, and the old one would read as a claim about the new.
    pub fn forget_proxy_test(&mut self, id: ProxyId) {
        self.proxy_tasks.remove(&id);
        self.proxy_tests.remove(&id);
        let reason = self.text().proxy_check_dropped();
        self.cancel_pending_starts(id, reason);
    }

    /// What the last test of this proxy found, if there was one.
    pub fn proxy_test(&self, id: ProxyId) -> Option<&ProxyTest> {
        self.proxy_tests.get(&id)
    }

    /// The SOCKS port of an engine already up for a profile using this proxy.
    ///
    /// An engine that is up is the one that would have to forward, so probing
    /// it asks about the live path instead of a rehearsal.
    ///
    /// The port is only considered when the profile says it is running. The
    /// supervisor clears the port when a profile stops, but this decision is
    /// worth its own gate: probing a port nothing is listening on would report
    /// a timeout, and a timeout here reads as "this proxy is broken" when the
    /// truth is that nothing was asked in the first place.
    fn live_port_for(&self, id: ProxyId) -> Option<u16> {
        self.rows
            .iter()
            .filter(|row| row.profile.proxy_id == Some(id))
            .filter(|row| matches!(row.state(), RuntimeState::Running))
            .find_map(|row| row.socks_port())
    }
}
