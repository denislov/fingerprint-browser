//! Window close decisions and tray lifecycle.
use super::*;

impl AppView {
    /// Exit through the same path as closing the last window: `run` returns and
    /// `main` then asks the supervisor to reclaim every child process.
    /// The window's Quit button, which is the close affordance the user can
    /// reach without the title bar and therefore asks the same question.
    pub(super) fn on_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.decide_exit(window, cx);
    }

    /// The window's close button, as a question that can be refused.
    ///
    /// `false` keeps the window open, which is what two of the four modes mean:
    /// "ask" has not been answered yet, and "keep running" is not a close at all.
    pub(super) fn on_close_requested(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.state.exit_mode().choice() {
            // The one answer that is not a close: the window goes away and the
            // program does not, so the close is refused.
            Some(exit) if exit.stays_running() => {
                self.enter_background(window, cx);
                false
            }
            Some(exit) => {
                self.leave(exit, window, cx);
                true
            }
            None => {
                self.open_exit_dialog(window, cx);
                false
            }
        }
    }

    /// The one place the answer is read: a remembered mode is carried out, and
    /// "ask" is asked.
    pub(super) fn decide_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.state.exit_mode().choice() {
            Some(exit) if exit.stays_running() => self.enter_background(window, cx),
            Some(exit) => self.leave(exit, window, cx),
            None => self.open_exit_dialog(window, cx),
        }
    }

    /// Carries out one of the three answers.
    ///
    /// "Keep running" tells the runtime before it quits: the supervisor is the
    /// only thing that can mark the session records and give up the handles, and
    /// it is about to be gone. "Stop everything" needs to say nothing - what this
    /// program has always done at exit is stop its children, and `main` does it
    /// after the window loop returns.
    pub(super) fn leave(&mut self, exit: Exit, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        match exit {
            Exit::Background => self.enter_background(window, cx),
            Exit::KeepRunning => {
                self.state.release_runtime();
                cx.quit();
            }
            Exit::ExitAll => cx.quit(),
        }
    }

    /// Stays, with no window visible.
    ///
    /// The window is closed/hidden from view and the taskbar, and the program
    /// keeps running and managing the profiles. The tray icon is what says so
    /// and what brings the window back or exits completely.
    pub(super) fn enter_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // No banner for the ordinary case: it would be painted into a window the
        // user has just put away. The activity log is where that is recorded,
        // because the log is what outlives the window.
        tracing::info!("{}", self.state.text().exit_in_background);
        if self.tray.is_none() {
            match (self.tray_starter)(self.state.text()) {
                Ok(tray) => self.tray = Some(tray),
                Err(error) => {
                    // The tray icon is the only way back to a window that is not
                    // on screen, so a desktop that refuses one is a desktop where
                    // hiding the window would lose it: the user would be left
                    // with a running program, no window, and no icon to click.
                    // The window stays, and the banner - which can be read
                    // precisely because the window is still there - says why.
                    tracing::warn!("no tray icon: {error}");
                    let message = self.state.text().no_tray_keeps_window(&error);
                    self.state.push_notice(message, true);
                    cx.notify();
                    return;
                }
            }
        }
        window_visibility::hide(window);
    }

    /// What the tray asked for, if anything.
    pub(super) fn drain_tray(&self) -> Vec<TrayEvent> {
        self.tray
            .as_ref()
            .map(|tray| tray.drain())
            .unwrap_or_default()
    }

    /// Carries out one of those requests.
    pub(super) fn handle_tray(
        &mut self,
        event: TrayEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TrayEvent::Show => {
                window_visibility::show(window);
                cx.notify();
            }
            TrayEvent::Quit => {
                self.leave(Exit::ExitAll, window, cx);
            }
        }
    }

    /// The question itself: three answers, and whether to stop asking.
    pub(super) fn open_exit_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let p = palette(cx);
        // How many profiles this decision is about. It travels with each answer
        // because the three of them differ in what they do to those profiles, and
        // the count is the difference between a choice that matters and one that
        // does not.
        let running = self.state.active_profile_count();
        let body = if running > 0 {
            t.exit_dialog_body
        } else {
            t.exit_dialog_body_idle
        };
        let view = cx.entity().downgrade();
        // The box's state is a cell rather than a field of the view, because the
        // dialog is rebuilt from this closure on every frame and reading the view
        // from inside a render is exactly what cannot be done. The view's own copy
        // is what a *reopened* dialog starts from, and is written on every tick.
        let remember = std::rc::Rc::new(std::cell::Cell::new(self.exit_remember));
        window.open_dialog(cx, move |dialog, _window, _cx| {
            let ticked = remember.get();
            let choices = [Exit::Background, Exit::KeepRunning, Exit::ExitAll]
                .into_iter()
                .map(|exit| {
                    let view = view.clone();
                    let remember = std::rc::Rc::clone(&remember);
                    exit_choice(exit, running, p, t)
                        .on_click(move |_, window, cx| {
                            let ticked = remember.get();
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    if ticked {
                                        // Written before it is acted on, like
                                        // every other stored choice: a config
                                        // file that cannot be written refuses
                                        // the change, and the exit still
                                        // happens - the answer was given.
                                        let _ = view.state.set_exit_mode(exit.mode());
                                    }
                                    view.leave(exit, window, cx);
                                });
                            }
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>();

            dialog
                .title(t.exit_dialog_title)
                .w(px(560.0))
                // One child rather than three: the dialog puts its children in a
                // `flex_1` box whose height does not come out equal to what it
                // paints, so a stack of children is a stack of separate
                // measurement problems. What is inside is one box that is either
                // laid out or not.
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_xs().text_color(rgb(p.muted)).child(body))
                        .children(choices)
                        .child(
                            Checkbox::new("exit-remember")
                                .label(t.exit_remember)
                                .checked(ticked)
                                .on_click({
                                    let view = view.clone();
                                    let remember = std::rc::Rc::clone(&remember);
                                    move |checked, window, cx| {
                                        remember.set(*checked);
                                        if let Some(view) = view.upgrade() {
                                            // `Entity::update` is infallible
                                            // here: the entity was just
                                            // upgraded, so it is alive.
                                            view.update(cx, |view, cx| {
                                                view.exit_remember = *checked;
                                                cx.notify();
                                            });
                                        }
                                        // The dialog is rebuilt from the cell, so
                                        // the box has to be redrawn for the tick to
                                        // appear.
                                        window.refresh();
                                    }
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(t.exit_remember_note),
                        ),
                )
                .footer(
                    DialogFooter::new().child(
                        DialogClose::new().trigger(|button| button.label(t.cancel).outline()),
                    ),
                )
        });
    }
}
