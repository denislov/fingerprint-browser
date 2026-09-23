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

/// Use the same OS entropy source as service-created profiles and duplicates.
fn profile_seed() -> u32 {
    application::profile_service::random_seed()
}

mod render;
#[cfg(test)]
use render::*;
#[cfg(test)]
mod tests;
