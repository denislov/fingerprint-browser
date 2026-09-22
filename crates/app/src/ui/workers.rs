//! Runtime polling and background result dispatch.
use super::*;

impl AppView {
    pub(super) fn poll_runtime(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut ticks: u64 = 0;
            loop {
                cx.background_executor().timer(TICK).await;
                ticks += 1;
                let reconcile = ticks.is_multiple_of(RECONCILE_EVERY);
                let mut quit = false;

                let updated = this.update(cx, |view, cx| {
                    // A window that is gone must not leave the app (and its
                    // browser processes) running without a UI.
                    if cx.windows().is_empty() {
                        quit = true;
                        cx.quit();
                        return (false, Vec::new(), Vec::new(), Vec::new());
                    }

                    let notified = view.drain_events();
                    let verified = view.drain_verifications();
                    let tested = view.drain_proxy_tests();
                    let opened = view.drain_open_results();
                    let copied = view.drain_browser_data();
                    let maintained = view.drain_maintenance();
                    if notified || reconcile || verified || tested || opened || copied || maintained
                    {
                        view.state.refresh_runtime();
                        cx.notify();
                    }
                    // The toasts and the tray's requests are both carried out
                    // from outside this update: one updates the root view the
                    // notification layer lives on, and the other needs a window,
                    // and both are different objects from this entity.
                    (
                        true,
                        view.state.drain_toasts(),
                        view.drain_tray(),
                        cx.windows(),
                    )
                });

                // The entity is gone, or the last window was closed.
                let Ok((alive, toasts, asked, windows)) = updated else {
                    break;
                };
                if !alive || quit {
                    break;
                }
                if !toasts.is_empty() {
                    for window in &windows {
                        let _ = cx.update_window(*window, |_, window, cx| {
                            push_toasts(&toasts, window, cx);
                        });
                    }
                }

                // The tray is another thread: what it noticed is carried out
                // here, where there is a window to carry it out on.
                for event in asked {
                    for window in &windows {
                        let _ = cx.update_window(*window, |_, window, cx| {
                            let _ = this.update(cx, |view, cx| view.handle_tray(event, window, cx));
                        });
                    }
                }
            }
        })
        .detach();
    }

    /// Drain queued notifications into the activity log. Events are never
    /// replayed as state, but they are the only record of a crash or a warning.
    pub(super) fn drain_events(&mut self) -> bool {
        let mut notified = false;
        while let Ok(event) = self.events.try_recv() {
            self.state.record_event(&event);
            notified = true;
        }
        notified
    }

    /// Collect finished readings from the worker threads.
    pub(super) fn drain_verifications(&mut self) -> bool {
        let mut received = false;
        while let Ok((job, outcome)) = self.verifications.try_recv() {
            self.state.complete_verification(&job, outcome);
            received = true;
        }
        received
    }

    /// Collect finished proxy tests from the worker threads.
    ///
    /// A proxy deleted or edited while the test ran is dropped: the answer
    /// describes an upstream that is no longer the one on the row.
    pub(super) fn drain_proxy_tests(&mut self) -> bool {
        let mut received = false;
        while let Ok((job, outcome)) = self.proxy_tests.try_recv() {
            self.state.complete_proxy_test(&job, outcome);
            received = true;
        }
        received
    }

    /// Collect what the opener reported. The window never waited for it, so the
    /// answer arrives a tick later.
    pub(super) fn drain_open_results(&mut self) -> bool {
        let t = self.state.text();
        let mut received = false;
        while let Ok((path, result)) = self.open_results.try_recv() {
            match result {
                Ok(()) => self
                    .state
                    .push_notice(t.opened(&path.display().to_string()), false),
                Err(reason) => self.state.push_notice(reason, true),
            }
            received = true;
        }
        received
    }

    /// Collect finished browser-data copies from the worker thread.
    ///
    /// Nothing is dropped for having gone stale the way a verification is: a
    /// copy is not about a runtime state that can change under it, and the
    /// report is the only record of what it did.
    /// Collect what the configuration workers reported.
    ///
    /// Nothing is dropped for having gone stale: a task is about the whole
    /// installation rather than about a runtime state that can change under it,
    /// and its answer is the only record of what it did.
    pub(super) fn drain_maintenance(&mut self) -> bool {
        let mut received = false;
        while let Ok(outcome) = self.maintenance.try_recv() {
            self.state.finish_maintenance(outcome);
            received = true;
        }
        received
    }

    pub(super) fn drain_browser_data(&mut self) -> bool {
        let mut received = false;
        while let Ok((direction, outcome)) = self.browser_data.try_recv() {
            self.state.finish_browser_data(direction, outcome);
            received = true;
        }
        received
    }
}
