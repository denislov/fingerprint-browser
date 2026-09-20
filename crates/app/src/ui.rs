//! The GPUI window: renders [`AppState`] and forwards user actions into it.
//!
//! The view owns no runtime state. Notifications only mark the snapshot cache
//! dirty; the snapshot itself stays the single source of truth, exactly as the
//! runtime façade contract requires.

use crate::core_editor::CoreEditor;
use crate::editor::{ProfileEdit, ProfileEditor};
use crate::open_dir::DirectoryOpener;
use crate::proxy_editor::ProxyEditor;
use crate::proxy_import::ProxyImport;
use crate::proxy_tester::ProxyTester;
use crate::settings::SettingKey;
use crate::state::{
    AppState, CoreRow, DetailsTab, LogFilter, LogLevel, LogRow, Page, ProfileRow, ProxyRow,
    ProxyTest, Toast, ToastKind, Verification,
};
use crate::verifier::{FingerprintVerifier, VerificationReport};
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
use gpui_kit::prelude::*;
use gpui_kit::*;
use runtime::{Diagnosis, Fault, RuntimeEvent};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const BG: u32 = 0x18181b;
const PANEL: u32 = 0x1f1f23;
const BORDER: u32 = 0x27272a;
const TEXT: u32 = 0xf4f4f5;
const MUTED: u32 = 0x71717a;
const DIM: u32 = 0x52525b;

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
    verifier: Arc<dyn FingerprintVerifier>,
    /// Sends one request through a proxy. Injected so a test never dials one.
    tester: Arc<dyn ProxyTester>,
    /// Opens a profile's data directory. Injected so a test does not open one.
    opener: Arc<dyn DirectoryOpener>,
    verifications: Receiver<(ProfileId, Result<VerificationReport, String>)>,
    verification_tx: Sender<(ProfileId, Result<VerificationReport, String>)>,
    /// Finished proxy tests, with whether the engine probed was already up.
    proxy_tests: Receiver<(ProxyId, bool, Result<Diagnosis, Fault>)>,
    proxy_test_tx: Sender<(ProxyId, bool, Result<Diagnosis, Fault>)>,
    /// What the opener reported, once it was done handing the request off.
    open_results: Receiver<(PathBuf, Result<(), String>)>,
    open_tx: Sender<(PathBuf, Result<(), String>)>,
    state: AppState,
    events: Receiver<RuntimeEvent>,
    /// Kept alive: dropping a GPUI subscription unregisters the observer.
    window_closed: Option<Subscription>,
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
    ) -> Self {
        // A reading takes seconds and blocks on the browser, so it runs on a
        // worker thread and reports back through this channel. Testing a proxy
        // starts an engine and waits for one request, which blocks the same way
        // and for the same reason. Opening a directory waits for the desktop's
        // opener too: none of the three may block the window.
        let (verification_tx, verifications) = crossbeam_channel::unbounded();
        let (proxy_test_tx, proxy_tests) = crossbeam_channel::unbounded();
        let (open_tx, open_results) = crossbeam_channel::unbounded();
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
            verifier,
            tester,
            opener,
            verifications,
            verification_tx,
            proxy_tests,
            proxy_test_tx,
            open_results,
            open_tx,
            state,
            events,
            window_closed: None,
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

    fn on_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.state.set_page(page);
        cx.notify();
    }

    /// Load storage once, then keep reconciling from snapshots in the background.
    pub fn boot(&mut self, cx: &mut Context<Self>) {
        let _ = self.state.load();
        // Quitting on the last closed window is the default on Windows/Linux but
        // not on macOS; make it uniform so the supervisor shutdown always runs.
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
                        return (false, Vec::new(), Vec::new());
                    }

                    let notified = view.drain_events();
                    let verified = view.drain_verifications();
                    let tested = view.drain_proxy_tests();
                    let opened = view.drain_open_results();
                    if notified || reconcile || verified || tested || opened {
                        view.state.refresh_runtime();
                        cx.notify();
                    }
                    // The toasts are pushed from outside this update: showing
                    // one updates the root view the notification layer lives
                    // on, which is a different entity.
                    (true, view.state.drain_toasts(), cx.windows())
                });

                // The entity is gone, or the last window was closed.
                let Ok((alive, toasts, windows)) = updated else {
                    break;
                };
                if !alive || quit {
                    break;
                }
                if !toasts.is_empty() {
                    for window in windows {
                        let _ = cx.update_window(window, |_, window, cx| {
                            push_toasts(&toasts, window, cx);
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
        let mut received = false;
        while let Ok((path, result)) = self.open_results.try_recv() {
            match result {
                Ok(()) => self
                    .state
                    .push_notice(format!("Opened {}", path.display()), false),
                Err(reason) => self.state.push_notice(reason, true),
            }
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
        let path = match self.state.profile(id) {
            Some(profile) => profile.user_data_dir,
            None => {
                self.state
                    .push_notice(format!("profile {id} is no longer there"), true);
                cx.notify();
                return;
            }
        };
        let opener = Arc::clone(&self.opener);
        let sender = self.open_tx.clone();
        std::thread::spawn(move || {
            let result = opener.open(&path);
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
        let text: String = self
            .state
            .log_rows()
            .iter()
            .map(|row| format!("{} [{}] {}", row.who, row.level.label(), row.message))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Opens the editor for a profile and saves it through the dialog.
    fn on_edit(&mut self, id: ProfileId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.state.profile(id) else {
            self.state
                .push_notice(format!("profile {id} is no longer there"), true);
            cx.notify();
            return;
        };
        let cores = self.state.core_choices();
        let proxies = self.state.proxy_choices();
        let editor = cx.new(|cx| ProfileEditor::new(&profile, &cores, &proxies, window, cx));
        self.open_profile_form(editor, "Edit profile", "Save", window, cx);
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
                            DialogClose::new().trigger(|button| button.label("Cancel").outline()),
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
                            .unwrap_or_else(|_| Err("the window is gone".to_string())),
                        Ok(ProfileEdit::Save(profile)) => view
                            .update(cx, |view, _| match view.state.update_profile(profile) {
                                Ok(()) => {
                                    view.state.push_notice("Saved.", false);
                                    Ok(())
                                }
                                Err(error) => {
                                    let message = error.to_string();
                                    view.state.push_notice(message.clone(), true);
                                    Err(message)
                                }
                            })
                            .unwrap_or_else(|_| Err("the window is gone".to_string())),
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
        match self.state.duplicate_profile(id) {
            Ok(_) => self.state.push_notice("Copied the profile.", false),
            Err(error) => self.state.push_notice(error.to_string(), true),
        }
        cx.notify();
    }

    /// Deleting asks first: it removes the profile, not its sessions on disk.
    fn on_delete(&mut self, id: ProfileId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .state
            .profile(id)
            .map(|profile| profile.name)
            .unwrap_or_else(|| id.to_string());
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            alert
                .title("Delete profile")
                .description(format!(
                    "\"{name}\" will be removed from the list. Its browser data stays on disk.",
                ))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete")
                        .cancel_text("Keep")
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    match view.state.delete_profile(id) {
                                        Ok(()) => {
                                            view.state.push_notice("Deleted the profile.", false)
                                        }
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

    /// Asks for a new value for one editable setting.
    ///
    /// The dialog says when the value takes effect, because a setting that
    /// looks live and is not is the thing this page exists to avoid.
    fn on_edit_setting(&mut self, key: SettingKey, window: &mut Window, cx: &mut Context<Self>) {
        let row = self
            .state
            .setting_rows()
            .into_iter()
            .find(|row| row.key == key);
        let Some(row) = row else {
            self.state
                .push_notice(format!("{} is not a setting", key.label()), true);
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
                .title(format!(
                    "{} - takes effect at the {}",
                    key.label(),
                    key.effect()
                ))
                .w(px(680.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(format!("Now: {now}")),
                        )
                        .child(
                            // Not `setting-{key}`: the row card already
                            // owns that id, and two elements sharing one
                            // id in a single tree is ambiguous.
                            Input::new(&field)
                                .id(format!("setting-field-{}", key.id()))
                                .aria_label(key.label()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x71717a))
                                .child(key_help(key)),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label("Cancel").outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label("Save"))),
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
        let editing = id.and_then(|id| self.state.core(id));
        if id.is_some() && editing.is_none() {
            self.state
                .push_notice("that browser core is no longer there".to_string(), true);
            cx.notify();
            return;
        }
        let core_editor = cx.new(|cx| match &editing {
            Some(core) => CoreEditor::for_core(core, window, cx),
            None => CoreEditor::new(window, cx),
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
                            DialogClose::new().trigger(|button| button.label("Cancel").outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label("Save"))),
                )
                .on_ok(move |_, _, cx| {
                    let editing = core_editor.read(cx).is_edit();
                    let name = core_editor.read(cx).name(cx);
                    let path = core_editor.read(cx).executable(cx);
                    let built = if editing {
                        core_editor.read(cx).build_core(cx).map(Some)
                    } else if path.as_os_str().is_empty() {
                        Err("the executable path cannot be empty".to_string())
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
                            .unwrap_or_else(|_| Err("the window is gone".to_string())),
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
            format!("\"{name}\" ({version}) will be removed.")
        } else {
            format!(
                "\"{name}\" is used by {}. Point those profiles at another core first.",
                used_by.join(", ")
            )
        };
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let description = description.clone();
            alert
                .title("Delete browser core")
                .description(description)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete")
                        .cancel_text("Keep")
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
        let editing = id.and_then(|id| self.state.proxy(id));
        if id.is_some() && editing.is_none() {
            self.state
                .push_notice("that proxy is no longer there".to_string(), true);
            cx.notify();
            return;
        }
        let proxy_editor = cx.new(|cx| match &editing {
            Some(proxy) => ProxyEditor::for_proxy(proxy, window, cx),
            None => ProxyEditor::new(window, cx),
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
                            DialogClose::new().trigger(|button| button.label("Cancel").outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label("Save"))),
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
                            .unwrap_or_else(|_| Err("the window is gone".to_string()))
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
        let proxy_import = cx.new(|cx| ProxyImport::new(window, cx));
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
                            DialogClose::new().trigger(|button| button.label("Cancel").outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label("Import"))),
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
                            .unwrap_or_else(|_| Err("the window is gone".to_string())),
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
            format!("\"{name}\" ({endpoint}) will be removed.")
        } else {
            format!(
                "\"{name}\" is assigned to {}. Assign those profiles to another proxy or to Direct first.",
                used_by.join(", ")
            )
        };
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let description = description.clone();
            alert
                .title("Delete proxy")
                .description(description)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete")
                        .cancel_text("Keep")
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
        let cores = self.state.core_choices();
        let Some(core) = cores.first().map(|choice| choice.id) else {
            self.state.push_notice(
                "no browser core is registered yet; add one on the Browser Cores page \
                 before creating a profile",
                true,
            );
            cx.notify();
            return;
        };
        let proxies = self.state.proxy_choices();
        let name = self.state.next_profile_name();
        let editor =
            cx.new(|cx| ProfileEditor::new_profile(&name, core, &cores, &proxies, window, cx));
        self.open_profile_form(editor, "New profile", "Create", window, cx);
    }

    fn on_start(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        let _ = self.state.start(id);
        cx.notify();
    }

    fn on_stop(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        let _ = self.state.stop(id);
        cx.notify();
    }

    fn on_restart(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        let _ = self.state.restart(id);
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
        if let Some(input) = &self.filter_input {
            return input.clone();
        }

        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filter by name, seed, core or proxy")
        });
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
        if let Some(input) = &self.export_input {
            return input.clone();
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Leave empty for a new file under the data directory")
        });
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

    /// Builds the Settings page's import path field on first use, the way the
    /// export one is built.
    fn ensure_import_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = &self.import_input {
            return input.clone();
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Path of a configuration backup to read")
        });
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
    fn on_quit(&mut self, cx: &mut Context<Self>) {
        cx.quit();
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let export_destination = self.state.export_destination();
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
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .child(header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar(self.state.page(), cx))
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
                                Page::Proxies => proxies_header(cx),
                                Page::Cores => cores_header(cx),
                                Page::Log => logs_header(log_filter, &log_status, cx),
                                Page::Settings => settings_header(),
                                Page::Profiles => profiles_header(
                                    &ProfilesHeader {
                                        search: filter_input.clone(),
                                        filter: filter.clone(),
                                        total,
                                        visible: visible.len(),
                                    },
                                    cx,
                                ),
                            })
                            .children(notice.map(|notice| notice_banner(notice, cx)))
                            .when(page == Page::Proxies, |this| {
                                this.child(proxies_body(&proxy_rows, &proxy_tests, cx))
                            })
                            .when(page == Page::Cores, |this| {
                                this.child(cores_body(&core_rows, cx))
                            })
                            .when(page == Page::Log, |this| {
                                this.child(logs_body(&log_rows, log_count, log_filter, cx))
                            })
                            .when(page == Page::Settings, |this| {
                                this.child(settings_body(
                                    &setting_rows,
                                    SettingsExport {
                                        path: export_input.clone(),
                                        destination: export_destination.clone(),
                                        include_credentials: export_includes_credentials,
                                    },
                                    import_input.clone(),
                                    cx,
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
                                        ))
                                        .child(profile_list(
                                            &visible,
                                            selected_id,
                                            &verifications,
                                            cx,
                                        )),
                                )
                                .child(details_panel(
                                    selected.as_ref(),
                                    verification,
                                    details_tab,
                                    &log_tail,
                                    cx,
                                ))
                            })
                    }),
            )
            .children(dialogs)
            .children(sheets)
            .children(notifications)
    }
}

fn header(cx: &mut Context<AppView>) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px_6()
        .py_4()
        .border_b_1()
        .border_color(rgb(BORDER))
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
                        .text_color(rgb(DIM))
                        .child("Rust + GPUI Kit + Fingerprint-Chromium"),
                )
                .child(
                    Button::new("quit")
                        .label("Quit")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.on_quit(cx))),
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

fn sidebar(page: Page, cx: &mut Context<AppView>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w(px(220.0))
        .flex_shrink_0()
        .border_r_1()
        .border_color(rgb(BORDER))
        .p_4()
        .children(PAGES.map(|candidate| {
            let active = candidate == page;
            let label = if candidate.is_ready() {
                candidate.label().to_string()
            } else {
                format!("{} (soon)", candidate.label())
            };
            div()
                .id(format!("nav-{}", candidate.label()))
                .test_support()
                .px_3()
                .py_2()
                .rounded_md()
                .text_sm()
                .when(active, |this| {
                    this.bg(rgb(BORDER)).font_weight(FontWeight::MEDIUM)
                })
                .when(!active && candidate.is_ready(), |this| {
                    this.text_color(rgb(MUTED)).cursor_pointer()
                })
                .when(!candidate.is_ready(), |this| this.text_color(rgb(0x52525b)))
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
}

fn profiles_header(header: &ProfilesHeader, cx: &mut Context<AppView>) -> Div {
    // While a filter is on, the count is the useful sentence: it is how the user
    // finds out that the list is not the whole list. Without one, the header
    // goes back to explaining what a profile is. The accessible name spells out
    // both either way, because a status line is worth hearing in full.
    let filtering = header.total > 0 && !header.filter.trim().is_empty();
    let subtitle = if filtering {
        format!("Showing {} of {} profiles.", header.visible, header.total)
    } else {
        "Each profile owns its seed, data directory and browser process.".to_string()
    };
    let announcement = if filtering {
        format!(
            "Showing {} of {} profiles, filtered by \"{}\".",
            header.visible,
            header.total,
            header.filter.trim()
        )
    } else {
        format!("{} profiles.", header.total)
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
                                .child("Profiles"),
                        )
                        .child(
                            div()
                                .id("profile-count")
                                .test_support()
                                .aria_label(announcement)
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(subtitle),
                        ),
                )
                .child(
                    Button::new("new-profile")
                        .label("New Profile")
                        .primary()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_new_profile(window, cx)),
                        ),
                ),
        )
        .child(
            div().w(px(PROFILE_FILTER_WIDTH)).child(
                Input::new(&header.search)
                    .id("profile-filter")
                    .aria_label("Filter profiles")
                    .cleanable(true),
            ),
        )
}

fn proxies_header(cx: &mut Context<AppView>) -> Div {
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
                        .child("Proxies"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Assign a proxy to a profile to route its traffic through it."),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("import-proxy")
                        .label("Import from link")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_import_proxy(window, cx)),
                        ),
                )
                .child(
                    Button::new("new-proxy")
                        .label("New Proxy")
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
) -> impl IntoElement {
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
                    .bg(rgb(PANEL))
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(
                        "No proxies yet. A profile with no proxy goes direct from this machine.",
                    ),
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
                .border_color(rgb(BORDER))
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
                        .child(div().text_xs().text_color(rgb(MUTED)).child(row.endpoint()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(if row.is_used() { 0x86efac } else { 0x71717a }))
                                .child(row.usage_label()),
                        )
                        .children(proxy_test_reading(test, id)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new(format!("test-proxy-{index}"))
                                .label("Test")
                                .outline()
                                .disabled(test.is_some_and(ProxyTest::is_running))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.on_test_proxy(id, cx)),
                                ),
                        )
                        .child(
                            Button::new(format!("edit-proxy-{index}"))
                                .label("Edit")
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_edit_proxy(Some(id), window, cx)
                                })),
                        )
                        .child(
                            Button::new(format!("delete-proxy-{index}"))
                                .label("Delete")
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
fn proxy_test_reading(test: Option<&ProxyTest>, id: ProxyId) -> Option<AnyElement> {
    let test = test?;
    let colour = match test {
        ProxyTest::Running => DIM,
        ProxyTest::Passed(reading) if reading.live => 0x86efac,
        ProxyTest::Passed(_) => 0x7dd3fc,
        ProxyTest::Failed(_) => 0xfca5a5,
    };
    let text = match test {
        ProxyTest::Running => test.label(),
        ProxyTest::Passed(reading) => format!(
            "{} {}",
            test.label(),
            if reading.live {
                "through the engine a running profile is using"
            } else {
                "through a temporary engine"
            }
        ),
        // The evidence behind the class is in the activity log: a row is one
        // line, and an engine's own words are not.
        ProxyTest::Failed(_) => format!("{}; see the log", test.label()),
    };
    Some(
        div()
            .id(format!("proxy-test-{id}"))
            .test_support()
            .text_xs()
            .text_color(rgb(colour))
            .child(text)
            .into_any_element(),
    )
}

fn cores_header(cx: &mut Context<AppView>) -> Div {
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
                        .child("Browser Cores"),
                )
                .child(div().text_xs().text_color(rgb(MUTED)).child(
                    "Each core is a fingerprint-chromium binary; its detected version decides which switches a profile may claim.",
                )),
        )
        .child(
            Button::new("new-core")
                .label("Add Core")
                .primary()
                .on_click(cx.listener(|this, _, window, cx| this.on_edit_core(None, window, cx))),
        )
}

fn cores_body(rows: &[CoreRow], cx: &mut Context<AppView>) -> impl IntoElement {
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
                    .bg(rgb(PANEL))
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(
                        "No browser core yet. Add a fingerprint-chromium binary to launch profiles with it.",
                    ),
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
                .border_color(rgb(BORDER))
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
                                            .text_color(rgb(0xfca5a5))
                                            .child("executable missing"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(format!("{} · major {}", row.core.version, row.core.major)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(match row.generation_label() {
                                    Some(label) if label.contains("honoured") => 0x86efac,
                                    Some(_) => 0xfbbf24,
                                    None => 0xfca5a5,
                                }))
                                .child(row.generation_label().unwrap_or_else(|| {
                                    "no detected version: no switches can be claimed".to_string()
                                })),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x71717a))
                                .child(row.core.executable.to_string_lossy().to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(if row.is_used() { 0x86efac } else { 0x71717a }))
                                .child(row.usage_label()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new(format!("redetect-core-{index}"))
                                .label("Re-detect")
                                .outline()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.on_redetect_core(id, cx)
                                })),
                        )
                        .child(
                            Button::new(format!("edit-core-{index}"))
                                .label("Edit")
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_edit_core(Some(id), window, cx)
                                })),
                        )
                        .child(
                            Button::new(format!("delete-core-{index}"))
                                .label("Delete")
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_delete_core(id, window, cx)
                                })),
                        ),
                )
        }))
}

/// What a setting does, in one line, under its field.
fn key_help(key: SettingKey) -> String {
    match key {
        SettingKey::DataDir => {
            "Where profiles, cores and the database live. A new directory starts empty; \
             the current one keeps being used until the next start."
                .to_string()
        }
        SettingKey::XrayExecutable => {
            "Used when a profile has a proxy. The running process keeps the executable \
             it started with."
                .to_string()
        }
        _ => String::new(),
    }
}

fn settings_header() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Settings"),
        )
        .child(div().text_xs().text_color(rgb(MUTED)).child(
            "The value in force and where it came from. An environment variable wins over the config file, and the row says so.",
        ))
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

fn settings_body(
    rows: &[crate::settings::SettingRow],
    export: SettingsExport,
    import_input: Entity<InputState>,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    // Built first, with their lifetimes erased: each card borrows the context
    // and the chain below borrows it again for its own listeners, and an
    // opaque return type would keep the first borrow alive to the end of the
    // chain.
    let export_card: AnyElement = export_card(&export, cx).into_any_element();
    let import_card: AnyElement = import_card(&import_input, cx).into_any_element();
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
                .border_color(rgb(BORDER))
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
                                        .child(key.label()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(
                                            if row.source == crate::settings::Source::Environment {
                                                0xfbbf24
                                            } else {
                                                0x71717a
                                            },
                                        ))
                                        .child(row.source_label()),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xd4d4d8))
                                .child(row.value.clone()),
                        )
                        .children(
                            row.shadowed_label().map(|label| {
                                div().text_xs().text_color(rgb(0xfbbf24)).child(label)
                            }),
                        )
                        .children(
                            row.note
                                .clone()
                                .map(|note| div().text_xs().text_color(rgb(0x71717a)).child(note)),
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
                                .text_color(rgb(0x71717a))
                                .child(row.key.effect()),
                        )
                        .when(editable, |this| {
                            this.child(
                                Button::new(format!("edit-setting-{}", key.id()))
                                    .label("Change")
                                    .outline()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.on_edit_setting(key, window, cx)
                                    })),
                            )
                        }),
                )
        }))
        .child(export_card)
        .child(import_card)
}

/// The export card at the foot of the Settings page.
///
/// The path is typed rather than picked from a native file dialog. A chooser
/// would mean another dependency, and opening one is the part of a desktop
/// integration most likely to behave differently over RDP - which is a supported
/// way to run this. The field starts empty, and the line under it names the file
/// an empty field would write, so the default is shown rather than described.
fn export_card(export: &SettingsExport, cx: &mut Context<AppView>) -> impl IntoElement {
    let view = cx.entity().downgrade();
    div()
        .id("export-configuration")
        .test_support()
        .flex()
        .flex_col()
        .gap_3()
        .px_4()
        .py_4()
        .rounded_md()
        .border_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Export configuration"),
                )
                .child(div().text_xs().text_color(rgb(MUTED)).child(
                    "Writes cores, proxies and profiles to a JSON file, for another machine or for keeping. Browser data is not included: it is far larger, and a copy belongs beside the profile it came from.",
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&export.path)
                            .id("export-path")
                            .aria_label("Export file path"),
                    ),
                )
                .child(
                    Button::new("export-run")
                        .label("Export")
                        .on_click(cx.listener(|this, _, _, cx| this.on_export_configuration(cx))),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(DIM))
                .child(format!("Empty writes {}", export.destination.display())),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Checkbox::new("export-credentials")
                        .label("Include proxy credentials")
                        .checked(export.include_credentials)
                        .on_change(move |&checked, _, cx: &mut App| {
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    view.on_toggle_export_credentials(checked, cx)
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Left off, the passwords are dropped and the rest of each proxy is kept."),
                ),
        )
        .child(
            div().text_xs().text_color(rgb(0xfbbf24)).child(
                "A file that includes credentials holds them in plain text. Do not send it to anyone casually.",
            ),
        )
}

/// The import card under the export one.
///
/// The import is the quieter of the two verbs, and the card says why in one
/// line: nothing already here is overwritten. That is not a promise the
/// program can keep on its own - it is what the rules do - but it is the
/// sentence a reader needs before they type a path and press the button.
fn import_card(input: &Entity<InputState>, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .id("import-configuration")
        .test_support()
        .flex()
        .flex_col()
        .gap_3()
        .px_4()
        .py_4()
        .rounded_md()
        .border_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Import configuration"),
                )
                .child(div().text_xs().text_color(rgb(MUTED)).child(
                    "Reads a backup written by the export above. Nothing already here is overwritten: identifiers that exist are kept as they are, a profile whose core is not here is skipped, and the report says which was which.",
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(input)
                            .id("import-path")
                            .aria_label("Import file path"),
                    ),
                )
                .child(
                    Button::new("import-run")
                        .label("Import")
                        .on_click(cx.listener(|this, _, _, cx| this.on_import_configuration(cx))),
                ),
        )
        .child(
            div().text_xs().text_color(rgb(DIM)).child(
                "Nothing is confirmed first: import only adds, so undoing one is deleting the rows it named.",
            ),
        )
}

fn logs_header(
    filter: LogFilter,
    status: &Result<String, String>,
    cx: &mut Context<AppView>,
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
                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("Log"))
                .child(div().text_xs().text_color(rgb(MUTED)).child(
                    "What this window has done and seen: starts, stops, warnings, errors and reads, newest first.",
                ))
                .child(match status {
                    Ok(path) => div()
                        .id("log-file-status")
                        .test_support()
                        .aria_label(format!("Also written to {path}"))
                        .text_xs()
                        .text_color(rgb(DIM))
                        .child(format!("Also written to {path}")),
                    Err(error) => div()
                        .id("log-file-status")
                        .test_support()
                        .aria_label(format!("Not written to a file: {error}"))
                        .text_xs()
                        .text_color(rgb(0xfca5a5))
                        .child(format!("Not written to a file: {error}")),
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
                                .label(candidate.label())
                                .when(active, |button| button.primary())
                                .when(!active, |button| button.outline())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.on_set_log_filter(candidate, cx)
                                }))
                        })),
                )
                .child(
                    Button::new("copy-log")
                        .label("Copy")
                        .outline()
                        .on_click(cx.listener(|this, _, _, cx| this.on_copy_log(cx))),
                )
                .child(
                    Button::new("clear-log")
                        .label("Clear")
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
                0 => "Nothing has happened yet in this window.".to_string(),
                count => format!(
                    "No {} lines; {count} were recorded - switch the filter to All to see them",
                    filter.noun()
                ),
            };
            this.child(
                div()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(message),
            )
        })
        .children(rows.iter().enumerate().map(|(index, row)| {
            let label = format!("{} [{}] {}", row.who, row.level.label(), row.message);
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
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .w(px(64.0))
                        .flex_shrink_0()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(log_level_color(row.level)))
                        .child(row.level.label()),
                )
                .child(
                    div()
                        .w(px(56.0))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(rgb(DIM))
                        .child(format_age(row.at)),
                )
                .child(
                    div()
                        .w(px(140.0))
                        .flex_shrink_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(row.who.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(rgb(0xd4d4d8))
                        .child(row.message.clone()),
                )
        }))
}

fn log_level_color(level: LogLevel) -> u32 {
    match level {
        LogLevel::Info => MUTED,
        LogLevel::Warning => 0xfbbf24,
        LogLevel::Error => 0xf87171,
    }
}

/// How long ago a line was written, freshly computed on each render.
fn format_age(at: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(at)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    match seconds {
        0..=1 => "now".to_string(),
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

fn notice_banner(notice: crate::state::Notice, cx: &mut Context<AppView>) -> Div {
    let (background, foreground) = if notice.error {
        (0x2a1a1a, 0xfca5a5)
    } else {
        (PANEL, 0xa1a1aa)
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
                .label("Dismiss")
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
) -> Option<Div> {
    // Two kinds of empty look the same in a list and mean different things:
    // there are no profiles, or a filter is hiding the ones there are. Only the
    // second can be undone from here, so only it offers to.
    let filtering = total > 0 && visible == 0;
    if total > 0 && !filtering {
        return None;
    }

    let message = if filtering {
        format!(
            "No profile answers to \"{}\"; all {total} are hidden by it.",
            filter.trim()
        )
    } else if has_core {
        "No profiles yet. Create one to start a browser.".to_string()
    } else {
        "No browser core found. Set FP_BROWSER_CHROMIUM_BIN to a fingerprint-chromium \
         (or Chromium) executable and restart."
            .to_string()
    };

    Some(
        div()
            .p_4()
            .rounded_md()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(PANEL))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(div().text_xs().text_color(rgb(MUTED)).child(message))
            .when(filtering, |this| {
                this.child(
                    Button::new("clear-filter").label("Clear filter").on_click(
                        cx.listener(|this, _, window, cx| this.on_clear_filter(window, cx)),
                    ),
                )
            }),
    )
}

fn profile_list(
    rows: &[ProfileRow],
    selected_id: Option<ProfileId>,
    verifications: &std::collections::HashMap<ProfileId, Verification>,
    cx: &mut Context<AppView>,
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
                .border_color(rgb(if is_selected { DIM } else { BORDER }))
                .bg(rgb(if is_selected { BORDER } else { PANEL }))
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
                        .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                            "seed {} · {} · {}",
                            row.profile.fingerprint.seed,
                            row.profile.fingerprint.brand,
                            row.profile.fingerprint.platform,
                        )))
                        .child(div().text_xs().text_color(rgb(DIM)).child(route_label(row))),
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
                                .child(state_badge(row))
                                .children(verification_badge(verifications.get(&id)))
                                .children(row.last_warning().map(|warning| {
                                    // The full text lives in Runtime Details; the row
                                    // only needs to say that something is off.
                                    div()
                                        .id(format!("warning-{id}"))
                                        .test_support()
                                        .max_w(px(WARNING_WIDTH))
                                        .truncate()
                                        .text_xs()
                                        .text_color(rgb(0xfbbf24))
                                        .child(format!("warning: {warning}"))
                                }))
                                .children(row.last_error().map(|error| {
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0xf87171))
                                        .child(error.to_string())
                                })),
                        )
                        .child(row_actions(row, cx)),
                )
        }))
}

fn row_actions(row: &ProfileRow, cx: &mut Context<AppView>) -> Div {
    let id = row.profile.id;

    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            Button::new(format!("start-{id}"))
                .label("Start")
                .primary()
                .disabled(!row.can_start())
                .on_click(cx.listener(move |this, _, _, cx| this.on_start(id, cx))),
        )
        .child(
            Button::new(format!("stop-{id}"))
                .label("Stop")
                .danger()
                .disabled(!row.can_stop())
                .on_click(cx.listener(move |this, _, _, cx| this.on_stop(id, cx))),
        )
        .child(
            Button::new(format!("restart-{id}"))
                .label("Restart")
                .outline()
                .disabled(!row.can_restart())
                .on_click(cx.listener(move |this, _, _, cx| this.on_restart(id, cx))),
        )
}

fn route_label(row: &ProfileRow) -> String {
    match &row.proxy_name {
        Some(proxy) => format!("{} · proxy: {proxy}", row.core_name),
        None => format!("{} · direct", row.core_name),
    }
}

fn state_badge(row: &ProfileRow) -> impl IntoElement {
    let (background, foreground) = match row.state() {
        RuntimeState::Running => (0x14351f, 0x4ade80),
        RuntimeState::Starting | RuntimeState::Stopping => (0x3a2f12, 0xfbbf24),
        RuntimeState::Stopped => (BORDER, 0xa1a1aa),
        RuntimeState::Failed { .. } | RuntimeState::Crashed { .. } => (0x3a1717, 0xf87171),
    };

    div()
        .id(format!("state-{}", row.profile.id))
        .role(Role::Status)
        .test_support()
        .aria_label(row.state_label())
        .px_2()
        .py_1()
        .rounded_full()
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(row.state_label())
}

fn details_panel(
    selected: Option<&ProfileRow>,
    verification: Option<Verification>,
    tab: DetailsTab,
    log_tail: &[LogRow],
    cx: &mut Context<AppView>,
) -> Div {
    // The three views are different element types once an id makes them
    // stateful, so the panel erases them before choosing one.
    let body: AnyElement = match selected {
        None => div()
            .text_xs()
            .text_color(rgb(MUTED))
            .child("Select a profile to inspect its runtime.")
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
                ("Profile ID", row.profile.id.to_string()),
                (
                    "State",
                    match row.state_message() {
                        Some(message) => format!("{}: {message}", row.state_label()),
                        None => row.state_label().to_string(),
                    },
                ),
                ("Seed", row.profile.fingerprint.seed.to_string()),
                ("Brand", row.profile.fingerprint.brand.to_string()),
                ("Platform", row.profile.fingerprint.platform.to_string()),
                ("Language", row.profile.fingerprint.language.clone()),
                ("Timezone", row.profile.fingerprint.timezone.clone()),
                ("Core", row.core_name.clone()),
                (
                    "Proxy",
                    row.proxy_name
                        .clone()
                        .unwrap_or_else(|| "direct".to_string()),
                ),
                ("Data dir", row.profile.user_data_dir.display().to_string()),
                ("Browser PID", optional(row.browser_pid())),
                ("Xray PID", optional(row.xray_pid())),
                ("CDP port", optional(row.cdp_port())),
                ("SOCKS port", optional(row.socks_port())),
                ("Started", elapsed(row)),
                ("Dropped events", row.dropped_events().to_string()),
            ] {
                grid = grid.child(key_value(label, value));
            }

            let details = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(grid)
                .child(verification_block(verification))
                .children(row.last_warning().map(|warning| {
                    div()
                        .text_xs()
                        .text_color(rgb(0xfbbf24))
                        .child(format!("warning: {warning}"))
                }))
                .children(row.last_error().map(|error| {
                    div()
                        .text_xs()
                        .text_color(rgb(0xf87171))
                        .child(format!("error: {error}"))
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
                    .child(effective_args(row))
                    .into_any_element(),
                DetailsTab::Log => panel_log(log_tail).into_any_element(),
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
        .border_color(rgb(BORDER))
        .bg(rgb(PANEL))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Runtime Details"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("copy-args")
                                .label("Copy args")
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
                            .label("Open data dir")
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
                            .label("Verify fingerprint")
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
                            .label("Edit")
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
                            .label("Duplicate")
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
                            .label("Delete")
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
                        .label(candidate.label())
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
fn panel_log(rows: &[LogRow]) -> impl IntoElement {
    if rows.is_empty() {
        return div()
            .id("panel-log-body")
            .test_support()
            .text_xs()
            .text_color(rgb(DIM))
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
                .text_color(rgb(MUTED))
                .child(format!("This profile, newest first ({})", rows.len())),
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
                        .text_color(rgb(log_level_color(row.level)))
                        .child(row.level.label()),
                )
                .child(
                    div()
                        .w(px(48.0))
                        .flex_shrink_0()
                        .text_color(rgb(DIM))
                        .child(format_age(row.at)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(rgb(0xd4d4d8))
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
fn verification_block(verification: Option<Verification>) -> Div {
    let Some(verification) = verification else {
        return div().text_xs().text_color(rgb(DIM)).child(
            "Fingerprint not verified in this session. Verification reads the \
                 running browser in its own tab and compares it with the profile.",
        );
    };
    if verification.is_running() {
        return div()
            .text_xs()
            .text_color(rgb(MUTED))
            .child("Reading the fingerprint out of the running browser...");
    }
    if let Some(reason) = verification.failure() {
        return div()
            .text_xs()
            .text_color(rgb(0xf87171))
            .child(format!("Could not read the fingerprint: {reason}"));
    }
    let found = verification.disagreements();
    let headline = if found.is_empty() {
        div()
            .text_xs()
            .text_color(rgb(0x4ade80))
            .child("Confirmed: every claim this profile makes was read back from the browser.")
    } else {
        div().text_xs().text_color(rgb(0xfbbf24)).child(format!(
            "{} claim(s) the browser did not reproduce:",
            found.len()
        ))
    };

    let mut block = div().flex().flex_col().gap_1().child(headline);
    // The path is the half of the answer no other panel carries, and it is the
    // reason to trust a proxy at all: an address that matches the one the proxy
    // was tested at is worth seeing without having to open the log.
    if let Some(report) = verification.report()
        && let Some(label) = report.exit_label()
    {
        let colour = if report.exit_ip.is_some() {
            0x7dd3fc
        } else {
            DIM
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
            .text_color(rgb(0xfbbf24))
            .child(format!(
                "  {}: expected {}, observed {}",
                discrepancy.claim, discrepancy.expected, discrepancy.observed
            ))
    }))
}

/// A compact marker for the row: the user should not have to select a profile
/// to know whether its fingerprint was confirmed.
fn verification_badge(verification: Option<&Verification>) -> Option<impl IntoElement> {
    let verification = verification?;
    let (background, foreground) = match verification {
        Verification::Confirmed(_) => (0x14351f, 0x4ade80),
        Verification::Running => (BORDER, 0xa1a1aa),
        Verification::Disagreements(_) => (0x3a2f12, 0xfbbf24),
        Verification::Unreadable(_) => (0x3a1717, 0xf87171),
    };
    Some(
        div()
            .id(format!("verification-{}", verification.label()))
            .px_2()
            .py_1()
            .rounded_full()
            .bg(rgb(background))
            .text_color(rgb(foreground))
            .text_xs()
            .child(verification.label()),
    )
}

fn effective_args(row: &ProfileRow) -> Div {
    let args = row.effective_args();
    if args.is_empty() {
        return div()
            .text_xs()
            .text_color(rgb(DIM))
            .child("No launch recorded yet.");
    }

    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(format!("Effective args ({})", args.len())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(rgb(BG))
                .text_xs()
                .text_color(rgb(0xa1a1aa))
                .children(args.iter().map(|arg| div().child(arg.clone()))),
        )
}

fn key_value(label: &str, value: String) -> Div {
    div()
        .flex()
        .gap_2()
        .text_xs()
        .child(
            div()
                .w(px(96.0))
                .flex_shrink_0()
                .text_color(rgb(MUTED))
                .child(label.to_string()),
        )
        .child(div().text_color(rgb(0xd4d4d8)).child(value))
}

fn optional<T: ToString>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".to_string())
}

fn elapsed(row: &ProfileRow) -> String {
    let started = row
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.started_at);
    match started.and_then(|started| SystemTime::now().duration_since(started).ok()) {
        Some(elapsed) => format!("{}s ago", elapsed.as_secs()),
        None => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::AppView;
    use crate::open_dir::testing::FakeOpener;
    use crate::proxy_tester::testing::FakeProxyTester;
    use crate::state::AppState;
    use crate::state::Verification;
    use crate::state::testing::{FakeRuntime, core};
    use crate::verifier::testing::FakeVerifier;
    use application::{DefaultProfileService, DefaultProxyService, ProxyService, RuntimeService};
    use domain::{CoreId, ProfileId, ProxyId};
    use gpui_kit::component::Root;
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext, px, size};
    use runtime::{Diagnosis, Discrepancy, Fault, FaultClass, RuntimeEvent};
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
        let (view, runtime, opener, _tester) = view_with_tester(
            cx,
            verifier,
            Arc::new(FakeProxyTester::passing()),
            config,
            opener,
            log_file,
        );
        (view, runtime, opener)
    }

    /// The same view, with the proxy tester the test drives.
    fn view_with_tester(
        cx: &mut TestAppContext,
        verifier: Arc<FakeVerifier>,
        tester: Arc<FakeProxyTester>,
        config: Option<&std::path::Path>,
        opener: Arc<FakeOpener>,
        log_file: Option<crate::log_file::LogFile>,
    ) -> (
        gpui_kit::Entity<AppView>,
        Arc<FakeRuntime>,
        Arc<FakeOpener>,
        Arc<FakeProxyTester>,
    ) {
        let profile_repo: Arc<MemProfileRepository> = Arc::new(MemProfileRepository::new());
        let core_repo: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let proxy_repo: Arc<MemProxyRepository> = Arc::new(MemProxyRepository::new());
        core_repo
            .save(&core(CoreId::new()))
            .expect("seed a browser core");

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
            profiles,
            runtime_service,
            cores,
            proxies,
            settings,
            log_file,
            None,
        );
        let (_command_tx, event_rx) = crossbeam_channel::bounded(16);

        let view = cx.new(|cx| {
            let mut view = AppView::new(state, event_rx, verifier, tester.clone(), opener.clone());
            view.boot(cx);
            view
        });
        (view, runtime, opener, tester)
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
        let (view, _runtime, _opener, tester) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            Arc::new(FakeProxyTester::passing_from("198.51.100.9")),
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
        let (view, _runtime, _opener, tester) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            Arc::new(FakeProxyTester::with_outcome(Err(Fault::new(
                FaultClass::Unreachable,
                "no route to the upstream",
            )))),
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
        assert_eq!(test.label(), "no traffic (unreachable)");
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
        let (view, _runtime, _opener, tester) = view_with_tester(
            cx,
            Arc::new(FakeVerifier::passing()),
            Arc::new(FakeProxyTester::passing()),
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
            state.begin_proxy_test(proxy_id).expect("begin");
            state.finish_proxy_test(
                proxy_id,
                false,
                Ok(Diagnosis {
                    exit_ip: "198.51.100.9".to_string(),
                    elapsed: Duration::from_millis(80),
                }),
            );
            let mut profile = state.profile(id).expect("the profile");
            profile.proxy_id = Some(proxy_id);
            state.update_profile(profile).expect("assign");
            cx.notify();
        });
        cx.update(|window, cx| window.click(format!("start-{id}"), cx));
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
            Some("198.51.100.9"),
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
            assert_eq!(verification.label(), "1 claim not confirmed");
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

        cx.update(|window, cx| window.click("nav-Browser Cores", cx));
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

        cx.update(|window, cx| window.click("nav-Browser Cores", cx));
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

    #[gpui_kit::test]
    fn a_refused_core_form_says_why_and_stays_open(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, _runtime) = view(cx);
        let cx = window(cx, &view);
        // A file that answers nothing when asked for --version.
        let binary = crate::state::testing::CoreBinary::new("ui-silent", None);
        let path = binary.path_buf();

        cx.update(|window, cx| window.click("nav-Browser Cores", cx));
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
        cx.update(|window, cx| window.click("nav-Browser Cores", cx));
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

        cx.update(|window, cx| window.click("nav-Browser Cores", cx));
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

        for key in ["data-dir", "xray-executable"] {
            assert!(
                cx.update(|window, _| window.try_find(format!("edit-setting-{key}")).is_some()),
                "{key} can be changed"
            );
        }
        for key in [
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
        cx.update(|window, cx| window.click("edit-setting-data-dir", cx));
        settle(cx);
        assert!(
            cx.update(|window, _| window.try_find("setting-field-data-dir").is_some()),
            "the field is open"
        );
        assert!(
            cx.update(|window, _| window.try_find("setting-data-dir").is_some()),
            "the row it belongs to is still there"
        );

        let field = view
            .read_with(cx, |view, _| view.setting_editor())
            .expect("the settings field");
        cx.update(|window, cx| {
            field.update(cx, |state, cx| state.set_value("/srv/fp", window, cx));
        });
        cx.update(|window, cx| window.click("ok", cx));
        settle(cx);

        let stored = std::fs::read_to_string(&config).expect("the config file was written");
        assert!(stored.contains("/srv/fp"), "{stored}");
        let row = view.read_with(cx, |view, _| {
            view.state()
                .setting_rows()
                .into_iter()
                .find(|row| row.key == crate::settings::SettingKey::DataDir)
                .expect("the row")
        });
        assert_eq!(row.value, "/srv/fp");
        assert_eq!(row.source, crate::settings::Source::ConfigFile);
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
        cx.update(|window, cx| window.click("edit-setting-data-dir", cx));
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
    fn starting_without_a_core_surfaces_the_error_in_the_banner(cx: &mut TestAppContext) {
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
        let proxies: Arc<dyn ProxyService> =
            Arc::new(DefaultProxyService::new(proxy_repo, profile_repo));
        let settings = crate::state::testing::settings();
        let state = AppState::for_test(profiles, runtime_service, cores, proxies, settings);
        let (_command_tx, event_rx) = crossbeam_channel::bounded(16);
        let view = cx.new(|cx| {
            let mut view = AppView::new(
                state,
                event_rx,
                Arc::new(FakeVerifier::passing()),
                Arc::new(FakeProxyTester::passing()),
                Arc::new(FakeOpener::working()),
            );
            view.boot(cx);
            view
        });

        let handle = cx.open_window(size(px(900.), px(600.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("new-profile", cx);

            let notice = view.read_with(cx, |view, _| {
                view.state()
                    .notice()
                    .expect("recorded notice")
                    .message
                    .clone()
            });
            assert!(notice.contains("no browser core"));
        })
        .unwrap();
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

    /// Brings the import card into view.
    ///
    /// The Settings page scrolls and the import card sits under the export
    /// one, below the fold of the test window; a click on an off-screen
    /// element is refused. The wheel event is dispatched over the export card
    /// (the scrolling container is not a leaf the helpers can aim at) and
    /// bubbles up to the page that scrolls.
    fn scroll_settings_to_the_import_card(cx: &mut gpui_kit::VisualTestContext) {
        cx.update(|window, cx| {
            window.scroll(
                "export-run",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(-4000.),
                )),
                cx,
            );
        });
        settle(cx);
    }

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
        scroll_settings_to_the_import_card(cx);
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
        scroll_settings_to_the_import_card(cx);
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
}
