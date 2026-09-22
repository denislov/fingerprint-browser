//! The GPUI window: renders [`AppState`] and forwards user actions into it.
//!
//! The view owns no runtime state. Notifications only mark the snapshot cache
//! dirty; the snapshot itself stays the single source of truth, exactly as the
//! runtime façade contract requires.

use crate::browser_data::BrowserDataCopier;
use crate::core_editor::CoreEditor;
use crate::editor::{ProfileEdit, ProfileEditor};
use crate::exit::{Exit, ExitMode};
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
                    if notified || reconcile || verified || tested || opened || copied {
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
    fn on_redetect_core(&mut self, id: CoreId, cx: &mut Context<Self>) {
        let _ = self.state.redetect_core(id);
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
        // through the banner, the toast and the activity log, all of which
        // `export_configuration` reaches by way of `set_notice`.
        let _ = self.state.export_configuration();
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
        // clean imports to a toast.
        let _ = self.state.import_configuration();
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
            Ok(true) => {
                let _ = self.state.restore_configuration(RestoreMode::OnlyWhenEmpty);
            }
            Ok(false) => self.confirm_restore(window, cx),
            Err(error) => self.state.push_notice(error.to_string(), true),
        }
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
                                    let _ = view.state.restore_configuration(RestoreMode::Replace);
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

fn profiles_header(header: &ProfilesHeader, cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    // While a filter is on, the count is the useful sentence: it is how the user
    // finds out that the list is not the whole list. Without one, the header
    // goes back to explaining what a profile is. The accessible name spells out
    // both either way, because a status line is worth hearing in full.
    let filtering = header.total > 0 && !header.filter.trim().is_empty();
    let subtitle = if filtering {
        t.profiles_showing(header.visible, header.total)
    } else {
        t.profiles_intro.to_string()
    };
    let announcement = if filtering {
        t.profiles_showing_filtered(header.visible, header.total, header.filter.trim())
    } else {
        t.profiles_total(header.total)
    };

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t.nav_profiles),
                        )
                        .child(
                            div()
                                .id("profile-count")
                                .test_support()
                                .aria_label(announcement)
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(subtitle),
                        ),
                )
                .child(
                    Button::new("new-profile")
                        .label(t.new_profile)
                        .primary()
                        // A profile needs a core to launch, and the empty state
                        // below says where to get one. The button is disabled
                        // rather than opening a dialog the service would refuse.
                        .disabled(!header.has_core)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_new_profile(window, cx)),
                        ),
                ),
        )
        .child(
            div().w(px(PROFILE_FILTER_WIDTH)).child(
                Input::new(&header.search)
                    .id("profile-filter")
                    .aria_label(t.profiles_filter_aria)
                    .cleanable(true),
            ),
        )
}

fn proxies_header(cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t.nav_proxies),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(t.proxies_intro),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("import-proxy")
                        .label(t.import_from_link)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_import_proxy(window, cx)),
                        ),
                )
                .child(
                    Button::new("new-proxy")
                        .label(t.new_proxy)
                        .primary()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_edit_proxy(None, window, cx)),
                        ),
                ),
        )
}

fn proxies_body(
    rows: &[ProxyRow],
    tests: &std::collections::HashMap<ProxyId, ProxyTest>,
    cx: &mut Context<AppView>,
    t: &Text,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id("proxies-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_2()
        .overflow_y_scroll()
        .when(rows.is_empty(), |this| {
            this.child(
                div()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .bg(rgb(p.panel))
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child(t.proxies_empty),
            )
        })
        // The cards are built inline: a helper would have to return a type
        // borrowing the context, which the closure cannot hand back.
        .children(rows.iter().enumerate().map(|(index, row)| {
            let id = row.proxy.id;
            let test = tests.get(&id);
            div()
                .id(format!("proxy-{index}"))
                .test_support()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .px_4()
                .py_3()
                .rounded_md()
                .border_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(row.proxy.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(row.endpoint()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(if row.is_used() { p.success } else { p.muted }))
                                .child(row.usage_label(t)),
                        )
                        .children(proxy_test_reading(test, id, p, t)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new(format!("test-proxy-{index}"))
                                .label(t.test)
                                .outline()
                                .disabled(test.is_some_and(ProxyTest::is_running))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.on_test_proxy(id, cx)),
                                ),
                        )
                        .child(
                            Button::new(format!("edit-proxy-{index}"))
                                .label(t.edit)
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_edit_proxy(Some(id), window, cx)
                                })),
                        )
                        .child(
                            Button::new(format!("delete-proxy-{index}"))
                                .label(t.delete)
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_delete_proxy(id, window, cx)
                                })),
                        ),
                )
        }))
}

/// The result of the last test of one proxy, or nothing when it has not run.
///
/// The row says which engine was probed, because "this proxy works" and "this
/// profile's traffic is going through it" are different claims and only the
/// second one is about a leak.
fn proxy_test_reading(
    test: Option<&ProxyTest>,
    id: ProxyId,
    p: Palette,
    t: &Text,
) -> Option<AnyElement> {
    let test = test?;
    let colour = match test {
        ProxyTest::Running => p.dim,
        ProxyTest::Passed(reading) if reading.live => p.success,
        ProxyTest::Passed(_) => p.info,
        ProxyTest::Failed(_) => p.danger,
    };
    let summary = match test {
        ProxyTest::Running => test.label(t),
        ProxyTest::Passed(reading) => t.proxy_test_reading(
            &test.label(t),
            if reading.live {
                t.engine_running_profile
            } else {
                t.engine_temporary
            },
        ),
        // The evidence behind the class is in the activity log: a row is one
        // line, and an engine's own words are not.
        ProxyTest::Failed(_) => t.proxy_test_failed(&test.label(t)),
    };
    Some(
        div()
            .id(format!("proxy-test-{id}"))
            .test_support()
            .text_xs()
            .text_color(rgb(colour))
            .child(summary)
            .into_any_element(),
    )
}

fn cores_header(cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t.nav_cores),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(t.cores_intro),
                ),
        )
        .child(
            Button::new("new-core")
                .label(t.add_core)
                .primary()
                .on_click(cx.listener(|this, _, window, cx| this.on_edit_core(None, window, cx))),
        )
}

fn cores_body(rows: &[CoreRow], cx: &mut Context<AppView>, t: &Text) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id("cores-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_2()
        .overflow_y_scroll()
        .when(rows.is_empty(), |this| {
            this.child(
                div()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .bg(rgb(p.panel))
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child(t.cores_empty),
            )
        })
        // Built inline: a helper returning a borrowed type cannot escape the
        // closure that owns the context.
        .children(rows.iter().enumerate().map(|(index, row)| {
            let id = row.core.id;
            div()
                .id(format!("core-{index}"))
                .test_support()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .px_4()
                .py_3()
                .rounded_md()
                .border_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(row.core.name.clone()),
                                )
                                .when(!row.present, |this| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(p.danger))
                                            .child(t.core_executable_missing),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(t.core_label(&row.core.version, row.core.major)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(match row.generation_label() {
                                    Some(label) if label.contains("honoured") => p.success,
                                    Some(_) => p.warning,
                                    None => p.danger,
                                }))
                                .child(
                                    row.generation_label()
                                        .unwrap_or_else(|| t.core_no_version.to_string()),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(row.core.executable.to_string_lossy().to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(if row.is_used() { p.success } else { p.muted }))
                                .child(row.usage_label(t)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new(format!("redetect-core-{index}"))
                                .label(t.redetect)
                                .outline()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.on_redetect_core(id, cx)
                                    }),
                                ),
                        )
                        .child(
                            Button::new(format!("edit-core-{index}"))
                                .label(t.edit)
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_edit_core(Some(id), window, cx)
                                })),
                        )
                        .child(
                            Button::new(format!("delete-core-{index}"))
                                .label(t.delete)
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_delete_core(id, window, cx)
                                })),
                        ),
                )
        }))
}

/// What a setting does, in one line, under its field.
///
/// The rows that cannot be edited have no field, so this covers the editable
/// ones - and the help a row carries on the page is its own (`SettingRow::note`),
/// because a page has room for a sentence a dialog does not.
fn key_help(key: SettingKey, t: &Text) -> String {
    match key {
        SettingKey::XrayExecutable => t.help_xray_executable_field.to_string(),
        _ => String::new(),
    }
}

fn settings_header(p: Palette, t: &Text) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t.nav_settings),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(t.settings_intro),
        )
}

/// Everything the export card needs that is not already in [`AppState`].
///
/// A value rather than four positional parameters, for the reason
/// [`crate::verifier::VerificationJob`] is one: the list grows, and every call
/// site should not have to change when it does.
struct SettingsExport {
    path: Entity<InputState>,
    /// Where an empty field would write right now, shown under the field so the
    /// default is never a guess.
    destination: PathBuf,
    include_credentials: bool,
}

/// The four backup cards at the foot of the Settings page, and the fields they
/// read.
///
/// A value rather than four positional parameters, for the reason
/// [`SettingsExport`] is one: the set grows, and every call site should not have
/// to change when it does.
struct SettingsCards {
    export: SettingsExport,
    import: Entity<InputState>,
    restore: Entity<InputState>,
    browser_data: Entity<InputState>,
    /// Where the diagnostics card's button would write. Computed while
    /// rendering, so the line under the button names the file this press would
    /// produce rather than one named a minute ago.
    diagnostics: PathBuf,
    theme: ThemeChoice,
    language: Lang,
    exit_mode: ExitMode,
}

fn settings_body(
    rows: &[crate::settings::SettingRow],
    cards: SettingsCards,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> impl IntoElement {
    // Built first, with their lifetimes erased: each card borrows the context
    // and the chain below borrows it again for its own listeners, and an
    // opaque return type would keep the first borrow alive to the end of the
    // chain.
    let appearance: AnyElement =
        appearance_card(cards.theme, cards.language, cx, t).into_any_element();
    let exit_card: AnyElement = exit_mode_card(cards.exit_mode, cx, t).into_any_element();
    let export_card: AnyElement = export_card(&cards.export, cx, t).into_any_element();
    let import_card: AnyElement = import_card(&cards.import, cx, t).into_any_element();
    let restore_card: AnyElement = restore_card(&cards.restore, cx, t).into_any_element();
    let browser_data_card: AnyElement =
        browser_data_card(&cards.browser_data, cx, t).into_any_element();
    let diagnostics_card: AnyElement =
        diagnostics_card(&cards.diagnostics, cx, t).into_any_element();
    div()
        .id("settings-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_2()
        .overflow_y_scroll()
        // Built inline: a helper returning a borrowed type cannot escape the
        // closure that owns the context.
        .children(rows.iter().map(|row| {
            let key = row.key;
            let editable = key.editable();
            div()
                .id(format!("setting-{}", key.id()))
                .test_support()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .px_4()
                .py_3()
                .rounded_md()
                .border_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(key.label(t)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(
                                            if row.source == crate::settings::Source::Environment {
                                                p.warning
                                            } else {
                                                p.muted
                                            },
                                        ))
                                        .child(row.source_label(t)),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.text_soft))
                                .child(row.value.clone()),
                        )
                        .children(
                            row.shadowed_label(t).map(|label| {
                                div().text_xs().text_color(rgb(p.warning)).child(label)
                            }),
                        )
                        .children(
                            row.note
                                .clone()
                                .map(|note| div().text_xs().text_color(rgb(p.muted)).child(note)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(row.key.effect().label(t)),
                        )
                        .when(editable, |this| {
                            this.child(
                                Button::new(format!("edit-setting-{}", key.id()))
                                    .label(t.change)
                                    .outline()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.on_edit_setting(key, window, cx)
                                    })),
                            )
                        }),
                )
        }))
        .child(appearance)
        .child(exit_card)
        .child(export_card)
        .child(import_card)
        .child(restore_card)
        .child(browser_data_card)
        .child(diagnostics_card)
}

/// The appearance card, first on the page.
///
/// It leads because it is the one setting that changes what the reader is
/// looking at rather than what the program will do next, and because it is the
/// one whose effect is immediate: every other card here describes a future start.
///
/// The control is a pair of chips rather than a toggle, so both options are
/// visible at once. A toggle hides the alternative behind the label of the thing
/// you are not currently looking at, which is the one thing the reader cannot
/// check against the window in front of them.
fn appearance_card(
    choice: ThemeChoice,
    language: Lang,
    cx: &mut Context<AppView>,
    t: &Text,
) -> impl IntoElement {
    let p = palette(cx);
    settings_card(p)
        .id("appearance")
        .test_support()
        .child(settings_card_heading(
            t.interface_title,
            t.interface_body,
            p,
        ))
        // Appearance: two chips, so both options are visible at once. A toggle
        // hides the alternative behind the label of the mode you are not looking
        // at, which is the one thing the reader cannot check against the window.
        .child(
            settings_card_row(t.appearance_title, p).children(ThemeChoice::ALL.map(|option| {
                let active = option == choice;
                chip(
                    format!("theme-{}", option.code()),
                    option.label(t),
                    active,
                    p,
                )
                .test_support()
                .aria_label(t.theme_aria(option.label(t)))
                .on_click(cx.listener(move |this, _, _, cx| this.on_choose_theme(option, cx)))
            })),
        )
        // Language: each chip is labelled in its own language, so someone who
        // cannot read the one they are in can still find the way out of it.
        .child(
            settings_card_row(t.language_title, p).children(Lang::ALL.map(|option| {
                let active = option == language;
                chip(
                    format!("language-{}", option.code()),
                    option.label(),
                    active,
                    p,
                )
                .test_support()
                .on_click(cx.listener(move |this, _, _, cx| this.on_choose_language(option, cx)))
            })),
        )
}

/// What closing the window does, and the four answers to it.
///
/// Beside the appearance and the language because it is the same kind of setting:
/// a standing choice about this installation, kept in the config file, read when
/// it matters rather than when the page is drawn. The difference is *when* it
/// matters - the appearance and the language change what is on screen, while this
/// one is read at the moment a window is closed, which may be days later.
///
/// Four chips rather than a menu, for the reason the appearance has two: every
/// answer is visible at once, and the one in force is the one that is lit. The
/// sentence under them is the chosen mode's own, so what each answer costs is
/// readable without hovering anything.
fn exit_mode_card(mode: ExitMode, cx: &mut Context<AppView>, t: &Text) -> impl IntoElement {
    let p = palette(cx);
    settings_card(p)
        .id("exit-mode")
        .test_support()
        .child(settings_card_heading(
            t.exit_card_title,
            t.exit_card_body,
            p,
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .children(ExitMode::ALL.map(|option| {
                    let active = option == mode;
                    chip(
                        format!("exit-{}", option.code()),
                        option.label(t),
                        active,
                        p,
                    )
                    .test_support()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.on_choose_exit_mode(option, cx)),
                    )
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(mode.note(t).to_string()),
        )
}

/// One of the dialog's three answers: what it is, and what it does.
///
/// A row rather than a footer button, because the three answers need their
/// sentences: "leave browsers running" and "stop everything" are one word apart
/// and opposite in effect, and a button that only carried the word would be a
/// decision made on a guess.
fn exit_choice(exit: &Exit, p: Palette, t: &Text) -> Stateful<Div> {
    // A given height, because the three answers have to look like three of the
    // same thing. Left to itself the dialog's content box gave the last row a
    // text line less than the two above it, and that row's own note then painted
    // over where its bottom border was - the text, the order and the line height
    // were each ruled out by measurement, so the box is what is pinned. The
    // height leaves room for a note that wraps to two lines.
    div()
        .id(format!("exit-choice-{}", exit.code()))
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .h(px(84.0))
        .px_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(p.dim))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(exit.label(t)),
        )
        .child(div().text_xs().text_color(rgb(p.muted)).child(exit.note(t)))
}

/// The frame the Settings cards share: a bordered column.
fn settings_card(p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .px_4()
        .py_4()
        .rounded_md()
        .border_1()
        .border_color(rgb(p.border))
}

/// A card's title and the sentence under it.
fn settings_card_heading(title: &str, body: &str, p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(title.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(body.to_string()),
        )
}

/// A labelled row of chips inside a card.
fn settings_card_row(label: &str, p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(label.to_string()),
        )
        .child(div().flex().items_center().gap_2())
}

/// One chip of a choice: the same control the editors use, for the same reason -
/// the chosen one is filled, the others are not, and both are always on screen.
fn chip(id: String, label: &str, active: bool, p: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(if active { p.dim } else { p.border }))
        .text_xs()
        .when(active, |this| {
            this.bg(rgb(p.border))
                .text_color(rgb(p.text))
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |this| this.text_color(rgb(p.muted)))
        .child(label.to_string())
}

/// The export card at the foot of the Settings page.
///
/// The path is typed rather than picked from a native file dialog. A chooser
/// would mean another dependency, and opening one is the part of a desktop
/// integration most likely to behave differently over RDP - which is a supported
/// way to run this. The field starts empty, and the line under it names the file
/// an empty field would write, so the default is shown rather than described.
fn export_card(export: &SettingsExport, cx: &mut Context<AppView>, t: &Text) -> impl IntoElement {
    let p = palette(cx);
    let view = cx.entity().downgrade();
    settings_card(p)
        .id("export-configuration")
        .test_support()
        .child(settings_card_heading(t.export_title, t.export_body, p))
        .child(
            path_row(&export.path, "export-path", t.export_path_label, p).child(
                Button::new("export-run")
                    .label(t.export)
                    .on_click(cx.listener(|this, _, _, cx| this.on_export_configuration(cx))),
            ),
        )
        .child(card_note(
            t.export_empty_writes(&export.destination.display().to_string()),
            p,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Checkbox::new("export-credentials")
                        .label(t.export_credentials)
                        .checked(export.include_credentials)
                        .on_change(move |&checked, _, cx: &mut App| {
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    view.on_toggle_export_credentials(checked, cx)
                                });
                            }
                        }),
                )
                .child(card_hint(t.export_credentials_note, p)),
        )
        // The one sentence on this page that is a warning rather than a
        // description: the file is about to hold passwords in the clear.
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.warning))
                .child(t.export_credentials_warning),
        )
}

/// The import card under the export one.
///
/// The import is the quieter of the two verbs, and the card says why in one
/// line: nothing already here is overwritten. That is not a promise the
/// program can keep on its own - it is what the rules do - but it is the
/// sentence a reader needs before they type a path and press the button.
fn import_card(
    input: &Entity<InputState>,
    cx: &mut Context<AppView>,
    t: &Text,
) -> impl IntoElement {
    let p = palette(cx);
    settings_card(p)
        .id("import-configuration")
        .test_support()
        .child(settings_card_heading(t.import_title, t.import_body, p))
        .child(
            path_row(input, "import-path", t.import_path_label, p).child(
                Button::new("import-run")
                    .label(t.import)
                    .on_click(cx.listener(|this, _, _, cx| this.on_import_configuration(cx))),
            ),
        )
        .child(card_note(t.import_note.to_string(), p))
}

/// The restore card, under the import one.
///
/// Restore and import sit together because they are the two ways to read the
/// same file, and the card says what separates them in one line: import adds,
/// restore replaces. The confirmation is not on the card but behind the button,
/// and only when there is something to replace, so the card states the rule
/// rather than describing a dialog.
fn restore_card(
    input: &Entity<InputState>,
    cx: &mut Context<AppView>,
    t: &Text,
) -> impl IntoElement {
    let p = palette(cx);
    settings_card(p)
        .id("restore-configuration")
        .test_support()
        .child(settings_card_heading(t.restore_title, t.restore_body, p))
        .child(
            path_row(input, "restore-path", t.restore_path_label, p).child(
                Button::new("restore-run").label(t.restore).on_click(
                    cx.listener(|this, _, window, cx| this.on_restore_configuration(window, cx)),
                ),
            ),
        )
        .child(card_note(t.restore_note.to_string(), p))
}

/// The browser-data card, below the configuration ones.
///
/// Browser data is the other artifact: too large to travel in a configuration
/// backup, and the half that carries the logins. One directory field with two
/// buttons, because a copy out writes to a place and a copy back in reads from
/// the same one.
fn browser_data_card(
    input: &Entity<InputState>,
    cx: &mut Context<AppView>,
    t: &Text,
) -> impl IntoElement {
    let p = palette(cx);
    settings_card(p)
        .id("browser-data")
        .test_support()
        .child(settings_card_heading(
            t.browser_data_title,
            t.browser_data_body,
            p,
        ))
        .child(
            path_row(input, "browser-data-path", t.browser_data_path_label, p)
                .child(Button::new("browser-data-out").label(t.copy_out).on_click(
                    cx.listener(|this, _, _, cx| this.on_browser_data(Direction::ToBackup, cx)),
                ))
                .child(
                    Button::new("browser-data-in")
                        .label(t.copy_in)
                        .outline()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.on_browser_data(Direction::FromBackup, cx)
                        })),
                ),
        )
        .child(card_note(t.browser_data_note.to_string(), p))
}

/// The diagnostics card, last on the Settings page.
///
/// Last because it is about none of the settings above it: it is the file you
/// write when one of them is not doing what you expect. One button and no path
/// field, unlike the export above it - the destination is the data directory's
/// own `diagnostics` folder, the line under the button names the exact file, and
/// the report says where it went. A path to type would be a second way to say
/// something the program already knows.
// `std::path::Path` spelled out: `gpui_kit::*` brings its own `Path`, and the
// two are unrelated - one is a file path, the other a drawing primitive.
fn diagnostics_card(
    destination: &std::path::Path,
    cx: &mut Context<AppView>,
    t: &Text,
) -> impl IntoElement {
    let p = palette(cx);
    settings_card(p)
        .id("diagnostics")
        .test_support()
        .child(settings_card_heading(
            t.diag_card_title,
            t.diag_card_body,
            p,
        ))
        .child(
            div().flex().items_center().gap_2().child(
                Button::new("diagnostics-run")
                    .label(t.diag_write)
                    .on_click(cx.listener(|this, _, _, cx| this.on_write_diagnostics(cx))),
            ),
        )
        .child(card_note(
            t.diag_card_note(&destination.display().to_string()),
            p,
        ))
}

/// A card's path field with room for the buttons beside it.
///
/// The field takes what is left rather than a fixed width, so a narrow window
/// narrows the path instead of pushing the button off the card.
fn path_row(input: &Entity<InputState>, id: &str, label: &str, p: Palette) -> Div {
    let _ = p;
    div().flex().items_center().gap_2().child(
        div().flex_1().min_w_0().child(
            Input::new(input)
                .id(id.to_string())
                .aria_label(label.to_string()),
        ),
    )
}

/// The dim line under a card's controls: what would happen, or what did.
fn card_note(text: String, p: Palette) -> Div {
    div().text_xs().text_color(rgb(p.dim)).child(text)
}

/// The muted sentence beside a checkbox, saying what the box does.
fn card_hint(text: &str, p: Palette) -> Div {
    div()
        .text_xs()
        .text_color(rgb(p.muted))
        .child(text.to_string())
}

fn logs_header(
    filter: LogFilter,
    status: &Result<String, String>,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w_0()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t.nav_log),
                )
                .child(div().text_xs().text_color(rgb(p.muted)).child(t.log_intro))
                .child(match status {
                    Ok(path) => div()
                        .id("log-file-status")
                        .test_support()
                        .aria_label(t.log_written_to(path))
                        .text_xs()
                        .text_color(rgb(p.dim))
                        .child(t.log_written_to(path)),
                    Err(error) => div()
                        .id("log-file-status")
                        .test_support()
                        .aria_label(t.log_not_written(error))
                        .text_xs()
                        .text_color(rgb(p.danger))
                        .child(t.log_not_written(error)),
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .children(LogFilter::ALL.map(|candidate| {
                            let active = candidate == filter;
                            Button::new(candidate.id())
                                .label(candidate.label(t))
                                .when(active, |button| button.primary())
                                .when(!active, |button| button.outline())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.on_set_log_filter(candidate, cx)
                                }))
                        })),
                )
                .child(
                    Button::new("copy-log")
                        .label(t.copy)
                        .outline()
                        .on_click(cx.listener(|this, _, _, cx| this.on_copy_log(cx))),
                )
                .child(
                    Button::new("clear-log")
                        .label(t.clear)
                        .outline()
                        .on_click(cx.listener(|this, _, _, cx| this.on_clear_log(cx))),
                ),
        )
}

fn logs_body(
    rows: &[LogRow],
    total: usize,
    filter: LogFilter,
    _cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> impl IntoElement {
    div()
        .id("logs-scroll")
        .test_support()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_1()
        .overflow_y_scroll()
        .when(rows.is_empty(), |this| {
            // An empty page says which kind of empty it is: nothing happened,
            // or the filter is hiding what did.
            let message = match total {
                0 => t.log_empty.to_string(),
                count => t.log_none_of_kind(filter.noun(t), count),
            };
            this.child(
                div()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .bg(rgb(p.panel))
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child(message),
            )
        })
        .children(rows.iter().enumerate().map(|(index, row)| {
            let label = format!("{} [{}] {}", row.who, row.level.label(t), row.message);
            div()
                .id(format!("log-{index}"))
                .test_support()
                .aria_label(label)
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .w(px(64.0))
                        .flex_shrink_0()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(log_level_color(row.level, p)))
                        .child(row.level.label(t)),
                )
                .child(
                    div()
                        .w(px(56.0))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(rgb(p.dim))
                        .child(format_age(row.at, t)),
                )
                .child(
                    div()
                        .w(px(140.0))
                        .flex_shrink_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(row.who.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(rgb(p.text_soft))
                        .child(row.message.clone()),
                )
        }))
}

fn log_level_color(level: LogLevel, p: Palette) -> u32 {
    match level {
        LogLevel::Info => p.muted,
        LogLevel::Warning => p.warning,
        LogLevel::Error => p.danger_strong,
    }
}

/// How long ago a line was written, freshly computed on each render.
fn format_age(at: SystemTime, t: &Text) -> String {
    let seconds = SystemTime::now()
        .duration_since(at)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    match seconds {
        0..=1 => t.just_now.to_string(),
        seconds if seconds < 60 => format!("{seconds}s"),
        seconds if seconds < 3600 => format!("{}m", seconds / 60),
        seconds => format!("{}h", seconds / 3600),
    }
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

fn empty_hint(
    total: usize,
    visible: usize,
    has_core: bool,
    filter: &str,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Option<impl IntoElement> {
    // Two kinds of empty look the same in a list and mean different things:
    // there are no profiles, or a filter is hiding the ones there are. Only the
    // second can be undone from here, so only it offers to.
    let filtering = total > 0 && visible == 0;
    if total > 0 && !filtering {
        return None;
    }

    let message = if filtering {
        t.empty_no_match(filter.trim(), total)
    } else if has_core {
        t.empty_no_profiles.to_string()
    } else {
        t.no_core_found.to_string()
    };

    Some(
        div()
            .id("empty-hint")
            .test_support()
            .aria_label(message.clone())
            .p_4()
            .rounded_md()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(div().text_xs().text_color(rgb(p.muted)).child(message))
            .when(filtering, |this| {
                this.child(
                    Button::new("clear-filter").label(t.clear_filter).on_click(
                        cx.listener(|this, _, window, cx| this.on_clear_filter(window, cx)),
                    ),
                )
            })
            // An empty list with no core to launch is the one empty state that
            // cannot be acted on where it is read: the sentence names an
            // environment variable and the page that matters is another one. So
            // it carries the way there instead of leaving the reader to find it.
            .when(!has_core && !filtering, |this| {
                this.child(
                    Button::new("empty-add-core")
                        .label(t.add_browser_core)
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.on_page(Page::Cores, cx);
                        })),
                )
            }),
    )
}

fn profile_list(
    rows: &[ProfileRow],
    selected_id: Option<ProfileId>,
    verifications: &std::collections::HashMap<ProfileId, Verification>,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .children(rows.iter().map(|row| {
            let id = row.profile.id;
            let is_selected = selected_id == Some(id);

            div()
                .id(format!("profile-{id}"))
                .test_support()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .px_4()
                .py_3()
                .rounded_md()
                .border_1()
                .border_color(rgb(if is_selected { p.dim } else { p.border }))
                .bg(rgb(if is_selected { p.border } else { p.panel }))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.on_select(id, cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(row.profile.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(t.profile_seed_line(
                                    row.profile.fingerprint.seed,
                                    row.profile.fingerprint.brand,
                                    row.profile.fingerprint.platform,
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.dim))
                                .child(route_label(row, t)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .gap_1()
                                .child(state_badge(row, p, t))
                                .children(verification_badge(verifications.get(&id), p, t))
                                .children(row.last_warning().map(|warning| {
                                    // The full text lives in Runtime Details; the row
                                    // only needs to say that something is off.
                                    div()
                                        .id(format!("warning-{id}"))
                                        .test_support()
                                        .max_w(px(WARNING_WIDTH))
                                        .truncate()
                                        .text_xs()
                                        .text_color(rgb(p.warning))
                                        .child(t.warning_line(warning))
                                }))
                                .children(row.last_error().map(|error| {
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.danger_strong))
                                        .child(error.to_string())
                                })),
                        )
                        .child(row_actions(row, cx, t)),
                )
        }))
}

fn row_actions(row: &ProfileRow, cx: &mut Context<AppView>, t: &Text) -> Div {
    let id = row.profile.id;

    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            Button::new(format!("start-{id}"))
                .label(t.start)
                .primary()
                .disabled(!row.can_start())
                .on_click(cx.listener(move |this, _, _, cx| this.on_start(id, cx))),
        )
        .child(
            Button::new(format!("stop-{id}"))
                .label(t.stop)
                .danger()
                .disabled(!row.can_stop())
                .on_click(cx.listener(move |this, _, _, cx| this.on_stop(id, cx))),
        )
        .child(
            Button::new(format!("restart-{id}"))
                .label(t.restart)
                .outline()
                .disabled(!row.can_restart())
                .on_click(cx.listener(move |this, _, _, cx| this.on_restart(id, cx))),
        )
}

fn route_label(row: &ProfileRow, t: &Text) -> String {
    match &row.proxy_name {
        Some(proxy) => t.profile_meta_proxy(&row.core_name, proxy),
        None => t.profile_meta_direct(&row.core_name),
    }
}

fn state_badge(row: &ProfileRow, p: Palette, t: &Text) -> impl IntoElement {
    let (background, foreground) = match row.state() {
        RuntimeState::Running => (p.success_bg, p.success_strong),
        RuntimeState::Starting | RuntimeState::Stopping => (p.warning_bg, p.warning),
        RuntimeState::Stopped => (p.border, p.secondary),
        RuntimeState::Failed { .. } | RuntimeState::Crashed { .. } => {
            (p.danger_bg_soft, p.danger_strong)
        }
    };

    div()
        .id(format!("state-{}", row.profile.id))
        .role(Role::Status)
        .test_support()
        .aria_label(row.state_label(t))
        .px_2()
        .py_1()
        .rounded_full()
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(row.state_label(t))
}

fn details_panel(
    selected: Option<&ProfileRow>,
    verification: Option<Verification>,
    tab: DetailsTab,
    log_tail: &[LogRow],
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Div {
    // The three views are different element types once an id makes them
    // stateful, so the panel erases them before choosing one.
    let body: AnyElement = match selected {
        None => div()
            .text_xs()
            .text_color(rgb(p.muted))
            .child(t.details_empty)
            .into_any_element(),
        Some(row) => {
            let mut grid = div()
                .id("details-grid")
                .test_support()
                .flex()
                .flex_wrap()
                .gap_x_6()
                .gap_y_2();
            let verification = verification.clone();
            for (label, value) in [
                (t.field_profile_id, row.profile.id.to_string()),
                (
                    t.field_state,
                    match row.state_message() {
                        Some(message) => t.profile_state_message(row.state_label(t), message),
                        None => row.state_label(t).to_string(),
                    },
                ),
                (t.field_seed, row.profile.fingerprint.seed.to_string()),
                ("Brand", row.profile.fingerprint.brand.to_string()),
                ("Platform", row.profile.fingerprint.platform.to_string()),
                (t.language_title, row.profile.fingerprint.language.clone()),
                ("Timezone", row.profile.fingerprint.timezone.clone()),
                (t.field_core, row.core_name.clone()),
                (
                    t.field_proxy,
                    row.proxy_name
                        .clone()
                        .unwrap_or_else(|| "direct".to_string()),
                ),
                (
                    t.field_data_dir,
                    row.profile.user_data_dir.display().to_string(),
                ),
                (t.field_browser_pid, optional(row.browser_pid())),
                (t.field_xray_pid, optional(row.xray_pid())),
                (t.field_cdp_port, optional(row.cdp_port())),
                (t.field_socks_port, optional(row.socks_port())),
                (t.field_started, elapsed(row, t)),
                (t.field_dropped_events, row.dropped_events().to_string()),
            ] {
                grid = grid.child(key_value(label, value, p));
            }

            let details = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(grid)
                .child(verification_block(verification, p, t))
                .children(row.last_warning().map(|warning| {
                    div()
                        .text_xs()
                        .text_color(rgb(p.warning))
                        .child(t.warning_line(warning))
                }))
                .children(row.last_error().map(|error| {
                    div()
                        .text_xs()
                        .text_color(rgb(p.danger_strong))
                        .child(t.error_line(error))
                }));

            // Only one of the three questions is answered at a time, so the
            // panel scrolls a view rather than the whole history of the session.
            match tab {
                DetailsTab::Details => details.into_any_element(),
                DetailsTab::Args => div()
                    .id("args-body")
                    .test_support()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(effective_args(row, p, t))
                    .into_any_element(),
                DetailsTab::Log => panel_log(log_tail, p, t).into_any_element(),
            }
        }
    };

    div()
        .flex()
        .flex_col()
        .gap_3()
        .max_h(px(320.0))
        .p_4()
        .rounded_md()
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t.runtime_details),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("copy-args")
                                .label(t.copy_args)
                                .outline()
                                .disabled(
                                    selected
                                        .map(|row| row.effective_args().is_empty())
                                        .unwrap_or(true),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.on_copy_args(cx))),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("open-dir-{}", row.profile.id))
                                    .unwrap_or_else(|| "open-dir".to_string()),
                            )
                            .label(t.open_data_dir)
                            .outline()
                            .disabled(selected.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(row) = this.state.selected() {
                                    let id = row.profile.id;
                                    this.on_open_data_dir(id, cx);
                                }
                            })),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("verify-{}", row.profile.id))
                                    .unwrap_or_else(|| "verify".to_string()),
                            )
                            .label(t.verify_fingerprint)
                            .outline()
                            .disabled(!can_verify(selected, verification.as_ref()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(row) = this.state.selected() {
                                    let id = row.profile.id;
                                    this.on_verify(id, cx);
                                }
                            })),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("edit-{}", row.profile.id))
                                    .unwrap_or_else(|| "edit".to_string()),
                            )
                            .label(t.edit)
                            .outline()
                            .disabled(selected.is_none())
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    if let Some(row) = this.state.selected() {
                                        let id = row.profile.id;
                                        this.on_edit(id, window, cx);
                                    }
                                },
                            )),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("duplicate-{}", row.profile.id))
                                    .unwrap_or_else(|| "duplicate".to_string()),
                            )
                            .label(t.duplicate)
                            .outline()
                            .disabled(selected.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(row) = this.state.selected() {
                                    let id = row.profile.id;
                                    this.on_duplicate(id, cx);
                                }
                            })),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("delete-{}", row.profile.id))
                                    .unwrap_or_else(|| "delete".to_string()),
                            )
                            .label(t.delete)
                            .outline()
                            .disabled(selected.is_none())
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    if let Some(row) = this.state.selected() {
                                        let id = row.profile.id;
                                        this.on_delete(id, window, cx);
                                    }
                                },
                            )),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .children(DetailsTab::ALL.map(|candidate| {
                    let active = candidate == tab;
                    Button::new(candidate.id())
                        .label(candidate.label(t))
                        .when(active, |button| button.primary())
                        .when(!active, |button| button.ghost())
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.on_set_details_tab(candidate, cx)
                            }),
                        )
                })),
        )
        .child(
            div()
                .id("details-scroll")
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
}

/// The tail of one profile's activity log, for the panel's Log view.
fn panel_log(rows: &[LogRow], p: Palette, t: &Text) -> impl IntoElement {
    if rows.is_empty() {
        return div()
            .id("panel-log-body")
            .test_support()
            .text_xs()
            .text_color(rgb(p.dim))
            .child(
                "Nothing logged for this profile yet. The Log page has the whole session, \
                 including window-level lines.",
            );
    }

    div()
        .id("panel-log-body")
        .test_support()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(t.panel_log_title(rows.len())),
        )
        .children(rows.iter().enumerate().map(|(index, row)| {
            div()
                .id(("panel-log", index))
                .test_support()
                .flex()
                .items_center()
                .gap_3()
                .text_xs()
                .child(
                    div()
                        .w(px(56.0))
                        .flex_shrink_0()
                        .text_color(rgb(log_level_color(row.level, p)))
                        .child(row.level.label(t)),
                )
                .child(
                    div()
                        .w(px(48.0))
                        .flex_shrink_0()
                        .text_color(rgb(p.dim))
                        .child(format_age(row.at, t)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(rgb(p.text_soft))
                        .child(row.message.clone()),
                )
        }))
}

/// Whether the selected profile can be verified right now.
///
/// A fingerprint can only be read out of a live browser that is not already
/// being read, and the browser publishes its debug port only once running.
fn can_verify(selected: Option<&ProfileRow>, verification: Option<&Verification>) -> bool {
    let Some(row) = selected else {
        return false;
    };
    row.cdp_port().is_some()
        && row.state() == RuntimeState::Running
        && !verification.is_some_and(Verification::is_running)
}

/// The verification result for one profile, or a hint that it has not run.
fn verification_block(verification: Option<Verification>, p: Palette, t: &Text) -> Div {
    let Some(verification) = verification else {
        return div().text_xs().text_color(rgb(p.dim)).child(
            "Fingerprint not verified in this session. Verification reads the \
                 running browser in its own tab and compares it with the profile.",
        );
    };
    if verification.is_running() {
        return div()
            .text_xs()
            .text_color(rgb(p.muted))
            .child(t.reading_fingerprint);
    }
    if let Some(reason) = verification.failure() {
        return div()
            .text_xs()
            .text_color(rgb(p.danger_strong))
            .child(t.fingerprint_unreadable(reason));
    }
    let found = verification.disagreements();
    let headline = if found.is_empty() {
        div()
            .text_xs()
            .text_color(rgb(p.success_strong))
            .child(t.fingerprint_confirmed)
    } else {
        div()
            .text_xs()
            .text_color(rgb(p.warning))
            .child(t.claims_not_reproduced(found.len()))
    };

    let mut block = div().flex().flex_col().gap_1().child(headline);
    // The path is the half of the answer no other panel carries, and it is the
    // reason to trust a proxy at all: an address that matches the one the proxy
    // was tested at is worth seeing without having to open the log.
    if let Some(report) = verification.report()
        && let Some(label) = report.exit_label(t)
    {
        let colour = if report.exit_ip.is_some() {
            p.info
        } else {
            p.dim
        };
        block = block.child(
            div()
                .id("exit-address")
                .test_support()
                .text_xs()
                .text_color(rgb(colour))
                .child(label),
        );
    }
    if found.is_empty() {
        return block;
    }
    // The panel scrolls, so a long list of findings stays reachable instead of
    // being clipped to the first few.
    block.children(found.iter().enumerate().map(|(index, discrepancy)| {
        div()
            .id(("disagreement", index))
            .test_support()
            .text_xs()
            .text_color(rgb(p.warning))
            .child(t.claim_line(
                discrepancy.claim,
                &discrepancy.expected,
                &discrepancy.observed,
            ))
    }))
}

/// A compact marker for the row: the user should not have to select a profile
/// to know whether its fingerprint was confirmed.
fn verification_badge(
    verification: Option<&Verification>,
    p: Palette,
    t: &Text,
) -> Option<impl IntoElement> {
    let verification = verification?;
    let (background, foreground) = match verification {
        Verification::Confirmed(_) => (p.success_bg, p.success_strong),
        Verification::Running => (p.border, p.secondary),
        Verification::Disagreements(_) => (p.warning_bg, p.warning),
        Verification::Unreadable(_) => (p.danger_bg_soft, p.danger_strong),
    };
    Some(
        div()
            .id(format!("verification-{}", verification.label(t)))
            .px_2()
            .py_1()
            .rounded_full()
            .bg(rgb(background))
            .text_color(rgb(foreground))
            .text_xs()
            .child(verification.label(t)),
    )
}

fn effective_args(row: &ProfileRow, p: Palette, t: &Text) -> Div {
    let args = row.effective_args();
    if args.is_empty() {
        return div()
            .text_xs()
            .text_color(rgb(p.dim))
            .child(t.no_launch_recorded);
    }

    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(t.effective_args(args.len())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(rgb(p.bg))
                .text_xs()
                .text_color(rgb(p.secondary))
                .children(args.iter().map(|arg| div().child(arg.clone()))),
        )
}

fn key_value(label: &str, value: String, p: Palette) -> Div {
    div()
        .flex()
        .gap_2()
        .text_xs()
        .child(
            div()
                .w(px(96.0))
                .flex_shrink_0()
                .text_color(rgb(p.muted))
                .child(label.to_string()),
        )
        .child(div().text_color(rgb(p.text_soft)).child(value))
}

fn optional<T: ToString>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".to_string())
}

fn elapsed(row: &ProfileRow, t: &Text) -> String {
    let started = row
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.started_at);
    match started.and_then(|started| SystemTime::now().duration_since(started).ok()) {
        Some(elapsed) => t.elapsed_seconds(elapsed.as_secs()),
        None => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::text::en;

    use super::AppView;
    use crate::browser_data::testing::FakeBrowserDataCopier;
    use crate::exit::ExitMode;
    use crate::open_dir::testing::FakeOpener;
    use crate::proxy_tester::testing::FakeProxyTester;
    use crate::state::AppState;
    use crate::state::Verification;
    use crate::state::testing::{FakeRuntime, core};
    use crate::theme::{Palette, ThemeChoice, palette};
    use crate::verifier::testing::FakeVerifier;
    use application::Direction;
    use application::{DefaultProfileService, DefaultProxyService, ProxyService, RuntimeService};
    use domain::{CoreId, ProfileId, ProxyId, RuntimeState};
    use gpui_kit::component::Root;
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::theme::Theme;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext, px, size};
    use runtime::{Discrepancy, Fault, FaultClass, RuntimeEvent};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use storage::{
        CoreRepository as _, MemCoreRepository, MemProfileRepository, MemProxyRepository,
    };

    /// Builds the real view over in-memory storage, a synchronous façade and a
    /// verifier the test drives.
    fn view_with_verifier(
        cx: &mut TestAppContext,
        verifier: Arc<FakeVerifier>,
    ) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
        view_full(cx, verifier, None)
    }

    /// The same view, with the settings file somewhere the test can read it.
    fn view_with_config(
        cx: &mut TestAppContext,
        config: &std::path::Path,
    ) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
        view_full(cx, Arc::new(FakeVerifier::passing()), Some(config))
    }

    fn view_full(
        cx: &mut TestAppContext,
        verifier: Arc<FakeVerifier>,
        config: Option<&std::path::Path>,
    ) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
        let (view, runtime, _) =
            view_with_opener(cx, verifier, config, Arc::new(FakeOpener::working()));
        (view, runtime)
    }

    /// The same view, with the directory opener the test drives.
    fn view_with_opener(
        cx: &mut TestAppContext,
        verifier: Arc<FakeVerifier>,
        config: Option<&std::path::Path>,
        opener: Arc<FakeOpener>,
    ) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>, Arc<FakeOpener>) {
        view_with_log(cx, verifier, config, opener, None)
    }

    /// The same view, with an activity log the test can read back.
    ///
    /// The proxy tester is a passing fake here: tests that are about something
    /// else should not have to think about it.
    fn view_with_log(
        cx: &mut TestAppContext,
        verifier: Arc<FakeVerifier>,
        config: Option<&std::path::Path>,
        opener: Arc<FakeOpener>,
        log_file: Option<crate::log_file::LogFile>,
    ) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>, Arc<FakeOpener>) {
        let (view, runtime, opener, _tester, _copier) = view_with_tester(
            cx,
            verifier,
            Arc::new(FakeProxyTester::passing()),
            Arc::new(FakeBrowserDataCopier::passing()),
            config,
            opener,
            log_file,
        );
        (view, runtime, opener)
    }

    /// Everything the harness builds: the view, and the fakes a test drives.
    ///
    /// A named type rather than a five-part tuple so a test can destructure it
    /// without repeating the shape, and so adding a fake does not change every
    /// signature that returns one.
    type Harness = (
        gpui_kit::Entity<AppView>,
        Arc<FakeRuntime>,
        Arc<FakeOpener>,
        Arc<FakeProxyTester>,
        Arc<FakeBrowserDataCopier>,
    );

    /// The same view, with the proxy tester the test drives.
    fn view_with_tester(
        cx: &mut TestAppContext,
        verifier: Arc<FakeVerifier>,
        tester: Arc<FakeProxyTester>,
        copier: Arc<FakeBrowserDataCopier>,
        config: Option<&std::path::Path>,
        opener: Arc<FakeOpener>,
        log_file: Option<crate::log_file::LogFile>,
    ) -> Harness {
        let profile_repo: Arc<MemProfileRepository> = Arc::new(MemProfileRepository::new());
        let core_repo: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let proxy_repo: Arc<MemProxyRepository> = Arc::new(MemProxyRepository::new());
        core_repo
            .save(&core(CoreId::new()))
            .expect("seed a browser core");
        // Built from the same three repositories the services get, before they
        // are moved into them: the replacement a test drives has to be visible to
        // the services, which is what "one configuration" means.
        let configuration = Arc::new(storage::MemConfiguration::new(
            core_repo.clone(),
            proxy_repo.clone(),
            profile_repo.clone(),
        ));

        let runtime = Arc::new(FakeRuntime::new());
        let profiles = Arc::new(DefaultProfileService::new(
            profile_repo.clone(),
            PathBuf::from("data"),
        ));
        let runtime_service = Arc::new(RuntimeService::new(
            profile_repo.clone(),
            core_repo.clone(),
            proxy_repo.clone(),
            runtime.clone(),
        ));
        let cores = crate::state::testing::core_service(core_repo.clone(), profile_repo.clone());
        let proxies: Arc<dyn ProxyService> =
            Arc::new(DefaultProxyService::new(proxy_repo, profile_repo));
        let settings = match config {
            Some(config) => crate::state::testing::settings_at(config),
            None => crate::state::testing::settings(),
        };
        let state = AppState::with_log(
            crate::state::Services {
                profiles,
                runtime: runtime_service,
                cores,
                proxies,
                configuration,
            },
            settings,
            log_file,
            None,
        );
        let (_command_tx, event_rx) = crossbeam_channel::bounded(16);

        let view = cx.new(|cx| {
            let mut view = AppView::new(
                state,
                event_rx,
                verifier,
                tester.clone(),
                opener.clone(),
                copier.clone(),
            );
            view.boot(cx);
            view
        });
        (view, runtime, opener, tester, copier)
    }

    fn view(cx: &mut TestAppContext) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
        view_with_verifier(cx, Arc::new(FakeVerifier::passing()))
    }

    /// A profile to work with, made without going through the form.
    ///
    /// Tests that are about something else should not have to drive the New
    /// Profile dialog for a row; creating through the window has its own test.
    fn seed_profile<C: gpui_kit::AppContext>(
        cx: &mut C,
        view: &gpui_kit::Entity<AppView>,
    ) -> ProfileId {
        view.update(cx, |view, cx| {
            let id = view
                .state_mut()
                .create_profile("Profile 1")
                .expect("seed a profile");
            cx.notify();
            id
        })
    }

    /// A profile whose name a test chose, for the filter to match on.
    fn seed_named_profile<C: gpui_kit::AppContext>(
        cx: &mut C,
        view: &gpui_kit::Entity<AppView>,
        name: &str,
    ) -> ProfileId {
        view.update(cx, |view, cx| {
            let id = view
                .state_mut()
                .create_profile(name)
                .expect("seed a profile");
            cx.notify();
            id
        })
    }

    /// Types into the Profiles page's filter, the way the field reports an edit.
    fn type_filter(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
        text: &str,
    ) {
        let field = view
            .read_with(cx, |view, _| view.filter_input())
            .expect("the first render builds the filter field");
        cx.update(|window, cx| {
            field.update(cx, |state, cx| state.set_value(text, window, cx));
        });
        settle(cx);
    }

    /// Types into the Settings page's export path field.
    fn type_export_path(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
        text: &str,
    ) {
        let field = view
            .read_with(cx, |view, _| view.export_input())
            .expect("rendering the Settings page builds the export field");
        cx.update(|window, cx| {
            field.update(cx, |state, cx| state.set_value(text, window, cx));
        });
        settle(cx);
    }

    /// The last thing the window was told, from the state rather than a toast
    /// that has already been drained.
    fn last_message(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
    ) -> String {
        view.read_with(cx, |view, _| {
            view.state()
                .toasts()
                .last()
                .map(|toast| toast.message.clone())
        })
        .expect("the window was told something")
    }

    /// A proxy to work with, made without going through the dialog.
    ///
    /// Building one through the form has its own test; a test about testing a
    /// proxy should not have to drive a dialog to get one.
    fn seed_proxy(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
        name: &str,
    ) -> ProxyId {
        view.update(cx, |view, _| {
            view.state_mut()
                .create_proxy(
                    name,
                    domain::ProxyOutbound::Socks5(domain::Socks5Outbound {
                        host: "10.0.0.1".to_string(),
                        port: 1080,
                        username: None,
                        password: None,
                    }),
                )
                .expect("seed a proxy")
        })
    }

    /// The Settings card switches what closing the window does, and the choice is
    /// written down rather than only shown.
    #[gpui_kit::test]
    fn the_exit_mode_card_switches_and_stores_the_choice(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        // A config file of this test's own: the choice is stored, and the shared
        // one would make every other test that reads it see this test's answer.
        let dir = std::env::temp_dir().join(format!("fp-ui-exit-card-{}", std::process::id()));
        let (view, _) = view_with_config(cx, &dir.join("config.json"));
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("exit-keep-running").is_some()),
            "the card offers the answers, not just the one in force"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().exit_mode()),
            ExitMode::Ask,
            "a fresh installation asks rather than deciding"
        );

        scroll_settings_to(cx, "exit-keep-running");
        cx.update(|window, cx| window.click("exit-keep-running", cx));
        settle(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.state().exit_mode()),
            ExitMode::KeepRunning,
            "the chip stores the answer it names"
        );

        scroll_settings_to(cx, "exit-exit-all");
        cx.update(|window, cx| window.click("exit-exit-all", cx));
        settle(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.state().exit_mode()),
            ExitMode::ExitAll
        );
    }

    /// "Ask" is answered with a question rather than a guess: a close request
    /// while the mode is "ask" opens the three choices and refuses the close.
    #[gpui_kit::test]
    fn closing_the_window_asks_when_the_mode_is_ask(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = std::env::temp_dir().join(format!("fp-ui-exit-ask-{}", std::process::id()));
        let (view, _) = view_with_config(cx, &dir.join("config.json"));
        let cx = window(cx, &view);
        let ask = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)))
        };

        assert!(!ask(cx), "an unanswered question must not close the window");
        assert!(
            cx.update(|window, _| window.try_find("exit-choice-exit-all").is_some()),
            "the three answers are the dialog"
        );

        // Choosing one carries it out and takes the dialog away, and nothing is
        // remembered unless the box was ticked. "Stop everything" is the answer
        // this test presses because the harness window cannot be minimized, which
        // is what "keep running" asks of it.
        cx.update(|window, cx| window.click("exit-choice-exit-all", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("exit-choice-exit-all").is_none()),
            "the answer closes the question"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().exit_mode()),
            ExitMode::Ask,
            "without the box ticked, nothing is remembered"
        );
    }

    /// The three answers are three of the same thing, and the last one is whole.
    ///
    /// Both halves were wrong before this was a test: the last row came out a text
    /// line shorter than the two above it and its own note painted over where its
    /// bottom border was. The text, the order and the line height were each ruled
    /// out by measuring, which is why the height is now given rather than derived.
    #[gpui_kit::test]
    fn the_exit_dialog_shows_three_equal_answers_inside_itself(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = std::env::temp_dir().join(format!("fp-ui-exit-equal-{}", std::process::id()));
        let (view, _) = view_with_config(cx, &dir.join("config.json"));
        let cx = window(cx, &view);

        let _ =
            cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
        settle(cx);

        let bounds = |id: &str, cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, _| window.try_find(id.to_string()).expect(id).bounds())
        };
        let surface = cx.debug_bounds("dialog-0").expect("the dialog's surface");
        let rows = [
            "exit-choice-background",
            "exit-choice-keep-running",
            "exit-choice-exit-all",
        ]
        .map(|id| bounds(id, cx));

        assert_eq!(rows[0].size.height, rows[1].size.height, "{rows:?}");
        assert_eq!(rows[1].size.height, rows[2].size.height, "{rows:?}");
        for (index, row) in rows.iter().enumerate() {
            assert!(
                row.bottom() <= surface.bottom(),
                "answer {index} is outside the dialog: {row:?} in {surface:?}"
            );
            if let Some(next) = rows.get(index + 1) {
                assert!(
                    row.bottom() <= next.origin.y,
                    "answer {index} overlaps the next one: {rows:?}"
                );
            }
        }
    }

    /// A remembered mode is an answer, so no question is asked: the close is
    /// allowed (or, for "keep running", refused) without a dialog.
    #[gpui_kit::test]
    fn a_remembered_exit_mode_is_carried_out_without_asking(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = std::env::temp_dir().join(format!("fp-ui-exit-remember-{}", std::process::id()));
        let (view, runtime) = view_with_config(cx, &dir.join("config.json"));
        let cx = window(cx, &view);
        let commands = || runtime.commands.lock().expect("commands").clone();

        // "Stop everything" says nothing to the runtime: what this program has
        // always done at exit is stop its children, and `main` does that after
        // the window loop returns.
        view.update(cx, |view, _| {
            view.state_mut()
                .set_exit_mode(ExitMode::ExitAll)
                .expect("store the answer")
        });
        let allowed =
            cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
        assert!(allowed, "a remembered answer closes the window");
        assert!(
            cx.update(|window, _| window.try_find("exit-remember").is_none()),
            "and it was not asked about"
        );
        assert!(
            !commands().iter().any(|command| command == "release-all"),
            "stopping everything does not release anything: {:?}",
            commands()
        );

        // "Leave browsers running" does: it is the one answer the runtime has to
        // be told about, and the program is about to end.
        view.update(cx, |view, _| {
            view.state_mut()
                .set_exit_mode(ExitMode::KeepRunning)
                .expect("store the answer")
        });
        let allowed =
            cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
        assert!(allowed);
        assert!(
            commands().iter().any(|command| command == "release-all"),
            "leaving with the browsers running must tell the runtime: {:?}",
            commands()
        );

        // "Background" keeps running, refuses the close, hides the window and starts the tray icon.
        view.update(cx, |view, _| {
            view.state_mut()
                .set_exit_mode(ExitMode::Background)
                .expect("store the answer")
        });
        let allowed =
            cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
        assert!(
            !allowed,
            "background mode refuses the close so app keeps running"
        );
        view.update(cx, |view, _| {
            assert!(
                view.tray.is_some(),
                "entering background starts the tray icon"
            );
        });

        // TrayEvent::Show wakes/restores the window.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_tray(crate::tray::TrayEvent::Show, window, cx);
            });
        });
    }

    #[gpui_kit::test]
    fn profiles_can_be_created_started_and_stopped_from_the_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, runtime) = view(cx);
        let cx = window(cx, &view);

        assert!(
            cx.update(|window, _| window.try_find("quit").is_some()),
            "the header renders a quit affordance"
        );
        assert_eq!(view.read_with(cx, |view, _| view.state().rows().len()), 0);

        let label = |cx: &mut gpui_kit::VisualTestContext, id: ProfileId| {
            cx.update(|window, _| {
                window
                    .find(format!("state-{id}"))
                    .label()
                    .map(|label| label.to_string())
            })
        };

        // New Profile opens the form: the row appears when the form is accepted,
        // not when the button is clicked.
        cx.update(|window, cx| window.click("new-profile", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("editor-name").is_some()),
            "the New Profile button opens the form"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().rows().len()),
            0,
            "nothing is written while the form is still open"
        );
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        cx.update(|window, cx| window.click("new-profile", cx));
        settle(cx);
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        let ids = view.read_with(cx, |view, _| {
            view.state()
                .rows()
                .iter()
                .map(|row| row.profile.id)
                .collect::<Vec<_>>()
        });
        assert_eq!(ids.len(), 2, "two profiles after two accepted forms");

        let (first, second) = (ids[0], ids[1]);
        assert_eq!(label(cx, first).as_deref(), Some("Stopped"));
        assert_eq!(label(cx, second).as_deref(), Some("Stopped"));

        cx.update(|window, cx| window.click(format!("start-{first}"), cx));
        settle(cx);
        assert_eq!(label(cx, first).as_deref(), Some("Running"));
        assert_eq!(
            label(cx, second).as_deref(),
            Some("Stopped"),
            "starting one profile leaves the other alone"
        );

        cx.update(|window, cx| window.click(format!("stop-{first}"), cx));
        settle(cx);
        assert_eq!(label(cx, first).as_deref(), Some("Stopped"));

        let commands = runtime.commands.lock().expect("command log").clone();
        assert_eq!(
            commands.len(),
            2,
            "one start and one stop reached the façade"
        );
        assert!(commands[0].starts_with("start:"));
        assert!(commands[1].starts_with("stop:"));
    }

    /// A new profile is the one that was configured, on the core that was picked.
    #[gpui_kit::test]
    fn a_new_profile_is_created_from_the_form_with_the_core_it_was_given(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);
        let seeded_core = view.read_with(cx, |view, _| {
            view.state()
                .core_rows()
                .expect("rows")
                .first()
                .expect("the fixture seeds a core")
                .core
                .id
        });

        cx.update(|window, cx| window.click("new-profile", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("editor-core-0").is_some()),
            "the form offers the core the profile will run on"
        );

        let editor = view.read_with(cx, |view, _| view.editor()).expect("a form");
        let name = editor.read_with(cx, |editor, _| editor.name_input());
        cx.update(|window, cx| {
            name.update(cx, |state, cx| state.set_value("Shop account", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        let row = view.read_with(cx, |view, _| view.state().rows()[0].clone());
        assert_eq!(row.profile.name, "Shop account");
        assert_eq!(
            row.profile.core_id, seeded_core,
            "the profile runs on the core the form was opened with"
        );
        assert!(
            cx.update(|window, _| window.try_find("editor-name").is_none()),
            "accepting the form closes it"
        );
    }

    #[gpui_kit::test]
    fn a_long_compatibility_warning_stays_inside_the_row(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, runtime) = view(cx);

        let handle = cx.open_window(size(px(1200.), px(800.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let id = seed_profile(cx, &view);

            // The longest warning the compatibility layer produces.
            runtime.set_warning(
                id,
                "chrome 128 (major 128) is older than the verified fingerprint generation \
                 (major 144) and omits --fingerprinting-canvas-image-data-noise,\
                 --fingerprinting-client-rects-noise; requested by profile canvas and \
                 client-rects spoofing",
            );
            view.update(cx, |view, cx| {
                view.state_mut().refresh_runtime();
                cx.notify();
            });
            window.render_frame(cx);

            let warning = window.find(format!("warning-{id}"));
            assert!(warning.visible(), "the row reports the warning");
            assert!(
                warning.bounds().size.width <= px(420.0),
                "the row warning is capped instead of overflowing: {:?}",
                warning.bounds().size
            );
        })
        .unwrap();
    }

    /// One request through a proxy, and the address it left from on the row.
    ///
    /// This is the whole capability: without it the window only ever knows that
    /// the engine's local port is open, and a proxy that accepts the connection
    /// and carries nothing is indistinguishable from a working one.
    #[gpui_kit::test]
    fn testing_a_proxy_from_the_window_reports_where_the_traffic_left(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime, _opener, tester, _copier) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            Arc::new(FakeProxyTester::passing_from("198.51.100.9")),
            Arc::new(FakeBrowserDataCopier::passing()),
            None,
            Arc::new(FakeOpener::working()),
            None,
        );
        let cx = window(cx, &view);
        let proxy_id = seed_proxy(cx, &view, "Office");

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("test-proxy-0", cx));
        settle(cx);

        wait_for_state(cx, &view, |state| {
            state
                .proxy_test(proxy_id)
                .is_some_and(|test| !test.is_running())
        });

        let test = view
            .read_with(cx, |view, _| view.state().proxy_test(proxy_id).cloned())
            .expect("a result");
        let reading = test.reading().expect("a reading, not a fault");
        assert_eq!(reading.exit_ip, "198.51.100.9");
        assert!(
            !reading.live,
            "nothing was running, so the test started an engine of its own"
        );
        assert_eq!(tester.calls(), 1);
        assert_eq!(
            tester.ports_asked(),
            vec![None],
            "nothing was running, so no port was handed to the tester"
        );

        // And the row says what the request found, not just that it ran.
        let row = cx.update(|window, _| window.find(format!("proxy-test-{proxy_id}")));
        assert!(row.visible(), "the row reports the reading");
    }

    /// A proxy that carries nothing must not read as a working one, and the row
    /// must name the class rather than only that something failed.
    #[gpui_kit::test]
    fn a_proxy_that_carries_nothing_says_so_on_the_row(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime, _opener, tester, _copier) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            Arc::new(FakeProxyTester::with_outcome(Err(Fault::new(
                FaultClass::Unreachable,
                "no route to the upstream",
            )))),
            Arc::new(FakeBrowserDataCopier::passing()),
            None,
            Arc::new(FakeOpener::working()),
            None,
        );
        let cx = window(cx, &view);
        let proxy_id = seed_proxy(cx, &view, "Office");

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("test-proxy-0", cx));
        settle(cx);

        wait_for_state(cx, &view, |state| {
            state
                .proxy_test(proxy_id)
                .is_some_and(|test| !test.is_running())
        });

        let test = view
            .read_with(cx, |view, _| view.state().proxy_test(proxy_id).cloned())
            .expect("a result");
        assert!(
            test.reading().is_none(),
            "nothing left, so nothing may be reported as a reading"
        );
        assert_eq!(
            test.fault().expect("the fault").class,
            FaultClass::Unreachable
        );
        assert_eq!(test.label(en()), "no traffic (unreachable)");
        assert_eq!(tester.calls(), 1);
        cx.update(|window, _| {
            assert!(
                window.find(format!("proxy-test-{proxy_id}")).visible(),
                "the row reports the failure"
            )
        });
    }

    /// A proxy still being tested cannot be tested again: that would start a
    /// second engine for an answer already on its way.
    #[gpui_kit::test]
    fn a_proxy_already_being_tested_is_not_tested_twice(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime, _opener, tester, _copier) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            Arc::new(FakeProxyTester::passing()),
            Arc::new(FakeBrowserDataCopier::passing()),
            None,
            Arc::new(FakeOpener::working()),
            None,
        );
        let cx = window(cx, &view);
        let proxy_id = seed_proxy(cx, &view, "Office");

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("test-proxy-0", cx));
        settle(cx);
        // The first click's worker has not reported yet, so the slot is taken.
        cx.update(|window, cx| window.click("test-proxy-0", cx));
        settle(cx);

        wait_for_state(cx, &view, |state| {
            state
                .proxy_test(proxy_id)
                .is_some_and(|test| !test.is_running())
        });
        assert_eq!(tester.calls(), 1, "the second click was refused");
    }

    /// Drives the verification channel until the worker reports, so the test
    /// never depends on the tick timer firing.
    fn wait_for_verification<C: gpui_kit::AppContext>(
        cx: &mut C,
        view: &gpui_kit::Entity<AppView>,
    ) {
        wait_for_state(cx, view, |state| {
            state
                .selected()
                .and_then(|row| state.verification(row.profile.id))
                .is_some_and(|verification| !verification.is_running())
        });
    }

    /// Drives every background queue until the state says what the test is
    /// waiting for. A background action reports on a later tick, and a test
    /// should not have to wait for the timer to fire to see it.
    fn wait_for_state<C: gpui_kit::AppContext>(
        cx: &mut C,
        view: &gpui_kit::Entity<AppView>,
        done: impl Fn(&AppState) -> bool,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if view.read_with(cx, |view, _| done(view.state())) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the background action never reported back"
            );
            view.update(cx, |view, cx| {
                view.drain_verifications();
                view.drain_proxy_tests();
                view.drain_open_results();
                view.drain_browser_data();
                cx.notify();
            });
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[gpui_kit::test]
    fn a_confirmed_fingerprint_is_reported_in_the_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let verifier = Arc::new(FakeVerifier::passing());
        let (view, runtime) = view_with_verifier(cx, verifier.clone());

        let handle = cx.open_window(size(px(1200.), px(900.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let id = seed_profile(cx, &view);

            // A stopped profile has no browser to read.
            assert!(
                view.read_with(cx, |view, _| view.state().verification(id).is_none()),
                "nothing is verified before it is asked for"
            );
            window.click(format!("start-{id}"), cx);
            runtime.set_cdp_port(id, 9333);
            view.update(cx, |view, cx| {
                view.state_mut().refresh_runtime();
                cx.notify();
            });
            window.render_frame(cx);

            window.click(format!("verify-{id}"), cx);
            window.render_frame(cx);

            wait_for_verification(cx, &view);
            window.render_frame(cx);

            let verification = view
                .read_with(cx, |view, _| view.state().verification(id).cloned())
                .expect("a result is recorded");
            assert!(matches!(verification, Verification::Confirmed(_)));
            assert_eq!(verifier.calls(), 1);
        })
        .unwrap();
    }

    /// A running profile leaving by a proxy that has been tested.
    ///
    /// The whole path a proxied profile takes to being verified: a proxy, its
    /// pre-flight, the profile pointed at it, and the browser up with a debug
    /// port. Tests about the address question should not each re-derive it.
    fn started_proxied_profile(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
        runtime: &Arc<FakeRuntime>,
    ) -> ProfileId {
        let id = seed_profile(cx, view);
        let proxy_id = seed_proxy(cx, view, "Office");
        view.update(cx, |view, cx| {
            let state = view.state_mut();
            let mut profile = state.profile(id).expect("the profile");
            profile.proxy_id = Some(proxy_id);
            state.update_profile(profile).expect("assign");
            cx.notify();
        });

        // Pressing Start on a proxied profile asks the proxy first now, so the
        // reading this profile is judged against is the one the start's own check
        // took - the harness's tester, which leaves from `FAKE_EXIT_IP` - and the
        // command is only queued once that answer is in.
        cx.update(|window, cx| window.click(format!("start-{id}"), cx));
        wait_for_state(cx, view, |state| {
            state
                .row(id)
                .is_some_and(|row| row.state() == RuntimeState::Running)
        });
        runtime.set_cdp_port(id, 9333);
        view.update(cx, |view, cx| {
            view.state_mut().refresh_runtime();
            cx.notify();
        });
        settle(cx);
        id
    }

    /// Verifying a proxied profile also asks where its traffic leaves from. The
    /// expectation is the address the proxy was measured at, because that is the
    /// only place in the product that knows where the traffic should have gone.
    #[gpui_kit::test]
    fn verifying_a_proxied_profile_asks_the_endpoint_its_proxy_was_tested_at(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let verifier = Arc::new(FakeVerifier::passing_from("203.0.113.7"));
        let (view, runtime) = view_with_verifier(cx, verifier.clone());
        let cx = window(cx, &view);
        let id = started_proxied_profile(cx, &view, &runtime);

        cx.update(|window, cx| window.click(format!("verify-{id}"), cx));
        wait_for_verification(cx, &view);

        let asked = verifier.egrees_asked();
        assert_eq!(asked.len(), 1, "the address question is asked once");
        let egress = asked[0]
            .clone()
            .expect("a proxied profile is asked about its address");
        assert_eq!(
            egress.expected.as_deref(),
            Some(crate::proxy_tester::testing::FAKE_EXIT_IP),
            "the expectation is what the proxy was measured at, not the reading"
        );
        assert!(
            egress.echo_url.starts_with("http://"),
            "the same endpoint the pre-flight used: {}",
            egress.echo_url
        );
    }

    /// Where the traffic went is the half of the answer that says the path is
    /// the intended one, so it is shown rather than left in the log.
    #[gpui_kit::test]
    fn the_address_a_verified_profile_left_from_is_shown_on_the_profile(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, runtime) =
            view_with_verifier(cx, Arc::new(FakeVerifier::passing_from("203.0.113.7")));
        let cx = window(cx, &view);
        let id = started_proxied_profile(cx, &view, &runtime);

        cx.update(|window, cx| window.click(format!("verify-{id}"), cx));
        wait_for_verification(cx, &view);
        settle(cx);

        let reading = cx.update(|window, _| window.find("exit-address"));
        assert!(
            reading.visible(),
            "the address the traffic left from belongs on the profile"
        );
    }

    #[gpui_kit::test]
    fn disagreements_are_listed_claim_by_claim(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let verifier = Arc::new(FakeVerifier::disagreeing(vec![Discrepancy {
            claim: "platform",
            expected: "Win32".to_string(),
            observed: "Linux x86_64".to_string(),
        }]));
        let (view, runtime) = view_with_verifier(cx, verifier);

        let handle = cx.open_window(size(px(1200.), px(900.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let id = seed_profile(cx, &view);
            window.click(format!("start-{id}"), cx);
            runtime.set_cdp_port(id, 9333);
            view.update(cx, |view, cx| {
                view.state_mut().refresh_runtime();
                cx.notify();
            });
            window.render_frame(cx);

            window.click(format!("verify-{id}"), cx);

            wait_for_verification(cx, &view);
            window.render_frame(cx);

            let verification = view
                .read_with(cx, |view, _| view.state().verification(id).cloned())
                .expect("a result is recorded");
            assert_eq!(verification.label(en()), "1 claim not confirmed");
            assert_eq!(verification.disagreements()[0].observed, "Linux x86_64");
            assert!(
                !view.read_with(cx, |view, _| view
                    .state()
                    .verification(id)
                    .is_some_and(Verification::is_running)),
                "the result replacement is not a spinner left behind"
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn every_disagreement_is_reachable_from_a_short_panel(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let found: Vec<Discrepancy> = (0usize..7)
            .map(|index| Discrepancy {
                claim: "platform",
                expected: format!("expected-{index}"),
                observed: format!("observed-{index}"),
            })
            .collect();
        let (view, runtime) = view_with_verifier(cx, Arc::new(FakeVerifier::disagreeing(found)));

        let handle = cx.open_window(size(px(1200.), px(700.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let id = seed_profile(cx, &view);
            window.click(format!("start-{id}"), cx);
            runtime.set_cdp_port(id, 9333);
            view.update(cx, |view, cx| {
                view.state_mut().refresh_runtime();
                cx.notify();
            });
            window.render_frame(cx);

            window.click(format!("verify-{id}"), cx);
            wait_for_verification(cx, &view);
            window.render_frame(cx);

            // A panel that only ever showed the first few claims would hide
            // the rest of the answer. The list is longer than the panel, so the
            // later claims become visible by scrolling, not by being dropped.
            assert!(
                window.find(("disagreement", 0usize)).visible(),
                "the first claim is rendered"
            );
            let recorded = view.read_with(cx, |view, _| {
                view.state()
                    .verification(id)
                    .map(|verification| verification.disagreements().len())
            });
            assert_eq!(recorded, Some(7), "no claim is dropped before rendering");
            window.scroll(
                "details-scroll",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-400.0))),
                cx,
            );
            window.render_frame(cx);
            assert!(
                window.find(("disagreement", 6usize)).visible(),
                "the last claim is reachable by scrolling"
            );
        })
        .unwrap();
    }

    /// Opens the window the way the component library's own dialog tests do:
    /// a `VisualTestContext` can park the async work a dialog needs to mount.
    fn window<'a>(
        cx: &'a mut TestAppContext,
        view: &gpui_kit::Entity<AppView>,
    ) -> &'a mut gpui_kit::VisualTestContext {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (_, cx) = cx.add_window_view({
            let view = view.clone();
            move |window, cx| Root::new(view, window, cx)
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx
    }

    #[gpui_kit::test]
    fn the_sidebar_switches_to_the_proxies_page(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        assert!(
            cx.update(|window, _| window.try_find("proxies-scroll").is_none()),
            "the proxies page is not shown first"
        );

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("new-proxy").is_some()),
            "the proxies page is shown"
        );
        assert!(
            cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
            "the profiles body is not rendered on the proxies page"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().page()),
            crate::state::Page::Proxies
        );
    }

    #[gpui_kit::test]
    fn a_proxy_can_be_created_from_the_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("new-proxy", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("proxy-name").is_some()),
            "the proxy form opens"
        );

        let editor = view
            .read_with(cx, |view, _| view.proxy_editor())
            .expect("a proxy editor");
        let (name, host) =
            editor.read_with(cx, |editor, _| (editor.name_input(), editor.host_input()));
        cx.update(|window, cx| {
            name.update(cx, |state, cx| state.set_value("Office", window, cx));
            host.update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        let rows = view.read_with(cx, |view, cx| {
            let _ = cx;
            view.state().proxy_rows().expect("rows")
        });
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].proxy.name, "Office");
        assert_eq!(rows[0].endpoint(), "socks5://10.0.0.1:1080");
        assert!(
            cx.update(|window, _| window.try_find("proxy-name").is_none()),
            "saving closes the dialog"
        );
    }

    /// A real link of the shape a provider hands out, remark and all.
    const SHARE_LINK: &str = "vless://b831381d-6324-4d53-ad4f-8cda48b30811@node.example:443\
        ?encryption=none&security=reality&sni=front.example&fp=chrome\
        &pbk=LOLTG162EtSegCnMAofVY3oKrbrCvH8zOTZEPd1GRQU&sid=ab12#Tokyo";

    fn set_link(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
        link: &str,
    ) {
        let import = view
            .read_with(cx, |view, _| view.proxy_import())
            .expect("an importer");
        let field = import.read_with(cx, |import, _| import.link_input());
        cx.update(|window, cx| field.update(cx, |state, cx| state.set_value(link, window, cx)));
    }

    fn import_error(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
    ) -> Option<String> {
        view.read_with(cx, |view, _| view.proxy_import())
            .expect("an importer")
            .read_with(cx, |import, _| import.error().map(str::to_string))
    }

    #[gpui_kit::test]
    fn a_pasted_link_becomes_a_proxy(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("import-proxy", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("proxy-link").is_some()),
            "the import dialog opens"
        );

        set_link(cx, &view, SHARE_LINK);
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        let rows = view.read_with(cx, |view, cx| {
            let _ = cx;
            view.state().proxy_rows().expect("rows")
        });
        assert_eq!(rows.len(), 1);
        // The remark in the link is the name, and the endpoint is what the link
        // said rather than what a form would have rebuilt.
        assert_eq!(rows[0].proxy.name, "Tokyo");
        assert_eq!(rows[0].endpoint(), "vless://node.example:443");
        assert!(
            cx.update(|window, _| window.try_find("proxy-link").is_none()),
            "importing closes the dialog"
        );
    }

    #[gpui_kit::test]
    fn a_link_this_program_cannot_read_says_so_and_creates_nothing(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("import-proxy", cx));
        settle(cx);

        set_link(cx, &view, "hysteria2://secret@node.example:443");
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        let error = import_error(cx, &view).expect("a refusal");
        assert!(error.contains("hysteria2"), "{error}");
        assert!(
            cx.update(|window, _| window.try_find("proxy-import-error").is_some()),
            "the dialog shows the reason"
        );
        assert!(
            cx.update(|window, _| window.try_find("proxy-link").is_some()),
            "the dialog stays open so the link can be fixed"
        );
        let rows = view.read_with(cx, |view, cx| {
            let _ = cx;
            view.state().proxy_rows().expect("rows")
        });
        assert!(rows.is_empty(), "a refused link creates nothing");
    }

    #[gpui_kit::test]
    fn a_refused_proxy_form_says_why_and_stays_open(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("new-proxy", cx));
        settle(cx);

        let editor = view
            .read_with(cx, |view, _| view.proxy_editor())
            .expect("a proxy editor");
        let name = editor.read_with(cx, |editor, _| editor.name_input());
        cx.update(|window, cx| {
            name.update(cx, |state, cx| state.set_value("No host", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("proxy-error").is_some()),
            "the form says what is wrong"
        );
        let shown = editor
            .read_with(cx, |editor, _| editor.error().map(str::to_string))
            .expect("the reason is recorded on the form");
        assert!(shown.contains("host"), "{shown}");
        assert!(
            cx.update(|window, _| window.try_find("proxy-name").is_some()),
            "a refused save leaves the form open"
        );
        assert!(
            view.read_with(cx, |view, cx| {
                let _ = cx;
                view.state().proxy_rows().expect("rows").is_empty()
            }),
            "nothing was written"
        );
    }

    #[gpui_kit::test]
    fn assigning_a_proxy_from_the_profile_editor_reaches_storage(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        // A proxy to choose, made through its own page.
        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("new-proxy", cx));
        settle(cx);
        let proxy_editor = view
            .read_with(cx, |view, _| view.proxy_editor())
            .expect("a proxy editor");
        let (name, host) =
            proxy_editor.read_with(cx, |editor, _| (editor.name_input(), editor.host_input()));
        cx.update(|window, cx| {
            name.update(cx, |state, cx| state.set_value("Office", window, cx));
            host.update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);
        let proxy_id = view.read_with(cx, |view, cx| {
            let _ = cx;
            view.state().proxy_rows().expect("rows")[0].proxy.id
        });

        // A profile to assign it to.
        cx.update(|window, cx| window.click("nav-Profiles", cx));
        settle(cx);
        let id = seed_profile(cx, &view);
        cx.update(|window, cx| window.click(format!("edit-{id}"), cx));
        settle(cx);

        cx.update(|window, cx| window.click("editor-proxy-0", cx));
        settle(cx);
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        assert_eq!(
            view.read_with(cx, |view, _| view
                .state()
                .profile(id)
                .expect("the profile is loaded")
                .proxy_id),
            Some(proxy_id),
            "the assignment reached the state"
        );
    }

    #[gpui_kit::test]
    fn deleting_a_proxy_in_use_is_refused_in_the_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("new-proxy", cx));
        settle(cx);
        let proxy_editor = view
            .read_with(cx, |view, _| view.proxy_editor())
            .expect("a proxy editor");
        let (name, host) =
            proxy_editor.read_with(cx, |editor, _| (editor.name_input(), editor.host_input()));
        cx.update(|window, cx| {
            name.update(cx, |state, cx| state.set_value("Office", window, cx));
            host.update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        cx.update(|window, cx| window.click("nav-Profiles", cx));
        settle(cx);
        let id = seed_profile(cx, &view);
        cx.update(|window, cx| window.click(format!("edit-{id}"), cx));
        settle(cx);
        cx.update(|window, cx| window.click("editor-proxy-0", cx));
        settle(cx);
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("delete-proxy-0", cx));
        settle(cx);
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        assert_eq!(
            view.read_with(cx, |view, cx| {
                let _ = cx;
                view.state().proxy_rows().expect("rows").len()
            }),
            1,
            "the proxy is still there"
        );
        let notice = view
            .read_with(cx, |view, _| view.state().notice().cloned())
            .expect("the refusal is shown");
        assert!(notice.error, "the refusal is an error");
        assert!(
            notice.message.contains("Office") && notice.message.contains("Profile 1"),
            "the message names the proxy and who holds it: {}",
            notice.message
        );
    }

    #[gpui_kit::test]
    fn the_sidebar_switches_to_the_cores_page(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        assert!(
            cx.update(|window, _| window.try_find("cores-scroll").is_none()),
            "the cores page is not shown first"
        );

        cx.update(|window, cx| window.click("nav-Cores", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("new-core").is_some()),
            "the cores page is shown"
        );
        // Only one page is rendered at a time: the profiles list and its
        // details panel used to follow the cores page onto the screen.
        assert!(
            cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
            "the profiles body is not rendered on the cores page"
        );
        assert!(
            cx.update(|window, _| window.try_find("new-profile").is_none()),
            "the profiles header is not rendered on the cores page"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().page()),
            crate::state::Page::Cores
        );
    }

    #[gpui_kit::test]
    fn a_core_can_be_added_from_the_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);
        let binary =
            crate::state::testing::CoreBinary::new("ui-add", Some("Chromium 148.0.7778.215"));
        let path = binary.path_buf();

        cx.update(|window, cx| window.click("nav-Cores", cx));
        settle(cx);
        cx.update(|window, cx| window.click("new-core", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("core-executable").is_some()),
            "the core form opens"
        );

        let editor = view
            .read_with(cx, |view, _| view.core_editor())
            .expect("a core editor");
        let executable = editor.read_with(cx, |editor, _| editor.executable_input());
        cx.update(|window, cx| {
            executable.update(cx, |state, cx| {
                state.set_value(path.to_string_lossy().to_string(), window, cx)
            });
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        let rows = view.read_with(cx, |view, _| view.state().core_rows().expect("rows"));
        assert_eq!(rows.len(), 2, "the seeded core plus the added one");
        let added = rows
            .iter()
            .find(|row| row.core.executable == path)
            .expect("the added core is listed");
        assert_eq!(added.core.major, 148);
        assert_eq!(added.core.version, "Chromium 148.0.7778.215");
        assert!(
            cx.update(|window, _| window.try_find("core-executable").is_none()),
            "saving closes the dialog"
        );
    }

    /// The walk a fresh installation asks for, in one test.
    ///
    /// Every step of it is covered on its own elsewhere; what this pins is the
    /// handover between them - that the button the empty state offers lands on
    /// the page where a core can be added, that adding one stops the list asking
    /// for it and lets a profile be created, and that the profile created that
    /// way really starts. First-run guidance is a path, and a path is what a
    /// per-step test cannot check.
    #[gpui_kit::test]
    fn a_fresh_installation_can_be_walked_from_no_core_to_a_running_profile(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, runtime) = view(cx);
        let cx = window(cx, &view);
        let binary =
            crate::state::testing::CoreBinary::new("ui-first-run", Some("Chromium 148.0.7778.215"));

        // A first start: no cores at all, which the fixture does not build.
        view.update(cx, |view, _| {
            let seeded = view.state().core_rows().expect("rows")[0].core.id;
            view.state_mut().delete_core(seeded).expect("remove it");
        });
        settle(cx);

        // 1. The list says what is missing and offers the way to fix it. The
        //    sentence has to name both routes, because the button is only one.
        let hint = cx.update(|window, _| {
            window
                .find("empty-hint")
                .label()
                .map(|label| label.to_string())
        });
        let hint = hint.expect("an empty state with no core says so");
        assert!(hint.contains("FP_BROWSER_CHROMIUM_BIN"), "{hint}");
        assert!(hint.contains("Browser Cores"), "{hint}");
        assert_eq!(
            view.read_with(cx, |view, _| view.state().rows().len()),
            0,
            "there is nothing to list yet"
        );

        // 2. The button the empty state offers goes where a core is added.
        cx.update(|window, cx| window.click("empty-add-core", cx));
        settle(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.state().page()),
            crate::state::Page::Cores
        );

        // 3. A core, through its own form, ending with a detected version.
        cx.update(|window, cx| window.click("new-core", cx));
        settle(cx);
        let editor = view
            .read_with(cx, |view, _| view.core_editor())
            .expect("the Add Core button opens the form");
        let executable = editor.read_with(cx, |editor, _| editor.executable_input());
        cx.update(|window, cx| {
            executable.update(cx, |state, cx| {
                state.set_value(binary.path_buf().to_string_lossy().to_string(), window, cx)
            });
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);
        let rows = view.read_with(cx, |view, _| view.state().core_rows().expect("rows"));
        assert_eq!(rows.len(), 1, "the installation has a core now");
        assert_eq!(rows[0].core.major, 148);

        // 4. Back on the list, the same hint now says something else, and the
        //    button that could not work before is offered.
        cx.update(|window, cx| window.click("nav-Profiles", cx));
        settle(cx);
        let hint = cx
            .update(|window, _| {
                window
                    .find("empty-hint")
                    .label()
                    .map(|label| label.to_string())
            })
            .expect("the list is still empty, and now for the other reason");
        assert!(!hint.contains("FP_BROWSER_CHROMIUM_BIN"), "{hint}");

        // 5. And a profile can be created with the core that was just added.
        cx.update(|window, cx| window.click("new-profile", cx));
        settle(cx);
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);
        let ids: Vec<ProfileId> = view.read_with(cx, |view, _| {
            view.state()
                .rows()
                .iter()
                .map(|row| row.profile.id)
                .collect()
        });
        assert_eq!(ids.len(), 1, "the form was accepted");
        let id = ids[0];
        assert_eq!(
            view.read_with(cx, |view, _| view.state().profile(id).map(|p| p.core_id)),
            Some(rows[0].core.id),
            "the profile was created with the core the walk added"
        );

        // 6. And it starts, which is the end of the path this test is about.
        cx.update(|window, cx| window.click(format!("start-{id}"), cx));
        settle(cx);
        // `Running` rather than `Starting`: the fake façade applies a launch as
        // soon as it is asked, where the real one would publish `Starting` and
        // the browser later. What this asserts is that the click reached it.
        assert_eq!(
            view.read_with(cx, |view, _| view
                .state()
                .rows()
                .first()
                .and_then(|row| row.snapshot.as_ref())
                .map(|snapshot| snapshot.state.clone())),
            Some(domain::RuntimeState::Running),
            "the launch reached the runtime"
        );
        let commands = runtime.commands.lock().expect("command log").clone();
        assert_eq!(
            commands.len(),
            1,
            "one launch was asked for, through the supervisor channel"
        );
        assert!(commands[0].starts_with("start:"), "{commands:?}");
    }

    #[gpui_kit::test]
    fn a_refused_core_form_says_why_and_stays_open(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);
        // A file that answers nothing when asked for --version.
        let binary = crate::state::testing::CoreBinary::new("ui-silent", None);
        let path = binary.path_buf();

        cx.update(|window, cx| window.click("nav-Cores", cx));
        settle(cx);
        let before = view.read_with(cx, |view, _| view.state().core_rows().expect("rows").len());
        cx.update(|window, cx| window.click("new-core", cx));
        settle(cx);

        let editor = view
            .read_with(cx, |view, _| view.core_editor())
            .expect("a core editor");
        let executable = editor.read_with(cx, |editor, _| editor.executable_input());
        cx.update(|window, cx| {
            executable.update(cx, |state, cx| {
                state.set_value(path.to_string_lossy().to_string(), window, cx)
            });
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("core-executable").is_some()),
            "a refused save leaves the form open"
        );
        assert!(
            cx.update(|window, _| window.try_find("core-error").is_some()),
            "the form says what is wrong"
        );
        let shown = editor
            .read_with(cx, |editor, _| editor.error().map(str::to_string))
            .expect("the reason is recorded on the form");
        assert!(shown.contains("--version"), "{shown}");
        assert!(
            shown.contains("FP_BROWSER_CHROMIUM_MAJOR"),
            "the refusal says how to record it anyway: {shown}"
        );
        assert!(
            shown.split("; ").count() == 2,
            "the message is written as two clauses, one line each: {shown}"
        );
        let notice = view
            .read_with(cx, |view, _| view.state().notice().cloned())
            .expect("the refusal is shown");
        assert!(notice.error, "the refusal is an error");
        assert!(notice.message.contains("--version"), "{}", notice.message);
        assert_eq!(
            view.read_with(cx, |view, _| view.state().core_rows().expect("rows").len()),
            before,
            "nothing was stored"
        );
    }

    #[gpui_kit::test]
    fn deleting_a_core_in_use_is_refused_in_the_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        // The fixture seeds one core; a profile is created against it.
        seed_profile(cx, &view);
        settle(cx);
        cx.update(|window, cx| window.click("nav-Cores", cx));
        settle(cx);
        cx.update(|window, cx| window.click("delete-core-0", cx));
        settle(cx);
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        assert_eq!(
            view.read_with(cx, |view, _| view.state().core_rows().expect("rows").len()),
            1,
            "the core is still there"
        );
        let notice = view
            .read_with(cx, |view, _| view.state().notice().cloned())
            .expect("the refusal is shown");
        assert!(notice.error);
        assert!(
            notice.message.contains("Profile 1"),
            "the message names the profile holding it: {}",
            notice.message
        );
    }

    #[gpui_kit::test]
    fn re_detecting_from_the_window_reports_what_it_found(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);
        let binary =
            crate::state::testing::CoreBinary::new("ui-redetect", Some("Chromium 148.0.7778.215"));
        let path = binary.path_buf();

        cx.update(|window, cx| window.click("nav-Cores", cx));
        settle(cx);
        cx.update(|window, cx| window.click("new-core", cx));
        settle(cx);
        let editor = view
            .read_with(cx, |view, _| view.core_editor())
            .expect("a core editor");
        let executable = editor.read_with(cx, |editor, _| editor.executable_input());
        cx.update(|window, cx| {
            executable.update(cx, |state, cx| {
                state.set_value(path.to_string_lossy().to_string(), window, cx)
            });
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        // The binary behind that path is replaced by an older build.
        binary.replace(Some("Chromium 128.0.0.0"));
        // The listing order is not part of the contract, so the button is found
        // by the row it belongs to.
        let index = view
            .read_with(cx, |view, _| view.state().core_rows().expect("rows"))
            .iter()
            .position(|row| row.core.executable == path)
            .expect("the added core is listed");
        cx.update(|window, cx| window.click(format!("redetect-core-{index}"), cx));
        settle(cx);

        let rows = view.read_with(cx, |view, _| view.state().core_rows().expect("rows"));
        let redetected = rows
            .iter()
            .find(|row| row.core.executable == path)
            .expect("the core is listed");
        assert_eq!(redetected.core.major, 128);
        assert_eq!(
            redetected.generation_label().as_deref(),
            Some("Chrome 143 and older · spoofing exclusions not honoured")
        );
    }

    #[gpui_kit::test]
    fn the_sidebar_switches_to_the_settings_page(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        assert!(
            cx.update(|window, _| window.try_find("settings-scroll").is_none()),
            "the settings page is not shown first"
        );

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("setting-data-dir").is_some()),
            "the settings page is shown"
        );
        assert!(
            cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
            "the profiles body is not rendered on the settings page"
        );
        assert!(
            cx.update(|window, _| window.try_find("setting-chromium-bin").is_some()),
            "the read-only rows are listed too"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().page()),
            crate::state::Page::Settings
        );
    }

    #[gpui_kit::test]
    fn only_the_editable_settings_offer_a_button(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);
        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);

        for key in ["xray-executable", "echo-url"] {
            assert!(
                cx.update(|window, _| window.try_find(format!("edit-setting-{key}")).is_some()),
                "{key} can be changed"
            );
        }
        for key in [
            "data-dir",
            "chromium-bin",
            "chromium-major",
            "config-file",
            "runtime-dir",
        ] {
            assert!(
                cx.update(|window, _| window.try_find(format!("edit-setting-{key}")).is_none()),
                "{key} is read-only"
            );
        }
        let _ = view;
    }

    #[gpui_kit::test]
    fn a_setting_can_be_changed_from_the_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = std::env::temp_dir().join(format!("fp-ui-settings-{}", std::process::id()));
        let config = dir.join("config.json");
        let (view, _runtime) = view_with_config(cx, &config);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        cx.update(|window, cx| window.click("edit-setting-xray-executable", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("setting-field-xray-executable").is_some()),
            "the field is open"
        );
        assert!(
            cx.update(|window, _| window.try_find("setting-xray-executable").is_some()),
            "the row it belongs to is still there"
        );

        let field = view
            .read_with(cx, |view, _| view.setting_editor())
            .expect("the settings field");
        cx.update(|window, cx| {
            field.update(cx, |state, cx| state.set_value("/opt/xray", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        let stored = std::fs::read_to_string(&config).expect("the config file was written");
        assert!(stored.contains("/opt/xray"), "{stored}");
        let row = view.read_with(cx, |view, _| {
            view.state()
                .setting_rows()
                .into_iter()
                .find(|row| row.key == crate::settings::SettingKey::XrayExecutable)
                .expect("the row")
        });
        assert_eq!(row.value, "/opt/xray");
        assert_eq!(row.source, crate::settings::Source::ConfigFile);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Choosing an appearance repaints the window at once and stores the choice.
    ///
    /// This is the one setting on the page whose effect is now rather than at the
    /// next start, so it has to be shown to be now: the assertion is on the
    /// palette in force, not on the stored value, and on the file as well,
    /// because a repaint that is forgotten by the next start is the failure the
    /// two halves of the test exist to separate.
    #[gpui_kit::test]
    fn choosing_an_appearance_repaints_and_is_remembered(cx: &mut TestAppContext) {
        // The component theme is process-wide; see `crate::theme::testing`.
        let _exclusive = crate::theme::testing::exclusive();
        cx.update(gpui_kit::init);
        let dir = std::env::temp_dir().join(format!("fp-ui-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config = dir.join("config.json");
        let (view, _runtime) = view_with_config(cx, &config);
        let cx = window(cx, &view);

        // Named rather than assumed: the library's own default is not this
        // program's default, so the starting point is set to what the window
        // would be showing at boot.
        cx.update(|_, cx| Theme::change(ThemeChoice::Dark.mode(), None, cx));
        cx.update(|_, cx| assert_eq!(palette(cx), Palette::DARK, "the starting palette"));
        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("theme-light").is_some()),
            "the appearance card is on the page"
        );

        cx.update(|window, cx| window.click("theme-light", cx));
        settle(cx);

        cx.update(|_, cx| assert_eq!(palette(cx), Palette::LIGHT, "the window repainted"));
        let stored = std::fs::read_to_string(&config).expect("the config file was written");
        assert!(stored.contains("\"light\""), "{stored}");
        view.read_with(cx, |view, _| {
            assert_eq!(view.state().theme(), ThemeChoice::Light);
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The switch repaints the window, and the identifiers it is addressed by do
    /// not move with the language: every test that clicks `nav-Settings`, and
    /// every script built on top of them, keeps working in either language.
    #[gpui_kit::test]
    fn choosing_a_language_repaints_and_leaves_the_identifiers_alone(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = std::env::temp_dir().join(format!("fp-ui-language-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let config = dir.join("config.json");
        let (view, _runtime) = view_with_config(cx, &config);
        let cx = window(cx, &view);
        seed_named_profile(cx, &view, "Work");

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        scroll_settings_to(cx, "language-zh");
        assert!(
            cx.update(|window, _| window.try_find("language-zh").is_some()),
            "the language sits beside the appearance"
        );

        cx.update(|window, cx| window.click("language-zh", cx));
        settle(cx);

        view.read_with(cx, |view, _| {
            assert_eq!(
                view.state().language(),
                crate::text::Lang::Zh,
                "the choice took effect"
            );
        });
        let stored = std::fs::read_to_string(&config).expect("the config file was written");
        assert!(stored.contains("\"zh\""), "{stored}");

        // The frozen identifiers: a language is a way of reading the window, not
        // a second window with its own names.
        assert!(
            cx.update(|window, _| window.try_find("nav-Settings").is_some()),
            "the page identifier did not move"
        );
        assert!(
            cx.update(|window, _| window.try_find("setting-data-dir").is_some()),
            "a setting's row identifier did not move either"
        );

        // A sentence the window computed rather than a label it looked up: the
        // profile count is built from the table on every render.
        cx.update(|window, cx| window.click("nav-Profiles", cx));
        settle(cx);
        assert_eq!(
            cx.update(|window, _| window.find("profile-count").label().map(str::to_string)),
            Some("共 1 个档案。".to_string()),
            "the header counts in the language now in force"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Show whatever the state has queued, the way the tick does.
    fn flush_toasts(cx: &mut gpui_kit::VisualTestContext, view: &gpui_kit::Entity<AppView>) {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.show_toasts(window, cx));
        });
    }

    #[gpui_kit::test]
    fn an_action_is_acknowledged_with_a_toast(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        seed_profile(cx, &view);
        flush_toasts(cx, &view);

        assert_eq!(
            cx.update(|window, cx| window.notifications(cx).len()),
            1,
            "creating a profile is acknowledged"
        );

        // A toast is shown once. Draining what was already shown must not
        // duplicate it on the next tick.
        flush_toasts(cx, &view);
        assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 1);
    }

    #[gpui_kit::test]
    fn a_problem_keeps_the_banner_and_still_gets_a_toast(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        // A value the settings file refuses is a problem and not a success: it
        // stays in the banner and is toasted while it happens.
        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        cx.update(|window, cx| window.click("edit-setting-echo-url", cx));
        settle(cx);
        let field = view
            .read_with(cx, |view, _| view.setting_editor())
            .expect("the settings field");
        cx.update(|window, cx| {
            field.update(cx, |state, cx| state.set_value("   ", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("dismiss-notice").is_some()),
            "a problem stays in the banner until it is dismissed"
        );
        flush_toasts(cx, &view);
        assert!(
            cx.update(|window, cx| window.notifications(cx).len()) >= 1,
            "the problem is also toasted while it happens"
        );
    }

    #[gpui_kit::test]
    fn the_sidebar_switches_to_the_log_page(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        assert!(
            cx.update(|window, _| window.try_find("logs-scroll").is_none()),
            "the log page is not shown first"
        );

        cx.update(|window, cx| window.click("nav-Log", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("logs-scroll").is_some()),
            "the log page is shown"
        );
        assert!(
            cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
            "only one page is rendered at a time"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().page()),
            crate::state::Page::Log
        );
    }

    #[gpui_kit::test]
    fn the_log_page_lists_what_happened_and_can_be_cleared(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        seed_profile(cx, &view);
        cx.update(|window, cx| window.click("nav-Log", cx));
        settle(cx);

        let label = cx
            .update(|window, _| window.find("log-0").label().map(|label| label.to_string()))
            .expect("the newest line is rendered");
        assert!(
            label.contains("Created Profile 1"),
            "the line says what happened: {label}"
        );
        assert!(label.contains("[info]"), "and at what level: {label}");

        cx.update(|window, cx| window.click("clear-log", cx));
        settle(cx);
        assert!(
            view.read_with(cx, |view, _| view.state().log_rows().is_empty()),
            "clearing empties the history"
        );
        assert!(
            cx.update(|window, _| window.try_find("logs-scroll").is_some()),
            "the page stays put with an empty history"
        );
    }

    #[gpui_kit::test]
    fn the_details_panel_switches_between_views(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, runtime) = view(cx);
        let cx = window(cx, &view);

        let id = seed_profile(cx, &view);
        // A launch line to show, and a line of this profile's own history.
        runtime.set_args(
            id,
            vec!["--fingerprint=1".to_string(), "about:blank".to_string()],
        );
        view.update(cx, |view, cx| {
            view.state_mut()
                .record_event(&RuntimeEvent::Stopped { profile_id: id });
            view.state_mut().refresh_runtime();
            cx.notify();
        });
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("details-grid").is_some()),
            "the panel opens on the profile itself"
        );
        assert!(cx.update(|window, _| window.try_find("args-body").is_none()));

        cx.update(|window, cx| window.click("details-tab-args", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("args-body").is_some()),
            "the launch line has a view of its own"
        );
        assert!(
            cx.update(|window, _| window.try_find("details-grid").is_none()),
            "one view at a time: the panel is no longer one long scroll"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view
                .state()
                .selected()
                .map(|row| row.effective_args().len())),
            Some(2)
        );

        cx.update(|window, cx| window.click("details-tab-log", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("panel-log-body").is_some()),
            "the third view is this profile's own history"
        );
        assert!(
            cx.update(|window, _| window.try_find(("panel-log", 0usize)).is_some()),
            "with the newest line in it"
        );
        assert!(cx.update(|window, _| window.try_find("details-grid").is_none()));
        assert_eq!(
            view.read_with(cx, |view, _| view.state().details_tab()),
            crate::state::DetailsTab::Log
        );

        cx.update(|window, cx| window.click("details-tab-details", cx));
        settle(cx);
        assert!(cx.update(|window, _| window.try_find("details-grid").is_some()));
        assert!(cx.update(|window, _| window.try_find("panel-log-body").is_none()));
    }

    #[gpui_kit::test]
    fn the_log_page_filter_can_be_narrowed(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        seed_profile(cx, &view);
        cx.update(|window, cx| window.click("nav-Log", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("log-0").is_some()),
            "the created profile is an info line"
        );

        cx.update(|window, cx| window.click("log-filter-errors", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("log-0").is_none()),
            "an info line is hidden by the error filter"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().log_filter()),
            crate::state::LogFilter::Errors
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().log_len()),
            1,
            "the line is hidden, not dropped"
        );

        cx.update(|window, cx| window.click("log-filter-all", cx));
        settle(cx);
        assert!(cx.update(|window, _| window.try_find("log-0").is_some()));
    }

    /// The page has to say where the log is being kept, and the file has to
    /// really hold what the page shows.
    #[gpui_kit::test]
    fn the_log_page_names_the_file_it_writes_to(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let dir = std::env::temp_dir().join(format!("fp-ui-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = crate::log_file::LogFile::open(&dir).expect("open the log file");
        let path = log.path().to_path_buf();
        let (view, _runtime, _opener) = view_with_log(
            cx,
            Arc::new(FakeVerifier::passing()),
            None,
            Arc::new(FakeOpener::working()),
            Some(log),
        );
        let cx = window(cx, &view);

        seed_profile(cx, &view);
        cx.update(|window, cx| window.click("nav-Log", cx));
        settle(cx);

        let label = cx
            .update(|window, _| {
                window
                    .find("log-file-status")
                    .label()
                    .map(|label| label.to_string())
            })
            .expect("the page says where the log is");
        assert!(label.contains(&path.display().to_string()), "{label}");

        let text = std::fs::read_to_string(&path).expect("the file was written");
        assert!(text.contains("Created Profile 1"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn a_profile_data_directory_can_be_opened(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let opener = Arc::new(FakeOpener::working());
        let (view, _runtime, opener) =
            view_with_opener(cx, Arc::new(FakeVerifier::passing()), None, opener);
        let cx = window(cx, &view);

        let id = seed_profile(cx, &view);
        let path = view.read_with(cx, |view, _| {
            view.state().profile(id).expect("the profile").user_data_dir
        });

        cx.update(|window, cx| window.click(format!("open-dir-{id}"), cx));
        wait_for_state(cx, &view, |state| {
            state
                .log_rows()
                .iter()
                .any(|row| row.message.contains("Opened"))
        });

        assert_eq!(
            opener.opened(),
            vec![path.clone()],
            "the button opens the data directory of the selected profile"
        );
        let toast = view
            .read_with(cx, |view, _| view.state().toasts().last().cloned())
            .expect("the open is acknowledged");
        assert!(
            toast.message.contains(&path.display().to_string()),
            "{}",
            toast.message
        );
        assert!(
            cx.update(|window, cx| window.notifications(cx).len()) == 0,
            "the click queues a toast; the tick shows it, not the click itself"
        );
    }

    #[gpui_kit::test]
    fn a_failed_open_is_refused_with_the_reason(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let opener = Arc::new(FakeOpener::failing("could not run xdg-open: not found"));
        let (view, _runtime, _opener) =
            view_with_opener(cx, Arc::new(FakeVerifier::passing()), None, opener);
        let cx = window(cx, &view);

        let id = seed_profile(cx, &view);
        cx.update(|window, cx| window.click(format!("open-dir-{id}"), cx));
        wait_for_state(cx, &view, |state| state.notice().is_some());

        let notice = view
            .read_with(cx, |view, _| view.state().notice().cloned())
            .expect("the failure is reported");
        assert!(notice.error);
        assert!(notice.message.contains("xdg-open"), "{}", notice.message);
    }

    /// Draws enough frames for a layer to mount and then paint at rest.
    fn settle(cx: &mut gpui_kit::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    #[gpui_kit::test]
    fn a_profile_can_be_edited_from_the_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        let id = seed_profile(cx, &view);

        cx.update(|window, cx| window.click(format!("edit-{id}"), cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("editor-name").is_some()),
            "the editor opens on the profile"
        );

        let editor = view
            .read_with(cx, |view, _| view.editor())
            .expect("an editor");
        let name = editor.read_with(cx, |editor, _| editor.name_input());
        cx.update(|window, cx| {
            name.update(cx, |state, cx| state.set_value("Renamed", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        let row = view.read_with(cx, |view, _| view.state().rows()[0].clone());
        assert_eq!(row.profile.name, "Renamed");
        assert!(
            cx.update(|window, _| window.try_find("editor-name").is_none()),
            "saving closes the dialog"
        );
    }

    #[gpui_kit::test]
    fn the_filter_narrows_the_profile_list(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        let work = seed_named_profile(cx, &view, "Work laptop");
        let shopping = seed_named_profile(cx, &view, "Shopping");
        settle(cx);
        assert_eq!(
            cx.update(|window, _| window.find("profile-count").label().map(str::to_string)),
            Some("2 profiles.".to_string()),
            "without a filter the header says how many there are"
        );

        type_filter(cx, &view, "work");

        assert!(
            cx.update(|window, _| window.try_find(format!("profile-{work}")).is_some()),
            "the profile that matches is listed"
        );
        assert!(
            cx.update(|window, _| window.try_find(format!("profile-{shopping}")).is_none()),
            "the one that does not match is not"
        );
        assert_eq!(
            cx.update(|window, _| window.find("profile-count").label().map(str::to_string)),
            Some("Showing 1 of 2 profiles, filtered by \"work\".".to_string()),
            "the header says the list is not the whole list"
        );
    }

    #[gpui_kit::test]
    fn a_filter_that_matches_nothing_offers_the_way_back(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        let id = seed_named_profile(cx, &view, "Work laptop");
        settle(cx);

        type_filter(cx, &view, "nothing is called this");

        assert!(
            cx.update(|window, _| window.try_find(format!("profile-{id}")).is_none()),
            "the list is empty"
        );
        assert!(
            cx.update(|window, _| window.try_find("clear-filter").is_some()),
            "an empty list says which kind of empty it is, and offers the way out"
        );

        cx.update(|window, cx| window.click("clear-filter", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find(format!("profile-{id}")).is_some()),
            "clearing brings the profile back"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().profile_filter().to_string()),
            "",
            "the list is unfiltered again"
        );
        let field = view
            .read_with(cx, |view, _| view.filter_input())
            .expect("the field is built by now");
        assert_eq!(
            field.read_with(cx, |state, _| state.value().to_string()),
            "",
            "the field was emptied too: setting a value emits no change event"
        );
        assert!(
            cx.update(|window, _| window.try_find("clear-filter").is_none()),
            "and the hint goes with it"
        );
    }

    #[gpui_kit::test]
    fn the_filter_survives_a_trip_to_another_page(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        let work = seed_named_profile(cx, &view, "Work laptop");
        seed_named_profile(cx, &view, "Shopping");
        settle(cx);

        type_filter(cx, &view, "work");

        cx.update(|window, cx| window.click("nav-Proxies", cx));
        settle(cx);
        cx.update(|window, cx| window.click("nav-Profiles", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find(format!("profile-{work}")).is_some()),
            "the filter is still narrowing the list"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().visible_rows().len()),
            1
        );
        let field = view
            .read_with(cx, |view, _| view.filter_input())
            .expect("the field outlives the page it was built for");
        assert_eq!(
            field.read_with(cx, |state, _| state.value().to_string()),
            "work",
            "and it still shows the term"
        );
    }

    #[gpui_kit::test]
    fn a_refused_edit_keeps_the_dialog_open_and_shows_why(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        let id = seed_profile(cx, &view);
        let name_before = view.read_with(cx, |view, _| view.state().rows()[0].profile.name.clone());

        cx.update(|window, cx| window.click(format!("edit-{id}"), cx));
        settle(cx);

        let editor = view
            .read_with(cx, |view, _| view.editor())
            .expect("an editor");
        let seed = editor.read_with(cx, |editor, _| editor.seed_input());
        cx.update(|window, cx| {
            seed.update(cx, |state, cx| state.set_value("not a number", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("editor-name").is_some()),
            "a refused save leaves the form open"
        );
        assert!(
            cx.update(|window, _| window.try_find("editor-error").is_some()),
            "the form says what is wrong"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().rows()[0].profile.name.clone()),
            name_before,
            "nothing was written"
        );
    }

    #[gpui_kit::test]
    fn duplicating_adds_a_profile_and_deleting_removes_one(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        let id = seed_profile(cx, &view);

        cx.update(|window, cx| window.click(format!("duplicate-{id}"), cx));
        settle(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.state().rows().len()),
            2,
            "the copy is listed"
        );
        let copy = view
            .read_with(cx, |view, _| view.state().selected_id())
            .expect("selected");
        assert_ne!(copy, id);

        cx.update(|window, cx| window.click(format!("delete-{copy}"), cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("ok").is_some()),
            "deleting asks first"
        );

        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.state().rows().len()),
            1,
            "only the copy was removed"
        );
        assert!(
            view.read_with(cx, |view, _| view.state().profile(id).is_some()),
            "the original is untouched"
        );
    }

    #[gpui_kit::test]
    fn starting_without_a_core_says_so_and_offers_the_way_there(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let profile_repo: Arc<MemProfileRepository> = Arc::new(MemProfileRepository::new());
        let core_repo: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let proxy_repo: Arc<MemProxyRepository> = Arc::new(MemProxyRepository::new());
        let runtime = Arc::new(FakeRuntime::new());

        let profiles = Arc::new(DefaultProfileService::new(
            profile_repo.clone(),
            PathBuf::from("data"),
        ));
        let runtime_service = Arc::new(RuntimeService::new(
            profile_repo.clone(),
            core_repo.clone(),
            proxy_repo.clone(),
            runtime,
        ));
        let cores = crate::state::testing::core_service(core_repo.clone(), profile_repo.clone());
        let configuration = Arc::new(storage::MemConfiguration::new(
            core_repo.clone(),
            proxy_repo.clone(),
            profile_repo.clone(),
        ));
        let proxies: Arc<dyn ProxyService> =
            Arc::new(DefaultProxyService::new(proxy_repo, profile_repo));
        let settings = crate::state::testing::settings();
        let state = AppState::for_test(
            crate::state::Services {
                profiles,
                runtime: runtime_service,
                cores,
                proxies,
                configuration,
            },
            settings,
        );
        let (_command_tx, event_rx) = crossbeam_channel::bounded(16);
        let view = cx.new(|cx| {
            let mut view = AppView::new(
                state,
                event_rx,
                Arc::new(FakeVerifier::passing()),
                Arc::new(FakeProxyTester::passing()),
                Arc::new(FakeOpener::working()),
                Arc::new(FakeBrowserDataCopier::passing()),
            );
            view.boot(cx);
            view
        });

        let handle = cx.open_window(size(px(900.), px(600.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);

            // The situation is legible where it matters, without a click: the
            // list says there is no core, it names both of the ways to supply
            // one, and the button that would open a form the service refuses is
            // disabled rather than describing the refusal after it happens.
            let hint = window
                .find("empty-hint")
                .label()
                .map(str::to_string)
                .expect("the empty state says what is missing");
            assert!(hint.contains("No browser core yet"), "{hint}");
            assert!(hint.contains("Browser Cores"), "{hint}");
            assert!(hint.contains("FP_BROWSER_CHROMIUM_BIN"), "{hint}");
            window.click("new-profile", cx);
            assert!(
                window.try_find("editor-name").is_none(),
                "a profile needs a core to launch, so the form does not open"
            );

            // And the one thing the reader can do about it is one click away,
            // on a page that is not the one they are looking at.
            assert!(
                window.try_find("empty-add-core").is_some(),
                "the empty state carries the way to the fix"
            );
        })
        .unwrap();

        // The button is wired to the page switch; the state is what says it
        // worked, because a click on a button inside a scrolled container is a
        // question about the harness rather than about this program.
        view.update(cx, |view, cx| view.on_page(crate::state::Page::Cores, cx));
        assert_eq!(
            view.read_with(cx, |view, _| view.state().page()),
            crate::state::Page::Cores
        );
    }

    #[gpui_kit::test]
    fn exporting_from_the_settings_page_writes_the_file_and_says_what_it_did(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        let dir = std::env::temp_dir().join(format!("fp-ui-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("config.json");

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        // The export card is below the fold of the test window.
        scroll_settings_to(cx, "export-run");
        // The field exists only after the page has been rendered, which is where
        // an `InputState` gets the window it needs.
        type_export_path(cx, &view, path.to_string_lossy().as_ref());
        cx.update(|window, cx| window.click("export-run", cx));
        settle(cx);

        assert!(path.exists(), "{} should exist", path.display());
        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(text.contains("fp-browser/config-backup"), "{text}");

        // This fixture has no proxies, so the sentence is the one about there
        // having been nothing to leave out - not a claim about credentials that
        // were not there.
        let message = last_message(cx, &view);
        assert!(message.contains("config.json"), "{message}");
        assert!(
            message.contains("no proxy credentials to leave out"),
            "{message}"
        );

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    /// The card's whole job: one press, one file, and the window says where.
    ///
    /// The file it looks for is the one the window says it wrote, not the one the
    /// card named before the press. The name carries the second, and the card
    /// computes it while it is being drawn, so a press that lands in the next
    /// second writes a different name - which is not worth a mechanism in the
    /// card, but is worth not asserting on in a test that runs for hours next to
    /// a hundred others.
    #[gpui_kit::test]
    fn writing_a_diagnostics_report_from_the_settings_page_says_where_it_went(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        scroll_settings_to(cx, "diagnostics-run");
        // The folder is the card's claim that does not move: one report per
        // press, in `diagnostics` under the data directory.
        let folder = view
            .read_with(cx, |view, _| view.state().diagnostics_destination())
            .parent()
            .expect("a diagnostics folder")
            .to_path_buf();
        let _ = std::fs::remove_dir_all(&folder);

        cx.update(|window, cx| window.click("diagnostics-run", cx));
        settle(cx);

        let written: Vec<PathBuf> = std::fs::read_dir(&folder)
            .expect("the card's folder exists")
            .map(|entry| entry.expect("an entry").path())
            .collect();
        assert_eq!(written.len(), 1, "one press writes one report");
        let written = &written[0];
        assert!(
            written
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .starts_with("fp-browser-diagnostics-"),
            "{}",
            written.display()
        );

        let text = std::fs::read_to_string(written).expect("read back");
        assert!(text.contains(crate::version::VERSION), "{text}");
        assert!(text.contains("Fingerprint Browser diagnostics"), "{text}");

        let message = last_message(cx, &view);
        assert!(
            message.contains(&written.display().to_string()),
            "the window names the file it wrote: {message}"
        );

        // The report is the only thing this test leaves, and it leaves nothing:
        // the fixture's data directory is shared with the rest of the suite.
        std::fs::remove_file(written).expect("clean up");
        let _ = std::fs::remove_dir(&folder);
    }

    #[gpui_kit::test]
    fn the_export_card_offers_the_choice_about_credentials(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);

        // Left out by default, because a file carrying plain-text passwords is
        // the thing to choose deliberately.
        assert!(!view.read_with(cx, |view, _| view.state().export_includes_credentials()));

        // And the line under the field names the file an empty field would
        // write, so the default is shown rather than described.
        let shown = view
            .read_with(cx, |view, _| view.state().export_destination())
            .display()
            .to_string();
        assert!(shown.ends_with(".json"), "{shown}");
        assert!(shown.contains("exports"), "{shown}");
    }

    /// Types into the Settings page's import path field.
    fn type_import_path(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
        text: &str,
    ) {
        let field = view
            .read_with(cx, |view, _| view.import_input())
            .expect("rendering the Settings page builds the import field");
        cx.update(|window, cx| {
            field.update(cx, |state, cx| state.set_value(text, window, cx));
        });
        settle(cx);
    }

    /// The band along the bottom edge of the window where a click does not reach
    /// what is under it.
    ///
    /// Measured rather than read out of the component library: a button whose
    /// middle is inside the window and whose bounds are inside the viewport is
    /// still not clickable once it has scrolled to within about forty pixels of
    /// the bottom - the library's window border keeps a resize band and an
    /// overlay layer there, and a click that lands in it is swallowed without an
    /// error. So a target has to be this clear of the bottom before it counts.
    const CLICKABLE_BOTTOM_MARGIN: f32 = 48.;

    /// Whether an element is fully on screen and clickable.
    ///
    /// `visible()` alone is not enough, twice over. It is true for an element
    /// with one pixel showing at the edge of the scrolled area, and a click is
    /// aimed at the middle of the element and hit-tested there: an element that
    /// has just scrolled into view by one step reports visible while half of it
    /// is still past the bottom of the page, so the click lands on the scroll
    /// view instead of on the button - and nothing reports an error, because the
    /// window simply received a click somewhere else. And an element in the
    /// bottom band above is swallowed even when it is entirely inside the
    /// viewport. Requiring the whole element to be clear of both is what tells
    /// those states apart from a click that will land.
    ///
    /// The id is copied because `try_find` wants an owned one: the ids here are
    /// built at the call site (`setting-data-dir`, and so on) rather than being
    /// the `&'static str` the lookup asks for.
    fn is_clickable(cx: &mut gpui_kit::VisualTestContext, id: &str) -> bool {
        let id = id.to_string();
        cx.update(|window, _| {
            let Some(found) = window.try_find(id) else {
                return false;
            };
            if !found.visible() {
                return false;
            }
            let size = window.viewport_size();
            let usable = gpui_kit::Bounds::new(
                gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
                gpui_kit::Size {
                    width: size.width,
                    height: size.height - gpui_kit::px(CLICKABLE_BOTTOM_MARGIN),
                },
            );
            found.bounds().is_contained_within(&usable)
        })
    }

    /// Scrolls the Settings page until `target` is on screen.
    ///
    /// The page is longer than the test window and its cards sit below the fold,
    /// and a click on an off-screen element is refused. The wheel event has to be
    /// dispatched over a child of the scrolling container - the container is not
    /// something the helpers can aim at - and that child has to be on screen
    /// itself. A single long scroll aimed at one row stops working the moment the
    /// page grows past it, which is what happened when a card was added at the
    /// bottom; so each step aims at whichever row or card is on screen now, which
    /// is also where the wheel would really land.
    fn scroll_settings_to(cx: &mut gpui_kit::VisualTestContext, target: &str) {
        let mut previous = None;
        for _ in 0..40 {
            if is_clickable(cx, target) {
                return;
            }
            let now =
                cx.update(|window, _| window.try_find(target.to_string()).map(|id| id.bounds()));
            if now.is_some() && now == previous {
                // The page is at its end: the element is as far into view as it
                // is going to get, and another wheel event would only spin.
                panic!("{target} cannot be scrolled clear of the bottom of the window");
            }
            previous = now;
            let aim = crate::settings::SettingKey::ALL
                .iter()
                .map(|key| format!("setting-{}", key.id()))
                .chain(SETTINGS_CARDS.iter().map(|id| (*id).to_string()))
                .find(|id| is_clickable(cx, id))
                .unwrap_or_else(|| {
                    panic!("nothing on the Settings page is on screen to scroll from")
                });
            cx.update(|window, cx| {
                window.scroll(
                    aim,
                    gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                        gpui_kit::px(0.),
                        gpui_kit::px(-200.),
                    )),
                    cx,
                );
            });
            settle(cx);
        }
        panic!("{target} never came into view");
    }

    /// The Settings cards, in the order they appear, for the scroll helper to aim
    /// at once the setting rows above them have scrolled away.
    const SETTINGS_CARDS: [&str; 6] = [
        "appearance",
        "export-configuration",
        "import-configuration",
        "restore-configuration",
        "browser-data",
        "diagnostics",
    ];

    #[gpui_kit::test]
    fn importing_from_the_settings_page_reads_the_file_and_says_what_it_did(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        // The file to import is written the honest way: through the same
        // document the export writes, for an installation that holds nothing.
        // The state-level tests cover a full round trip; what is under test
        // here is the wiring from the card to the use case.
        let dir = std::env::temp_dir().join(format!("fp-ui-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("config.json");
        let snapshot = application::ConfigSnapshot {
            cores: Vec::new(),
            proxies: Vec::new(),
            profiles: Vec::new(),
        };
        let document = application::ConfigBackup::build(
            snapshot,
            application::Credentials::Excluded,
            application::ExportOrigin {
                exported_at: "2026-09-21T00:00:00Z".to_string(),
                source_data_dir: dir.display().to_string(),
            },
        );
        std::fs::write(&path, document.to_json().expect("serialise")).expect("write");

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        type_import_path(cx, &view, path.to_string_lossy().as_ref());
        scroll_settings_to(cx, "import-run");
        cx.update(|window, cx| window.click("import-run", cx));
        settle(cx);

        // An empty installation importing an empty file is the quiet case: a
        // toast that says so, not a banner that pretends something happened.
        let message = last_message(cx, &view);
        assert!(message.contains("config.json"), "{message}");
        assert!(message.contains("nothing was added"), "{message}");
        assert!(
            view.read_with(cx, |view, _| view.state().notice().is_none()),
            "the quiet case does not sit in the banner"
        );

        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[gpui_kit::test]
    fn importing_without_a_path_is_refused_in_the_banner(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        scroll_settings_to(cx, "import-run");
        cx.update(|window, cx| window.click("import-run", cx));
        settle(cx);

        // Refused where the field is, and stayed to be dismissed - the same
        // treatment every other refusal in the window gets.
        let notice = view.read_with(cx, |view, _| view.state().notice().cloned());
        let notice = notice.expect("the refusal is shown");
        assert!(notice.error, "{:?}", notice.message);
        assert!(
            notice.message.contains("Type the path"),
            "{:?}",
            notice.message
        );
    }

    /// Types into the Settings page's restore path field.
    fn type_restore_path(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
        text: &str,
    ) {
        let field = view
            .read_with(cx, |view, _| view.restore_input())
            .expect("rendering the Settings page builds the restore field");
        cx.update(|window, cx| {
            field.update(cx, |state, cx| state.set_value(text, window, cx));
        });
        settle(cx);
    }

    #[gpui_kit::test]
    fn restoring_without_a_path_is_refused_in_the_banner(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        // The restore card sits below the import one; the same downward scroll
        // brings it into view.
        scroll_settings_to(cx, "restore-run");
        cx.update(|window, cx| window.click("restore-run", cx));
        settle(cx);

        // An empty field is refused where it is, the same way an import is.
        let notice = view.read_with(cx, |view, _| view.state().notice().cloned());
        let notice = notice.expect("the refusal is shown");
        assert!(notice.error, "{:?}", notice.message);
        assert!(
            notice.message.contains("Type the path"),
            "{:?}",
            notice.message
        );
    }

    /// A populated installation is not replaced without being asked: the button
    /// opens a confirmation, and only the confirmation runs the replacement.
    #[gpui_kit::test]
    fn restoring_over_a_populated_installation_asks_before_replacing(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);
        // The fixture seeds a core, so there is already something to replace.
        seed_profile(cx, &view);

        // A file to restore, written through the document the export writes.
        let dir = std::env::temp_dir().join(format!("fp-ui-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("config.json");
        let document = application::ConfigBackup::build(
            application::ConfigSnapshot::default(),
            application::Credentials::Excluded,
            application::ExportOrigin {
                exported_at: "2026-09-21T00:00:00Z".to_string(),
                source_data_dir: dir.display().to_string(),
            },
        );
        std::fs::write(&path, document.to_json().expect("serialise")).expect("write");

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        type_restore_path(cx, &view, path.to_string_lossy().as_ref());
        scroll_settings_to(cx, "restore-run");
        cx.update(|window, cx| window.click("restore-run", cx));
        settle(cx);

        assert!(
            cx.update(|window, _| window.try_find("ok").is_some()),
            "restoring a populated installation asks first"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().rows().len()),
            1,
            "nothing is replaced before the confirmation"
        );

        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        // The file held nothing, so the replacement took what was here away.
        assert_eq!(
            view.read_with(cx, |view, _| view.state().rows().len()),
            0,
            "the confirmed restore replaced the configuration"
        );
        let message = last_message(cx, &view);
        assert!(message.contains("nothing was added"), "{message}");
        assert!(message.contains("were replaced"), "{message}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A stopped profile that leaves through a proxy: the shape every pre-flight
    /// test starts from.
    fn seed_proxied_profile(
        cx: &mut gpui_kit::VisualTestContext,
        view: &gpui_kit::Entity<AppView>,
    ) -> (ProfileId, ProxyId) {
        let id = seed_profile(cx, view);
        let proxy_id = seed_proxy(cx, view, "Office");
        view.update(cx, |view, cx| {
            let state = view.state_mut();
            let mut profile = state.profile(id).expect("the profile");
            profile.proxy_id = Some(proxy_id);
            state.update_profile(profile).expect("assign");
            cx.notify();
        });
        (id, proxy_id)
    }

    /// Pressing Start on a proxied profile asks the proxy first, and a proxy that
    /// carries nothing means the browser is never launched.
    ///
    /// This is the whole point of the gate at the level the user meets it: the
    /// window must not be able to produce a browser whose traffic has nowhere to
    /// go, and the sentence has to say which link of the chain failed.
    #[gpui_kit::test]
    fn starting_a_proxied_profile_is_refused_when_its_proxy_has_no_traffic(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let tester = Arc::new(FakeProxyTester::with_outcome(Err(Fault::new(
            FaultClass::Unreachable,
            "no route to the upstream",
        ))));
        let (view, runtime, _opener, tester, _copier) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            tester,
            Arc::new(FakeBrowserDataCopier::passing()),
            None,
            Arc::new(FakeOpener::working()),
            None,
        );
        let cx = window(cx, &view);
        let (id, proxy_id) = seed_proxied_profile(cx, &view);

        cx.update(|window, cx| window.click(format!("start-{id}"), cx));
        wait_for_state(cx, &view, |state| {
            state
                .proxy_test(proxy_id)
                .is_some_and(|test| !test.is_running())
        });

        assert_eq!(tester.calls(), 1, "the proxy was asked once");
        assert!(
            runtime.commands.lock().expect("commands").is_empty(),
            "the browser was not launched"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().row(id).expect("the row").state()),
            RuntimeState::Stopped,
            "and the profile is not left reading as starting"
        );
        let message = last_message(cx, &view);
        assert!(message.contains("Profile 1"), "{message}");
        assert!(message.contains("Office"), "{message}");
        assert!(message.contains("no route to the upstream"), "{message}");
    }

    /// The same press, with a proxy that answers: the browser starts, and the
    /// window says where the traffic it will use leaves from.
    #[gpui_kit::test]
    fn starting_a_proxied_profile_goes_ahead_once_its_proxy_answers(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let tester = Arc::new(FakeProxyTester::passing_from("198.51.100.9"));
        let (view, runtime, _opener, tester, _copier) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            tester,
            Arc::new(FakeBrowserDataCopier::passing()),
            None,
            Arc::new(FakeOpener::working()),
            None,
        );
        let cx = window(cx, &view);
        let (id, _proxy_id) = seed_proxied_profile(cx, &view);

        cx.update(|window, cx| window.click(format!("start-{id}"), cx));
        wait_for_state(cx, &view, |state| {
            state
                .row(id)
                .is_some_and(|row| row.state() == RuntimeState::Running)
        });

        assert_eq!(tester.calls(), 1, "the proxy was asked once");
        assert_eq!(
            runtime.commands.lock().expect("commands").clone(),
            [format!("start:{id}")]
        );
        let message = last_message(cx, &view);
        assert!(
            message.contains("Started Profile 1 through Office"),
            "{message}"
        );
        assert!(message.contains("198.51.100.9"), "{message}");
    }

    /// A profile without a proxy is not slowed down or gated by any of this: the
    /// request a check would send is the one the browser makes for its own first
    /// page.
    #[gpui_kit::test]
    fn starting_a_profile_without_a_proxy_asks_nothing(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let tester = Arc::new(FakeProxyTester::passing());
        let (view, runtime, _opener, tester, _copier) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            tester,
            Arc::new(FakeBrowserDataCopier::passing()),
            None,
            Arc::new(FakeOpener::working()),
            None,
        );
        let cx = window(cx, &view);
        let id = seed_profile(cx, &view);

        cx.update(|window, cx| window.click(format!("start-{id}"), cx));
        settle(cx);

        assert_eq!(tester.calls(), 0, "no proxy, nothing to ask");
        assert_eq!(
            runtime.commands.lock().expect("commands").clone(),
            [format!("start:{id}")]
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.state().row(id).expect("the row").state()),
            RuntimeState::Running
        );
    }

    /// The same view, with the browser-data copier the test drives.
    fn view_with_browser_data(
        cx: &mut TestAppContext,
        copier: Arc<FakeBrowserDataCopier>,
    ) -> (
        gpui_kit::Entity<AppView>,
        Arc<FakeRuntime>,
        Arc<FakeBrowserDataCopier>,
    ) {
        let (view, runtime, _opener, _tester, copier) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            Arc::new(FakeProxyTester::passing()),
            copier,
            None,
            Arc::new(FakeOpener::working()),
            None,
        );
        (view, runtime, copier)
    }

    /// A browser-data copy is hundreds of megabytes, so it is handed to a
    /// worker; what the card has to get right is the job it is handed - the
    /// direction, the directory and the profiles - and the sentence it reports.
    #[gpui_kit::test]
    fn a_browser_data_copy_from_the_window_asks_the_worker_and_says_what_it_did(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let copier = Arc::new(FakeBrowserDataCopier::passing());
        let (view, _runtime, copier) = view_with_browser_data(cx, copier);
        let cx = window(cx, &view);
        seed_profile(cx, &view);

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        let field = view
            .read_with(cx, |view, _| view.browser_data_input())
            .expect("the Settings page builds the browser-data field");
        cx.update(|window, cx| {
            field.update(cx, |state, cx| {
                state.set_value("/backups/browser-data", window, cx)
            });
        });
        settle(cx);

        scroll_settings_to(cx, "browser-data-out");
        cx.update(|window, cx| window.click("browser-data-out", cx));
        wait_for_state(cx, &view, |state| {
            state
                .toasts()
                .iter()
                .any(|toast| toast.message.contains("Copied"))
        });

        // The worker was handed the direction and the directory the field held,
        // and every profile on the list.
        let jobs = copier.jobs();
        assert_eq!(copier.calls(), 1);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].direction, Direction::ToBackup);
        assert_eq!(jobs[0].directory, PathBuf::from("/backups/browser-data"));
        assert_eq!(jobs[0].profiles.len(), 1);
        assert!(jobs[0].running.is_empty());

        let message = last_message(cx, &view);
        assert!(message.contains("Copied the browser data"), "{message}");
        assert!(message.contains("/backups/browser-data"), "{message}");
    }

    /// A copy in flight is a state the window can be asked about. The lease the
    /// job carries is held by the worker, so while the first copy is still running
    /// a second press and a start are both refused - and both work again once it
    /// has finished. The worker is a fake this test holds open, which is what
    /// makes "still running" something to assert against rather than to race.
    #[gpui_kit::test]
    fn a_copy_in_flight_refuses_a_second_copy_and_a_start(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (copier, pause) = FakeBrowserDataCopier::paused();
        let (view, runtime, copier) = view_with_browser_data(cx, Arc::new(copier));
        let cx = window(cx, &view);
        let id = seed_profile(cx, &view);

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        let field = view
            .read_with(cx, |view, _| view.browser_data_input())
            .expect("the Settings page builds the browser-data field");
        cx.update(|window, cx| {
            field.update(cx, |state, cx| {
                state.set_value("/backups/browser-data", window, cx)
            });
        });
        settle(cx);

        // The first copy starts and stops inside the worker.
        scroll_settings_to(cx, "browser-data-out");
        cx.update(|window, cx| window.click("browser-data-out", cx));
        pause.wait_until_copying();

        // A second press is refused, and starts no worker of its own.
        cx.update(|window, cx| window.click("browser-data-out", cx));
        settle(cx);
        let message = last_message(cx, &view);
        assert!(message.contains("try again"), "{message}");
        assert_eq!(copier.calls(), 1, "the refused press started no worker");

        // So is starting one of the profiles the copy is holding.
        cx.update(|window, cx| window.click("nav-Profiles", cx));
        settle(cx);
        cx.update(|window, cx| window.click(format!("start-{id}"), cx));
        settle(cx);
        let message = last_message(cx, &view);
        assert!(message.contains("try again"), "{message}");
        assert!(
            runtime.commands.lock().expect("commands").is_empty(),
            "the refused start never reached the runtime"
        );

        // The copy finishes. Everything it held is free, so the next one runs.
        pause.release();
        wait_for_state(cx, &view, |state| {
            state
                .toasts()
                .iter()
                .any(|toast| toast.message.contains("Copied"))
        });
        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        scroll_settings_to(cx, "browser-data-out");
        cx.update(|window, cx| window.click("browser-data-out", cx));
        pause.wait_until_copying();
        assert_eq!(copier.calls(), 2, "the copy after it finished is allowed");
        pause.release();
    }

    #[gpui_kit::test]
    fn a_browser_data_copy_without_a_directory_is_refused_in_the_banner(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let copier = Arc::new(FakeBrowserDataCopier::passing());
        let (view, _runtime, copier) = view_with_browser_data(cx, copier);
        let cx = window(cx, &view);
        seed_profile(cx, &view);

        cx.update(|window, cx| window.click("nav-Settings", cx));
        settle(cx);
        scroll_settings_to(cx, "browser-data-out");
        cx.update(|window, cx| window.click("browser-data-out", cx));
        settle(cx);

        let notice = view.read_with(cx, |view, _| view.state().notice().cloned());
        let notice = notice.expect("the refusal is shown");
        assert!(notice.error, "{:?}", notice.message);
        assert!(
            notice.message.contains("Type the directory"),
            "{:?}",
            notice.message
        );
        assert_eq!(
            copier.calls(),
            0,
            "no worker is started without a directory"
        );
    }
}
