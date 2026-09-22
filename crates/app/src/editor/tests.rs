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
fn choosing_another_core_changes_which_engine_the_profile_is_created_on(cx: &mut TestAppContext) {
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

fn created(editor: &Entity<ProfileEditor>, cx: &mut VisualTestContext) -> application::NewProfile {
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
