//! The Settings page: appearance, exit behaviour, language, the configuration
//! backup cards, and the activity log's own settings.
//!
//! The largest of the page modules, and it is one page: every card here changes
//! something about the program rather than about a profile, which is what the page
//! is for. The cards are separate functions so a change to one does not mean
//! reading the other five.

use super::*;

/// What a setting does, in one line, under its field.
///
/// The rows that cannot be edited have no field, so this covers the editable
/// ones - and the help a row carries on the page is its own (`SettingRow::note`),
/// because a page has room for a sentence a dialog does not.
pub(super) fn key_help(key: SettingKey, t: &Text) -> String {
    match key {
        SettingKey::XrayExecutable => t.help_xray_executable_field.to_string(),
        _ => String::new(),
    }
}

pub(super) fn settings_header(p: Palette, t: &Text) -> Div {
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
pub(super) struct SettingsExport {
    pub(super) path: Entity<InputState>,
    /// Where an empty field would write right now, shown under the field so the
    /// default is never a guess.
    pub(super) destination: PathBuf,
    pub(super) include_credentials: bool,
}

/// The four backup cards at the foot of the Settings page, and the fields they
/// read.
///
/// A value rather than four positional parameters, for the reason
/// [`SettingsExport`] is one: the set grows, and every call site should not have
/// to change when it does.
pub(super) struct SettingsCards {
    pub(super) export: SettingsExport,
    pub(super) import: Entity<InputState>,
    pub(super) restore: Entity<InputState>,
    pub(super) browser_data: Entity<InputState>,
    /// Where the diagnostics card's button would write. Computed while
    /// rendering, so the line under the button names the file this press would
    /// produce rather than one named a minute ago.
    pub(super) diagnostics: PathBuf,
    pub(super) theme: ThemeChoice,
    pub(super) language: Lang,
    pub(super) exit_mode: ExitMode,
}

pub(super) fn settings_body(
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
pub(super) fn appearance_card(
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
pub(super) fn exit_mode_card(
    mode: ExitMode,
    cx: &mut Context<AppView>,
    t: &Text,
) -> impl IntoElement {
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
pub(super) fn exit_choice(exit: &Exit, p: Palette, t: &Text) -> Stateful<Div> {
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
pub(super) fn settings_card(p: Palette) -> Div {
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
pub(super) fn settings_card_heading(title: &str, body: &str, p: Palette) -> Div {
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
pub(super) fn settings_card_row(label: &str, p: Palette) -> Div {
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
pub(super) fn chip(id: String, label: &str, active: bool, p: Palette) -> Stateful<Div> {
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
pub(super) fn export_card(
    export: &SettingsExport,
    cx: &mut Context<AppView>,
    t: &Text,
) -> impl IntoElement {
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
pub(super) fn import_card(
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
pub(super) fn restore_card(
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
pub(super) fn browser_data_card(
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
pub(super) fn diagnostics_card(
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
pub(super) fn path_row(input: &Entity<InputState>, id: &str, label: &str, p: Palette) -> Div {
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
pub(super) fn card_note(text: String, p: Palette) -> Div {
    div().text_xs().text_color(rgb(p.dim)).child(text)
}

/// The muted sentence beside a checkbox, saying what the box does.
pub(super) fn card_hint(text: &str, p: Palette) -> Div {
    div()
        .text_xs()
        .text_color(rgb(p.muted))
        .child(text.to_string())
}

mod actions;
