//! Profile form rendering: the groups the form is read in, and the selectors it
//! picks resources with.
//!
//! The form used to be one column of seventeen fields, ordered roughly by how
//! they had been added. A reader arriving at it had to scan the whole thing to
//! find the three answers that matter - what it is called, which engine runs it,
//! and where its traffic goes - and the overrides that most profiles never touch
//! sat at the same level as the ones everybody sets.
//!
//! So it is three groups now: what the profile is, the fingerprint it claims,
//! and the overrides that are folded away until they are wanted. The folded
//! group says when it holds something, because a hidden field that has been
//! changed is the one thing a folded group can hide badly.
//!
//! The two resources - the browser core and the proxy - are chosen with a
//! searchable selector rather than a row of chips: a core list grows with every
//! engine installed, and a proxy list grows with every provider, so both are
//! lists the reader has to search rather than read.

use super::*;
use crate::ui::{components, icons};
use gpui_kit::component::IndexPath;
use gpui_kit::component::combobox::Combobox;
use gpui_kit::component::searchable_list::SearchableListItem;

/// One browser core as the selector offers it.
///
/// The label is built once, when the form is opened, because the item trait has
/// no access to the language table: a selector that looked its labels up per
/// frame would need the table threaded through the library's own list.
#[derive(Clone)]
pub(crate) struct CoreOption {
    pub id: CoreId,
    pub label: String,
}

impl SearchableListItem for CoreOption {
    type Value = CoreId;

    fn title(&self) -> SharedString {
        self.label.clone().into()
    }

    fn value(&self) -> &CoreId {
        &self.id
    }
}

/// One proxy as the selector offers it: the direct connection, or a stored one.
///
/// `None` is Direct rather than an absence: a profile with no proxy is a profile
/// whose traffic leaves unproxied, which is a choice and not an empty field.
#[derive(Clone)]
pub(crate) struct ProxyOption {
    pub id: Option<ProxyId>,
    pub name: String,
}

impl SearchableListItem for ProxyOption {
    type Value = Option<ProxyId>;

    fn title(&self) -> SharedString {
        self.name.clone().into()
    }

    fn value(&self) -> &Option<ProxyId> {
        &self.id
    }
}

/// The cores the selector offers, with the one in force added if it is gone.
///
/// A profile whose core was removed keeps it and shows it as missing: the
/// alternative is a form that silently moves the profile onto an engine nobody
/// chose.
pub(super) fn core_options(cores: &[CoreChoice], selected: CoreId, t: &Text) -> Vec<CoreOption> {
    let mut options: Vec<CoreOption> = cores
        .iter()
        .map(|choice| CoreOption {
            id: choice.id,
            label: choice.label(t),
        })
        .collect();
    if !options.iter().any(|option| option.id == selected) {
        options.push(CoreOption {
            id: selected,
            label: t.missing_core(&selected.to_string()),
        });
    }
    options
}

/// The same for the proxies, with Direct first.
pub(super) fn proxy_options(
    proxies: &[(ProxyId, String)],
    selected: Option<ProxyId>,
    t: &Text,
) -> Vec<ProxyOption> {
    let mut options = vec![ProxyOption {
        id: None,
        name: t.direct.to_string(),
    }];
    options.extend(proxies.iter().map(|(id, name)| ProxyOption {
        id: Some(*id),
        name: name.clone(),
    }));
    if let Some(selected) = selected
        && !proxies.iter().any(|(id, _)| *id == selected)
    {
        options.push(ProxyOption {
            id: Some(selected),
            name: t.missing_proxy(&selected.to_string()),
        });
    }
    options
}

/// Where the selected option sits in its list, which is how the library carries
/// a selection: it hands back the value, and takes back a position.
pub(super) fn index_of<T>(options: &[T], matches: impl Fn(&T) -> bool) -> Vec<IndexPath> {
    options
        .iter()
        .position(matches)
        .map(IndexPath::new)
        .into_iter()
        .collect()
}

/// A group's heading: what the fields under it are, and whether they hold
/// anything the profile would not otherwise use.
fn group_heading(title: &str, modified: bool, p: Palette, t: &Text) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .pt_2()
        .child(
            div()
                .text_size(px(components::SECTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(p.text))
                .child(title.to_string()),
        )
        .when(modified, |this| {
            this.child(components::status_badge(
                "editor-advanced-modified",
                components::Tone::Warning,
                t.form_modified,
                t.form_modified,
                p,
            ))
        })
}

/// The heading of the group that folds, which is also the control that folds it.
fn fold_heading(
    open: bool,
    modified: bool,
    cx: &mut Context<ProfileEditor>,
    p: Palette,
    t: &Text,
) -> impl IntoElement {
    div()
        .id("editor-advanced-toggle")
        .test_support()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .pt_2()
        .cursor_pointer()
        .on_click(cx.listener(|this, _, _, cx| {
            this.advanced_open = !this.advanced_open;
            cx.notify();
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_color(rgb(p.text))
                .child(icons::action(if open {
                    icons::glyph::COLLAPSE
                } else {
                    icons::glyph::EXPAND
                }))
                .child(
                    div()
                        .text_size(px(components::SECTION))
                        .font_weight(FontWeight::MEDIUM)
                        .child(t.form_group_advanced.to_string()),
                ),
        )
        .when(modified, |this| {
            this.child(components::status_badge(
                "editor-advanced-modified",
                components::Tone::Warning,
                t.form_modified,
                t.form_modified,
                p,
            ))
        })
}

/// One row of mutually exclusive choices, rendered as chips.
///
/// Three or four fixed alternatives that fit on a line - a brand, a platform, a
/// WebRTC policy - are read faster than they are searched, so they stay chips:
/// a selector here would hide two thirds of a set this small behind a click.
pub(super) fn choice_row<T: std::marker::Copy + PartialEq + 'static>(
    editor: Entity<ProfileEditor>,
    prefix: &'static str,
    options: &[(T, &'static str)],
    selected: T,
    apply: fn(&mut ProfileEditor, T),
    p: Palette,
) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children(options.iter().map(|(value, label)| {
            let value = *value;
            let active = value == selected;
            let editor = editor.clone();
            components::chip(format!("{prefix}-{label}"), label, active, p).on_click(
                move |_, _, cx| {
                    editor.update(cx, |editor, cx| {
                        apply(editor, value);
                        cx.notify();
                    });
                },
            )
        }))
}

/// The empty state of a selector: what it looked for and did not find.
fn select_empty(label: &'static str, p: Palette) -> impl Fn(&mut Window, &App) -> Div + 'static {
    move |_, _| {
        div()
            .px_3()
            .py_2()
            .text_xs()
            .text_color(rgb(p.muted))
            .child(label.to_string())
    }
}

impl Render for ProfileEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.text;
        let p = palette(cx);
        let editor = cx.entity();
        let core_note = self.core_note(cx);
        let advanced_open = self.advanced_open;
        let advanced_modified = self.advanced_modified(cx);
        let core_select = self.core_select.clone();
        let proxy_select = self.proxy_select.clone();

        // One `Form` per group rather than one form with everything in it: the
        // form's own container is a grid of labelled fields and has nowhere to
        // put a heading, and three grids that share a label width still line up
        // down the page.
        let basics = Form::new()
            .label_layout(Axis::Horizontal)
            .label_width(px(LABEL_WIDTH))
            .child(
                Field::new().label(t.name_field).child(
                    Input::new(&self.name)
                        .id("editor-name")
                        .aria_label(t.profile_name),
                ),
            )
            .child(
                Field::new()
                    .label(t.browser_core)
                    .description(core_note)
                    .child(
                        Combobox::new(&core_select)
                            .placeholder(t.editor_core_placeholder)
                            .search_placeholder(t.editor_core_search)
                            .empty(select_empty(t.editor_no_match, p)),
                    ),
            )
            .child(
                Field::new()
                    .label(t.field_proxy)
                    .description(t.proxy_field_help)
                    .child(
                        Combobox::new(&proxy_select)
                            .placeholder(t.editor_proxy_placeholder)
                            .search_placeholder(t.editor_proxy_search)
                            .empty(select_empty(t.editor_no_match, p)),
                    ),
            );
        let fingerprint = Form::new()
            .label_layout(Axis::Horizontal)
            .label_width(px(LABEL_WIDTH))
            .child(
                Field::new()
                    .label(t.field_seed)
                    .description(t.seed_help)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Input::new(&self.seed)
                                    .id("editor-seed")
                                    .aria_label(t.fingerprint_seed),
                            )
                            .child(
                                Button::new("editor-reroll")
                                    .label(t.new_seed)
                                    .outline()
                                    .on_click({
                                        let editor = editor.clone();
                                        move |_, window, cx| {
                                            editor.update(cx, |editor, cx| {
                                                editor.reroll_seed(window, cx);
                                                cx.notify();
                                            });
                                        }
                                    }),
                            ),
                    ),
            )
            .child(Field::new().label(t.brand_field).child(choice_row(
                editor.clone(),
                "editor-brand",
                &BRANDS,
                self.brand,
                |editor, value| editor.brand = value,
                p,
            )))
            .child(Field::new().label("Platform").child(choice_row(
                editor.clone(),
                "editor-platform",
                &PLATFORMS,
                self.platform,
                |editor, value| editor.platform = value,
                p,
            )))
            .child(
                Field::new().label(t.language_title).child(
                    Input::new(&self.language)
                        .id("editor-language")
                        .aria_label(t.language_title),
                ),
            )
            .child(
                Field::new().label(t.timezone_field).child(
                    Input::new(&self.timezone)
                        .id("editor-timezone")
                        .aria_label(t.timezone_field),
                ),
            )
            .child(
                Field::new().label(t.window_field).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div().w(px(90.0)).child(
                                Input::new(&self.window_width)
                                    .id("editor-window-width")
                                    .aria_label(t.window_width),
                            ),
                        )
                        .child(div().text_xs().text_color(rgb(p.muted)).child("x"))
                        .child(
                            div().w(px(90.0)).child(
                                Input::new(&self.window_height)
                                    .id("editor-window-height")
                                    .aria_label(t.window_height),
                            ),
                        ),
                ),
            )
            .child(Field::new().label(t.webrtc_field).child(choice_row(
                editor.clone(),
                "editor-webrtc",
                &webrtc_policies(t),
                self.webrtc_policy,
                |editor, value| editor.webrtc_policy = value,
                p,
            )));

        let advanced = Form::new()
            .label_layout(Axis::Horizontal)
            .label_width(px(LABEL_WIDTH))
            .child(
                Field::new()
                    .label(t.brand_version)
                    .description(t.brand_version_help)
                    .child(
                        Input::new(&self.brand_version)
                            .id("editor-brand-version")
                            .aria_label(t.brand_version),
                    ),
            )
            .child(
                Field::new().label("Platform version").child(
                    Input::new(&self.platform_version)
                        .id("editor-platform-version")
                        .aria_label("Platform version"),
                ),
            )
            .child(
                Field::new()
                    .label(t.accept_language)
                    .description(t.accept_language_help)
                    .child(
                        Input::new(&self.accept_language)
                            .id("editor-accept-language")
                            .aria_label(t.accept_language),
                    ),
            )
            .child(
                Field::new()
                    .label(t.cpu_cores)
                    .description(t.cpu_cores_help)
                    .child(
                        Input::new(&self.hardware_concurrency)
                            .id("editor-hardware-concurrency")
                            .aria_label(t.hardware_concurrency),
                    ),
            )
            .child(
                Field::new()
                    .label(t.excluded_spoofing)
                    .description(t.excluded_spoofing_help)
                    .child(div().flex().flex_wrap().gap_3().children(
                        SPOOFING_FEATURES.iter().map(|(feature, label)| {
                            let feature = *feature;
                            Checkbox::new(format!("editor-spoofing-{label}"))
                                .label(*label)
                                .checked(self.disabled_spoofing.contains(&feature))
                                .on_change(cx.listener(
                                    move |this: &mut Self, checked: &bool, _, cx| {
                                        let wanted = this.disabled_spoofing.contains(&feature);
                                        if *checked != wanted {
                                            this.toggle_spoofing(feature);
                                            cx.notify();
                                        }
                                    },
                                ))
                        }),
                    )),
            );

        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            // ---- what the profile is ----
            .child(group_heading(t.form_group_basics, false, p, t))
            .child(basics)
            // ---- what it claims to be ----
            .child(group_heading(t.form_group_fingerprint, false, p, t))
            .child(fingerprint)
            // ---- the overrides, folded away ----
            .child(fold_heading(advanced_open, advanced_modified, cx, p, t));
        if advanced_open {
            body = body.child(advanced);
        }

        if let Some(error) = self.error.clone() {
            body = body.child(
                div()
                    .id("editor-error")
                    .test_support()
                    .text_xs()
                    .text_color(rgb(p.danger_strong))
                    // One line per clause: gpui does not wrap a single line, and
                    // a refusal that runs off the edge is a refusal half read.
                    .children(
                        error
                            .split("; ")
                            .map(|clause| div().child(clause.to_string())),
                    ),
            );
        }
        body
    }
}
