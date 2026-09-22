//! Launch preparation, cancellation and readiness.
use super::*;

impl RuntimeSupervisor {
    pub(super) fn start_profile(&mut self, params: StartParams) {
        let profile_id = params.profile.id;

        if self.active_sessions.contains_key(&profile_id) {
            self.emit(RuntimeEvent::Warning {
                profile_id,
                message: format!("profile {profile_id} is already running"),
            });
            return;
        }

        // Set state to Starting
        self.set_snapshot_state(profile_id, RuntimeState::Starting);
        self.emit(RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Starting,
        });

        if self.poll_start_commands(profile_id) {
            self.stop_profile(profile_id);
            return;
        }

        // Everything this attempt acquires lives in one value from here on, and
        // every way out of this function that is not the session below drops it -
        // which ends the children it still holds, removes the temporary config and
        // removes the record. The five hand-written rollbacks that used to be here
        // each had to remember every resource acquired before them, and a sixth
        // failure point added later would have had to remember them too.
        let (mut attempt, record) = match self.begin_attempt(&params) {
            Ok(started) => started,
            Err(failure) => {
                // The attempt was dropped inside `begin_attempt`: nothing is
                // running by the time the profile is told it failed.
                self.report_start_failure(profile_id, failure);
                return;
            }
        };

        // Probe CDP readiness. The record is on disk and both children are
        // running; this is the part that decides whether they become a session.
        let deadline = std::time::Instant::now() + self.cdp_ready_timeout;
        let readiness = loop {
            attempt.note_cancelled(self.poll_start_commands(profile_id));
            if attempt.cancelled() {
                break Err("startup cancelled".to_string());
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break Err("CDP readiness timed out".to_string());
            }
            let live = (|| -> Result<(), String> {
                if let Some(status) = attempt
                    .browser_mut()
                    .try_wait()
                    .map_err(|e| e.to_string())?
                {
                    return Err(format!("browser exited during CDP readiness: {status}"));
                }
                if let Some(state) = attempt.xray_status()
                    && let Some(status) = state.map_err(|e| e.to_string())?
                {
                    return Err(format!("Xray exited during CDP readiness: {status}"));
                }
                Ok(())
            })();
            if let Err(error) = live {
                break Err(error);
            }
            match self.cdp_probe.wait_ready(
                cdp_port_of(&attempt),
                remaining.min(Duration::from_millis(100)),
            ) {
                Ok(info) => {
                    attempt.note_cancelled(self.poll_start_commands(profile_id));
                    if attempt.cancelled() {
                        break Err("startup cancelled".to_string());
                    }
                    if !matches!(attempt.browser_mut().try_wait(), Ok(None)) {
                        break Err("browser exited during CDP readiness".into());
                    }
                    // Recheck Xray after the probe before advertising Running.
                    if let Some(status) = attempt.xray_status()
                        && !matches!(status, Ok(None))
                    {
                        break Err("Xray exited during CDP readiness".into());
                    }
                    break Ok(info);
                }
                Err(crate::CdpError::Timeout { .. }) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => break Err(error.to_string()),
            }
        };
        match readiness {
            Ok(_info) => {
                let cancelled = attempt.cancelled();
                debug_assert!(!cancelled, "a cancelled start never reports readiness");
                let browser = attempt.take_browser();
                let xray = attempt.take_xray();
                let xray_config = attempt.config().map(std::path::Path::to_path_buf);
                let (cdp_port, socks_port, effective_args) = (
                    attempt.cdp_port,
                    attempt.socks_port,
                    attempt.effective_args.clone(),
                );
                // From here the children, the config and the record belong to the
                // running session, and must not be undone by the drop below.
                attempt.keep();

                let browser_pid = record.browser.pid;
                let xray_pid = record.xray.as_ref().map(|xray| xray.pid);
                let session = ActiveSession {
                    _profile_id: profile_id,
                    browser: Held::Owned(browser),
                    xray: xray.map(Held::Owned),
                    xray_config,
                    _cdp_port: cdp_port,
                    _socks_port: socks_port,
                    _effective_args: effective_args.clone(),
                    stopping: false,
                    stop_failed: None,
                };

                self.active_sessions.insert(profile_id, session);

                // The record has to be readable back, or the next run will find
                // a process it cannot identify. A disagreement is reported now
                // rather than at the next start.
                if let Some(disagreement) =
                    journal::confirm(&record, self.process_inspector.as_ref())
                {
                    self.emit(RuntimeEvent::Warning {
                        profile_id,
                        message: format!(
                            "the session record does not match the running processes ({disagreement}); \
                             the next run will not reclaim them"
                        ),
                    });
                }

                self.update_full_snapshot(RuntimeSnapshot {
                    acknowledged_start: 0,
                    profile_id,
                    state: RuntimeState::Running,
                    browser_pid: Some(browser_pid),
                    xray_pid,
                    cdp_port: Some(cdp_port),
                    socks_port,
                    started_at: Some(SystemTime::now()),
                    effective_args,
                    last_error: None,
                    last_warning: None,
                    dropped_events: 0,
                });

                self.emit(RuntimeEvent::Started {
                    profile_id,
                    browser_pid,
                    xray_pid,
                    cdp_port,
                    socks_port,
                });
                self.emit(RuntimeEvent::StateChanged {
                    profile_id,
                    state: RuntimeState::Running,
                });
            }
            Err(e) => {
                // Dropping the attempt is the rollback: the browser, the Xray
                // process, the temporary config and the record all go, in that
                // order, whatever failed.
                let failure = StartFailure {
                    message: format!("CDP readiness probe failed: {e}"),
                    cancelled: attempt.cancelled(),
                };
                drop(attempt);
                self.report_start_failure(profile_id, failure);
            }
        }
    }

    /// Everything a start does before the browser is ready to be probed.
    ///
    /// Acquires the ports, resolves capabilities, builds the plan, starts Xray and
    /// Chromium and writes the session record - and returns the value that owns
    /// all of it, or the reason there is nothing to own. It does not decide the
    /// profile's state: its caller is what turns either answer into a snapshot and
    /// an event, so every failure leaves the same way.
    pub(super) fn begin_attempt(
        &mut self,
        params: &StartParams,
    ) -> Result<(StartAttempt, SessionRecord), StartFailure> {
        let profile_id = params.profile.id;
        let mut attempt = StartAttempt::new(
            profile_id,
            Arc::clone(&self.process_tree),
            self.runtime_dir.clone(),
        );

        // 1. Allocate CDP port
        let cdp_reservation = self
            .port_allocator
            .reserve_loopback()
            .map_err(|e| StartFailure::failed(format!("port allocation failed: {e}")))?;
        attempt.hold_cdp(cdp_reservation);

        // 2. Allocate SOCKS port if proxy present
        if params.proxy.is_some() {
            let socks_reservation = self
                .port_allocator
                .reserve_loopback()
                .map_err(|e| StartFailure::failed(format!("socks port allocation failed: {e}")))?;
            attempt.hold_socks(socks_reservation);
        }
        let cdp_port = attempt.cdp();
        let socks_port = attempt.socks();

        // 3. Resolve capabilities
        let capabilities = self
            .capability_resolver
            .resolve(&params.core)
            .map_err(|e| StartFailure::failed(format!("capability error: {e}")))?;

        // 3b. Report every switch the core cannot honour. The serializer omits
        // them, so without this the profile would claim a fingerprint the
        // engine never applies.
        let compatibility = crate::compat::check(&params.core, &params.profile, &capabilities);
        if let Some(message) = compatibility.message() {
            self.emit(RuntimeEvent::Warning {
                profile_id,
                message,
            });
        }

        // 4. Build LaunchPlan
        let ctx = LaunchContext {
            profile: &params.profile,
            core: &params.core,
            proxy: params.proxy.as_ref(),
            capabilities: &capabilities,
            cdp_port,
            socks_port,
            xray_executable: Some(self.xray_executable.clone()),
            xray_config_dir: Some(self.runtime_dir.join(profile_id.to_string())),
        };

        let plan = self
            .planner
            .build(ctx)
            .map_err(|e| StartFailure::failed(format!("launch plan error: {e}")))?;

        let effective_args: Vec<String> = plan
            .browser_args
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        self.emit(RuntimeEvent::EffectiveLaunchArgs {
            profile_id,
            args: effective_args.clone(),
        });
        attempt.note_ports(cdp_port, socks_port, effective_args);

        // The config path is remembered before it is written, so a failure
        // partway through writing it still removes it.
        attempt.note_config(plan.xray.as_ref().map(|xray| xray.config_path.clone()));

        // Prepare and verify Xray before Chromium can issue any requests.
        if let Some(xray_plan) = &plan.xray {
            let proxy = params
                .proxy
                .as_ref()
                .ok_or_else(|| StartFailure::failed("missing proxy configuration"))?;
            self.xray_builder
                .build(proxy, xray_plan.socks_port, &xray_plan.config_path)
                .map_err(|e| StartFailure::failed(e.to_string()))?;

            let mut command = std::process::Command::new(&xray_plan.executable);
            command
                .args(xray_plan.args())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            // Xray binds its own socket; release only at the handoff.
            attempt.release_socks();

            let child = crate::process::spawn_managed(&mut command)
                .map_err(|e| StartFailure::failed(format!("Xray spawn failed: {e}")))?;
            attempt.hold_xray(child);

            let mut cancelled = false;
            let ready = {
                let child = attempt
                    .xray_mut()
                    .expect("the Xray process was held just above");
                crate::xray::wait_ready(
                    child,
                    xray_plan.socks_port,
                    self.xray_ready_timeout,
                    || {
                        cancelled = self.poll_start_commands(profile_id);
                        !cancelled
                    },
                )
            };
            if let Err(error) = ready {
                // Dropped on the way out, which ends the Xray process and removes
                // the config it was reading.
                return Err(if cancelled {
                    StartFailure::cancelled(error.to_string())
                } else {
                    StartFailure::failed(error.to_string())
                });
            }
        }

        if self.poll_start_commands(profile_id) {
            return Err(StartFailure::cancelled("startup cancelled"));
        }

        // 5. Spawn Chromium
        let mut cmd = std::process::Command::new(&plan.browser_executable);
        cmd.args(&plan.browser_args);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        attempt.release_cdp();
        let child = crate::process::spawn_managed(&mut cmd).map_err(|e| {
            StartFailure::failed(format!(
                "failed to spawn executable {:?}: {e}",
                plan.browser_executable
            ))
        })?;
        attempt.hold_browser(child);

        let browser_pid = attempt.browser_id();
        let xray_pid = attempt.xray_id();

        // Write the session record as soon as both children exist, before the
        // readiness wait: a process killed during that wait would otherwise
        // leave a browser running that no later run can find. The start is still
        // not failed for a record that cannot be written - the browser is up -
        // and the warning says what is lost.
        let record = SessionRecord {
            profile_id,
            cdp_port,
            socks_port,
            started_at: journal::now_millis(),
            left_running: false,
            browser: ProcessRecord::captured(
                browser_pid,
                &plan.browser_executable,
                &attempt.effective_args,
                self.process_inspector.as_ref(),
            ),
            xray: plan.xray.as_ref().and_then(|xray_plan| {
                xray_pid.map(|pid| {
                    ProcessRecord::captured(
                        pid,
                        &xray_plan.executable,
                        &xray_plan.args(),
                        self.process_inspector.as_ref(),
                    )
                })
            }),
        };
        if let Err(error) = journal::write(&self.runtime_dir, &record) {
            self.emit(RuntimeEvent::Warning {
                profile_id,
                message: format!(
                    "the session record could not be written ({error}); if this process is \
                     killed, the browser it started will not be reclaimed by the next run"
                ),
            });
        }
        // Whether or not the write landed, a failure from here removes whatever
        // is there: a half-written record is worse than none.
        attempt.note_record();

        Ok((attempt, record))
    }

    /// Publishes what a start that did not become a session leaves behind.
    ///
    /// A start that was called off is stopped; a start that went wrong is failed,
    /// with the reason. Either way everything it acquired has already been given
    /// back: the attempt was dropped before this is called.
    pub(super) fn report_start_failure(&mut self, profile_id: ProfileId, failure: StartFailure) {
        if failure.cancelled {
            self.stop_profile(profile_id);
        } else {
            self.fail_start(profile_id, failure.message);
        }
    }

    /// Service cancellation without recursively starting another profile.
    /// A bounded batch leaves time for readiness and process monitoring.
    pub(super) fn poll_start_commands(&mut self, starting: ProfileId) -> bool {
        self.poll_active_sessions();
        for _ in 0..64 {
            let command = match self.command_rx.try_recv() {
                Ok(command) => command,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.shutting_down = true;
                    self.pending_commands.clear();
                    self.cleanup_all();
                    return true;
                }
            };
            match command {
                RuntimeCommand::ShutdownAll => {
                    self.shutting_down = true;
                    self.pending_commands.clear();
                    self.cleanup_all();
                    return true;
                }
                RuntimeCommand::Stop(id) => {
                    let mut cancelled = 0;
                    self.pending_commands.retain(|command| match command {
                        RuntimeCommand::Start(p) | RuntimeCommand::Restart(p) => {
                            if p.profile_id() == id {
                                cancelled = cancelled.max(p.request_id);
                            }
                            p.profile_id() != id
                        }
                        _ => true,
                    });
                    if id == starting {
                        return true;
                    }
                    self.stop_profile(id);
                    self.acknowledge_start(id, cancelled);
                }
                RuntimeCommand::Start(params) if params.profile_id() == starting => {
                    self.emit(RuntimeEvent::Warning {
                        profile_id: starting,
                        message: format!("profile {starting} is already starting"),
                    });
                }
                RuntimeCommand::Restart(params) if params.profile_id() == starting => {
                    self.pending_commands
                        .push_back(RuntimeCommand::Restart(params));
                    return true;
                }
                command => self.pending_commands.push_back(command),
            }
        }
        false
    }
}
