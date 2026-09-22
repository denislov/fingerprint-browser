//! The GPUI window: renders [`AppState`] and forwards user actions into it.
//!
//! The view owns no runtime state. Notifications only mark the snapshot cache
//! dirty; the snapshot itself stays the single source of truth, exactly as the
//! runtime façade contract requires.

use crate::browser_data::BrowserDataCopier;
use crate::core_editor::CoreEditor;
use crate::editor::{ProfileEdit, ProfileEditor};
use crate::exit::{Exit, ExitMode};
use crate::maintenance;
use crate::open_dir::DirectoryOpener;
use crate::proxy_editor::ProxyEditor;
use crate::proxy_import::ProxyImport;
use crate::proxy_tester::ProxyTester;
use crate::settings::SettingKey;
use crate::state::{
    AppState, CoreRow, DetailsTab, LogFilter, LogLevel, LogRow, Opening, Page, ProfileRow,
    ProxyRow, ProxyTest, StartGate, Toast, ToastKind, Verification,
};
use crate::text::{Lang, Text};
use crate::theme::{Palette, ThemeChoice, palette};
use crate::tray::{Tray, TrayEvent};
use crate::verifier::{FingerprintVerifier, VerificationReport};
use crate::window_visibility;
use application::{BrowserDataReport, Direction, RestoreMode};
use crossbeam_channel::{Receiver, Sender};
use domain::{CoreId, ProfileId, ProxyId, RuntimeState};
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Root;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::{DialogAction, DialogButtonProps, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::theme::Theme;
use gpui_kit::prelude::*;
use gpui_kit::*;
use runtime::{Diagnosis, Fault, RuntimeEvent};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// How often the window drains runtime notifications and reconciles snapshots.
const TICK: Duration = Duration::from_millis(200);
/// Full reconciliation every N ticks, for any notification that was dropped.
const RECONCILE_EVERY: u64 = 5;
/// Cap on the per-row warning line; the full text is in Runtime Details.
const WARNING_WIDTH: f32 = 420.0;
/// Wide enough for a profile name or a proxy name without crowding the title.
const PROFILE_FILTER_WIDTH: f32 = 320.0;

pub struct AppView {
    /// The editor behind the open dialog, if any.
    editor: Option<Entity<ProfileEditor>>,
    /// The proxy editor behind the open dialog, if any.
    proxy_editor: Option<Entity<ProxyEditor>>,
    /// The link importer behind the open dialog, if any.
    proxy_import: Option<Entity<ProxyImport>>,
    /// The browser-core editor behind the open dialog, if any.
    core_editor: Option<Entity<CoreEditor>>,
    /// The settings value field behind the open dialog, if any.
    setting_editor: Option<Entity<InputState>>,
    /// Which setting that field belongs to.
    setting_key: Option<SettingKey>,
    /// The Profiles page's filter field.
    ///
    /// Built on the first render rather than here: an `InputState` needs a
    /// window, and neither this constructor nor `boot` has one.
    filter_input: Option<Entity<InputState>>,
    /// The Settings page's export path field. Built on the first render for the
    /// same reason, and watched by nobody: an export is the only thing the text
    /// is for, so the button reads it when it is pressed.
    export_input: Option<Entity<InputState>>,
    /// The Settings page's import path field. Built and read the same way the
    /// export one is: the text exists for one button, which reads it when it is
    /// pressed.
    import_input: Option<Entity<InputState>>,
    /// The Settings page's restore path field. Its own field rather than the
    /// import's, because the two verbs have different consequences.
    restore_input: Option<Entity<InputState>>,
    /// The Settings page's browser-data directory field, shared by the copy out
    /// and the copy back in: it is one place.
    browser_data_input: Option<Entity<InputState>>,
    verifier: Arc<dyn FingerprintVerifier>,
    /// Sends one request through a proxy. Injected so a test never dials one.
    tester: Arc<dyn ProxyTester>,
    /// Opens a profile's data directory. Injected so a test does not open one.
    opener: Arc<dyn DirectoryOpener>,
    /// Copies browser data. Injected so a test never writes hundreds of
    /// megabytes, and never touches a disk.
    copier: Arc<dyn BrowserDataCopier>,
    verifications: Receiver<(ProfileId, Result<VerificationReport, String>)>,
    verification_tx: Sender<(ProfileId, Result<VerificationReport, String>)>,
    /// Finished proxy tests, with whether the engine probed was already up.
    proxy_tests: Receiver<(ProxyId, bool, Result<Diagnosis, Fault>)>,
    proxy_test_tx: Sender<(ProxyId, bool, Result<Diagnosis, Fault>)>,
    /// What the opener reported, once it was done handing the request off.
    open_results: Receiver<(PathBuf, Result<(), String>)>,
    open_tx: Sender<(PathBuf, Result<(), String>)>,
    /// Finished browser-data copies, with the direction each was taken in.
    browser_data: Receiver<(Direction, Result<BrowserDataReport, String>)>,
    browser_data_tx: Sender<(Direction, Result<BrowserDataReport, String>)>,
    /// What a configuration task reported: one channel and one answer type for
    /// the four of them, so a fifth cannot report somewhere the window does not
    /// hear it.
    maintenance: Receiver<maintenance::Outcome>,
    maintenance_tx: Sender<maintenance::Outcome>,
    state: AppState,
    events: Receiver<RuntimeEvent>,
    /// Kept alive: dropping a GPUI subscription unregisters the observer.
    window_closed: Option<Subscription>,
    /// The tray icon, once "keep running" has put the window away. It is created
    /// on the first request rather than at start, because a program with its
    /// window open does not need one - and it is never removed, because the user
    /// who put the window away once may do it again.
    tray: Option<Tray>,
    /// Whether the close hook has been registered on this window. The hook needs
    /// a `Window`, and this view is built before there is one, so the first frame
    /// is when it is installed - once.
    close_hook: bool,
    /// Whether the exit dialog's "remember" box is ticked. It lives here rather
    /// than in the dialog because the dialog is rebuilt from a closure on every
    /// frame, and a checkbox that forgot its own state between frames would be a
    /// checkbox nobody could tick.
    exit_remember: bool,
    /// The same, for the filter field's change events.
    filter_changed: Option<Subscription>,
}

impl AppView {
    pub fn new(
        state: AppState,
        events: Receiver<RuntimeEvent>,
        verifier: Arc<dyn FingerprintVerifier>,
        tester: Arc<dyn ProxyTester>,
        opener: Arc<dyn DirectoryOpener>,
        copier: Arc<dyn BrowserDataCopier>,
    ) -> Self {
        // A reading takes seconds and blocks on the browser, so it runs on a
        // worker thread and reports back through this channel. Testing a proxy
        // starts an engine and waits for one request, which blocks the same way
        // and for the same reason. Opening a directory waits for the desktop's
        // opener, and copying browser data moves hundreds of megabytes: none of
        // the four may block the window.
        let (verification_tx, verifications) = crossbeam_channel::unbounded();
        let (proxy_test_tx, proxy_tests) = crossbeam_channel::unbounded();
        let (open_tx, open_results) = crossbeam_channel::unbounded();
        let (browser_data_tx, browser_data) = crossbeam_channel::unbounded();
        let (maintenance_tx, maintenance) = crossbeam_channel::unbounded();
        Self {
            editor: None,
            proxy_editor: None,
            proxy_import: None,
            core_editor: None,
            setting_editor: None,
            setting_key: None,
            filter_input: None,
            export_input: None,
            import_input: None,
            restore_input: None,
            browser_data_input: None,
            verifier,
            tester,
            opener,
            copier,
            verifications,
            verification_tx,
            proxy_tests,
            proxy_test_tx,
            open_results,
            open_tx,
            browser_data,
            browser_data_tx,
            maintenance,
            maintenance_tx,
            state,
            events,
            window_closed: None,
            tray: None,
            close_hook: false,
            exit_remember: false,
            filter_changed: None,
        }
    }

    /// Read-only access for tests and diagnostics.
    #[cfg(test)]
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Mutable access for tests that drive reconciliation directly.
    #[cfg(test)]
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }

    /// The editor behind the open dialog, for tests that type into it.
    #[cfg(test)]
    pub fn editor(&self) -> Option<Entity<ProfileEditor>> {
        self.editor.clone()
    }

    /// The proxy editor behind the open dialog, for tests that type into it.
    #[cfg(test)]
    pub fn proxy_editor(&self) -> Option<Entity<ProxyEditor>> {
        self.proxy_editor.clone()
    }

    /// The link importer behind the open dialog, if any.
    #[cfg(test)]
    pub fn proxy_import(&self) -> Option<Entity<ProxyImport>> {
        self.proxy_import.clone()
    }

    /// The browser-core editor behind the open dialog.
    #[cfg(test)]
    pub fn core_editor(&self) -> Option<Entity<CoreEditor>> {
        self.core_editor.clone()
    }

    /// The settings field behind the open dialog, for tests that type into it.
    #[cfg(test)]
    pub fn setting_editor(&self) -> Option<Entity<InputState>> {
        self.setting_editor.clone()
    }

    /// The Profiles page's filter field, for tests that type into it.
    ///
    /// Built during the first render, so it is `None` until the view has been
    /// drawn once.
    #[cfg(test)]
    pub fn filter_input(&self) -> Option<Entity<InputState>> {
        self.filter_input.clone()
    }

    /// The Settings page's export path field, once a render has built it.
    #[cfg(test)]
    pub fn export_input(&self) -> Option<Entity<InputState>> {
        self.export_input.clone()
    }

    /// The Settings page's import path field, once a render has built it.
    #[cfg(test)]
    pub fn import_input(&self) -> Option<Entity<InputState>> {
        self.import_input.clone()
    }

    /// The Settings page's restore path field, once a render has built it.
    #[cfg(test)]
    pub fn restore_input(&self) -> Option<Entity<InputState>> {
        self.restore_input.clone()
    }

    /// The Settings page's browser-data directory field, once a render has built
    /// it.
    #[cfg(test)]
    pub fn browser_data_input(&self) -> Option<Entity<InputState>> {
        self.browser_data_input.clone()
    }

    fn on_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.state.set_page(page);
        cx.notify();
    }

    /// Load storage once, then keep reconciling from snapshots in the background.
    pub fn boot(&mut self, cx: &mut Context<Self>) {
        let _ = self.state.load();
        // Quitting on the last closed window is the default on Windows/Linux but
        // not on macOS; make it uniform so the supervisor shutdown always runs.
        //
        // "Keep running" is the exception, and it is answered by the close
        // request above: that path hides the window instead of closing it, so
        // this never sees an empty window list.
        self.window_closed = Some(cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        }));
        self.poll_runtime(cx);
    }

    fn poll_runtime(&self, cx: &mut Context<Self>) {
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
    fn drain_events(&mut self) -> bool {
        let mut notified = false;
        while let Ok(event) = self.events.try_recv() {
            self.state.record_event(&event);
            notified = true;
        }
        notified
    }

    /// Collect finished readings from the worker threads.
    fn drain_verifications(&mut self) -> bool {
        let mut received = false;
        while let Ok((id, outcome)) = self.verifications.try_recv() {
            // A profile stopped or restarted while the reading ran: the answer
            // describes a browser that is gone, so drop it.
            if self
                .state
                .verification(id)
                .is_some_and(Verification::is_running)
            {
                self.state.finish_verification(id, outcome);
            }
            received = true;
        }
        received
    }

    /// Collect finished proxy tests from the worker threads.
    ///
    /// A proxy deleted or edited while the test ran is dropped: the answer
    /// describes an upstream that is no longer the one on the row.
    fn drain_proxy_tests(&mut self) -> bool {
        let mut received = false;
        while let Ok((id, live, outcome)) = self.proxy_tests.try_recv() {
            if self.state.proxy_test(id).is_some_and(ProxyTest::is_running) {
                self.state.finish_proxy_test(id, live, outcome);
            }
            received = true;
        }
        received
    }

    /// Collect what the opener reported. The window never waited for it, so the
    /// answer arrives a tick later.
    fn drain_open_results(&mut self) -> bool {
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
    fn drain_maintenance(&mut self) -> bool {
        let mut received = false;
        while let Ok(outcome) = self.maintenance.try_recv() {
            self.state.finish_maintenance(outcome);
            received = true;
        }
        received
    }

    fn drain_browser_data(&mut self) -> bool {
        let mut received = false;
        while let Ok((direction, outcome)) = self.browser_data.try_recv() {
            self.state.finish_browser_data(direction, outcome);
            received = true;
        }
        received
    }

    /// Show every queued toast in one window.
    ///
    /// The tick drains the queue and calls `push_toasts` with each open window;
    /// a test calls this directly, because it should not have to wait for the
    /// timer to fire before it can assert that an action was acknowledged.
    #[cfg(test)]
    pub(crate) fn show_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let toasts = self.state.drain_toasts();
        push_toasts(&toasts, window, cx);
    }

    /// Opens a profile's browser data directory.
    ///
    /// The directory is not created here: a profile that has never run has no
    /// browser data, and the refusal says so rather than leaving an empty
    /// folder behind that looks like state. The opener waits for the desktop to
    /// accept the request, so it runs on a worker and the answer arrives on a
    /// later tick - the window is not blocked either way, and a spawn that found
    /// no handler is reported instead of being mistaken for an open.
    fn on_open_data_dir(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        let t = self.state.text();
        let path = match self.state.profile(id) {
            Some(profile) => profile.user_data_dir,
            None => {
                self.state
                    .push_notice(t.profile_gone(&id.to_string()), true);
                cx.notify();
                return;
            }
        };
        let opener = Arc::clone(&self.opener);
        let sender = self.open_tx.clone();
        std::thread::spawn(move || {
            let result = opener.open(&path, t);
            let _ = sender.send((path, result));
        });
        cx.notify();
    }

    fn on_clear_log(&mut self, cx: &mut Context<Self>) {
        self.state.clear_log();
        cx.notify();
    }

    fn on_set_log_filter(&mut self, filter: LogFilter, cx: &mut Context<Self>) {
        self.state.set_log_filter(filter);
        cx.notify();
    }

    fn on_set_details_tab(&mut self, tab: DetailsTab, cx: &mut Context<Self>) {
        self.state.set_details_tab(tab);
        cx.notify();
    }

    fn on_copy_log(&mut self, cx: &mut Context<Self>) {
        let t = self.state.text();
        let text: String = self
            .state
            .log_rows()
            .iter()
            .map(|row| format!("{} [{}] {}", row.who, row.level.label(t), row.message))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Opens the editor for a profile and saves it through the dialog.
    fn on_edit(&mut self, id: ProfileId, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let Some(profile) = self.state.profile(id) else {
            self.state
                .push_notice(t.profile_gone(&id.to_string()), true);
            cx.notify();
            return;
        };
        let cores = self.state.core_choices();
        let proxies = self.state.proxy_choices();
        let editor = cx.new(|cx| ProfileEditor::new(&profile, &cores, &proxies, t, window, cx));
        self.open_profile_form(editor, t.edit_profile, t.save, window, cx);
    }

    /// Opens the form for a profile and wires its accept button.
    ///
    /// Creating and saving share one dialog because they share one form: what
    /// the accepted form asks for is the editor's answer, not the dialog's.
    fn open_profile_form(
        &mut self,
        editor: Entity<ProfileEditor>,
        title: &'static str,
        confirm: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.state.text();
        self.editor = Some(editor.clone());
        let view = cx.entity().downgrade();
        let accepted = editor.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let editor = editor.clone();
            let view = view.clone();
            let accepted = accepted.clone();
            dialog
                .title(title)
                .w(px(760.0))
                .child(editor.clone())
                // `Dialog` renders its own footer, not `button_props`; the
                // confirm button carries the id the tests click.
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label(t.cancel).outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label(confirm))),
                )
                .on_ok(move |_, _, cx| {
                    // Both ways of refusing end the same way: the dialog stays
                    // open and says why. A refusal from the service is shown in
                    // the form as well as the banner, because the form is where
                    // the mistake is.
                    let asked = editor.read(cx).build(cx);
                    let saved: Result<(), String> = match asked {
                        Ok(ProfileEdit::Create(draft)) => view
                            .update(cx, |view, _| match view.state.create_profile_from(draft) {
                                Ok(_) => Ok(()),
                                Err(error) => {
                                    let message = error.to_string();
                                    view.state.push_notice(message.clone(), true);
                                    Err(message)
                                }
                            })
                            .unwrap_or_else(|_| Err(t.window_gone.to_string())),
                        Ok(ProfileEdit::Save(profile)) => view
                            .update(cx, |view, _| match view.state.update_profile(profile) {
                                Ok(()) => {
                                    view.state.push_notice(t.profile_saved, false);
                                    Ok(())
                                }
                                Err(error) => {
                                    let message = error.to_string();
                                    view.state.push_notice(message.clone(), true);
                                    Err(message)
                                }
                            })
                            .unwrap_or_else(|_| Err(t.window_gone.to_string())),
                        Err(message) => Err(message),
                    };
                    match saved {
                        Ok(()) => {
                            accepted.update(cx, |editor, cx| {
                                editor.set_error(None);
                                cx.notify();
                            });
                            true
                        }
                        Err(message) => {
                            accepted.update(cx, |editor, cx| {
                                editor.set_error(Some(message));
                                cx.notify();
                            });
                            false
                        }
                    }
                })
        });
    }

    fn on_duplicate(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        let t = self.state.text();
        match self.state.duplicate_profile(id) {
            Ok(_) => self.state.push_notice(t.profile_duplicated, false),
            Err(error) => self.state.push_notice(error.to_string(), true),
        }
        cx.notify();
    }

    /// Deleting asks first: it removes the profile, not its sessions on disk.
    fn on_delete(&mut self, id: ProfileId, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let name = self
            .state
            .profile(id)
            .map(|profile| profile.name)
            .unwrap_or_else(|| id.to_string());
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .title(t.delete_profile_title)
                .description(t.delete_profile_confirm(&name))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t.delete)
                        .cancel_text(t.keep)
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    match view.state.delete_profile(id) {
                                        Ok(()) => view.state.push_notice(t.profile_deleted, false),
                                        Err(error) => {
                                            view.state.push_notice(error.to_string(), true)
                                        }
                                    }
                                    cx.notify();
                                });
                            }
                            true
                        })
                        .on_cancel(|_, _, _| true),
                )
        });
        cx.notify();
    }

    /// Switches the window's palette, and remembers the choice.
    ///
    /// The component theme is changed *after* the choice is stored, not before:
    /// a config file that cannot be written refuses the change, and repainting a
    /// window into a mode that will not survive a restart would be the window
    /// claiming something the file does not hold. The refusal is in the banner.
    ///
    /// One `Theme::change` moves both layers, because the window reads its own
    /// colours back out of the component theme - see [`crate::theme`].
    fn on_choose_theme(&mut self, choice: ThemeChoice, cx: &mut Context<Self>) {
        if self.state.set_theme(choice).is_ok() {
            Theme::change(choice.mode(), None, cx);
        }
        cx.notify();
    }

    /// Stores what closing the window does.
    ///
    /// Nothing to apply: this choice is read when a window is closed, so there is
    /// nothing on screen that has to move except the chip that is lit and the
    /// sentence under it.
    fn on_choose_exit_mode(&mut self, mode: ExitMode, cx: &mut Context<Self>) {
        let _ = self.state.set_exit_mode(mode);
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
    fn on_choose_language(&mut self, language: Lang, cx: &mut Context<Self>) {
        let _ = self.state.set_language(language);
        if self.tray.is_some() {
            match Tray::start(self.state.text()) {
                Ok(tray) => self.tray = Some(tray),
                Err(error) => tracing::warn!("no tray icon: {error}"),
            }
        }
        cx.notify();
    }

    /// Asks for a new value for one editable setting.
    ///
    /// The dialog says when the value takes effect, because a setting that
    /// looks live and is not is the thing this page exists to avoid.
    fn on_edit_setting(&mut self, key: SettingKey, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Opens the form for a new core, or for one that is already registered.
    /// Opens the form for a new core, or for one that is already registered.
    ///
    /// Adding goes through the service, which probes the binary: the version a
    /// core claims decides what the engine may be asked to spoof, so it is read
    /// rather than typed.
    fn on_edit_core(&mut self, id: Option<CoreId>, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let editing = id.and_then(|id| self.state.core(id));
        if id.is_some() && editing.is_none() {
            self.state.push_notice(t.core_gone.to_string(), true);
            cx.notify();
            return;
        }
        let core_editor = cx.new(|cx| match &editing {
            Some(core) => CoreEditor::for_core(core, t, window, cx),
            None => CoreEditor::new(t, window, cx),
        });
        self.core_editor = Some(core_editor.clone());
        let view = cx.entity().downgrade();
        let accepted = core_editor.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let core_editor = core_editor.clone();
            let view = view.clone();
            let accepted = accepted.clone();
            let title = core_editor.read(cx).title();
            dialog
                .title(title)
                .w(px(680.0))
                .child(core_editor.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label(t.cancel).outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label(t.save))),
                )
                .on_ok(move |_, _, cx| {
                    let editing = core_editor.read(cx).is_edit();
                    let name = core_editor.read(cx).name(cx);
                    let path = core_editor.read(cx).executable(cx);
                    let built = if editing {
                        core_editor.read(cx).build_core(cx).map(Some)
                    } else if path.as_os_str().is_empty() {
                        Err(t.executable_cannot_be_empty.to_string())
                    } else {
                        Ok(None)
                    };

                    let saved: Result<(), String> = match built {
                        Ok(core) => view
                            .update(cx, |view, _| {
                                let result = match core {
                                    Some(core) => view.state.update_core(core),
                                    None => view.state.add_core(name, path).map(|_| ()),
                                };
                                match result {
                                    Ok(()) => Ok(()),
                                    Err(error) => {
                                        let message = error.to_string();
                                        view.state.push_notice(message.clone(), true);
                                        Err(message)
                                    }
                                }
                            })
                            .unwrap_or_else(|_| Err(t.window_gone.to_string())),
                        Err(error) => Err(error),
                    };
                    match saved {
                        Ok(()) => {
                            accepted.update(cx, |editor, cx| {
                                editor.set_error(None);
                                cx.notify();
                            });
                            true
                        }
                        Err(message) => {
                            accepted.update(cx, |editor, cx| {
                                editor.set_error(Some(message));
                                cx.notify();
                            });
                            false
                        }
                    }
                })
        });
    }

    /// Re-reads a core's version - for a binary that was replaced in place.
    /// Reads a core's version again.
    ///
    /// The probe starts the binary and waits for it to answer, which is a program
    /// this window did not write: it runs on a worker, so a core that hangs or a
    /// binary on a slow disk does not stop the window redrawing.
    fn on_redetect_core(&mut self, id: CoreId, cx: &mut Context<Self>) {
        let job = match self.state.begin_redetect(id) {
            Ok(job) => job,
            Err(message) => {
                self.state.push_notice(message, true);
                cx.notify();
                return;
            }
        };
        let sender = self.maintenance_tx.clone();
        std::thread::spawn(move || {
            let result = maintenance::run_redetect(job);
            let _ = sender.send(maintenance::Outcome::Redetected { id, result });
        });
        cx.notify();
    }

    /// Removing a core is refused while a profile still launches with it.
    fn on_delete_core(&mut self, id: CoreId, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let (name, version, used_by) = match self.state.core(id) {
            Some(core) => {
                let used_by = self
                    .state
                    .core_rows()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|row| row.core.id == id)
                    .map(|row| row.used_by)
                    .unwrap_or_default();
                (core.name, core.version, used_by)
            }
            None => (id.to_string(), String::new(), Vec::new()),
        };
        let description = if used_by.is_empty() {
            t.delete_core_confirm(&name, &version)
        } else {
            t.core_in_use(&name, &used_by.join(", "))
        };
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let description = description.clone();
            alert
                .title(t.delete_core_title)
                .description(description)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t.delete)
                        .cancel_text(t.keep)
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    let _ = view.state.delete_core(id);
                                    cx.notify();
                                });
                            }
                            true
                        })
                        .on_cancel(|_, _, _| true),
                )
        });
        cx.notify();
    }

    /// Opens the form for a new proxy, or for one that is already stored.
    fn on_edit_proxy(&mut self, id: Option<ProxyId>, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let editing = id.and_then(|id| self.state.proxy(id));
        if id.is_some() && editing.is_none() {
            self.state.push_notice(t.proxy_gone.to_string(), true);
            cx.notify();
            return;
        }
        let proxy_editor = cx.new(|cx| match &editing {
            Some(proxy) => ProxyEditor::for_proxy(proxy, t, window, cx),
            None => ProxyEditor::new(t, window, cx),
        });
        self.proxy_editor = Some(proxy_editor.clone());
        let view = cx.entity().downgrade();
        let accepted = proxy_editor.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let proxy_editor = proxy_editor.clone();
            let view = view.clone();
            let accepted = accepted.clone();
            let title = proxy_editor.read(cx).title();
            dialog
                .title(title)
                .w(px(640.0))
                .child(proxy_editor.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label(t.cancel).outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label(t.save))),
                )
                .on_ok(move |_, _, cx| {
                    // A refusal from the service is shown in the form as well as
                    // the banner: the form is where the mistake is.
                    let saved: Result<(), String> = match proxy_editor.read(cx).build_proxy(cx) {
                        Ok(proxy) => {
                            let existing = proxy_editor.read(cx).is_edit();
                            view.update(cx, |view, _| {
                                let result = if existing {
                                    view.state.update_proxy(proxy)
                                } else {
                                    view.state
                                        .create_proxy(&proxy.name, proxy.outbound)
                                        .map(|_| ())
                                };
                                match result {
                                    Ok(()) => Ok(()),
                                    Err(error) => {
                                        let message = error.to_string();
                                        view.state.push_notice(message.clone(), true);
                                        Err(message)
                                    }
                                }
                            })
                            .unwrap_or_else(|_| Err(t.window_gone.to_string()))
                        }
                        Err(error) => Err(error),
                    };
                    match saved {
                        Ok(()) => {
                            accepted.update(cx, |editor, cx| {
                                editor.set_error(None);
                                cx.notify();
                            });
                            true
                        }
                        Err(message) => {
                            accepted.update(cx, |editor, cx| {
                                editor.set_error(Some(message));
                                cx.notify();
                            });
                            false
                        }
                    }
                })
        });
    }

    /// Opens the paste dialog for a share link.
    ///
    /// The dialog parses and the view stores; on a refusal the dialog stays open
    /// with the parser's own sentence, because the link is the thing to fix.
    fn on_import_proxy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let proxy_import = cx.new(|cx| ProxyImport::new(t, window, cx));
        self.proxy_import = Some(proxy_import.clone());
        let view = cx.entity().downgrade();
        let accepted = proxy_import.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let proxy_import = proxy_import.clone();
            let view = view.clone();
            let accepted = accepted.clone();
            dialog
                .title(proxy_import.read(cx).title())
                .w(px(640.0))
                .child(proxy_import.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label(t.cancel).outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label(t.import))),
                )
                .on_ok(move |_, _, cx| {
                    let imported: Result<(), String> = match proxy_import.read(cx).read_link(cx) {
                        Ok((name, outbound)) => view
                            .update(cx, |view, _| {
                                match view.state.create_proxy(&name, outbound) {
                                    Ok(_) => Ok(()),
                                    Err(error) => {
                                        let message = error.to_string();
                                        view.state.push_notice(message.clone(), true);
                                        Err(message)
                                    }
                                }
                            })
                            .unwrap_or_else(|_| Err(t.window_gone.to_string())),
                        Err(error) => Err(error),
                    };
                    match imported {
                        Ok(()) => {
                            accepted.update(cx, |import, cx| {
                                import.set_error(None);
                                cx.notify();
                            });
                            true
                        }
                        Err(message) => {
                            accepted.update(cx, |import, cx| {
                                import.set_error(Some(message));
                                cx.notify();
                            });
                            false
                        }
                    }
                })
        });
        cx.notify();
    }

    /// Deleting a proxy that is still assigned is refused, and says by whom.
    fn on_delete_proxy(&mut self, id: ProxyId, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let (name, endpoint, used_by) = match self.state.proxy(id) {
            Some(proxy) => {
                let used_by = self
                    .state
                    .proxy_rows()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|row| row.proxy.id == id)
                    .map(|row| row.used_by)
                    .unwrap_or_default();
                let endpoint = proxy.endpoint();
                (proxy.name, endpoint, used_by)
            }
            None => (id.to_string(), String::new(), Vec::new()),
        };
        let description = if used_by.is_empty() {
            t.delete_proxy_confirm(&name, &endpoint)
        } else {
            t.proxy_in_use(&name, &used_by.join(", "))
        };
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let description = description.clone();
            alert
                .title(t.delete_proxy_title)
                .description(description)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t.delete)
                        .cancel_text(t.keep)
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    let _ = view.state.delete_proxy(id);
                                    cx.notify();
                                });
                            }
                            true
                        })
                        .on_cancel(|_, _, _| true),
                )
        });
        cx.notify();
    }

    fn on_verify(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        let job = match self.state.begin_verification(id) {
            Ok(job) => job,
            Err(error) => {
                self.state.push_notice(error.to_string(), true);
                cx.notify();
                return;
            }
        };
        let verifier = Arc::clone(&self.verifier);
        let sender = self.verification_tx.clone();
        std::thread::spawn(move || {
            let outcome = verifier.verify(&job);
            let _ = sender.send((job.profile_id, outcome));
        });
        cx.notify();
    }

    /// Sends one request through a proxy and reports what left.
    ///
    /// The work is a socket held open for as long as the far end takes, so it
    /// runs on a worker and the window stays responsive. `live` travels with
    /// the answer because the row has to say whether the engine probed was
    /// already carrying a profile's traffic or was started for the test.
    fn on_test_proxy(&mut self, id: ProxyId, cx: &mut Context<Self>) {
        let job = match self.state.begin_proxy_test(id) {
            Ok(job) => job,
            Err(error) => {
                self.state.push_notice(error.to_string(), true);
                cx.notify();
                return;
            }
        };
        let tester = Arc::clone(&self.tester);
        let sender = self.proxy_test_tx.clone();
        std::thread::spawn(move || {
            let live = job.is_live();
            let outcome = tester.test(&job);
            let _ = sender.send((job.proxy_id, live, outcome));
        });
        cx.notify();
    }

    /// A new profile starts a form rather than a row.
    ///
    /// The row is written when the form is accepted, so a cancelled form leaves
    /// nothing behind and the profile that appears is the one that was
    /// configured - core included, because which engine runs a profile is what
    /// decides the switches it may claim.
    fn on_new_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let cores = self.state.core_choices();
        let Some(core) = cores.first().map(|choice| choice.id) else {
            self.state.push_notice(t.no_core_registered, true);
            cx.notify();
            return;
        };
        let proxies = self.state.proxy_choices();
        let name = self.state.next_profile_name();
        let editor =
            cx.new(|cx| ProfileEditor::new_profile(&name, core, &cores, &proxies, t, window, cx));
        self.open_profile_form(editor, "New profile", "Create", window, cx);
    }

    fn on_start(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        self.open_profile(id, Opening::Start, cx);
    }

    fn on_stop(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        let _ = self.state.stop(id);
        cx.notify();
    }

    fn on_restart(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        self.open_profile(id, Opening::Restart, cx);
    }

    /// Starts or restarts a profile, asking its proxy first when it has one.
    ///
    /// A profile whose traffic leaves through a proxy is only as usable as that
    /// proxy, so the command is held back until a request has come back through
    /// the same engine the profile would use - the same question the Proxies page
    /// asks by hand, on the same worker, through the same channel, so the answer
    /// lands in the same place: the proxy row gets a fresh reading either way,
    /// and the opening goes ahead or is refused with a sentence saying which
    /// stage of the path failed.
    fn open_profile(&mut self, id: ProfileId, how: Opening, cx: &mut Context<Self>) {
        let gate = match self.state.begin_opening(id, how) {
            Ok(gate) => gate,
            Err(error) => {
                self.state.push_notice(error.to_string(), true);
                cx.notify();
                return;
            }
        };
        if let StartGate::Checking(job) = gate {
            let tester = Arc::clone(&self.tester);
            let sender = self.proxy_test_tx.clone();
            std::thread::spawn(move || {
                let live = job.is_live();
                let outcome = tester.test(&job);
                let _ = sender.send((job.proxy_id, live, outcome));
            });
        }
        cx.notify();
    }

    fn on_select(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        self.state.select(id);
        cx.notify();
    }

    /// The filter field, built the first time the view renders and kept after.
    ///
    /// Kept rather than rebuilt, so the text survives a trip to another page: a
    /// filter box that forgets what it was narrowing would be a small betrayal
    /// of what a filter box usually means.
    fn ensure_filter_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let t = self.state.text();
        if let Some(input) = &self.filter_input {
            return input.clone();
        }

        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t.profiles_filter_placeholder));
        // Observed rather than subscribed to change events: the field notifies
        // for every edit, and reading its value afterwards is what the listing
        // needs, so there is no event kind to match on and no way to miss one.
        self.filter_changed = Some(cx.observe(&input, Self::on_filter_changed));
        self.filter_input = Some(input.clone());
        input
    }

    /// Narrows the profiles list to what the field holds.
    ///
    /// Only what the page lists. A profile that stops matching keeps running,
    /// keeps its reading, and stays the profile the details panel answers for;
    /// the filter is a way of looking, not a way of acting.
    fn on_filter_changed(&mut self, input: Entity<InputState>, cx: &mut Context<Self>) {
        let typed = input.read(cx).value().to_string();
        if typed == self.state.profile_filter() {
            return;
        }
        self.state.set_profile_filter(typed);
        cx.notify();
    }

    /// Empties the filter, from a button rather than from the field.
    ///
    /// `InputState::set_value` deliberately emits no change event, so the
    /// filter is cleared here instead of waiting to be told about it.
    fn on_clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.set_profile_filter(String::new());
        if let Some(input) = self.filter_input.clone() {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    /// Builds the Settings page's export path field on first use.
    ///
    /// Started empty rather than filled with the default path. An empty field
    /// resolves to a new file each time it is used, so a window left open
    /// overnight does not propose a name from yesterday - and the sentence under
    /// the field says which path it would be right now.
    fn ensure_export_input(
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

    /// Writes a configuration backup to whatever the field says.
    fn on_export_configuration(&mut self, cx: &mut Context<Self>) {
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

    fn on_toggle_export_credentials(&mut self, include: bool, cx: &mut Context<Self>) {
        self.state.set_export_includes_credentials(include);
        cx.notify();
    }

    /// Writes a report about this installation, to the file the card named.
    ///
    /// Nothing is typed and nothing is confirmed: the destination is a new file
    /// under the data directory every second, and the report says where it went.
    fn on_write_diagnostics(&mut self, cx: &mut Context<Self>) {
        let _ = self.state.write_diagnostics();
        cx.notify();
    }

    /// Builds the Settings page's import path field on first use, the way the
    /// export one is built.
    fn ensure_import_input(
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

    /// Reads a configuration backup from whatever the field says.
    fn on_import_configuration(&mut self, cx: &mut Context<Self>) {
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

    /// Builds the Settings page's restore path field on first use.
    fn ensure_restore_input(
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

    /// Restores the configuration, asking first when there is something to
    /// replace.
    ///
    /// The precondition is the difference from an import: an empty installation
    /// is restored at once, because there is nothing to lose, and a populated
    /// one opens a confirmation that says what will be replaced. The mode is the
    /// confirmation, so a mistaken click cannot replace a live configuration.
    fn on_restore_configuration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Puts the restore on a worker and lets the confirmation close.
    ///
    /// The file is read, checked and applied as one transaction, so it is the
    /// largest piece of work any of these four do - and the one least able to
    /// happen where the window is drawn, since the window is what the user is
    /// watching while a whole configuration is replaced.
    fn start_restore(&mut self, mode: RestoreMode, cx: &mut Context<Self>) {
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

    /// Asks before a restore replaces a populated installation.
    fn confirm_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Builds the Settings page's browser-data directory field on first use.
    fn ensure_browser_data_input(
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
    fn on_browser_data(&mut self, direction: Direction, cx: &mut Context<Self>) {
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

    fn on_dismiss_notice(&mut self, cx: &mut Context<Self>) {
        self.state.dismiss_notice();
        cx.notify();
    }

    fn on_copy_args(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.state.selected() {
            let args = row.effective_args();
            if !args.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(args.join("\n")));
            }
        }
    }

    /// Exit through the same path as closing the last window: `run` returns and
    /// `main` then asks the supervisor to reclaim every child process.
    /// The window's Quit button, which is the close affordance the user can
    /// reach without the title bar and therefore asks the same question.
    fn on_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.decide_exit(window, cx);
    }

    /// The window's close button, as a question that can be refused.
    ///
    /// `false` keeps the window open, which is what two of the four modes mean:
    /// "ask" has not been answered yet, and "keep running" is not a close at all.
    fn on_close_requested(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
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
    fn decide_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    fn leave(&mut self, exit: Exit, window: &mut Window, cx: &mut Context<Self>) {
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
    fn enter_background(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        // No banner: it would be painted into a window the user has just put
        // away. The activity log is where this is recorded, because the log is
        // what outlives the window.
        tracing::info!("{}", self.state.text().exit_in_background);
        if self.tray.is_none() {
            // A desktop that will not take the icon is worth saying out loud and
            // is not worth refusing: the window still hides, and the log is
            // what says why there is no way back from the tray.
            match Tray::start(self.state.text()) {
                Ok(tray) => self.tray = Some(tray),
                Err(error) => tracing::warn!("no tray icon: {error}"),
            }
        }
        window_visibility::hide(window);
    }

    /// What the tray asked for, if anything.
    fn drain_tray(&self) -> Vec<TrayEvent> {
        self.tray
            .as_ref()
            .map(|tray| tray.drain())
            .unwrap_or_default()
    }

    /// Carries out one of those requests.
    fn handle_tray(&mut self, event: TrayEvent, window: &mut Window, cx: &mut Context<Self>) {
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
    fn open_exit_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let p = palette(cx);
        let body = if self.state.any_profile_active() {
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
                    exit_choice(&exit, p, t)
                        .test_support()
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

mod cores;
mod details;
mod logs;
mod profiles;
mod proxies;
mod settings;

use self::cores::{cores_body, cores_header};
use self::details::details_panel;
use self::logs::{format_age, logs_body, logs_header};
use self::profiles::{empty_hint, profile_list, profiles_header};
use self::proxies::{proxies_body, proxies_header};
use self::settings::{
    SettingsCards, SettingsExport, exit_choice, key_help, settings_body, settings_header,
};

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The close button asks before it closes, which is the one place that
        // can: `on_window_closed` is told after the fact, and by then the window -
        // and with it the chance to ask - is gone. The hook needs a window, and
        // this view is built before there is one, so the first frame installs it.
        if !self.close_hook {
            self.close_hook = true;
            let view = cx.entity().downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                view.update(cx, |view, cx| view.on_close_requested(window, cx))
                    // The view is gone, so there is nothing left to ask.
                    .unwrap_or(true)
            });
        }

        let t = self.state.text();
        let p = palette(cx);
        // The overlay layers live above the view and are rendered by the view
        // itself: without these, a dialog can be opened and never appear.
        let dialogs = Root::render_dialog_layer(window, cx);
        let sheets = Root::render_sheet_layer(window, cx);
        let notifications = Root::render_notification_layer(window, cx);
        // The filter field is built here because rendering is the first place
        // with a window to give an `InputState`; the view keeps it after that.
        let filter_input = self.ensure_filter_input(window, cx);
        let export_input = self.ensure_export_input(window, cx);
        let import_input = self.ensure_import_input(window, cx);
        let restore_input = self.ensure_restore_input(window, cx);
        let browser_data_input = self.ensure_browser_data_input(window, cx);
        let export_destination = self.state.export_destination();
        let diagnostics_destination = self.state.diagnostics_destination();
        let export_includes_credentials = self.state.export_includes_credentials();
        let filter = self.state.profile_filter().to_string();
        let total = self.state.rows().len();
        let visible = self.state.visible_rows();
        let selected = self.state.selected().cloned();
        let selected_id = self.state.selected_id();
        let verifications: std::collections::HashMap<ProfileId, Verification> = self
            .state
            .rows()
            .iter()
            .filter_map(|row| {
                self.state
                    .verification(row.profile.id)
                    .map(|verification| (row.profile.id, verification.clone()))
            })
            .collect();
        let verification = selected_id.and_then(|id| verifications.get(&id).cloned());
        let notice = self.state.notice().cloned();
        let has_core = self.state.has_core();
        let details_tab = self.state.details_tab();
        let log_tail = selected_id
            .map(|id| self.state.log_tail(id))
            .unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .child(header(cx, t))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar(self.state.page(), cx, t))
                    .child({
                        let page = self.state.page();
                        let proxy_rows = self.state.proxy_rows().unwrap_or_default();
                        let proxy_tests: std::collections::HashMap<ProxyId, ProxyTest> = proxy_rows
                            .iter()
                            .filter_map(|row| {
                                self.state
                                    .proxy_test(row.proxy.id)
                                    .map(|test| (row.proxy.id, test.clone()))
                            })
                            .collect();
                        let core_rows = self.state.core_rows().unwrap_or_default();
                        let setting_rows = self.state.setting_rows();
                        let log_rows = self.state.log_rows();
                        let log_count = self.state.log_len();
                        let log_filter = self.state.log_filter();
                        let log_status = match self.state.log_file_status() {
                            Ok(path) => Ok(path.display().to_string()),
                            Err(error) => Err(error.to_string()),
                        };
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .p_6()
                            .gap_4()
                            .child(match page {
                                Page::Proxies => proxies_header(cx, t),
                                Page::Cores => cores_header(cx, t),
                                Page::Log => logs_header(log_filter, &log_status, cx, p, t),
                                Page::Settings => settings_header(p, t),
                                Page::Profiles => profiles_header(
                                    &ProfilesHeader {
                                        search: filter_input.clone(),
                                        filter: filter.clone(),
                                        total,
                                        visible: visible.len(),
                                        has_core,
                                    },
                                    cx,
                                    t,
                                ),
                            })
                            .children(notice.map(|notice| notice_banner(notice, cx, t)))
                            .when(page == Page::Proxies, |this| {
                                this.child(proxies_body(&proxy_rows, &proxy_tests, cx, t))
                            })
                            .when(page == Page::Cores, |this| {
                                this.child(cores_body(&core_rows, cx, t))
                            })
                            .when(page == Page::Log, |this| {
                                this.child(logs_body(&log_rows, log_count, log_filter, cx, p, t))
                            })
                            .when(page == Page::Settings, |this| {
                                this.child(settings_body(
                                    &setting_rows,
                                    SettingsCards {
                                        export: SettingsExport {
                                            path: export_input.clone(),
                                            destination: export_destination.clone(),
                                            include_credentials: export_includes_credentials,
                                        },
                                        import: import_input.clone(),
                                        restore: restore_input.clone(),
                                        browser_data: browser_data_input.clone(),
                                        diagnostics: diagnostics_destination.clone(),
                                        theme: self.state.theme(),
                                        language: self.state.language(),
                                        exit_mode: self.state.exit_mode(),
                                    },
                                    cx,
                                    p,
                                    t,
                                ))
                            })
                            .when(page == Page::Profiles, |this| {
                                this.child(
                                    div()
                                        .id("profiles-scroll")
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_h_0()
                                        .gap_2()
                                        .overflow_y_scroll()
                                        .children(empty_hint(
                                            total,
                                            visible.len(),
                                            has_core,
                                            &filter,
                                            cx,
                                            p,
                                            t,
                                        ))
                                        .child(profile_list(
                                            &visible,
                                            selected_id,
                                            &verifications,
                                            cx,
                                            p,
                                            t,
                                        )),
                                )
                                .child(details_panel(
                                    selected.as_ref(),
                                    verification,
                                    details_tab,
                                    &log_tail,
                                    cx,
                                    p,
                                    t,
                                ))
                            })
                    }),
            )
            .children(dialogs)
            .children(sheets)
            .children(notifications)
    }
}

fn header(cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    div()
        .flex()
        .items_center()
        .justify_between()
        .px_6()
        .py_4()
        .border_b_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .child("Fingerprint Browser v1"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(p.dim))
                        .child("Rust + GPUI Kit + Fingerprint-Chromium"),
                )
                .child(
                    Button::new("quit")
                        .label(t.quit)
                        .ghost()
                        .on_click(cx.listener(|this, _, window, cx| this.on_quit(window, cx))),
                ),
        )
}

const PAGES: [Page; 5] = [
    Page::Profiles,
    Page::Proxies,
    Page::Cores,
    Page::Log,
    Page::Settings,
];

fn sidebar(page: Page, cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w(px(220.0))
        .flex_shrink_0()
        .border_r_1()
        .border_color(rgb(p.border))
        .p_4()
        .children(PAGES.map(|candidate| {
            let active = candidate == page;
            let label = if candidate.is_ready() {
                candidate.label(t).to_string()
            } else {
                t.nav_soon(candidate.label(t))
            };
            div()
                .id(format!("nav-{}", candidate.id()))
                .test_support()
                .px_3()
                .py_2()
                .rounded_md()
                .text_sm()
                .when(active, |this| {
                    this.bg(rgb(p.border)).font_weight(FontWeight::MEDIUM)
                })
                .when(!active && candidate.is_ready(), |this| {
                    this.text_color(rgb(p.muted)).cursor_pointer()
                })
                .when(!candidate.is_ready(), |this| this.text_color(rgb(p.dim)))
                .child(label)
                .when(candidate.is_ready(), |this| {
                    this.on_click(cx.listener(move |this, _, _, cx| this.on_page(candidate, cx)))
                })
        }))
}

/// What the Profiles page's header needs to render its filter.
///
/// One struct rather than four arguments, because this is where the page says
/// what it is showing, and the list of things it has to say grows.
struct ProfilesHeader {
    /// The filter field, built by the view on its first render.
    search: Entity<InputState>,
    /// What the field holds, so the subtitle can say whether it is narrowing.
    filter: String,
    /// Every profile, and how many of them the filter left.
    total: usize,
    visible: usize,
    /// Whether any core is registered at all: a profile needs one to launch, so
    /// the button that makes one is disabled until there is one.
    has_core: bool,
}

/// One toast as the notification layer shows it.
fn toast_notification(toast: &Toast) -> Notification {
    let note = Notification::new().message(toast.message.clone());
    match toast.kind {
        ToastKind::Success => note.with_type(NotificationType::Success),
        ToastKind::Warning => note.with_type(NotificationType::Warning),
        ToastKind::Error => note.with_type(NotificationType::Error),
    }
}

/// Push every toast into a window's notification layer.
fn push_toasts(toasts: &[Toast], window: &mut Window, cx: &mut App) {
    for toast in toasts {
        window.push_notification(toast_notification(toast), cx);
    }
}

fn notice_banner(notice: crate::state::Notice, cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    let (background, foreground) = if notice.error {
        (p.danger_bg, p.danger)
    } else {
        (p.panel, p.secondary)
    };

    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_4()
        .py_2()
        .rounded_md()
        .bg(rgb(background))
        .child(
            div()
                .text_xs()
                .text_color(rgb(foreground))
                .child(notice.message),
        )
        .child(
            Button::new("dismiss-notice")
                .label(t.dismiss)
                .ghost()
                .on_click(cx.listener(|this, _, _, cx| this.on_dismiss_notice(cx))),
        )
}

#[cfg(test)]
mod tests;
