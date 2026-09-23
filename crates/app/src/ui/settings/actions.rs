//! Settings events and background operation submission.
use super::*;

impl AppView {
    /// Starts a browser-data copy in the direction asked for.
    ///
    /// The job is gathered before anything is spawned, so an empty path, an
    /// empty installation, a running profile or a profile something else is
    /// already busy with is refused where the field is rather than on a worker
    /// that could not have copied anything. A copy is hundreds of megabytes, so
    /// the run itself is on a worker and the answer arrives on a later tick.
    ///
    /// The lease the job comes with goes into that worker: it is what refuses a
    /// start, or a second copy, of these profiles until the copy is over - and
    /// dropping it in the thread is what gives them back, whether the copy
    /// returned, failed, or panicked on the way.
    pub(in crate::ui) fn on_browser_data(&mut self, direction: Direction, cx: &mut Context<Self>) {
        if let Some(input) = self.browser_data_input.clone() {
            let typed = input.read(cx).value().to_string();
            self.state.set_browser_data_path(typed);
        }
        let (job, lease) = match self.state.browser_data_job(direction) {
            Ok(job) => job,
            Err(message) => {
                self.state.push_notice(message, true);
                cx.notify();
                return;
            }
        };
        let copier = Arc::clone(&self.copier);
        let sender = self.browser_data_tx.clone();
        std::thread::spawn(move || {
            let outcome = copier.run(&job);
            drop(lease);
            let _ = sender.send((direction, outcome));
        });
        cx.notify();
    }

    /// Builds the Settings page's browser-data directory field on first use.
    pub(in crate::ui) fn ensure_browser_data_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let t = self.state.text();
        if let Some(input) = &self.browser_data_input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t.browser_data_path_label));
        self.browser_data_input = Some(input.clone());
        input
    }

    /// Asks before a restore replaces a populated installation.
    pub(in crate::ui) fn confirm_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .title(t.restore_title)
                .description(
                    "Every core, proxy and profile here is replaced by the file's configuration. \
                     Profiles keep their browser data on disk.",
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t.replace)
                        .cancel_text(t.cancel)
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    view.start_restore(RestoreMode::Replace, cx);
                                });
                            }
                            true
                        })
                        .on_cancel(|_, _, _| true),
                )
        });
        cx.notify();
    }

    /// Puts the restore on a worker and lets the confirmation close.
    ///
    /// The file is read, checked and applied as one transaction, so it is the
    /// largest piece of work any of these four do - and the one least able to
    /// happen where the window is drawn, since the window is what the user is
    /// watching while a whole configuration is replaced.
    pub(in crate::ui) fn start_restore(&mut self, mode: RestoreMode, cx: &mut Context<Self>) {
        let job = match self.state.begin_restore(mode) {
            Ok(job) => job,
            Err(message) => {
                self.state.push_notice(message, true);
                cx.notify();
                return;
            }
        };
        let sender = self.maintenance_tx.clone();
        std::thread::spawn(move || {
            let source = job.source.clone();
            let result = maintenance::run_restore(job);
            let _ = sender.send(maintenance::Outcome::Restored { source, result });
        });
        cx.notify();
    }

    /// Restores the configuration, asking first when there is something to
    /// replace.
    ///
    /// The precondition is the difference from an import: an empty installation
    /// is restored at once, because there is nothing to lose, and a populated
    /// one opens a confirmation that says what will be replaced. The mode is the
    /// confirmation, so a mistaken click cannot replace a live configuration.
    pub(in crate::ui) fn on_restore_configuration(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.state.text();
        if let Some(input) = self.restore_input.clone() {
            let typed = input.read(cx).value().to_string();
            self.state.set_restore_path(typed);
        }
        // The path is checked first, so an empty field is refused where it is
        // rather than opening a confirmation for a restore that could not run.
        if self.state.restore_source().is_none() {
            self.state.push_notice(t.restore_needs_path, true);
            cx.notify();
            return;
        }
        match self.state.is_configuration_empty() {
            Ok(true) => self.start_restore(RestoreMode::OnlyWhenEmpty, cx),
            Ok(false) => self.confirm_restore(window, cx),
            Err(error) => self.state.push_notice(error.to_string(), true),
        }
        cx.notify();
    }

    /// Builds the Settings page's restore path field on first use.
    pub(in crate::ui) fn ensure_restore_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let t = self.state.text();
        if let Some(input) = &self.restore_input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t.restore_path_label));
        self.restore_input = Some(input.clone());
        input
    }

    /// Reads a configuration backup from whatever the field says.
    pub(in crate::ui) fn on_import_configuration(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self.import_input.clone() {
            let typed = input.read(cx).value().to_string();
            self.state.set_import_path(typed);
        }
        // The result - and every shortfall the report carries - is reported
        // through `set_notice`, which routes shortfalls to the banner and
        // clean imports to a toast. An import writes every record in the file one
        // at a time, so it runs on a worker.
        let job = match self.state.begin_import() {
            Ok(job) => job,
            Err(message) => {
                self.state.push_notice(message, true);
                cx.notify();
                return;
            }
        };
        let sender = self.maintenance_tx.clone();
        std::thread::spawn(move || {
            let source = job.source.clone();
            let result = maintenance::run_import(job);
            let _ = sender.send(maintenance::Outcome::Imported { source, result });
        });
        cx.notify();
    }

    /// Builds the Settings page's import path field on first use, the way the
    /// export one is built.
    pub(in crate::ui) fn ensure_import_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let t = self.state.text();
        if let Some(input) = &self.import_input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t.import_path_label));
        self.import_input = Some(input.clone());
        input
    }

    /// Writes a report about this installation, to the file the card named.
    ///
    /// Nothing is typed and nothing is confirmed: the destination is a new file
    /// under the data directory every second, and the report says where it went.
    pub(in crate::ui) fn on_write_diagnostics(&mut self, cx: &mut Context<Self>) {
        let _ = self.state.write_diagnostics();
        cx.notify();
    }

    pub(in crate::ui) fn on_toggle_export_credentials(
        &mut self,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        self.state.set_export_includes_credentials(include);
        cx.notify();
    }

    /// Writes a configuration backup to whatever the field says.
    pub(in crate::ui) fn on_export_configuration(&mut self, cx: &mut Context<Self>) {
        // Read here rather than watched, so there is no observer to keep in step
        // and no notification that can be missed: the text exists for this one
        // button, and reading it at the moment it is pressed is the whole of its
        // job.
        if let Some(input) = self.export_input.clone() {
            let typed = input.read(cx).value().to_string();
            self.state.set_export_path(typed);
        }
        // The result - and whether the file carries credentials - is reported
        // through the banner, the toast and the activity log, all of which the
        // worker's answer reaches by way of `finish_maintenance`. The read and the
        // write are the whole configuration and one file, so they run on a worker
        // like the other three.
        let job = match self.state.begin_export() {
            Ok(job) => job,
            Err(message) => {
                self.state.push_notice(message, true);
                cx.notify();
                return;
            }
        };
        let sender = self.maintenance_tx.clone();
        std::thread::spawn(move || {
            let destination = job.destination.clone();
            let result = maintenance::run_export(job);
            let _ = sender.send(maintenance::Outcome::Exported {
                destination,
                result,
            });
        });
        cx.notify();
    }

    /// Builds the Settings page's export path field on first use.
    ///
    /// Started empty rather than filled with the default path. An empty field
    /// resolves to a new file each time it is used, so a window left open
    /// overnight does not propose a name from yesterday - and the sentence under
    /// the field says which path it would be right now.
    pub(in crate::ui) fn ensure_export_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let t = self.state.text();
        if let Some(input) = &self.export_input {
            return input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t.leave_empty_new_file));
        self.export_input = Some(input.clone());
        input
    }

    /// Asks for a new value for one editable setting.
    ///
    /// The dialog says when the value takes effect, because a setting that
    /// looks live and is not is the thing this page exists to avoid.
    pub(in crate::ui) fn on_edit_setting(
        &mut self,
        key: SettingKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.state.text();
        let p = palette(cx);
        let row = self
            .state
            .setting_rows()
            .into_iter()
            .find(|row| row.key == key);
        let Some(row) = row else {
            self.state
                .push_notice(t.setting_not_a_setting(key.label(t)), true);
            cx.notify();
            return;
        };
        let current = row.value.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        self.setting_editor = Some(input.clone());
        self.setting_key = Some(key);
        let view = cx.entity().downgrade();
        let field = input.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let field = field.clone();
            let now = row.value.clone();
            dialog
                .title(t.setting_dialog_title(key.label(t), key.effect()))
                .w(px(680.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(t.setting_now(&now)),
                        )
                        .child(
                            // Not `setting-{key}`: the row card already
                            // owns that id, and two elements sharing one
                            // id in a single tree is ambiguous.
                            Input::new(&field)
                                .id(format!("setting-field-{}", key.id()))
                                .aria_label(key.label(t)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(key_help(key, t)),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label(t.cancel).outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label(t.save))),
                )
                .on_ok(move |_, _, cx| {
                    let value = field.read(cx).value().to_string();
                    view.update(cx, |view, _| view.state.update_setting(key, &value).is_ok())
                        .unwrap_or(false)
                })
        });
        cx.notify();
    }

    /// Switches the window's language, and remembers the choice.
    ///
    /// Nothing to apply beyond the record and a repaint: every string is read
    /// from the table during the render that is about to happen, so the whole
    /// window changes at once and nothing can be left half-translated. As with
    /// the appearance, the choice is stored *before* it is shown, so a config
    /// file that cannot be written refuses rather than showing a language that
    /// would be gone by the next start.
    pub(in crate::ui) fn on_choose_language(&mut self, language: Lang, cx: &mut Context<Self>) {
        let _ = self.state.set_language(language);
        if self.tray.is_some() {
            match Tray::start(self.state.text()) {
                Ok(tray) => self.tray = Some(tray),
                Err(error) => tracing::warn!("no tray icon: {error}"),
            }
        }
        cx.notify();
    }

    /// Stores what closing the window does.
    ///
    /// Nothing to apply: this choice is read when a window is closed, so there is
    /// nothing on screen that has to move except the chip that is lit and the
    /// sentence under it.
    pub(in crate::ui) fn on_choose_exit_mode(&mut self, mode: ExitMode, cx: &mut Context<Self>) {
        let _ = self.state.set_exit_mode(mode);
        cx.notify();
    }

    /// Switches the window's palette, and remembers the choice.
    ///
    /// The component theme is changed *after* the choice is stored, not before:
    /// a config file that cannot be written refuses the change, and repainting a
    /// window into a mode that will not survive a restart would be the window
    /// claiming something the file does not hold. The refusal is in the banner.
    ///
    /// One [`crate::theme::apply`] moves both layers, because the window reads
    /// its own colours back out of the component theme - see [`crate::theme`].
    pub(in crate::ui) fn on_choose_theme(&mut self, choice: ThemeChoice, cx: &mut Context<Self>) {
        if self.state.set_theme(choice).is_ok() {
            crate::theme::apply(choice, cx);
        }
        cx.notify();
    }
}
