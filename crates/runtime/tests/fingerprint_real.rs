//! Reads a fingerprint back out of a real fingerprint-chromium and compares it
//! with what the profile asked for.
//!
//! CHROMIUM_BIN=/path/chrome cargo test -p runtime --test fingerprint_real -- --ignored
//!
//! The switch vocabulary of a fingerprint engine is not documented and changes
//! between builds, so nothing here is asserted from the command line alone:
//! every expectation is a value read out of the page over CDP.
#![cfg(target_os = "linux")]

use ::runtime::supervisor::SupervisorComponents;
use ::runtime::*;
use domain::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Runs the browser without a display; the fingerprint surface under test does
/// not need one, and a headless session keeps the run reproducible.
struct HeadlessPlanner;
impl LaunchPlanner for HeadlessPlanner {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
        let mut plan = DefaultLaunchPlanner.build(ctx)?;
        for flag in [
            "--headless=new",
            "--disable-gpu",
            "--disable-background-networking",
        ] {
            plan.browser_args.insert(0, flag.into());
        }
        Ok(plan)
    }
}

/// Pins the capability table, so a measurement does not depend on the table
/// under test. The product's own table is what the measurement is compared
/// with afterwards, never what produces the readings.
struct FixedCapabilities(CoreCapabilities);

impl CapabilityResolver for FixedCapabilities {
    fn resolve(&self, _core: &BrowserCore) -> Result<CoreCapabilities, CapabilityError> {
        Ok(self.0.clone())
    }
}

/// Adds raw switches to the launch line.
///
/// The capability table decides what the *product* emits. Measuring a switch on
/// its own terms needs it added directly, so the answer does not depend on the
/// very table under test - a table that wrongly says "supported" would
/// otherwise hide the engine ignoring it.
struct RawArgsPlanner(Vec<String>);

impl LaunchPlanner for RawArgsPlanner {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
        let mut plan = HeadlessPlanner.build(ctx)?;
        for flag in self.0.iter().rev() {
            plan.browser_args.insert(0, flag.clone().into());
        }
        Ok(plan)
    }
}

/// A planner that re-enables the leaking ICE policy, to prove the probe can
/// see a leak at all. Nothing in the product emits this.
struct LeakyWebRtcPlanner;
impl LaunchPlanner for LeakyWebRtcPlanner {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
        let mut plan = HeadlessPlanner.build(ctx)?;
        plan.browser_args
            .insert(0, "--webrtc-ip-handling-policy=default".into());
        Ok(plan)
    }
}

struct Harness {
    facade: ChannelRuntimeFacade,
    sender: crossbeam_channel::Sender<RuntimeCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
    dir: PathBuf,
    core: BrowserCore,
}

impl Harness {
    /// `major` decides the capability table the supervisor resolves against, so
    /// a test can ask for a legacy or a verified core.
    fn with_major(major: u32) -> Self {
        Self::with_planner(major, Box::new(HeadlessPlanner))
    }

    fn with_planner(major: u32, planner: Box<dyn LaunchPlanner>) -> Self {
        Self::with_capabilities(major, planner, CoreCapabilities::for_major(major))
    }

    /// The harness with the table pinned, for measuring the engine itself.
    fn with_capabilities(
        major: u32,
        planner: Box<dyn LaunchPlanner>,
        capabilities: CoreCapabilities,
    ) -> Self {
        let dir = std::env::temp_dir().join(format!("fp-fingerprint-{}", ProfileId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let channels = RuntimeSupervisorChannels::new(128);
        let snapshots = Arc::new(RwLock::new(HashMap::new()));
        let facade = ChannelRuntimeFacade::new(channels.command_tx.clone(), snapshots.clone());
        let supervisor = RuntimeSupervisor::with_components(
            channels.command_rx,
            channels.event_tx,
            snapshots,
            SupervisorComponents {
                planner,
                capability_resolver: Box::new(FixedCapabilities(capabilities)),
                // No proxy is assigned in these tests, so Xray is never spawned.
                xray_executable: dir.join("unused-xray"),
                runtime_dir: dir.join("runtime"),
                ..Default::default()
            },
        );
        Self {
            facade,
            sender: channels.command_tx,
            thread: Some(supervisor.spawn()),
            dir,
            core: BrowserCore {
                id: CoreId::new(),
                name: format!("real fingerprint-chromium {major}"),
                executable: std::env::var_os("CHROMIUM_BIN")
                    .expect("set CHROMIUM_BIN")
                    .into(),
                version: major.to_string(),
                major,
            },
        }
    }

    /// The major the binary under test reports.
    ///
    /// Probing the binary rather than assuming a number is what lets this suite
    /// be run against any build: every expectation below is the table's claim
    /// for *that* major, so a wrong table fails here instead of hiding.
    fn detected_major() -> u32 {
        let binary = PathBuf::from(std::env::var_os("CHROMIUM_BIN").expect("set CHROMIUM_BIN"));
        ::runtime::version::VersionReport::probe(&binary, ::runtime::version::DEFAULT_TIMEOUT)
            .major
            .expect("the binary must report a version with --version")
    }

    /// The harness for the binary under test, at its own major.
    fn detected() -> Self {
        Self::with_major(Self::detected_major())
    }

    fn verified() -> Self {
        Self::detected()
    }

    /// Verified capabilities with the ICE policy forced back open.
    fn verified_with_leaking_webrtc() -> Self {
        Self::with_planner(Self::detected_major(), Box::new(LeakyWebRtcPlanner))
    }

    /// A document for the probe to run on, reachable as a `file:` URL.
    fn probe_document(&self) -> String {
        let path = self.dir.join("probe.html");
        if !path.exists() {
            std::fs::write(
                &path,
                "<!doctype html><meta charset=utf-8><title>probe</title>",
            )
            .expect("the probe document is writable");
        }
        format!("file://{}", path.display())
    }

    fn profile(&self, seed: u32, fingerprint: FingerprintProfile) -> BrowserProfile {
        let id = ProfileId::new();
        BrowserProfile {
            id,
            name: format!("fingerprint-{seed}"),
            core_id: self.core.id,
            user_data_dir: self.dir.join(id.to_string()),
            fingerprint: FingerprintProfile {
                seed,
                ..fingerprint
            },
            proxy_id: None,
            window: WindowProfile::new(800, 600),
            start_target: StartTarget::Blank,
        }
    }

    /// Starts a profile, waits for it to run, and reads its fingerprint back.
    ///
    /// The reading is taken on a document served from the harness directory: a
    /// blank page has no user agent data surface, so the brand claims would come
    /// back unverifiable.
    fn read(&self, profile: &BrowserProfile) -> (RuntimeSnapshot, ObservedFingerprint) {
        self.facade
            .start(StartParams::new(profile.clone(), self.core.clone()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let snapshot = loop {
            if let Some(snapshot) = self.facade.snapshot(profile.id) {
                assert!(
                    !matches!(snapshot.state, RuntimeState::Failed { .. }),
                    "startup failed: {snapshot:?}"
                );
                if snapshot.state == RuntimeState::Running && snapshot.cdp_port.is_some() {
                    break snapshot;
                }
            }
            assert!(
                Instant::now() < deadline,
                "waiting for running: {:?}",
                self.facade.snapshot(profile.id)
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        let port = snapshot.cdp_port.expect("a running profile has a CDP port");
        let observed = FingerprintProbe::default()
            .read_at(port, &self.probe_document())
            .expect("the fingerprint can be read back");
        (snapshot, observed)
    }

    fn stop(&self, id: ProfileId) {
        self.facade.stop(id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if self
                .facade
                .snapshot(id)
                .is_none_or(|snapshot| snapshot.state == RuntimeState::Stopped)
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("profile did not stop");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.sender.send(RuntimeCommand::ShutdownAll);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The profile defaults describe a Windows desktops with the locale and
/// timezone set; the engine is asked to reproduce exactly that.
#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn a_verified_core_honours_every_profile_claim() {
    let harness = Harness::verified();
    let profile = harness.profile(11111, FingerprintProfile::new_random(0));
    let (snapshot, observed) = harness.read(&profile);

    let capabilities = CoreCapabilities::for_major(harness.core.major);
    let discrepancies = verify_fingerprint(&profile.fingerprint, &capabilities, &observed);
    assert!(
        discrepancies.is_empty(),
        "the engine did not reproduce the profile: {discrepancies:#?}\nobserved: {observed:#?}"
    );

    // The seed is what makes the canvas surface unique; a core that reports no
    // noise would pass the checks above and still be traceable.
    assert!(
        observed
            .measure_text
            .is_some_and(|width| width.fract() != 0.0),
        "the seed must perturb the canvas text measurement: {observed:#?}"
    );
    assert!(
        observed.has_rect_noise(),
        "the seed must perturb client rects: {observed:#?}"
    );

    let args = snapshot.effective_args.join(" ");
    assert!(
        args.contains("--fingerprinting-canvas-image-data-noise"),
        "the verified generation requests the noise switch: {args}"
    );
    assert!(
        snapshot.last_warning.is_none(),
        "a verified core has nothing to report: {:?}",
        snapshot.last_warning
    );
}

#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn two_profiles_do_not_share_a_canvas_surface() {
    let harness = Harness::verified();
    let first = harness.profile(11111, FingerprintProfile::new_random(0));
    let second = harness.profile(22222, FingerprintProfile::new_random(0));

    let (_, first_observed) = harness.read(&first);
    let (_, second_observed) = harness.read(&second);

    assert_ne!(
        first_observed.canvas_signature(),
        second_observed.canvas_signature(),
        "two seeds produced the same canvas surface: {first_observed:#?} {second_observed:#?}"
    );
    // Reading the same session again must not drift, or a page could watch the
    // fingerprint change between visits.
    let port = harness
        .facade
        .snapshot(first.id)
        .and_then(|snapshot| snapshot.cdp_port)
        .expect("the first profile is still running");
    let reread = FingerprintProbe::default()
        .read_at(port, &harness.probe_document())
        .expect("the fingerprint can be read twice");
    assert_eq!(
        reread.canvas_signature(),
        first_observed.canvas_signature(),
        "a seed must produce a stable canvas surface"
    );

    harness.stop(first.id);
    harness.stop(second.id);
}

#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn disabling_canvas_spoofing_makes_the_canvas_seed_independent() {
    // The exclusion is a verified-generation feature: 148 honours it, 142
    // accepts and ignores it. What the product does must follow the table, so
    // the expectation follows the binary's own major.
    let harness = Harness::detected();
    let supported = CoreCapabilities::for_major(harness.core.major).supports_disable_spoofing;
    let mut fingerprint = FingerprintProfile::new_random(0);
    fingerprint.disabled_spoofing = vec![SpoofingFeature::Canvas];

    let first = harness.profile(11111, fingerprint.clone());
    let second = harness.profile(22222, fingerprint.clone());
    let (spoofing_disabled, first_observed) = harness.read(&first);
    let (_, second_observed) = harness.read(&second);

    let emitted = spoofing_disabled
        .effective_args
        .iter()
        .any(|arg| arg == "--disable-spoofing=canvas");
    assert_eq!(
        emitted, supported,
        "the command line must follow the table: {:?}",
        spoofing_disabled.effective_args
    );

    // Two seeds with nothing excluded, so the comparisons below are against a
    // canvas that is known to move.
    let spoonfed = harness.profile(11111, FingerprintProfile::new_random(0));
    let other = harness.profile(22222, FingerprintProfile::new_random(0));
    let (_, spoonfed_observed) = harness.read(&spoonfed);
    let (_, other_observed) = harness.read(&other);
    assert_ne!(
        spoonfed_observed.canvas_signature(),
        other_observed.canvas_signature(),
        "the seed must move the canvas when nothing is excluded"
    );

    if supported {
        assert_eq!(
            first_observed.canvas_signature(),
            second_observed.canvas_signature(),
            "with canvas spoofing disabled the seed must not reach the canvas: {first_observed:#?} {second_observed:#?}"
        );
        assert_eq!(
            spoofing_disabled.last_warning, None,
            "a core that honours the exclusion has nothing to report"
        );
    } else {
        assert_ne!(
            first_observed.canvas_signature(),
            second_observed.canvas_signature(),
            "an unsupported exclusion must not be claimed as honoured: {first_observed:#?}"
        );
        let warning = spoofing_disabled
            .last_warning
            .clone()
            .expect("the omission is reported");
        assert!(warning.contains("--disable-spoofing"), "{warning}");
    }
    harness.stop(spoofing_disabled.profile_id);
    harness.stop(spoonfed.id);
    harness.stop(other.id);
}

#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn a_macos_profile_is_not_left_on_the_host_platform() {
    // `mac` is accepted by the engine and ignored, which would leave a macOS
    // profile advertising the host platform: the regression this guards.
    let harness = Harness::verified();
    let mut fingerprint = FingerprintProfile::new_random(0);
    fingerprint.platform = Platform::MacOs;
    fingerprint.platform_version = Some("15.6.0".to_string());
    let profile = harness.profile(11111, fingerprint);

    let (snapshot, observed) = harness.read(&profile);

    assert_eq!(
        observed.platform.as_deref(),
        Some("MacIntel"),
        "a macOS profile must not stay on the host platform: {observed:#?}"
    );
    assert!(
        observed
            .user_agent
            .as_deref()
            .is_some_and(|agent| agent.contains("Macintosh")),
        "the user agent must agree with the platform: {observed:#?}"
    );
    assert_eq!(observed.platform_version.as_deref(), Some("15.6.0"));
    assert!(
        snapshot
            .effective_args
            .iter()
            .any(|arg| arg == "--fingerprint-platform=macos"),
        "the engine's spelling reaches the command line: {:?}",
        snapshot.effective_args
    );
    harness.stop(snapshot.profile_id);
}

#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn excluding_client_rects_removes_only_that_noise() {
    // `client-rects` is accepted by the engine and ignored; only the
    // unhyphenated token turns the ClientRects noise off - on a generation that
    // honours the exclusion at all.
    let harness = Harness::detected();
    let supported = CoreCapabilities::for_major(harness.core.major).supports_disable_spoofing;
    let mut fingerprint = FingerprintProfile::new_random(0);
    fingerprint.disabled_spoofing = vec![SpoofingFeature::ClientRects];
    let profile = harness.profile(11111, fingerprint);

    let (snapshot, observed) = harness.read(&profile);

    assert_eq!(
        snapshot
            .effective_args
            .iter()
            .any(|arg| arg == "--disable-spoofing=clientrects"),
        supported,
        "the engine's spelling reaches the command line only when the table says          the generation honours it: {:?}",
        snapshot.effective_args
    );
    if supported {
        assert!(
            !observed.has_rect_noise(),
            "client rects must be exact when the exclusion is honoured: {observed:#?}"
        );
    } else {
        assert!(
            observed.has_rect_noise(),
            "an ignored exclusion must not be reported as honoured: {observed:#?}"
        );
    }
    assert!(
        observed
            .measure_text
            .is_some_and(|width| width.fract() != 0.0),
        "the canvas is a separate surface and stays perturbed: {observed:#?}"
    );
    harness.stop(snapshot.profile_id);
}

#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn audio_spoofing_is_seed_driven_and_can_be_excluded() {
    let harness = Harness::detected();
    let supported = CoreCapabilities::for_major(harness.core.major).supports_disable_spoofing;
    let mut excluded = FingerprintProfile::new_random(0);
    excluded.disabled_spoofing = vec![SpoofingFeature::Audio];

    let first = harness.profile(11111, excluded.clone());
    let second = harness.profile(22222, excluded.clone());
    let (excluded_snapshot, first_observed) = harness.read(&first);
    let (_, second_observed) = harness.read(&second);

    assert!(
        first_observed.audio_signature().0.is_some(),
        "the audio surface must be readable: {first_observed:#?}"
    );

    // Two seeds with nothing excluded, so the comparisons below are against an
    // audio surface that is known to move.
    let spoonfed = harness.profile(11111, FingerprintProfile::new_random(0));
    let other = harness.profile(22222, FingerprintProfile::new_random(0));
    let (_, spoonfed_observed) = harness.read(&spoonfed);
    let (_, other_observed) = harness.read(&other);
    assert_ne!(
        spoonfed_observed.audio_signature(),
        other_observed.audio_signature(),
        "the seed must move the audio fingerprint when nothing is excluded"
    );

    assert_eq!(
        excluded_snapshot
            .effective_args
            .iter()
            .any(|arg| arg == "--disable-spoofing=audio"),
        supported,
        "the command line must follow the table: {:?}",
        excluded_snapshot.effective_args
    );
    if supported {
        assert_eq!(
            first_observed.audio_signature(),
            second_observed.audio_signature(),
            "with audio spoofing excluded the seed must not reach the audio fingerprint"
        );
    } else {
        assert_ne!(
            first_observed.audio_signature(),
            second_observed.audio_signature(),
            "an ignored exclusion must not be claimed as honoured"
        );
    }
    harness.stop(excluded_snapshot.profile_id);
    harness.stop(spoonfed.id);
    harness.stop(other.id);
}

#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn no_ice_candidate_leaks_an_address_on_the_product_path() {
    // First prove the probe can see a leak, or an empty candidate list means
    // nothing.
    let leaky = Harness::verified_with_leaking_webrtc();
    let mut relaxed = FingerprintProfile::new_random(0);
    relaxed.webrtc_policy = WebRtcPolicy::DefaultPublicAndPrivateInterfaces;
    let (_, leaky_observed) = leaky.read(&leaky.profile(11111, relaxed));

    assert_eq!(
        leaky_observed.webrtc_gathering.as_deref(),
        Some("complete"),
        "the probe waits for gathering to finish: {leaky_observed:#?}"
    );
    assert!(
        !leaky_observed.leaking_candidates().is_empty(),
        "with the policy re-opened the probe must see the machine's own \
         addresses, otherwise the check below is blind: {leaky_observed:#?}"
    );
    drop(leaky);

    // Now the product path: the default profile asks for the strict policy.
    let harness = Harness::verified();
    let profile = harness.profile(11111, FingerprintProfile::new_random(0));
    let (snapshot, observed) = harness.read(&profile);

    assert!(
        snapshot
            .effective_args
            .iter()
            .any(|arg| arg == "--disable-non-proxied-udp"),
        "the strict policy reaches the command line: {:?}",
        snapshot.effective_args
    );
    assert!(
        observed.leaking_candidates().is_empty(),
        "a local or public address reached the page: {:#?}",
        observed.leaking_candidates()
    );
    let discrepancies = verify_fingerprint(
        &profile.fingerprint,
        &CoreCapabilities::for_major(harness.core.major),
        &observed,
    );
    assert!(
        !discrepancies.iter().any(|d| d.claim == "webrtc leak"),
        "the leak check passes on its own: {discrepancies:#?}"
    );
    harness.stop(snapshot.profile_id);
}

#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn font_surface_is_readable_and_cjk_is_not_boxed() {
    let harness = Harness::verified();
    // The risky case is a spoofed platform whose font set differs from the
    // host's, which is what the font exclusion exists for.
    for platform in [Platform::Windows, Platform::MacOs] {
        let mut fingerprint = FingerprintProfile::new_random(0);
        fingerprint.platform = platform;
        fingerprint.disabled_spoofing = vec![SpoofingFeature::Font];
        let profile = harness.profile(11111, fingerprint);

        let (snapshot, observed) = harness.read(&profile);

        assert!(
            observed.fonts_readable(),
            "the font surface must be readable on {platform}: {observed:#?}"
        );
        assert_eq!(
            observed.has_missing_cjk(),
            Some(false),
            "CJK text must not render as missing glyphs on {platform}: {observed:#?}"
        );
        assert!(
            observed.font_emoji_width != observed.font_tofu_width,
            "emoji must not collapse into the missing-glyph box on {platform}: {observed:#?}"
        );
        harness.stop(snapshot.profile_id);
    }
}

#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn a_fresh_reading_repeats_and_leaves_the_session_as_it_was() {
    // A new target reports a complete empty document before the navigation that
    // opened it commits; reading too early fails or measures the wrong page.
    let harness = Harness::verified();
    let profile = harness.profile(11111, FingerprintProfile::new_random(0));
    let (snapshot, _) = harness.read(&profile);
    let port = snapshot.cdp_port.expect("a running profile has a CDP port");

    let pages = |port: u16| {
        HttpCdpProbe
            .page_targets(port, Duration::from_secs(3))
            .expect("the browser lists its pages")
    };
    let before = pages(port);

    let mut readings = Vec::new();
    for _ in 0..3 {
        readings.push(
            FingerprintProbe::default()
                .read_fresh(port)
                .expect("a fresh reading succeeds"),
        );
    }

    assert!(
        readings
            .iter()
            .all(|reading| reading.platform.as_deref() == Some("Win32")),
        "every reading is of the profile's own document: {readings:#?}"
    );
    assert_eq!(
        readings[0].canvas_signature(),
        readings[2].canvas_signature(),
        "repeated readings of one session must agree"
    );

    let after = pages(port);
    assert_eq!(
        before.len(),
        after.len(),
        "each reading opens a tab and closes it again"
    );
    assert_eq!(
        before
            .iter()
            .map(|page| page.url.clone())
            .collect::<Vec<_>>(),
        after
            .iter()
            .map(|page| page.url.clone())
            .collect::<Vec<_>>(),
        "the pages the user has keep their urls"
    );
    harness.stop(snapshot.profile_id);
}

/// Measures what one binary does with the switches whose support the table
/// claims, and holds the table to it.
///
/// The table is a claim about the engine, so it is checked against the engine:
/// the switches are added as raw arguments and their effect is measured, then
/// compared with what [`CoreCapabilities::for_major`] promises for the major
/// this binary reports. Running this against a build the table has never seen
/// is how a wrong table is found - which is what happened on 142, where the
/// noise switch is honoured and `--disable-spoofing` is not, the opposite of
/// what the table said.
#[test]
#[ignore = "requires CHROMIUM_BIN and a real browser"]
fn the_capability_table_matches_this_build() {
    let binary = PathBuf::from(std::env::var_os("CHROMIUM_BIN").expect("set CHROMIUM_BIN"));
    let report =
        ::runtime::version::VersionReport::probe(&binary, ::runtime::version::DEFAULT_TIMEOUT);
    let major = report
        .major
        .expect("the binary must report a version with --version");
    let capabilities = CoreCapabilities::for_major(major);

    // The engine under test is the binary. The harness table is pinned to one
    // that emits neither switch, so every difference below is the switch added
    // by hand and nothing else.
    let bare = CoreCapabilities {
        supports_canvas_noise: false,
        supports_disable_spoofing: false,
        ..CoreCapabilities::for_major(128)
    };
    let with_args = |flags: Vec<String>| {
        Harness::with_capabilities(128, Box::new(RawArgsPlanner(flags)), bare.clone())
    };

    // A seed pair with nothing extra: the reference for "the seed moves this
    // surface at all", without which every comparison below would be vacuous.
    let plain = with_args(Vec::new());
    let plain_first = plain.profile(11111, FingerprintProfile::new_random(0));
    let plain_second = plain.profile(22222, FingerprintProfile::new_random(0));
    let (_, plain_first) = plain.read(&plain_first);
    let (_, plain_second) = plain.read(&plain_second);
    let canvas_moves = plain_first.canvas_signature() != plain_second.canvas_signature();
    let audio_moves = plain_first.audio_signature() != plain_second.audio_signature();
    let rects_move = plain_first.rects != plain_second.rects;
    assert!(
        canvas_moves && audio_moves && rects_move,
        "the seed must move every surface on any build we support: {plain_first:#?} {plain_second:#?}"
    );

    // --- the exclusions: two seeds, each with the switch under test ---
    let mut exclusions = Vec::new();
    for (feature, flag) in [
        ("canvas", "--disable-spoofing=canvas"),
        ("clientrects", "--disable-spoofing=clientrects"),
        ("audio", "--disable-spoofing=audio"),
    ] {
        let harness = with_args(vec![flag.to_string()]);
        let first = harness.profile(11111, FingerprintProfile::new_random(0));
        let second = harness.profile(22222, FingerprintProfile::new_random(0));
        let (_, first_observed) = harness.read(&first);
        let (_, second_observed) = harness.read(&second);
        // Equal readings under two seeds mean the seed never reached that
        // surface, which is the exclusion working.
        let honoured = match feature {
            "audio" => first_observed.audio_signature() == second_observed.audio_signature(),
            "clientrects" => first_observed.rects == second_observed.rects,
            _ => first_observed.canvas_signature() == second_observed.canvas_signature(),
        };
        exclusions.push((feature, honoured));
    }

    // --- the noise switches: one seed, with and without the switch ---
    let noisy = with_args(vec![
        "--fingerprinting-canvas-image-data-noise".into(),
        "--fingerprinting-client-rects-noise".into(),
    ]);
    let noisy_profile = noisy.profile(11111, FingerprintProfile::new_random(0));
    let (_, noisy_observed) = noisy.read(&noisy_profile);
    let noise_changes_data_url = noisy_observed.canvas_data_url != plain_first.canvas_data_url;
    let noise_changes_pixels = noisy_observed.canvas_pixels != plain_first.canvas_pixels;

    println!(
        "fingerprint-chromium {version} (major {major})\n\
         table says: disable_spoofing={disable_spoofing}, canvas_noise={canvas_noise}\n\
         measured on this build:\n\
         {exclusions}\n\
         \t--fingerprinting-canvas-image-data-noise: changes toDataURL={data_url}, \
         changes getImageData={pixels}\n\
         \treference without the noise switches: canvas {plain_canvas:?}\n\
         \treference with them:                canvas {noisy_canvas:?}",
        version = report.banner.as_deref().unwrap_or("unknown"),
        disable_spoofing = capabilities.supports_disable_spoofing,
        canvas_noise = capabilities.supports_canvas_noise,
        exclusions = exclusions
            .iter()
            .map(|(feature, honoured)| format!(
                "\t--disable-spoofing={feature}: the surface is seed-independent (honoured)={honoured}"
            ))
            .collect::<Vec<_>>()
            .join("\n"),
        data_url = noise_changes_data_url,
        pixels = noise_changes_pixels,
        plain_canvas = plain_first.canvas_signature(),
        noisy_canvas = noisy_observed.canvas_signature(),
    );

    // The whole feature stands or falls together: the product emits one
    // `--disable-spoofing=<features>` argument.
    for (feature, honoured) in &exclusions {
        assert_eq!(
            *honoured, capabilities.supports_disable_spoofing,
            "the table and the engine disagree about --disable-spoofing={feature} on major {major}"
        );
    }
    assert_eq!(
        noise_changes_data_url, capabilities.supports_canvas_noise,
        "the table and the engine disagree about the canvas noise switch on major {major}"
    );
    assert!(
        !noise_changes_pixels,
        "the noise switch must not change getImageData, or the two canvas surfaces \
         would move together: {noisy_observed:#?}"
    );
}
