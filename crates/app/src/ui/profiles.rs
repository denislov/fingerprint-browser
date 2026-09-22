//! The Profiles page: the list, its rows, and the columns they carry.
//!
//! Separated from the rest of the window's drawing for the same reason the state's
//! profile lifecycle is the largest single topic: a row has to say what the profile
//! is, which core runs it, where its traffic goes, how long it has been up and
//! whether anything is wrong with it, and all of that reads better beside itself.
//!
//! The helper functions are `pub(super)` rather than private because the window's
//! `render` is what calls them, and `render` lives in [`super`].

use super::details::verification_badge;
use super::*;

pub(super) fn profiles_header(header: &ProfilesHeader, cx: &mut Context<AppView>, t: &Text) -> Div {
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

pub(super) fn empty_hint(
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

pub(super) fn profile_list(
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

pub(super) fn row_actions(row: &ProfileRow, cx: &mut Context<AppView>, t: &Text) -> Div {
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

pub(super) fn route_label(row: &ProfileRow, t: &Text) -> String {
    match &row.proxy_name {
        Some(proxy) => t.profile_meta_proxy(&row.core_name, proxy),
        None => t.profile_meta_direct(&row.core_name),
    }
}

pub(super) fn state_badge(row: &ProfileRow, p: Palette, t: &Text) -> impl IntoElement {
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
                let live = job.is_live();
                let outcome = tester.test(&job);
                let _ = sender.send((job.proxy_id, live, outcome));
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
            let _ = sender.send((job.profile_id, outcome));
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
