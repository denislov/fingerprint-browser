//! The GPUI window: renders [`AppState`] and forwards user actions into it.
//!
//! The view owns no runtime state. Notifications only mark the snapshot cache
//! dirty; the snapshot itself stays the single source of truth, exactly as the
//! runtime façade contract requires.

use crate::state::{AppState, ProfileRow};
use crossbeam_channel::Receiver;
use domain::{ProfileId, RuntimeState};
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use runtime::RuntimeEvent;
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

pub struct AppView {
    state: AppState,
    events: Receiver<RuntimeEvent>,
    /// Kept alive: dropping a GPUI subscription unregisters the observer.
    window_closed: Option<Subscription>,
}

impl AppView {
    pub fn new(state: AppState, events: Receiver<RuntimeEvent>) -> Self {
        Self {
            state,
            events,
            window_closed: None,
        }
    }

    /// Read-only access for tests and diagnostics.
    #[cfg(test)]
    pub fn state(&self) -> &AppState {
        &self.state
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
                        return;
                    }

                    let notified = view.drain_events();
                    if notified || reconcile {
                        view.state.refresh_runtime();
                        cx.notify();
                    }
                });

                // The entity is gone, or the last window was closed.
                if updated.is_err() || quit {
                    break;
                }
            }
        })
        .detach();
    }

    /// Drain queued notifications. Events are never replayed as state.
    fn drain_events(&self) -> bool {
        let mut notified = false;
        while self.events.try_recv().is_ok() {
            notified = true;
        }
        notified
    }

    fn on_new_profile(&mut self, cx: &mut Context<Self>) {
        let name = self.state.next_profile_name();
        let _ = self.state.create_profile(&name);
        cx.notify();
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.state.rows().to_vec();
        let selected = self.state.selected().cloned();
        let selected_id = self.state.selected_id();
        let notice = self.state.notice().cloned();
        let has_core = self.state.has_core();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .child(header(cx))
            .child(
                div().flex().flex_1().min_h_0().child(sidebar()).child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .p_6()
                        .gap_4()
                        .child(profiles_header(cx))
                        .children(notice.map(|notice| notice_banner(notice, cx)))
                        .child(
                            div()
                                .id("profiles-scroll")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h_0()
                                .gap_2()
                                .overflow_y_scroll()
                                .children(empty_hint(&rows, has_core))
                                .child(profile_list(&rows, selected_id, cx)),
                        )
                        .child(details_panel(selected.as_ref(), cx)),
                ),
            )
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

fn sidebar() -> Div {
    let items = [
        ("Profiles", true),
        ("Proxies", false),
        ("Browser Cores", false),
        ("Settings", false),
    ];

    div()
        .flex()
        .flex_col()
        .gap_2()
        .w(px(220.0))
        .flex_shrink_0()
        .border_r_1()
        .border_color(rgb(BORDER))
        .p_4()
        .children(items.map(|(label, active)| {
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .text_sm()
                .when(active, |this| {
                    this.bg(rgb(BORDER)).font_weight(FontWeight::MEDIUM)
                })
                .when(!active, |this| this.text_color(rgb(MUTED)))
                .child(if active {
                    label.to_string()
                } else {
                    format!("{label} (soon)")
                })
        }))
}

fn profiles_header(cx: &mut Context<AppView>) -> Div {
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
                        .child("Profiles"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Each profile owns its seed, data directory and browser process."),
                ),
        )
        .child(
            Button::new("new-profile")
                .label("New Profile")
                .primary()
                .on_click(cx.listener(|this, _, _, cx| this.on_new_profile(cx))),
        )
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

fn empty_hint(rows: &[ProfileRow], has_core: bool) -> Option<Div> {
    if !rows.is_empty() {
        return None;
    }

    let message = if has_core {
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
            .text_xs()
            .text_color(rgb(MUTED))
            .child(message),
    )
}

fn profile_list(
    rows: &[ProfileRow],
    selected_id: Option<ProfileId>,
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

fn details_panel(selected: Option<&ProfileRow>, cx: &mut Context<AppView>) -> Div {
    let body = match selected {
        None => div()
            .text_xs()
            .text_color(rgb(MUTED))
            .child("Select a profile to inspect its runtime."),
        Some(row) => {
            let mut grid = div().flex().flex_wrap().gap_x_6().gap_y_2();
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

            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(grid)
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
                }))
                .child(effective_args(row))
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
                    Button::new("copy-args")
                        .label("Copy args")
                        .outline()
                        .disabled(
                            selected
                                .map(|row| row.effective_args().is_empty())
                                .unwrap_or(true),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.on_copy_args(cx))),
                ),
        )
        .child(
            div()
                .id("details-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
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
    use crate::state::AppState;
    use crate::state::testing::{FakeRuntime, core};
    use application::{DefaultProfileService, RuntimeService};
    use domain::CoreId;
    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext, px, size};
    use std::path::PathBuf;
    use std::sync::Arc;
    use storage::{
        CoreRepository as _, MemCoreRepository, MemProfileRepository, MemProxyRepository,
    };

    /// Builds the real view over in-memory storage and a synchronous façade.
    fn view(cx: &mut TestAppContext) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
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
            profile_repo,
            core_repo.clone(),
            proxy_repo.clone(),
            runtime.clone(),
        ));
        let state = AppState::new(profiles, runtime_service, core_repo, proxy_repo);
        let (_command_tx, event_rx) = crossbeam_channel::bounded(16);

        let view = cx.new(|cx| {
            let mut view = AppView::new(state, event_rx);
            view.boot(cx);
            view
        });
        (view, runtime)
    }

    #[gpui_kit::test]
    fn profiles_can_be_created_started_and_stopped_from_the_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, runtime) = view(cx);

        let handle = cx.open_window(size(px(1200.), px(800.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("quit").is_some(),
                "the header renders a quit affordance"
            );
            assert_eq!(view.read_with(cx, |view, _| view.state().rows().len()), 0);

            window.click("new-profile", cx);
            window.click("new-profile", cx);

            let ids = view.read_with(cx, |view, _| {
                view.state()
                    .rows()
                    .iter()
                    .map(|row| row.profile.id)
                    .collect::<Vec<_>>()
            });
            assert_eq!(ids.len(), 2, "two profiles after two clicks");

            let (first, second) = (ids[0], ids[1]);
            assert_eq!(
                window.find(format!("state-{first}")).label(),
                Some("Stopped")
            );
            assert_eq!(
                window.find(format!("state-{second}")).label(),
                Some("Stopped")
            );

            window.click(format!("start-{first}"), cx);
            assert_eq!(
                window.find(format!("state-{first}")).label(),
                Some("Running")
            );
            assert_eq!(
                window.find(format!("state-{second}")).label(),
                Some("Stopped"),
                "starting one profile leaves the other alone"
            );

            window.click(format!("stop-{first}"), cx);
            assert_eq!(
                window.find(format!("state-{first}")).label(),
                Some("Stopped")
            );
        })
        .unwrap();

        let commands = runtime.commands.lock().expect("command log").clone();
        assert_eq!(
            commands.len(),
            2,
            "one start and one stop reached the façade"
        );
        assert!(commands[0].starts_with("start:"));
        assert!(commands[1].starts_with("stop:"));
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
            profile_repo,
            core_repo.clone(),
            proxy_repo.clone(),
            runtime,
        ));
        let state = AppState::new(profiles, runtime_service, core_repo, proxy_repo);
        let (_command_tx, event_rx) = crossbeam_channel::bounded(16);
        let view = cx.new(|cx| {
            let mut view = AppView::new(state, event_rx);
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
}
