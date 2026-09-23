//! Profile form rendering and choice controls.
use super::*;

/// The element id and the visible label of one proxy chip.
///
/// They are separate on purpose. The id is positional so a test can click it
/// even when two proxies share a name; the label is the name and nothing else.
/// Building the label out of the id is what put "0-Office" in the window.
pub(super) fn proxy_chip(index: usize, name: &str) -> (String, String) {
    (format!("editor-proxy-{index}"), name.to_string())
}

/// The proxy assignment, as chips: Direct plus every stored proxy.
/// Missing assignments stay visible rather than silently becoming Direct.
pub(super) fn proxy_row(
    editor: Entity<ProfileEditor>,
    selected: Option<ProxyId>,
    options: &[(ProxyId, String)],
    p: Palette,
    t: &Text,
) -> Div {
    let missing = selected.filter(|id| !options.iter().any(|(candidate, _)| candidate == id));

    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(proxy_choice(
            editor.clone(),
            "editor-proxy-direct".to_string(),
            t.direct.to_string(),
            selected.is_none(),
            None,
            p,
        ))
        .children(options.iter().enumerate().map(|(index, (id, name))| {
            let (chip_id, label) = proxy_chip(index, name);
            proxy_choice(
                editor.clone(),
                chip_id,
                label,
                selected == Some(*id),
                Some(*id),
                p,
            )
        }))
        .children(missing.map(|id| {
            proxy_choice(
                editor.clone(),
                "editor-proxy-missing".to_string(),
                t.missing_proxy(&id.to_string()),
                true,
                Some(id),
                p,
            )
        }))
}

/// One selectable chip: a proxy, a browser core.
///
/// They are the same control over different fields, so they differ in what the
/// click writes back and nothing else.
pub(super) fn chip<T: std::marker::Copy + PartialEq + 'static>(
    editor: Entity<ProfileEditor>,
    id: String,
    label: String,
    active: bool,
    value: Option<T>,
    apply: fn(&mut ProfileEditor, Option<T>),
    p: Palette,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
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
        .child(label)
        .on_click(move |_, _, cx| {
            editor.update(cx, |editor, cx| {
                apply(editor, value);
                cx.notify();
            });
        })
}

pub(super) fn proxy_choice(
    editor: Entity<ProfileEditor>,
    id: String,
    label: String,
    active: bool,
    value: Option<ProxyId>,
    p: Palette,
) -> impl IntoElement {
    chip(
        editor,
        id,
        label,
        active,
        value,
        |editor, value| editor.proxy = value,
        p,
    )
}

/// The browser cores, as chips.
///
/// There is no "none" here, unlike the proxy row: a profile without a core has
/// nothing to launch. A profile whose core was removed keeps it and shows it as
/// missing, so it cannot silently read as a profile on a core nobody chose.
pub(super) fn core_row(
    editor: Entity<ProfileEditor>,
    selected: CoreId,
    options: &[CoreChoice],
    p: Palette,
    t: &Text,
) -> Div {
    let missing = (!options.iter().any(|choice| choice.id == selected)).then_some(selected);
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .children(options.iter().enumerate().map(|(index, choice)| {
            core_choice(
                editor.clone(),
                format!("editor-core-{index}"),
                choice.label(t),
                selected == choice.id,
                Some(choice.id),
                p,
            )
        }))
        .children(missing.map(|id| {
            core_choice(
                editor.clone(),
                "editor-core-missing".to_string(),
                t.missing_core(&id.to_string()),
                true,
                Some(id),
                p,
            )
        }))
}

pub(super) fn core_choice(
    editor: Entity<ProfileEditor>,
    id: String,
    label: String,
    active: bool,
    value: Option<CoreId>,
    p: Palette,
) -> impl IntoElement {
    chip(
        editor,
        id,
        label,
        active,
        value,
        |editor, value| {
            if let Some(id) = value {
                editor.core = id;
            }
        },
        p,
    )
}

/// One row of mutually exclusive choices, rendered as chips.
///
/// The entity is passed rather than a listener closure because the choice needs
/// the value that was picked, which the listener signature does not carry.
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
            div()
                .id(format!("{prefix}-{label}"))
                .test_support()
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
                .child(*label)
                .on_click(move |_, _, cx| {
                    editor.update(cx, |editor, cx| {
                        apply(editor, value);
                        cx.notify();
                    });
                })
        }))
}

impl Render for ProfileEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.text;
        let p = palette(cx);
        let editor = cx.entity();
        let core_note = self.core_note();
        let mut form = Form::new()
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
                    .child(core_row(editor.clone(), self.core, &self.cores, p, t)),
            )
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
            .child(Field::new().label("Platform").child(choice_row(
                editor.clone(),
                "editor-platform",
                &PLATFORMS,
                self.platform,
                |editor, value| editor.platform = value,
                p,
            )))
            .child(
                Field::new().label("Platform version").child(
                    Input::new(&self.platform_version)
                        .id("editor-platform-version")
                        .aria_label("Platform version"),
                ),
            )
            .child(
                Field::new().label(t.language_title).child(
                    Input::new(&self.language)
                        .id("editor-language")
                        .aria_label(t.language_title),
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
                Field::new().label(t.timezone_field).child(
                    Input::new(&self.timezone)
                        .id("editor-timezone")
                        .aria_label(t.timezone_field),
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
            .child(
                Field::new()
                    .label(t.field_proxy)
                    .description(t.proxy_field_help)
                    .child(proxy_row(editor.clone(), self.proxy, &self.proxies, p, t)),
            )
            .child(Field::new().label(t.webrtc_field).child(choice_row(
                editor.clone(),
                "editor-webrtc",
                &webrtc_policies(t),
                self.webrtc_policy,
                |editor, value| editor.webrtc_policy = value,
                p,
            )))
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

        if let Some(error) = self.error.clone() {
            form = form.footer(
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
        form
    }
}
