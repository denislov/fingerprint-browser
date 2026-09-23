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
use crate::tray::{Tray, TrayEvent, TrayStarter};
use crate::verifier::{FingerprintVerifier, VerificationReport};
use crate::version;
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
/// Wide enough for a profile name or a proxy name without crowding the title.
const PROFILE_FILTER_WIDTH: f32 = 320.0;

/// The window width at which the details panel can sit beside the list.
///
/// The sum is the sidebar, the page's padding on both sides, the panel, the gap
/// between the two columns and the list: 200 + 48 + 380 + 16 + 720. Below it the
/// panel covers the list instead, because a list narrower than that cannot show
/// four columns of anything.
const DETAILS_DOCK_MIN: f32 = 1364.0;

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
    verifications: Receiver<(
        crate::verifier::VerificationJob,
        Result<VerificationReport, String>,
    )>,
    verification_tx: Sender<(
        crate::verifier::VerificationJob,
        Result<VerificationReport, String>,
    )>,
    /// Finished proxy tests, with whether the engine probed was already up.
    proxy_tests: Receiver<(crate::proxy_tester::ProxyTestJob, Result<Diagnosis, Fault>)>,
    proxy_test_tx: Sender<(crate::proxy_tester::ProxyTestJob, Result<Diagnosis, Fault>)>,
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
    /// How that icon is started. [`crate::tray::Tray::start`] in the program; a
    /// test hands in a desktop that refuses one, which is the case the window
    /// has to survive.
    tray_starter: TrayStarter,
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
        tray_starter: TrayStarter,
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
            tray_starter,
            close_hook: false,
            exit_remember: false,
            filter_changed: None,
        }
    }

    /// Replaces the way this window starts its tray icon.
    ///
    /// For the one test that needs a desktop which refuses the icon: that is the
    /// case where hiding the window would leave the user with no way back, and
    /// the behaviour worth pinning is what the window does instead.
    #[cfg(test)]
    pub fn set_tray_starter(&mut self, starter: TrayStarter) {
        self.tray_starter = starter;
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

    fn on_dismiss_notice(&mut self, cx: &mut Context<Self>) {
        self.state.dismiss_notice();
        cx.notify();
    }
}

pub(crate) mod components;
mod cores;
mod details;
pub(crate) mod icons;
mod lifecycle;
mod logs;
mod profiles;
mod proxies;
mod settings;
mod workers;

use self::cores::{cores_body, cores_header};
use self::details::details_panel;
use self::logs::{format_age, logs_body, logs_header};
use self::profiles::{empty_hint, list_header, profile_list, profiles_header};
use self::proxies::{proxies_body, proxies_header};
use self::settings::{SettingsCards, SettingsExport, exit_choice, settings_body, settings_header};
use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};

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
        // Where the details panel goes: beside the list when the window has
        // room for both, over it when a readable list and a readable panel
        // cannot both fit. Measured on every frame, so resizing the window moves
        // the panel rather than requiring a restart.
        let docked = window.viewport_size().width.as_f32() >= DETAILS_DOCK_MIN;
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
                            .p(px(components::PAGE_PADDING))
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
                                let list = div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .gap_2()
                                    .when(!visible.is_empty(), |this| this.child(list_header(p, t)))
                                    .child(
                                        div()
                                            .id("profiles-scroll")
                                            .test_support()
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
                                    );
                                // The panel answers for the profile the window
                                // has chosen, and is on screen only while it is
                                // being read: a panel fixed under the list cost
                                // the list a third of the window on every frame.
                                let panel = self.state.details_open().then(|| {
                                    details_panel(
                                        selected.as_ref(),
                                        verification,
                                        details_tab,
                                        &log_tail,
                                        docked,
                                        cx,
                                        p,
                                        t,
                                    )
                                });
                                this.child(profiles_workspace(list, panel, docked))
                            })
                    }),
            )
            .children(dialogs)
            .children(sheets)
            .children(notifications)
    }
}

/// The list, and the details panel if it is open.
///
/// Two shapes, one panel. Docked, the panel is a column beside the list and both
/// stay readable; covering, it takes the list's place and carries the button
/// that gives it back. Which one a window gets is [`DETAILS_DOCK_MIN`], decided
/// from the window's own width rather than from anything the user has to set.
fn profiles_workspace(
    list: impl IntoElement,
    panel: Option<impl IntoElement>,
    docked: bool,
) -> Div {
    if docked {
        let mut row = div().flex().flex_1().min_h_0().gap_4().child(list);
        if let Some(panel) = panel {
            row = row.child(panel);
        }
        row
    } else {
        let mut area = div().relative().flex().flex_1().min_h_0().child(list);
        if let Some(panel) = panel {
            area = area.child(div().absolute().top_0().left_0().size_full().child(panel));
        }
        area
    }
}

/// The pages that sit under the brand, in the order the sidebar lists them.
///
/// Settings is not one of them: it is about the program rather than about what
/// the program manages, so it is pinned to the foot of the sidebar where a
/// settings entry belongs.
const WORK_PAGES: [Page; 4] = [Page::Profiles, Page::Proxies, Page::Cores, Page::Log];

/// What a page looks like in the navigation.
fn nav_glyph(page: Page) -> IconName {
    match page {
        Page::Profiles => icons::glyph::NAV_PROFILES,
        Page::Proxies => icons::glyph::NAV_PROXIES,
        Page::Cores => icons::glyph::NAV_CORES,
        Page::Log => icons::glyph::NAV_LOG,
        Page::Settings => icons::glyph::NAV_SETTINGS,
    }
}

/// The window's navigation: the brand, the pages, and the way out.
///
/// The sidebar is the whole of the window's chrome. There is no second header
/// above the page repeating the product name, and no exit button beside it: the
/// window's own close control is the exit affordance, the brand's menu carries
/// the low-frequency variants of it, and the page below gets the height that the
/// removed header used to take.
fn sidebar(page: Page, cx: &mut Context<AppView>, t: &'static Text) -> Div {
    let p = palette(cx);
    // Built with a loop rather than a `map`: a navigation item borrows the
    // context it listens on, and a closure that captured the context mutably
    // could not hand one back.
    let mut work = div().flex().flex_col().gap_1().flex_1().min_h_0().px_3();
    for candidate in WORK_PAGES {
        work = work.child(nav_item(candidate, page, cx, t));
    }

    div()
        .flex()
        .flex_col()
        .w(px(components::SIDEBAR))
        .flex_shrink_0()
        .border_r_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .child(brand())
        .child(work)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .child(nav_item(Page::Settings, page, cx, t)),
        )
        .child(sidebar_footer(cx, t))
}

/// The product's mark and name, at the head of the sidebar.
///
/// The mark is the same SVG the packaging scripts turn into the window and tray
/// icons, so the window cannot end up wearing two different logos.
fn brand() -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_4()
        .child(img(icons::BRAND).w(px(24.0)).h(px(24.0)).flex_shrink_0())
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(version::NAME),
        )
}

/// One page in the navigation: an icon, the name, and the state of the choice.
fn nav_item(candidate: Page, page: Page, cx: &mut Context<AppView>, t: &Text) -> impl IntoElement {
    let p = palette(cx);
    let active = candidate == page;
    let ready = candidate.is_ready();
    let label = if ready {
        candidate.label(t).to_string()
    } else {
        t.nav_soon(candidate.label(t))
    };
    div()
        .id(format!("nav-{}", candidate.id()))
        .test_support()
        .aria_label(label.clone())
        .flex()
        .items_center()
        .gap_2()
        .h(px(36.0))
        .px_3()
        .rounded(px(components::RADIUS_CONTROL))
        .text_sm()
        .when(active, |this| {
            this.bg(rgb(p.selected))
                .text_color(rgb(p.accent))
                .font_weight(FontWeight::MEDIUM)
        })
        // The icon follows the label's colour rather than carrying one of its
        // own: one blue icon per row would make the sidebar a colour chart.
        .when(!active && ready, |this| {
            this.text_color(rgb(p.secondary))
                .cursor_pointer()
                .hover(|this| this.bg(rgb(p.hover)))
        })
        .when(!ready, |this| this.text_color(rgb(p.dim)))
        .child(icons::nav(nav_glyph(candidate)))
        .child(label)
        .when(ready, |this| {
            this.on_click(cx.listener(move |this, _, _, cx| this.on_page(candidate, cx)))
        })
}

/// The foot of the sidebar: the version, and the menu that is not worth a row.
///
/// What lives here is everything that used to be in the removed header and
/// everything too rare to deserve a page: about, closing the window through the
/// saved exit policy, and stopping everything.
fn sidebar_footer(cx: &mut Context<AppView>, t: &'static Text) -> Div {
    let p = palette(cx);
    let view = cx.entity().downgrade();
    let about = view.clone();
    let close = view.clone();
    let stop_all = view;
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .px_3()
        .py_3()
        .border_t_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.dim))
                .child(format!("v{}", version::VERSION)),
        )
        .child(
            components::icon_button("brand-more", icons::glyph::MORE, t.menu_more).dropdown_menu(
                move |menu, _window, _cx| {
                    menu.item(
                        PopupMenuItem::new(t.about_title)
                            .icon(icons::action(icons::glyph::ABOUT))
                            .on_click({
                                let view = about.clone();
                                move |_, _window, cx| {
                                    if let Some(view) = view.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.on_page(Page::Settings, cx)
                                        });
                                    }
                                }
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new(t.menu_close_window)
                            .icon(icons::action(icons::glyph::CLOSE_WINDOW))
                            .on_click({
                                let view = close.clone();
                                move |_, window, cx| {
                                    if let Some(view) = view.upgrade() {
                                        view.update(cx, |view, cx| view.on_quit(window, cx));
                                    }
                                }
                            }),
                    )
                    .item(
                        PopupMenuItem::new(t.menu_stop_all_and_quit)
                            .icon(icons::action(icons::glyph::QUIT_ALL))
                            .on_click({
                                let view = stop_all.clone();
                                move |_, window, cx| {
                                    if let Some(view) = view.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.leave(Exit::ExitAll, window, cx)
                                        });
                                    }
                                }
                            }),
                    )
                },
            ),
        )
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
