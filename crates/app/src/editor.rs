//! The profile editor: the form behind "Edit profile".
//!
//! The editor owns an editable copy of one profile and turns it back into a
//! [`BrowserProfile`] when the user saves. It never writes to storage itself:
//! the view hands the result to [`crate::state::AppState`], and a rejection
//! comes back as a message the form shows without closing.
//!
//! Fields the profile already owns but this form does not edit (id, core, user
//! data directory, start target) are carried through untouched, so saving
//! cannot silently drop them.

use domain::{
    BrowserBrand, BrowserProfile, Platform, ProxyId, SpoofingFeature, WebRtcPolicy,
    validate_fingerprint, validate_profile, validate_window,
};
use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::form::*;
use gpui_kit::component::input::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

const LABEL_WIDTH: f32 = 170.0;

/// An editable copy of one profile.
pub struct ProfileEditor {
    /// Everything the form does not edit, kept verbatim.
    base: BrowserProfile,
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

const WEBRTC_POLICIES: [(WebRtcPolicy, &str); 3] = [
    (WebRtcPolicy::DisableNonProxiedUdp, "No non-proxied UDP"),
    (
        WebRtcPolicy::DefaultPublicInterfaceOnly,
        "Public interface only",
    ),
    (
        WebRtcPolicy::DefaultPublicAndPrivateInterfaces,
        "Public and private",
    ),
];

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
    /// The proxies are passed in because they live outside the profile: the
    /// form needs their names to offer a choice, and the id it stores is the
    /// only part of the assignment the profile owns.
    pub fn new(
        profile: &BrowserProfile,
        proxies: &[(ProxyId, String)],
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let fingerprint = &profile.fingerprint;
        let field = |value: &str, window: &mut Window, cx: &mut App| {
            cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()))
        };
        Self {
            base: profile.clone(),
            name: field(&profile.name, window, cx),
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
            window_width: field(&profile.window.width.to_string(), window, cx),
            window_height: field(&profile.window.height.to_string(), window, cx),
            brand: fingerprint.brand,
            platform: fingerprint.platform,
            webrtc_policy: fingerprint.webrtc_policy,
            disabled_spoofing: fingerprint.disabled_spoofing.clone(),
            proxies: proxies.to_vec(),
            proxy: profile.proxy_id,
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

    /// Turns the form back into a profile, or explains what is wrong with it.
    pub fn build_profile(&self, cx: &App) -> Result<BrowserProfile, String> {
        let text = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        let optional = |input: &Entity<InputState>| {
            let value = text(input);
            (!value.is_empty()).then_some(value)
        };

        let seed: u32 = text(&self.seed)
            .parse()
            .map_err(|_| "seed must be a whole number".to_string())?;
        let hardware_concurrency = match optional(&self.hardware_concurrency) {
            Some(value) => Some(
                value
                    .parse::<u8>()
                    .map_err(|_| "hardware concurrency must be a whole number".to_string())?,
            ),
            None => None,
        };
        let width: u32 = text(&self.window_width)
            .parse()
            .map_err(|_| "window width must be a whole number".to_string())?;
        let height: u32 = text(&self.window_height)
            .parse()
            .map_err(|_| "window height must be a whole number".to_string())?;

        let mut profile = self.base.clone();
        profile.name = text(&self.name);
        profile.window.width = width;
        profile.window.height = height;
        profile.fingerprint.seed = seed;
        profile.fingerprint.brand = self.brand;
        profile.fingerprint.brand_version = optional(&self.brand_version);
        profile.fingerprint.platform = self.platform;
        profile.fingerprint.platform_version = optional(&self.platform_version);
        profile.fingerprint.language = text(&self.language);
        profile.fingerprint.accept_language = text(&self.accept_language);
        profile.fingerprint.timezone = text(&self.timezone);
        profile.fingerprint.hardware_concurrency = hardware_concurrency;
        profile.fingerprint.webrtc_policy = self.webrtc_policy;
        profile.fingerprint.disabled_spoofing = self.disabled_spoofing.clone();
        profile.proxy_id = self.proxy;

        // The same rules storage enforces, run before anything is written.
        validate_profile(&profile)
            .and_then(|()| validate_fingerprint(&profile.fingerprint))
            .and_then(|()| validate_window(&profile.window))
            .map_err(|error| error.to_string())?;
        Ok(profile)
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
            "Direct".to_string(),
            selected.is_none(),
            None,
        ))
        .children(options.iter().enumerate().map(|(index, (id, name))| {
            let (chip_id, label) = proxy_chip(index, name);
            proxy_choice(
                editor.clone(),
                chip_id,
                label,
                selected == Some(*id),
                Some(*id),
            )
        }))
        .children(missing.map(|id| {
            proxy_choice(
                editor.clone(),
                "editor-proxy-missing".to_string(),
                format!("(missing proxy {id})"),
                true,
                Some(id),
            )
        }))
}

fn proxy_choice(
    editor: Entity<ProfileEditor>,
    id: String,
    label: String,
    active: bool,
    value: Option<ProxyId>,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .px_3()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(if active { 0x52525b } else { 0x27272a }))
        .text_xs()
        .when(active, |this| {
            this.bg(rgb(0x27272a))
                .text_color(rgb(0xf4f4f5))
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |this| this.text_color(rgb(0x71717a)))
        .child(label)
        .on_click(move |_, _, cx| {
            editor.update(cx, |editor, cx| {
                editor.proxy = value;
                cx.notify();
            });
        })
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
                .border_color(rgb(if active { 0x52525b } else { 0x27272a }))
                .text_xs()
                .when(active, |this| {
                    this.bg(rgb(0x27272a))
                        .text_color(rgb(0xf4f4f5))
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!active, |this| this.text_color(rgb(0x71717a)))
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
        let editor = cx.entity();
        let mut form = Form::new()
            .label_layout(Axis::Horizontal)
            .label_width(px(LABEL_WIDTH))
            .child(
                Field::new().label("Name").child(
                    Input::new(&self.name)
                        .id("editor-name")
                        .aria_label("Profile name"),
                ),
            )
            .child(
                Field::new()
                    .label("Seed")
                    .description("Drives every noisy surface the profile claims.")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Input::new(&self.seed)
                                    .id("editor-seed")
                                    .aria_label("Fingerprint seed"),
                            )
                            .child(
                                Button::new("editor-reroll")
                                    .label("New seed")
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
            .child(Field::new().label("Brand").child(choice_row(
                editor.clone(),
                "editor-brand",
                &BRANDS,
                self.brand,
                |editor, value| editor.brand = value,
            )))
            .child(
                Field::new()
                    .label("Brand version")
                    .description(
                        "Reported to pages only when a brand is set. Blank means the engine's own.",
                    )
                    .child(
                        Input::new(&self.brand_version)
                            .id("editor-brand-version")
                            .aria_label("Brand version"),
                    ),
            )
            .child(Field::new().label("Platform").child(choice_row(
                editor.clone(),
                "editor-platform",
                &PLATFORMS,
                self.platform,
                |editor, value| editor.platform = value,
            )))
            .child(
                Field::new().label("Platform version").child(
                    Input::new(&self.platform_version)
                        .id("editor-platform-version")
                        .aria_label("Platform version"),
                ),
            )
            .child(
                Field::new().label("Language").child(
                    Input::new(&self.language)
                        .id("editor-language")
                        .aria_label("Language"),
                ),
            )
            .child(
                Field::new()
                    .label("Accept language")
                    .description("What navigator.language reports; the first entry wins.")
                    .child(
                        Input::new(&self.accept_language)
                            .id("editor-accept-language")
                            .aria_label("Accept language"),
                    ),
            )
            .child(
                Field::new().label("Timezone").child(
                    Input::new(&self.timezone)
                        .id("editor-timezone")
                        .aria_label("Timezone"),
                ),
            )
            .child(
                Field::new()
                    .label("CPU cores")
                    .description("Blank leaves the engine's own value.")
                    .child(
                        Input::new(&self.hardware_concurrency)
                            .id("editor-hardware-concurrency")
                            .aria_label("Hardware concurrency"),
                    ),
            )
            .child(
                Field::new().label("Window").child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div().w(px(90.0)).child(
                                Input::new(&self.window_width)
                                    .id("editor-window-width")
                                    .aria_label("Window width"),
                            ),
                        )
                        .child(div().text_xs().text_color(rgb(0x71717a)).child("x"))
                        .child(
                            div().w(px(90.0)).child(
                                Input::new(&self.window_height)
                                    .id("editor-window-height")
                                    .aria_label("Window height"),
                            ),
                        ),
                ),
            )
            .child(
                Field::new()
                    .label("Proxy")
                    .description(
                        "Chosen when the profile starts. A change applies to the next start.",
                    )
                    .child(proxy_row(editor.clone(), self.proxy, &self.proxies)),
            )
            .child(Field::new().label("WebRTC").child(choice_row(
                editor.clone(),
                "editor-webrtc",
                &WEBRTC_POLICIES,
                self.webrtc_policy,
                |editor, value| editor.webrtc_policy = value,
            )))
            .child(
                Field::new()
                    .label("Excluded spoofing")
                    .description("Features the engine must leave at the host's real value.")
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
                    .text_color(rgb(0xf87171))
                    .child(error),
            );
        }
        form
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: a glob here pulls the whole gpui surface into the test
    // macro's expansion and makes it recurse.
    use super::ProfileEditor;
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

    /// The editor inside a window, so its inputs can be driven.
    fn editor<'a>(
        cx: &'a mut TestAppContext,
        profile: &super::BrowserProfile,
    ) -> (Entity<ProfileEditor>, &'a mut VisualTestContext) {
        editor_with(cx, profile, &[])
    }

    /// The editor with proxies to choose from.
    fn editor_with<'a>(
        cx: &'a mut TestAppContext,
        profile: &super::BrowserProfile,
        proxies: &[(ProxyId, String)],
    ) -> (Entity<ProfileEditor>, &'a mut VisualTestContext) {
        let profile = profile.clone();
        let proxies = proxies.to_vec();
        cx.add_window_view(move |window, cx| ProfileEditor::new(&profile, &proxies, window, cx))
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

        let rebuilt = editor
            .read_with(cx, |editor, cx| editor.build_profile(cx))
            .expect("an untouched form is valid");

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

        let rebuilt = editor
            .read_with(cx, |editor, cx| editor.build_profile(cx))
            .expect("the edited form is valid");

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
            .read_with(cx, |editor, cx| editor.build_profile(cx))
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
            editor
                .read_with(cx, |editor, cx| editor.build_profile(cx))
                .is_err(),
            "a blank name is refused by the domain rules"
        );

        set(cx, &name, "Fine");
        set(cx, &width, "0");
        assert!(
            editor
                .read_with(cx, |editor, cx| editor.build_profile(cx))
                .is_err(),
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

        let rebuilt = editor
            .read_with(cx, |editor, cx| editor.build_profile(cx))
            .expect("a re-rolled seed is valid");

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

        let rebuilt = editor
            .read_with(cx, |editor, cx| editor.build_profile(cx))
            .expect("valid");
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
            editor
                .read_with(cx, |editor, cx| editor.build_profile(cx))
                .expect("valid")
                .proxy_id
                .is_none(),
            "a profile with no proxy starts on Direct"
        );

        click(cx, "editor-proxy-0");
        assert_eq!(
            editor
                .read_with(cx, |editor, cx| editor.build_profile(cx))
                .expect("valid")
                .proxy_id,
            Some(proxy),
            "picking a proxy assigns it"
        );

        click(cx, "editor-proxy-direct");
        assert_eq!(
            editor
                .read_with(cx, |editor, cx| editor.build_profile(cx))
                .expect("valid")
                .proxy_id,
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
            editor
                .read_with(cx, |editor, cx| editor.build_profile(cx))
                .expect("valid")
                .proxy_id,
            Some(gone),
            "the assignment survives even though the proxy is gone"
        );
        assert!(
            editor
                .read_with(cx, |editor, _| editor.proxy)
                .is_some_and(|id| id == gone)
        );
    }

    fn click(cx: &mut VisualTestContext, id: &str) {
        let id = id.to_string();
        cx.update(|window, cx| window.click(id, cx));
    }
}
