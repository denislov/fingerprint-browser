//! The Profiles page: the list, its rows, and the columns they carry.
//!
//! Separated from the rest of the window's drawing for the same reason the state's
//! profile lifecycle is the largest single topic: a row has to say what the profile
//! is, which core runs it, where its traffic goes, how long it has been up and
//! whether anything is wrong with it, and all of that reads better beside itself.
//!
//! The helper functions are `pub(super)` rather than private because the window's
//! `render` is what calls them, and `render` lives in [`super`].

use super::components::{EmptyState, PageHeader, Tone, status_badge};
use super::details::{can_verify, verification_badge};
use super::*;

pub(super) fn profiles_header(header: &ProfilesHeader, cx: &mut Context<AppView>, t: &Text) -> Div {
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
    let p = palette(cx);

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            PageHeader::new(t.nav_profiles)
                .summary("profile-count", subtitle, announcement)
                .action(
                    Button::new("new-profile")
                        .icon(icons::action(icons::glyph::NEW))
                        .label(t.new_profile)
                        .primary()
                        // A profile needs a core to launch, and the empty state
                        // below says where to get one. The button is disabled
                        // rather than opening a dialog the service would refuse.
                        .disabled(!header.has_core)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_new_profile(window, cx)),
                        ),
                )
                .render(p),
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

pub(super) fn empty_hint(
    total: usize,
    visible: usize,
    has_core: bool,
    filter: &str,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Option<impl IntoElement> {
    // Three kinds of empty look the same in a list and mean different things:
    // there are no profiles, a filter is hiding the ones there are, and there is
    // nothing to launch a profile with. Each says which it is and carries the one
    // action that changes it.
    let filtering = total > 0 && visible == 0;
    if total > 0 && !filtering {
        return None;
    }

    let state = if filtering {
        EmptyState::new(
            "empty-hint",
            icons::glyph::EMPTY_SEARCH,
            t.empty_search_title,
            t.empty_no_match(filter.trim(), total),
        )
        .action(
            Button::new("clear-filter")
                .label(t.clear_filter)
                .outline()
                .on_click(cx.listener(|this, _, window, cx| this.on_clear_filter(window, cx))),
        )
    } else if has_core {
        EmptyState::new(
            "empty-hint",
            icons::glyph::EMPTY_PROFILES,
            t.empty_profiles_title,
            t.empty_no_profiles,
        )
        .action(
            Button::new("empty-new-profile")
                .icon(icons::action(icons::glyph::NEW))
                .label(t.new_profile)
                .primary()
                .on_click(cx.listener(|this, _, window, cx| this.on_new_profile(window, cx))),
        )
    } else {
        // The one empty state that cannot be acted on where it is read: the
        // sentence names an environment variable and the page that matters is
        // another one. So it carries the way there.
        EmptyState::new(
            "empty-hint",
            icons::glyph::EMPTY_CORES,
            t.empty_core_title,
            t.no_core_found,
        )
        .action(
            Button::new("empty-add-core")
                .icon(icons::action(icons::glyph::NEW))
                .label(t.add_browser_core)
                .primary()
                .on_click(cx.listener(|this, _, _, cx| this.on_page(Page::Cores, cx))),
        )
    };
    Some(state.render(p))
}

/// The widths the list's columns keep, so a row and the heading above it line
/// up and every value starts in the same place down the page.
///
/// The name column is the one that gives: it takes what is left, so a wider
/// window widens the names rather than stretching the columns of state and
/// action. The fixed three are sized for the longest thing they hold in either
/// language - a proxy name, "Stopping" with its badge, a button beside its menu.
const COLUMN_ROUTE: f32 = 150.0;
const COLUMN_STATE: f32 = 190.0;
const COLUMN_ACTIONS: f32 = 140.0;

/// The height a row keeps when nothing has gone wrong.
///
/// Two lines of text and room around them. A row with a warning is taller: the
/// summary is worth the space, and the alternative is a list that hides the one
/// thing the reader needs to see.
const ROW_HEIGHT: f32 = 64.0;

/// What each column holds, above the rows.
///
/// The widths here are the rows' widths, and the transparent border is the
/// row's own border: without it the heading would sit one pixel to the left of
/// every value under it.
pub(super) fn list_header(p: Palette, t: &Text) -> Div {
    div()
        .flex()
        .items_center()
        .gap_4()
        .px_4()
        .py_2()
        .border_1()
        .border_color(hsla(0.0, 0.0, 0.0, 0.0))
        .text_xs()
        .text_color(rgb(p.dim))
        .child(
            div()
                .id("column-profile")
                .test_support()
                .flex_1()
                .min_w_0()
                .child(t.column_profile),
        )
        .child(
            div()
                .id("column-proxy")
                .test_support()
                .w(px(COLUMN_ROUTE))
                .flex_shrink_0()
                .child(t.field_proxy),
        )
        .child(
            div()
                .id("column-state")
                .test_support()
                .w(px(COLUMN_STATE))
                .flex_shrink_0()
                .child(t.field_state),
        )
        .child(
            div()
                .id("column-actions")
                .test_support()
                .w(px(COLUMN_ACTIONS))
                .flex_shrink_0()
                .text_right()
                .child(t.column_actions),
        )
}

pub(super) fn profile_list(
    rows: &[ProfileRow],
    selected_id: Option<ProfileId>,
    verifications: &std::collections::HashMap<ProfileId, Verification>,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &'static Text,
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
                .gap_4()
                .px_4()
                .py_2()
                .min_h(px(ROW_HEIGHT))
                .rounded(px(components::RADIUS_SURFACE))
                .border_1()
                .border_color(rgb(if is_selected { p.accent } else { p.border }))
                .bg(rgb(if is_selected { p.selected } else { p.panel }))
                .when(!is_selected, |this| {
                    this.hover(|this| this.bg(rgb(p.hover)))
                })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.on_select(id, cx)))
                // What the profile is: the name, and the engine under it. The
                // seed and the platform it claims are details, not a byline.
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(row.profile.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(row.core_name.clone()),
                        ),
                )
                // Where its traffic goes. "Direct" is said rather than left
                // blank: an empty cell reads as missing information.
                .child(
                    div()
                        .id(format!("route-{id}"))
                        .test_support()
                        .w(px(COLUMN_ROUTE))
                        .flex_shrink_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(if row.proxy_name.is_some() {
                            p.text_soft
                        } else {
                            p.muted
                        }))
                        .child(
                            row.proxy_name
                                .clone()
                                .unwrap_or_else(|| t.direct.to_string()),
                        ),
                )
                .child(
                    div()
                        .w(px(COLUMN_STATE))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_1()
                        .child(state_badge(row, p, t))
                        .children(verification_badge(verifications.get(&id), p, t))
                        .children(signal_line(row, p, t)),
                )
                .child(
                    div()
                        .w(px(COLUMN_ACTIONS))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_end()
                        .child(row_actions(row, verifications, cx, t)),
                )
        }))
}

/// The one summary line a row gets when something is wrong.
///
/// The full text is in the details panel; what the row owes the reader is
/// enough to tell a warning from a failure without being pushed out of shape by
/// the longest error the runtime can produce. The whole sentence is on the
/// tooltip, so it is still reachable without opening anything.
fn signal_line(row: &ProfileRow, p: Palette, t: &Text) -> Option<impl IntoElement> {
    let (colour, label) = match (row.last_error(), row.last_warning()) {
        (Some(error), _) => (p.danger_strong, t.error_line(error)),
        (None, Some(warning)) => (p.warning, t.warning_line(warning)),
        (None, None) => return None,
    };
    Some(
        div()
            .id(format!("warning-{}", row.profile.id))
            .test_support()
            .flex()
            .items_center()
            .gap_1()
            .max_w(px(COLUMN_STATE))
            .text_xs()
            .text_color(rgb(colour))
            .tooltip(components::tooltip(label.clone()))
            .child(icons::action(icons::glyph::FAILED))
            .child(div().truncate().child(label)),
    )
}

/// The one action a row offers, and everything else behind its menu.
///
/// A row used to carry Start, Stop and Restart at once, two of them disabled at
/// any moment, which made the list a wall of buttons that all had to be read to
/// find the one that worked. The state decides which single action is real; the
/// rest - edit, copy, open the data directory, verify, restart, delete - move
/// into the menu beside it.
///
/// The action's *label* follows the state rather than the command: a start
/// waiting on its proxy is called off, and a start that failed is retried, but
/// both are the same command sent to the same place.
pub(super) fn row_actions(
    row: &ProfileRow,
    verifications: &std::collections::HashMap<ProfileId, Verification>,
    cx: &mut Context<AppView>,
    t: &'static Text,
) -> Div {
    let id = row.profile.id;
    let state = row.state();
    let failed = matches!(
        state,
        RuntimeState::Failed { .. } | RuntimeState::Crashed { .. }
    );

    let name = row.profile.name.clone();
    let primary = if row.can_stop() {
        let label = if state == RuntimeState::Starting {
            t.cancel_start
        } else if state == RuntimeState::Stopping {
            t.state_stopping
        } else {
            t.stop
        };
        // Neutral rather than red: stopping is the ordinary way a browser is
        // closed, and a column of red buttons reads as a page of failures.
        Button::new(format!("stop-{id}"))
            .icon(icons::action(icons::glyph::STOP))
            .label(label)
            .accessibility_label(t.row_action(label, &name))
            .outline()
            .disabled(state == RuntimeState::Stopping)
            .on_click(cx.listener(move |this, _, _, cx| this.on_stop(id, cx)))
    } else {
        let label = if failed { t.retry } else { t.start };
        // Quieter than the page's "New Profile": every stopped row has a start,
        // and a column of filled blue buttons is a column that shouts.
        Button::new(format!("start-{id}"))
            .icon(icons::action(icons::glyph::START))
            .label(label)
            .accessibility_label(t.row_action(label, &name))
            .secondary()
            .disabled(!row.can_start())
            .on_click(cx.listener(move |this, _, _, cx| this.on_start(id, cx)))
    };

    div()
        .flex()
        .items_center()
        .gap_2()
        .child(primary)
        .child(row_menu(row, verifications, cx, t))
}

/// The row's overflow menu: what a row can do that is not worth a button.
///
/// Every item is here because it is either rare or destructive. The ones the
/// runtime would refuse are disabled here rather than after the click, so the
/// menu says what is possible before it is asked.
fn row_menu(
    row: &ProfileRow,
    verifications: &std::collections::HashMap<ProfileId, Verification>,
    cx: &mut Context<AppView>,
    t: &'static Text,
) -> impl IntoElement {
    let id = row.profile.id;
    let name = row.profile.name.clone();
    let view = cx.entity().downgrade();
    let can_verify = can_verify(Some(row), verifications.get(&id));
    let can_restart = row.can_restart();

    components::icon_button(format!("more-{id}"), icons::glyph::MORE, t.row_more(&name))
        .dropdown_menu(move |menu, _window, _cx| {
            menu.item(components::menu_item(
                &view,
                t.edit,
                icons::glyph::EDIT,
                false,
                move |view, window, cx| view.on_edit(id, window, cx),
            ))
            .item(components::menu_item(
                &view,
                t.duplicate,
                icons::glyph::DUPLICATE,
                false,
                move |view, _window, cx| view.on_duplicate(id, cx),
            ))
            .item(components::menu_item(
                &view,
                t.open_data_dir,
                icons::glyph::OPEN_DIR,
                false,
                move |view, _window, cx| view.on_open_data_dir(id, cx),
            ))
            .item(components::menu_item(
                &view,
                t.verify_fingerprint,
                icons::glyph::VERIFY,
                !can_verify,
                move |view, _window, cx| view.on_verify(id, cx),
            ))
            .item(components::menu_item(
                &view,
                t.restart,
                icons::glyph::RESTART,
                !can_restart,
                move |view, _window, cx| view.on_restart(id, cx),
            ))
            // Removing a profile is the one item here that cannot be undone
            // from this window, so it is set apart from the five above it.
            .separator()
            .item(components::menu_item(
                &view,
                t.delete,
                icons::glyph::DELETE,
                false,
                move |view, window, cx| view.on_delete(id, window, cx),
            ))
        })
}

/// The profile's state, as the row shows it.
///
/// The tone is the state's, never the fingerprint's: a running browser with an
/// unverified fingerprint is a running browser, and the verification badge
/// beside it says the rest. A green row would otherwise claim a check that was
/// never run.
pub(super) fn state_badge(row: &ProfileRow, p: Palette, t: &Text) -> impl IntoElement {
    let tone = match row.state() {
        RuntimeState::Running => Tone::Success,
        RuntimeState::Starting | RuntimeState::Stopping => Tone::Warning,
        RuntimeState::Stopped => Tone::Neutral,
        RuntimeState::Failed { .. } | RuntimeState::Crashed { .. } => Tone::Danger,
    };
    status_badge(
        format!("state-{}", row.profile.id),
        tone,
        row.state_label(t),
        row.state_label(t),
        p,
    )
}

impl AppView {
    pub(super) fn on_copy_args(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.state.selected() {
            let args = row.effective_args();
            if !args.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(args.join("\n")));
            }
        }
    }

    pub(super) fn on_select(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        self.state.select(id);
        cx.notify();
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
    pub(super) fn open_profile(&mut self, id: ProfileId, how: Opening, cx: &mut Context<Self>) {
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
                let outcome = tester.test(&job);
                let _ = sender.send((*job, outcome));
            });
        }
        cx.notify();
    }

    pub(super) fn on_restart(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        self.open_profile(id, Opening::Restart, cx);
    }

    pub(super) fn on_stop(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        let _ = self.state.stop(id);
        cx.notify();
    }

    pub(super) fn on_start(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        self.open_profile(id, Opening::Start, cx);
    }

    /// A new profile starts a form rather than a row.
    ///
    /// The row is written when the form is accepted, so a cancelled form leaves
    /// nothing behind and the profile that appears is the one that was
    /// configured - core included, because which engine runs a profile is what
    /// decides the switches it may claim.
    pub(super) fn on_new_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn on_verify(&mut self, id: ProfileId, cx: &mut Context<Self>) {
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
            let _ = sender.send((job, outcome));
        });
        cx.notify();
    }

    /// Deleting asks first: it removes the profile, not its sessions on disk.
    pub(super) fn on_delete(&mut self, id: ProfileId, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn on_duplicate(&mut self, id: ProfileId, cx: &mut Context<Self>) {
        let t = self.state.text();
        match self.state.duplicate_profile(id) {
            Ok(_) => self.state.push_notice(t.profile_duplicated, false),
            Err(error) => self.state.push_notice(error.to_string(), true),
        }
        cx.notify();
    }

    /// Opens the form for a profile and wires its accept button.
    ///
    /// Creating and saving share one dialog because they share one form: what
    /// the accepted form asks for is the editor's answer, not the dialog's.
    pub(super) fn open_profile_form(
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

    /// Opens the editor for a profile and saves it through the dialog.
    pub(super) fn on_edit(&mut self, id: ProfileId, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Opens a profile's browser data directory.
    ///
    /// The directory is not created here: a profile that has never run has no
    /// browser data, and the refusal says so rather than leaving an empty
    /// folder behind that looks like state. The opener waits for the desktop to
    /// accept the request, so it runs on a worker and the answer arrives on a
    /// later tick - the window is not blocked either way, and a spawn that found
    /// no handler is reported instead of being mistaken for an open.
    pub(super) fn on_open_data_dir(&mut self, id: ProfileId, cx: &mut Context<Self>) {
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
}
