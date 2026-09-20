//! The profile editor: the form behind "Edit profile" and "New Profile".
//!
//! The editor owns an editable copy of one profile - or, for a profile that
//! does not exist yet, the values the form starts from - and turns it into
//! either a [`BrowserProfile`] to write back or a [`NewProfile`] to create. It
//! never writes to storage itself: the view hands the result to
//! [`crate::state::AppState`], and a rejection comes back as a message the form
//! shows without closing.
//!
//! Fields an existing profile already owns but this form does not edit (id, user
//! data directory, start target) are carried through untouched, so saving cannot
//! silently drop them. A new profile has none of those yet, which is why creating
//! and saving are two different results rather than one profile with half-made
//! fields that a cancelled form would leave behind.

use crate::state::CoreChoice;
use crate::text::Text;
use crate::theme::{Palette, palette};
use application::NewProfile;
use domain::{
    BrowserBrand, BrowserProfile, CoreId, FingerprintProfile, Platform, ProfileId, ProxyId,
    SpoofingFeature, StartTarget, WebRtcPolicy, WindowProfile, validate_profile,
};
use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::form::*;
use gpui_kit::component::input::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;

const LABEL_WIDTH: f32 = 170.0;

/// What an accepted form asks the window to do.
#[derive(Debug)]
pub enum ProfileEdit {
    /// Write this back over the profile the form was opened on.
    Save(BrowserProfile),
    /// Create this.
    Create(NewProfile),
}

/// The profile the form was opened on.
enum Mode {
    /// One that is already stored: the fields this form does not edit are
    /// carried through untouched.
    ///
    /// Boxed because the other variant carries nothing: a form that creates a
    /// profile should not pay for a whole stored profile on every value of this
    /// enum.
    Edit(Box<BrowserProfile>),
    /// None yet. The id, the data directory and the start target belong to the
    /// service, not to a form that can still be cancelled.
    New,
}

/// The values a new form starts from.
struct Draft {
    name: String,
    fingerprint: FingerprintProfile,
    window: WindowProfile,
    proxy: Option<ProxyId>,
}

/// An editable copy of one profile.
pub struct ProfileEditor {
    /// The table the form's labels come from, taken when it is opened.
    ///
    /// Carried rather than looked up: the editor is its own entity and has no
    /// route to the application state. A language switched while this form is
    /// open therefore applies to the next time it is opened, which is the one
    /// place the window can be briefly half-translated - and the alternative is
    /// a global, which the tests would then have to serialise.
    text: &'static Text,
    mode: Mode,
    /// The core the profile runs on.
    ///
    /// Editable in both modes: which engine runs a profile decides which
    /// switches it may be asked for, so pointing it at another core is a real
    /// change, and the form shows which core is in force.
    core: CoreId,
    /// The cores the form offers, with the version each one answered with.
    cores: Vec<CoreChoice>,
    name: Entity<InputState>,
    seed: Entity<InputState>,
    brand_version: Entity<InputState>,
    platform_version: Entity<InputState>,
    language: Entity<InputState>,
    accept_language: Entity<InputState>,
    timezone: Entity<InputState>,
    hardware_concurrency: Entity<InputState>,
    window_width: Entity<InputState>,
    window_height: Entity<InputState>,
    brand: BrowserBrand,
    platform: Platform,
    webrtc_policy: WebRtcPolicy,
    disabled_spoofing: Vec<SpoofingFeature>,
    /// The proxies this profile can be assigned to, and which one it is on.
    proxies: Vec<(ProxyId, String)>,
    proxy: Option<ProxyId>,
    /// Why the last save attempt was refused.
    error: Option<String>,
}

const BRANDS: [(BrowserBrand, &str); 4] = [
    (BrowserBrand::Chrome, "Chrome"),
    (BrowserBrand::Edge, "Edge"),
    (BrowserBrand::Opera, "Opera"),
    (BrowserBrand::Vivaldi, "Vivaldi"),
];

const PLATFORMS: [(Platform, &str); 3] = [
    (Platform::Windows, "Windows"),
    (Platform::MacOs, "macOS"),
    (Platform::Linux, "Linux"),
];

/// The WebRTC policies, named in the window's language.
///
/// A function rather than a `const` because the labels are translated: the two
/// brand and platform lists below stay constants, because a brand and a platform
/// are proper names that read the same in both languages.
fn webrtc_policies(t: &Text) -> [(WebRtcPolicy, &'static str); 3] {
    [
        (WebRtcPolicy::DisableNonProxiedUdp, t.webrtc_none),
        (WebRtcPolicy::DefaultPublicInterfaceOnly, t.webrtc_public),
        (
            WebRtcPolicy::DefaultPublicAndPrivateInterfaces,
            t.webrtc_public_private,
        ),
    ]
}

const SPOOFING_FEATURES: [(SpoofingFeature, &str); 5] = [
    (SpoofingFeature::Font, "font"),
    (SpoofingFeature::Audio, "audio"),
    (SpoofingFeature::Canvas, "canvas"),
    (SpoofingFeature::ClientRects, "clientrects"),
    (SpoofingFeature::Gpu, "gpu"),
];

impl ProfileEditor {
    /// Builds the form from the profile as it is stored.
    ///
    /// The cores and the proxies are passed in because they live outside the
    /// profile: the form needs their names to offer a choice, and the ids it
    /// stores are the only part of those assignments the profile owns.
    pub fn new(
        profile: &BrowserProfile,
        cores: &[CoreChoice],
        proxies: &[(ProxyId, String)],
        text: &'static Text,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        Self::form(
            Mode::Edit(Box::new(profile.clone())),
            profile.core_id,
            Draft {
                name: profile.name.clone(),
                fingerprint: profile.fingerprint.clone(),
                window: profile.window,
                proxy: profile.proxy_id,
            },
            cores,
            proxies,
            text,
            window,
            cx,
        )
    }

    /// Builds the form for a profile that does not exist yet.
    ///
    /// `core` is the core the form starts on. The caller refuses to open this
    /// form when there is no core at all, because a profile with no core has
    /// nothing to launch and the service would have to invent one behind the
    /// user's back.
    pub fn new_profile(
        name: &str,
        core: CoreId,
        cores: &[CoreChoice],
        proxies: &[(ProxyId, String)],
        text: &'static Text,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        Self::form(
            Mode::New,
            core,
            Draft {
                name: name.to_string(),
                fingerprint: FingerprintProfile::new_random(profile_seed()),
                window: WindowProfile::default(),
                proxy: None,
            },
            cores,
            proxies,
            text,
            window,
            cx,
        )
    }

    /// Eight parameters, and each one is a different thing the form is built
    /// from: the mode, the starting core, the draft, the two lists it picks
    /// from, the language table, and the window pair. A struct would only move
    /// the list somewhere else.
    #[allow(clippy::too_many_arguments)]
    fn form(
        mode: Mode,
        core: CoreId,
        draft: Draft,
        cores: &[CoreChoice],
        proxies: &[(ProxyId, String)],
        text: &'static Text,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let fingerprint = draft.fingerprint;
        let field = |value: &str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
        };
        Self {
            text,
            mode,
            core,
            cores: cores.to_vec(),
            name: field(&draft.name, window, cx),
            seed: field(&fingerprint.seed.to_string(), window, cx),
            brand_version: field(
                fingerprint.brand_version.as_deref().unwrap_or(""),
                window,
                cx,
            ),
            platform_version: field(
                fingerprint.platform_version.as_deref().unwrap_or(""),
                window,
                cx,
            ),
            language: field(&fingerprint.language, window, cx),
            accept_language: field(&fingerprint.accept_language, window, cx),
            timezone: field(&fingerprint.timezone, window, cx),
            hardware_concurrency: field(
                &fingerprint
                    .hardware_concurrency
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                window,
                cx,
            ),
            window_width: field(&draft.window.width.to_string(), window, cx),
            window_height: field(&draft.window.height.to_string(), window, cx),
            brand: fingerprint.brand,
            platform: fingerprint.platform,
            webrtc_policy: fingerprint.webrtc_policy,
            disabled_spoofing: fingerprint.disabled_spoofing.clone(),
            proxies: proxies.to_vec(),
            proxy: draft.proxy,
            error: None,
        }
    }

    /// The name field, for callers that drive the form directly.
    #[cfg(test)]
    pub fn name_input(&self) -> Entity<InputState> {
        self.name.clone()
    }

    /// The seed field.
    #[cfg(test)]
    pub fn seed_input(&self) -> Entity<InputState> {
        self.seed.clone()
    }

    /// Turns the form into what the window should do, or explains what is wrong
    /// with it.
    pub fn build(&self, cx: &App) -> Result<ProfileEdit, String> {
        let t = self.text;
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let optional = |input: &Entity<InputState>| {
            let value = text(input);
            (!value.is_empty()).then_some(value)
        };

        let seed: u32 = text(&self.seed)
            .parse()
            .map_err(|_| t.seed_whole_number.to_string())?;
        let hardware_concurrency = match optional(&self.hardware_concurrency) {
            Some(value) => Some(
                value
                    .parse::<u8>()
                    .map_err(|_| t.concurrency_whole_number.to_string())?,
            ),
            None => None,
        };
        let width: u32 = text(&self.window_width)
            .parse()
            .map_err(|_| t.width_whole_number.to_string())?;
        let height: u32 = text(&self.window_height)
            .parse()
            .map_err(|_| t.height_whole_number.to_string())?;

        let fingerprint = FingerprintProfile {
            seed,
            brand: self.brand,
            brand_version: optional(&self.brand_version),
            platform: self.platform,
            platform_version: optional(&self.platform_version),
            language: text(&self.language),
            accept_language: text(&self.accept_language),
            timezone: text(&self.timezone),
            hardware_concurrency,
            webrtc_policy: self.webrtc_policy,
            disabled_spoofing: self.disabled_spoofing.clone(),
        };
        let window = WindowProfile::new(width, height);
        let name = text(&self.name);

        // The same rules storage enforces, run before anything is written.
        let validate =
            |profile: &BrowserProfile| validate_profile(profile).map_err(|error| error.to_string());

        match &self.mode {
            Mode::Edit(base) => {
                let mut profile = base.as_ref().clone();
                profile.name = name;
                profile.core_id = self.core;
                profile.window = window;
                profile.fingerprint = fingerprint;
                profile.proxy_id = self.proxy;
                validate(&profile)?;
                Ok(ProfileEdit::Save(profile))
            }
            Mode::New => {
                // Same rules, same shape: a draft profile is what they are
                // written against. Its id, data directory and start target are
                // placeholders - `NewProfile` asks the service for exactly the
                // rest of the fields and leaves those three to it.
                let draft = BrowserProfile {
                    id: ProfileId::new(),
                    name,
                    core_id: self.core,
                    user_data_dir: PathBuf::new(),
                    fingerprint: fingerprint.clone(),
                    proxy_id: self.proxy,
                    window,
                    start_target: StartTarget::default(),
                };
                validate(&draft)?;
                Ok(ProfileEdit::Create(NewProfile {
                    name: draft.name,
                    core_id: self.core,
                    user_data_dir: None,
                    fingerprint: Some(fingerprint),
                    proxy_id: self.proxy,
                    window: Some(window),
                    start_target: None,
                }))
            }
        }
    }

    /// Whether this form creates a profile rather than editing one.
    #[cfg(test)]
    pub fn is_new(&self) -> bool {
        matches!(self.mode, Mode::New)
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    /// A fresh seed, for a profile that should not share a canvas surface.
    pub fn reroll_seed(&mut self, window: &mut Window, cx: &mut App) {
        let seed = profile_seed();
        self.seed.update(cx, |state, cx| {
            state.set_value(seed.to_string(), window, cx)
        });
    }

    fn toggle_spoofing(&mut self, feature: SpoofingFeature) {
        if let Some(index) = self
            .disabled_spoofing
            .iter()
            .position(|candidate| *candidate == feature)
        {
            self.disabled_spoofing.remove(index);
        } else {
            self.disabled_spoofing.push(feature);
        }
    }

    /// What the form says about the core in force.
    ///
    /// The generation matters on this form, not only on the Cores page: it
    /// decides whether the exclusions below are honoured at all, and a form that
    /// kept quiet about it would let a checkbox be ticked and ignored.
    fn core_note(&self) -> String {
        let t = self.text;
        match self.cores.iter().find(|choice| choice.id == self.core) {
            Some(choice) => match &choice.generation {
                Some(generation) if choice.exclusions_honoured => generation.clone(),
                Some(generation) => t.generation_ignores_exclusions(generation),
                None => "this core answered no version, so a profile on it cannot be started \
                         until one is recorded"
                    .to_string(),
            },
            None => "the core this profile was on is no longer registered; pick another one \
                     before saving"
                .to_string(),
        }
    }
}

/// The proxy assignment, as chips: Direct plus every stored proxy.
///
/// A chip whose id is no longer in the list is still shown, marked as missing:
/// a profile assigned to a proxy that was deleted elsewhere must not silently
/// read as Direct.
/// The element id and the visible label of one proxy chip.
///
/// They are separate on purpose. The id is positional so a test can click it
/// even when two proxies share a name; the label is the name and nothing else.
/// Building the label out of the id is what put "0-Office" in the window.
fn proxy_chip(index: usize, name: &str) -> (String, String) {
    (format!("editor-proxy-{index}"), name.to_string())
}

fn proxy_row(
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
fn chip<T: std::marker::Copy + PartialEq + 'static>(
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

fn proxy_choice(
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
fn core_row(
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

fn core_choice(
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

/// A seed that is very unlikely to repeat, from the clock.
fn profile_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_nanos() & 0xFFFF_FFFF) as u32
}

/// One row of mutually exclusive choices, rendered as chips.
///
/// The entity is passed rather than a listener closure because the choice needs
/// the value that was picked, which the listener signature does not carry.
fn choice_row<T: std::marker::Copy + PartialEq + 'static>(
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

#[cfg(test)]
mod tests {
    use crate::text::en;

    // Explicit imports: a glob here pulls the whole gpui surface into the test
    // macro's expansion and makes it recurse.
    use super::{ProfileEdit, ProfileEditor};
    use domain::{
        BrowserBrand, CoreId, FingerprintProfile, Platform, ProfileId, ProxyId, SpoofingFeature,
        StartTarget, WebRtcPolicy, WindowProfile,
    };
    use gpui_kit::component::input::InputState;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    fn profile() -> super::BrowserProfile {
        super::BrowserProfile {
            id: ProfileId::new(),
            name: "Original".to_string(),
            core_id: CoreId::new(),
            user_data_dir: std::path::PathBuf::from("data/profiles/original"),
            fingerprint: FingerprintProfile::new_random(4242),
            proxy_id: None,
            window: WindowProfile::new(1280, 800),
            start_target: StartTarget::Blank,
        }
    }

    /// One browser core, as the form is offered it.
    fn choice(id: CoreId, name: &str, major: u32) -> super::CoreChoice {
        super::CoreChoice {
            id,
            name: name.to_string(),
            major,
            generation: Some("Chrome 144+ · spoofing exclusions honoured".to_string()),
            exclusions_honoured: true,
        }
    }

    /// The editor inside a window, so its inputs can be driven.
    fn editor<'a>(
        cx: &'a mut TestAppContext,
        profile: &super::BrowserProfile,
    ) -> (Entity<ProfileEditor>, &'a mut VisualTestContext) {
        editor_with(cx, profile, &[])
    }

    /// The editor with proxies to choose from.
    ///
    /// The profile's own core is in the list, so the form opens on it instead of
    /// reading as a profile whose core went missing.
    fn editor_with<'a>(
        cx: &'a mut TestAppContext,
        profile: &super::BrowserProfile,
        proxies: &[(ProxyId, String)],
    ) -> (Entity<ProfileEditor>, &'a mut VisualTestContext) {
        let cores = vec![choice(profile.core_id, "chrome 148", 148)];
        editor_full(cx, profile, &cores, proxies)
    }

    /// The editor over an explicit core list, for the cases the default one
    /// cannot express: several cores, or none of them the profile's own.
    fn editor_full<'a>(
        cx: &'a mut TestAppContext,
        profile: &super::BrowserProfile,
        cores: &[super::CoreChoice],
        proxies: &[(ProxyId, String)],
    ) -> (Entity<ProfileEditor>, &'a mut VisualTestContext) {
        let profile = profile.clone();
        let cores = cores.to_vec();
        let proxies = proxies.to_vec();
        cx.add_window_view(move |window, cx| {
            ProfileEditor::new(&profile, &cores, &proxies, en(), window, cx)
        })
    }

    /// The form for a profile that does not exist yet, on the first core given.
    fn new_editor<'a>(
        cx: &'a mut TestAppContext,
        cores: &[super::CoreChoice],
    ) -> (Entity<ProfileEditor>, &'a mut VisualTestContext) {
        let cores = cores.to_vec();
        let core = cores.first().expect("a core to open the form on").id;
        cx.add_window_view(move |window, cx| {
            ProfileEditor::new_profile("Profile 1", core, &cores, &[], en(), window, cx)
        })
    }

    /// The profile an edit form rebuilds.
    ///
    /// A form that creates rather than saves panics here: these tests are about
    /// the fields, not about which of the two results came back.
    fn built_profile(
        cx: &mut VisualTestContext,
        editor: &Entity<ProfileEditor>,
    ) -> super::BrowserProfile {
        match editor
            .read_with(cx, |editor, cx| editor.build(cx))
            .expect("a valid form")
        {
            super::ProfileEdit::Save(profile) => profile,
            super::ProfileEdit::Create(_) => panic!("this form creates a profile"),
        }
    }

    fn set(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
        let input = input.clone();
        let text = text.to_string();
        cx.update(|window, cx| {
            input.update(cx, |state, cx| state.set_value(text, window, cx));
        });
    }

    fn input(
        cx: &mut VisualTestContext,
        editor: &Entity<ProfileEditor>,
        pick: impl Fn(&ProfileEditor) -> Entity<InputState>,
    ) -> Entity<InputState> {
        editor.read_with(cx, |editor, _| pick(editor))
    }

    #[gpui_kit::test]
    fn the_form_rebuilds_the_profile_it_was_opened_on(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = profile();
        let (editor, cx) = editor(cx, &original);

        let rebuilt = built_profile(cx, &editor);

        assert_eq!(rebuilt.id, original.id);
        assert_eq!(rebuilt.core_id, original.core_id);
        assert_eq!(rebuilt.user_data_dir, original.user_data_dir);
        assert_eq!(rebuilt.start_target, original.start_target);
        assert_eq!(rebuilt.name, "Original");
        assert_eq!(rebuilt.fingerprint.seed, 4242);
        assert_eq!(rebuilt.window.width, 1280);
        assert_eq!(rebuilt.window.height, 800);
    }

    #[gpui_kit::test]
    fn every_edited_field_reaches_the_rebuilt_profile(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = profile();
        let (editor, cx) = editor(cx, &original);

        let (name, seed, brand_version, timezone, hc, width) = editor.read_with(cx, |editor, _| {
            (
                editor.name.clone(),
                editor.seed.clone(),
                editor.brand_version.clone(),
                editor.timezone.clone(),
                editor.hardware_concurrency.clone(),
                editor.window_width.clone(),
            )
        });
        set(cx, &name, "Edited name");
        set(cx, &seed, "777");
        set(cx, &brand_version, "148.0.0.0");
        set(cx, &timezone, "Europe/Berlin");
        set(cx, &hc, "4");
        set(cx, &width, "1600");
        editor.update(cx, |editor, cx| {
            editor.brand = BrowserBrand::Edge;
            editor.platform = Platform::MacOs;
            editor.webrtc_policy = WebRtcPolicy::DefaultPublicInterfaceOnly;
            editor.toggle_spoofing(SpoofingFeature::Canvas);
            cx.notify();
        });

        let rebuilt = built_profile(cx, &editor);

        assert_eq!(rebuilt.name, "Edited name");
        assert_eq!(rebuilt.fingerprint.seed, 777);
        assert_eq!(rebuilt.fingerprint.brand, BrowserBrand::Edge);
        assert_eq!(
            rebuilt.fingerprint.brand_version.as_deref(),
            Some("148.0.0.0")
        );
        assert_eq!(rebuilt.fingerprint.platform, Platform::MacOs);
        assert_eq!(rebuilt.fingerprint.timezone, "Europe/Berlin");
        assert_eq!(rebuilt.fingerprint.hardware_concurrency, Some(4));
        assert_eq!(
            rebuilt.fingerprint.webrtc_policy,
            WebRtcPolicy::DefaultPublicInterfaceOnly
        );
        assert_eq!(
            rebuilt.fingerprint.disabled_spoofing,
            vec![SpoofingFeature::Canvas]
        );
        assert_eq!(rebuilt.window.width, 1600);
        assert_eq!(rebuilt.window.height, 800);
    }

    #[gpui_kit::test]
    fn a_field_the_engine_cannot_read_is_refused_with_a_reason(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = profile();
        let (editor, cx) = editor(cx, &original);

        let seed = input(cx, &editor, |editor| editor.seed.clone());
        set(cx, &seed, "not a number");

        let error = editor
            .read_with(cx, |editor, cx| editor.build(cx))
            .expect_err("a seed that is not a number cannot be saved");

        assert!(error.contains("seed"), "unhelpful message: {error}");
        assert!(
            editor.read_with(cx, |editor, _| editor.error().is_none()),
            "the form only shows an error once a save was attempted"
        );
    }

    #[gpui_kit::test]
    fn the_domain_rules_still_apply(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = profile();
        let (editor, cx) = editor(cx, &original);

        let name = input(cx, &editor, |editor| editor.name.clone());
        let width = input(cx, &editor, |editor| editor.window_width.clone());

        set(cx, &name, "   ");
        assert!(
            editor.read_with(cx, |editor, cx| editor.build(cx)).is_err(),
            "a blank name is refused by the domain rules"
        );

        set(cx, &name, "Fine");
        set(cx, &width, "0");
        assert!(
            editor.read_with(cx, |editor, cx| editor.build(cx)).is_err(),
            "a zero-sized window is refused by the domain rules"
        );
    }

    #[gpui_kit::test]
    fn a_new_seed_is_different_and_still_valid(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = profile();
        let (editor, cx) = editor(cx, &original);

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.reroll_seed(window, cx);
                cx.notify();
            });
        });

        let rebuilt = built_profile(cx, &editor);

        assert_ne!(rebuilt.fingerprint.seed, 4242);
    }

    #[gpui_kit::test]
    fn the_proxy_the_profile_is_on_is_what_the_form_offers(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let proxy = ProxyId::new();
        let mut original = profile();
        original.proxy_id = Some(proxy);
        let proxies = vec![
            (proxy, "Office".to_string()),
            (ProxyId::new(), "Home".to_string()),
        ];
        let (editor, cx) = editor_with(cx, &original, &proxies);

        let rebuilt = built_profile(cx, &editor);
        assert_eq!(
            rebuilt.proxy_id,
            Some(proxy),
            "an edit that does not touch the assignment keeps it"
        );
    }

    /// The chip's id carries the index; its label must not.
    ///
    /// The first version built the id as `editor-proxy-{index}-{name}` and then
    /// showed that same string, so the window offered a proxy called
    /// "0-Office". The tests passed, because they clicked the string they had
    /// built; only the real window showed the mistake.
    #[test]
    fn a_proxy_chip_is_labelled_with_its_name_alone() {
        let (id, label) = super::proxy_chip(0, "Office");
        assert_eq!(id, "editor-proxy-0");
        assert_eq!(label, "Office");
        assert!(!label.contains('0'), "the index must not reach the label");
        let (_, label) = super::proxy_chip(3, "Home");
        assert_eq!(label, "Home");
    }

    #[gpui_kit::test]
    fn choosing_a_proxy_and_choosing_direct_change_the_assignment(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let proxy = ProxyId::new();
        let proxies = vec![(proxy, "Office".to_string())];
        let (editor, cx) = editor_with(cx, &profile(), &proxies);
        assert!(
            built_profile(cx, &editor).proxy_id.is_none(),
            "a profile with no proxy starts on Direct"
        );

        click(cx, "editor-proxy-0");
        assert_eq!(
            built_profile(cx, &editor).proxy_id,
            Some(proxy),
            "picking a proxy assigns it"
        );

        click(cx, "editor-proxy-direct");
        assert_eq!(
            built_profile(cx, &editor).proxy_id,
            None,
            "picking Direct clears the assignment"
        );
    }

    /// A profile assigned to a proxy that is no longer stored must not read as
    /// Direct: it is shown as missing instead.
    #[gpui_kit::test]
    fn an_assignment_to_a_missing_proxy_is_kept_and_shown(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let gone = ProxyId::new();
        let mut original = profile();
        original.proxy_id = Some(gone);
        let (editor, cx) = editor_with(cx, &original, &[]);

        assert_eq!(
            built_profile(cx, &editor).proxy_id,
            Some(gone),
            "the assignment survives even though the proxy is gone"
        );
        assert!(
            editor
                .read_with(cx, |editor, _| editor.proxy)
                .is_some_and(|id| id == gone)
        );
    }

    /// A new profile starts from the defaults and names the core it will run on.
    ///
    /// What a create form asks for is a `NewProfile`, not a `BrowserProfile`: the
    /// id, the data directory and the start target belong to the service, and a
    /// form that filled them in itself would be inventing state the user cannot
    /// see or cancel.
    #[gpui_kit::test]
    fn a_new_form_asks_for_a_profile_rather_than_inventing_one(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let core = CoreId::new();
        let (editor, cx) = new_editor(cx, &[choice(core, "chrome 148", 148)]);

        let draft = created(&editor, cx);

        assert!(editor.read_with(cx, |editor, _| editor.is_new()));
        assert_eq!(draft.name, "Profile 1");
        assert_eq!(
            draft.core_id, core,
            "the core the form opened on is the one the profile is created on"
        );
        assert!(
            draft.user_data_dir.is_none(),
            "the data directory is the service's to make"
        );
        assert!(draft.start_target.is_none(), "and so is the start target");
        assert!(
            draft.window.is_some(),
            "the window size comes from the form"
        );
        let fingerprint = draft.fingerprint.clone().expect("a fingerprint");
        assert_eq!(fingerprint.brand, BrowserBrand::Chrome);
        assert_eq!(fingerprint.platform, Platform::Windows);
        assert_eq!(fingerprint.language, "en-US");
    }

    /// The seed on the form is the seed the profile gets.
    ///
    /// The service used to roll its own when a draft carried none, so a form that
    /// showed a seed could still create a profile with a different one.
    #[gpui_kit::test]
    fn the_seed_on_a_new_form_is_the_seed_the_profile_is_created_with(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (editor, cx) = new_editor(cx, &[choice(CoreId::new(), "chrome 148", 148)]);

        let seed = input(cx, &editor, |editor| editor.seed.clone());
        set(cx, &seed, "31415");
        let drafted = created(&editor, cx);

        assert_eq!(
            drafted.fingerprint.expect("a fingerprint").seed,
            31415,
            "the created profile gets the seed the form showed"
        );
    }

    #[gpui_kit::test]
    fn choosing_another_core_changes_which_engine_the_profile_is_created_on(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let first = CoreId::new();
        let second = CoreId::new();
        let (editor, cx) = new_editor(
            cx,
            &[
                choice(first, "chrome 148", 148),
                choice(second, "chrome 142", 142),
            ],
        );

        assert_eq!(
            created(&editor, cx).core_id,
            first,
            "the first core is the default"
        );

        click(cx, "editor-core-1");
        assert_eq!(
            created(&editor, cx).core_id,
            second,
            "picking the other core creates the profile on it"
        );
    }

    /// A new form is refused by the same rules an edit is.
    #[gpui_kit::test]
    fn a_new_form_still_refuses_what_the_domain_refuses(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (editor, cx) = new_editor(cx, &[choice(CoreId::new(), "chrome 148", 148)]);

        let name = input(cx, &editor, |editor| editor.name.clone());
        set(cx, &name, "   ");

        assert!(
            editor.read_with(cx, |editor, cx| editor.build(cx)).is_err(),
            "a blank name is refused before anything is created"
        );
    }

    /// The form says whether the core in force honours the exclusions.
    ///
    /// A legacy engine accepts `--disable-spoofing` and applies the noise anyway,
    /// so a checkbox that looked effective would be a lie the launch warning then
    /// had to correct.
    #[gpui_kit::test]
    fn the_form_says_when_a_core_ignores_the_exclusions(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let modern = choice(CoreId::new(), "chrome 148", 148);
        let (editor, cx) = new_editor(cx, std::slice::from_ref(&modern));
        let note = editor.read_with(cx, |editor, _| editor.core_note());
        assert!(note.contains("spoofing exclusions honoured"), "{note}");
        assert!(!note.contains("ignored"), "{note}");

        let legacy = super::CoreChoice {
            exclusions_honoured: false,
            ..choice(CoreId::new(), "chrome 142", 142)
        };
        let (editor, cx) = new_editor(cx, &[legacy]);
        let note = editor.read_with(cx, |editor, _| editor.core_note());
        assert!(
            note.contains("ignored by this engine"),
            "the form says the exclusions will not be applied: {note}"
        );
    }

    /// The chip says which core it stands for.
    ///
    /// The automatic name a core gets already carries its major; repeating it
    /// would put "chrome 148 (Chrome 148)" in the form.
    #[test]
    fn a_core_chip_names_the_core_without_repeating_an_automatic_major() {
        assert_eq!(
            choice(CoreId::new(), "chrome 148", 148).label(en()),
            "chrome 148"
        );
        assert_eq!(
            choice(CoreId::new(), "Daily driver", 148).label(en()),
            "Daily driver (Chrome 148)",
            "a name the user chose does not carry the major, so the chip adds it"
        );
        assert_eq!(
            choice(CoreId::new(), "chrome", 0).label(en()),
            "chrome (version unknown)"
        );
    }

    /// A core whose version was never read is a real choice, not a hidden one.
    #[gpui_kit::test]
    fn a_core_with_no_version_is_shown_as_one_that_cannot_be_started(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let unread = super::CoreChoice {
            generation: None,
            exclusions_honoured: false,
            ..choice(CoreId::new(), "chrome", 0)
        };
        let (editor, cx) = new_editor(cx, &[unread]);

        assert_eq!(
            editor.read_with(cx, |editor, _| editor.cores[0].label(en())),
            "chrome (version unknown)"
        );
        let note = editor.read_with(cx, |editor, _| editor.core_note());
        assert!(note.contains("cannot be started"), "{note}");
    }

    #[gpui_kit::test]
    fn an_edit_can_move_a_profile_to_another_core(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = profile();
        let other = CoreId::new();
        let cores = vec![
            choice(original.core_id, "chrome 148", 148),
            choice(other, "chrome 142", 142),
        ];
        let (editor, cx) = editor_full(cx, &original, &cores, &[]);

        assert_eq!(
            built_profile(cx, &editor).core_id,
            original.core_id,
            "an edit that does not touch the core keeps it"
        );

        click(cx, "editor-core-1");
        let moved = built_profile(cx, &editor);
        assert_eq!(
            moved.core_id, other,
            "picking another core moves the profile"
        );
        assert_eq!(moved.id, original.id, "it is still the same profile");
    }

    /// A profile whose core was removed keeps it, and the form says so.
    #[gpui_kit::test]
    fn a_missing_core_is_kept_and_named(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let gone = CoreId::new();
        let mut original = profile();
        original.core_id = gone;
        let (editor, cx) = editor_full(cx, &original, &[], &[]);

        assert_eq!(
            built_profile(cx, &editor).core_id,
            gone,
            "the assignment survives even though the core is gone"
        );
        let note = editor.read_with(cx, |editor, _| editor.core_note());
        assert!(note.contains("no longer registered"), "{note}");
        assert!(
            cx.update(|window, _| window.try_find("editor-core-missing").is_some()),
            "the missing core is shown rather than silently replaced"
        );
    }

    fn created(
        editor: &Entity<ProfileEditor>,
        cx: &mut VisualTestContext,
    ) -> application::NewProfile {
        match editor
            .read_with(cx, |editor, cx| editor.build(cx))
            .expect("a valid new form")
        {
            ProfileEdit::Create(draft) => draft,
            ProfileEdit::Save(_) => panic!("this form saves a profile"),
        }
    }

    fn click(cx: &mut VisualTestContext, id: &str) {
        let id = id.to_string();
        cx.update(|window, cx| window.click(id, cx));
    }
}
