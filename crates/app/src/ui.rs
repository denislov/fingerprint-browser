//! The GPUI window: renders [`AppState`] and forwards user actions into it.
//!
//! The view owns no runtime state. Notifications only mark the snapshot cache
//! dirty; the snapshot itself stays the single source of truth, exactly as the
//! runtime façade contract requires.

use crate::state::{AppState, ProfileRow, Verification};
use crate::verifier::FingerprintVerifier;
use crossbeam_channel::{Receiver, Sender};
use domain::{ProfileId, RuntimeState};
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use runtime::{Discrepancy, RuntimeEvent};
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

pub struct AppView {
    verifier: Arc<dyn FingerprintVerifier>,
    verifications: Receiver<(ProfileId, Result<Vec<Discrepancy>, String>)>,
    verification_tx: Sender<(ProfileId, Result<Vec<Discrepancy>, String>)>,
    state: AppState,
    events: Receiver<RuntimeEvent>,
    /// Kept alive: dropping a GPUI subscription unregisters the observer.
    window_closed: Option<Subscription>,
}

impl AppView {
    pub fn new(
        state: AppState,
        events: Receiver<RuntimeEvent>,
        verifier: Arc<dyn FingerprintVerifier>,
    ) -> Self {
        // A reading takes seconds and blocks on the browser, so it runs on a
        // worker thread and reports back through this channel.
        let (verification_tx, verifications) = crossbeam_channel::unbounded();
        Self {
            verifier,
            verifications,
            verification_tx,
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

    /// Mutable access for tests that drive reconciliation directly.
    #[cfg(test)]
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
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
                    let verified = view.drain_verifications();
                    if notified || reconcile || verified {
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
            let outcome = verifier.verify(job.port, &job.profile, &job.capabilities);
            let _ = sender.send((job.profile_id, outcome));
        });
        cx.notify();
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
                                .child(profile_list(&rows, selected_id, &verifications, cx)),
                        )
                        .child(details_panel(selected.as_ref(), verification, cx)),
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
    cx: &mut Context<AppView>,
) -> Div {
    let body = match selected {
        None => div()
            .text_xs()
            .text_color(rgb(MUTED))
            .child("Select a profile to inspect its runtime."),
        Some(row) => {
            let mut grid = div().flex().flex_wrap().gap_x_6().gap_y_2();
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

            // The panel scrolls (see the container below), so a long list of
            // findings stays reachable instead of being clipped.
            div()
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
                        ),
                ),
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
    if found.is_empty() {
        return div()
            .text_xs()
            .text_color(rgb(0x4ade80))
            .child("Confirmed: every claim this profile makes was read back from the browser.");
    }
    // The panel scrolls, so a long list of findings stays reachable instead of
    // being clipped to the first few.
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(rgb(0xfbbf24)).child(format!(
            "{} claim(s) the browser did not reproduce:",
            found.len()
        )))
        .children(found.iter().enumerate().map(|(index, discrepancy)| {
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
        Verification::Confirmed => (0x14351f, 0x4ade80),
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
    use crate::state::AppState;
    use crate::state::Verification;
    use crate::state::testing::{FakeRuntime, core};
    use crate::verifier::testing::FakeVerifier;
    use application::{DefaultProfileService, RuntimeService};
    use domain::CoreId;
    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, TestAppContext, px, size};
    use runtime::Discrepancy;
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
            let mut view = AppView::new(state, event_rx, verifier);
            view.boot(cx);
            view
        });
        (view, runtime)
    }

    fn view(cx: &mut TestAppContext) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
        view_with_verifier(cx, Arc::new(FakeVerifier::passing()))
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
    fn a_long_compatibility_warning_stays_inside_the_row(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, runtime) = view(cx);

        let handle = cx.open_window(size(px(1200.), px(800.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("new-profile", cx);
            let id = view.read_with(cx, |view, _| view.state().rows()[0].profile.id);

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

    /// Drives the verification channel until the worker reports, so the test
    /// never depends on the tick timer firing.
    fn wait_for_verification(cx: &mut gpui_kit::App, view: &gpui_kit::Entity<AppView>) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let done = view.read_with(cx, |view, _| {
                view.state
                    .selected()
                    .and_then(|row| view.state.verification(row.profile.id))
                    .is_some_and(|verification| !verification.is_running())
            });
            if done {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the verification never reported back"
            );
            view.update(cx, |view, cx| {
                view.drain_verifications();
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
            window.click("new-profile", cx);
            let id = view.read_with(cx, |view, _| view.state().rows()[0].profile.id);

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
            assert_eq!(verification, Verification::Confirmed);
            assert_eq!(verifier.calls(), 1);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn disagreements_are_listed_claim_by_claim(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let verifier = Arc::new(FakeVerifier::with_outcome(Ok(vec![Discrepancy {
            claim: "platform",
            expected: "Win32".to_string(),
            observed: "Linux x86_64".to_string(),
        }])));
        let (view, runtime) = view_with_verifier(cx, verifier);

        let handle = cx.open_window(size(px(1200.), px(900.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("new-profile", cx);
            let id = view.read_with(cx, |view, _| view.state().rows()[0].profile.id);
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
        let (view, runtime) =
            view_with_verifier(cx, Arc::new(FakeVerifier::with_outcome(Ok(found))));

        let handle = cx.open_window(size(px(1200.), px(700.)), |window, cx| {
            Root::new(view.clone(), window, cx)
        });

        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("new-profile", cx);
            let id = view.read_with(cx, |view, _| view.state().rows()[0].profile.id);
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
            let mut view = AppView::new(state, event_rx, Arc::new(FakeVerifier::passing()));
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
