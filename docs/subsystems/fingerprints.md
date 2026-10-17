# Fingerprints

The fingerprint subsystem is the path from a stored `FingerprintProfile` to the
switches a launched browser receives, plus the fingerprint read-back that checks
a running browser reproduces them. At launch it classifies the core, gates every
switch on a per-generation capability table and reports what the core cannot
honour; after a launch it can read the fingerprint surface out of the page and
compare it with what the profile claims. It does not store profiles, own the
runtime state machine, or define the browser's switch vocabulary - the last is
measured on real
builds, and the measurements live in
[fingerprint-matrix.md](../fingerprint-matrix.md).

## Responsibilities and boundaries

Owned here:

- The seed and the default persona of a new profile
  (`crates/application/src/profile_service.rs`, `random_seed`;
  `FingerprintProfile::new_random` in `crates/domain/src/fingerprint.rs`).
- The capability table and the classification of a core into
  `FingerprintGeneration::Legacy` or `FingerprintGeneration::Chrome144Plus`
  (`crates/domain/src/core.rs`).
- Serialization of the profile into `--fingerprint*` switches
  (`serialize_fingerprint_args` in `crates/runtime/src/fingerprint_args.rs`) and
  the compatibility report for what is omitted (`check` in
  `crates/runtime/src/compat.rs`).
- The read-back verification of a running browser
  (`crates/runtime/src/verify.rs`; `crates/app/src/verifier.rs`).

Not done here:

- Deciding what a profile is, or where its browser data lives - see
  [domain-model.md](../domain-model.md) and
  [backup-and-restore.md](../backup-and-restore.md).
- Starting, stopping or restarting a browser - see
  [lifecycle.md](../lifecycle.md).
- Disabling a spoofing surface on its own. Only the features a profile lists in
  `disabled_spoofing` become `--disable-spoofing`; in particular the product
  never adds `font` to that list (the measurement that decided this is in
  [fingerprint-matrix.md](../fingerprint-matrix.md), "Fonts").
- Emitting switches for surfaces the model does not carry (screen size, device
  scale factor, geolocation, the pre-144 switch names); see
  [fingerprint-matrix.md](../fingerprint-matrix.md), "Not implemented yet".
- Preserving a user-typed command line. The command line is generated from the
  typed profile, and every omission is reported ("generate, then report" in
  [fingerprint-matrix.md](../fingerprint-matrix.md)).

## Source of truth

| What | Where |
| --- | --- |
| Profile fields and their engine tokens | `crates/domain/src/fingerprint.rs`: `FingerprintProfile`, `BrowserBrand::as_arg_value`, `Platform::as_arg_value`, `SpoofingFeature::as_flag_name`, `WebRtcPolicy` |
| Capability table | `crates/domain/src/core.rs`: `CoreCapabilities::for_major` is the one table; `FingerprintGeneration::PIVOT_MAJOR` is 144 |
| Measured boundary | `crates/domain/src/core.rs`: `CoreCapabilities::NEWEST_MEASURED_MAJOR` (148) and `is_newer_than_measured`; the evidence is in [fingerprint-matrix.md](../fingerprint-matrix.md) |
| Core classification, major 0 | `crates/domain/src/core.rs`: `FingerprintGeneration::for_major`, `BrowserCore::capabilities` |
| Resolution from a core | `crates/runtime/src/capability.rs`: `DefaultCapabilityResolver::resolve` |
| Omission vocabulary and message | `crates/runtime/src/compat.rs`: `Omission`, `Finding`, `CompatibilityReport::message` |
| Switch block | `crates/runtime/src/fingerprint_args.rs`: `serialize_fingerprint_args` |
| Probe and comparison | `crates/runtime/src/verify.rs`: `PROBE_EXPRESSION`, `ObservedFingerprint`, `verify` (re-exported as `verify_fingerprint`) |
| Seed source | `crates/application/src/profile_service.rs`: `SeedSource`, `random_seed` |

Capabilities are derived from `BrowserCore.major` every time they are needed and
are never persisted; a stored profile keeps only its `fingerprint_json`
(`crates/storage/src/migrations.rs`). The serializer and the compatibility layer
read the same `CoreCapabilities` value for a launch, which is what keeps the
command line and the reported omissions consistent.

## Main runtime paths

### Persona and seed

A new profile that brings no fingerprint of its own gets one from the injected
`SeedSource`: `DefaultProfileService::create` draws a seed and calls
`FingerprintProfile::new_random(seed)`. `random_seed` takes four bytes from a v4
UUID (the OS entropy source), not the clock, because a repeated or guessable
seed is two profiles a site can correlate. `DefaultProfileService::duplicate`
draws a fresh seed for the copy, and the profile editor's reroll button uses the
same function (`profile_seed` in `crates/app/src/editor.rs`). The seed is the
value the browser expands into the spoofed surfaces; the other profile fields
(platform, locale, brand, versions, WebRTC policy, exclusions) are explicit
values, gated by the capability table below. The model keeps only the surfaces
that genuinely need to be controlled explicitly; everything else is either left
to the seed to expand or not modelled. The editor validates the seed as a
`u32` before the profile is written.

### Launch: classify, report, serialize

1. `AppState::begin_opening` goes through `RuntimeService::launch_params`, which
   loads the profile, its core and its proxy and sends `StartParams` to the
   `RuntimeFacade`.
2. In `RuntimeSupervisor::begin_attempt`
   (`crates/runtime/src/supervisor/start.rs`) the ports are reserved, then
   `DefaultCapabilityResolver::resolve(&params.core)` runs. A core with
   `major == 0` is refused here with `CapabilityError::UnsupportedMajor`, and
   the start fails as `capability error: unsupported browser major version: 0`
   (`RuntimeState::Failed`). `BrowserCore::capabilities()` returns `None` for
   the same value, and `FingerprintGeneration::for_major` debug-asserts
   `major > 0` so a caller that skips the refusal trips in a debug build rather
   than receiving a classification.
3. `compat::check(&params.core, &params.profile, &capabilities)` runs before the
   plan is built; its `message()` is emitted as `RuntimeEvent::Warning` and the
   launch continues. The warning lands in `RuntimeSnapshot::last_warning` and is
   shown on the profile row and details panel.
4. `DefaultLaunchPlanner::build` calls `serialize_fingerprint_args` with the
   resolved capabilities and interleaves the fingerprint block with the rest of
   the arguments (`crates/runtime/src/planner.rs`).
5. The complete list is emitted as `RuntimeEvent::EffectiveLaunchArgs` and kept
   in `RuntimeSnapshot::effective_args` - what the runtime reports as passed is
   this list, not the profile.

### What a profile field becomes

The launch arguments are emitted in a fixed order - a standing convention so
that logs and debugging have a predictable layout; the numbered order lives in
`DefaultLaunchPlanner::build`, and the fingerprint block's internal order in
`serialize_fingerprint_args` follows the layout the upstream browser expects.

- `--fingerprint=<seed>` is emitted first and always.
- The brand and its version are one claim: `--fingerprint-brand` is emitted only
  when `CoreCapabilities::supports_brand` accepts the profile's brand, and
  `--fingerprint-brand-version` follows only when the brand was emitted,
  `CoreCapabilities::supports_brand_version` is true, and the version is
  non-blank (`non_empty` trims). Otherwise neither switch is emitted.
- `--fingerprint-platform` is emitted always; `--fingerprint-platform-version`
  only when `supports_platform_version` and the field is non-blank.
- `--lang`, `--accept-lang` and `--timezone` are emitted always.
- `--fingerprint-hardware-concurrency` only when `hardware_concurrency` is
  `Some`.
- `--disable-non-proxied-udp` only under `WebRtcPolicy::DisableNonProxiedUdp`;
  the two `Default*` policies emit nothing.
- `--fingerprinting-canvas-image-data-noise` and
  `--fingerprinting-client-rects-noise` are emitted as a pair when
  `supports_canvas_noise`.
- `--disable-spoofing=<features>` only when `supports_disable_spoofing` and
  `disabled_spoofing` is non-empty; the tokens are `as_flag_name`.

### Compatibility report

`compat::check` reports only what the profile actually asked for, in a fixed
order, so the warning is stable for the same inputs. A core whose major is newer
than `CoreCapabilities::NEWEST_MEASURED_MAJOR` is reported first, before any
finding: nothing is omitted for it, but the table's claims were not read back on
that build, and `CompatibilityReport::message` says so. (150 is above that line:
its Windows asset hangs on a seeded canvas read, so the suite could not certify
it; see [fingerprint-matrix.md](../fingerprint-matrix.md).)

1. `--fingerprint-brand` (and with it its version) for a brand outside
   `supported_brands` (`Omission::UnsupportedBrand`).
2. the canvas/client-rects noise pair when `supports_canvas_noise` is false -
   `UnsupportedSwitch` on a verified generation, `UnverifiedGeneration` below
   the pivot.
3. `--fingerprint-brand-version` when the field is `Some` and the capability is
   false.
4. `--fingerprint-platform-version` likewise.
5. `--disable-spoofing` when the list is non-empty and the capability is false -
   `UnsupportedSwitch` on a verified generation, `UnverifiedGeneration` below
   the pivot, because a legacy engine accepts the switch and applies the noise
   anyway.

Each `Finding` names the core, the major and what in the profile requested the
switch; `message` joins them with `; `. Because the serializer reads the same
capabilities, a reported switch is also absent from the command line.

### Read-back verification ("generate, then report")

The Verify action on a profile row and in the details panel calls
`AppState::begin_verification`, which refuses unless the profile is running with
a CDP port and its core has a detected version. It assembles a `VerificationJob`
(the profile's `FingerprintProfile`, the core's `CoreCapabilities`, the port and
the session's `acknowledged_start`) and marks `Verification::Running`. The view
runs `FingerprintVerifier::verify` on a worker thread
(`crates/app/src/ui/profiles.rs`, `on_verify`) and applies the result through
`complete_verification` / `finish_verification`.

`CdpFingerprintVerifier::verify` (`crates/app/src/verifier.rs`) reads with
`FingerprintProbe::read_fresh`, which opens a target of its own and closes it
afterwards, so the page the user is looking at is not navigated or reloaded. The
document is a local `file:` page because the user agent data surface is absent
on `about:blank` and `data:` URLs. The probe (`PROBE_EXPRESSION`) is plain ES5
and self-contained: it hashes the canvas and audio surfaces, measures text and
client rects, reads the WebGL vendor and renderer, `navigator.platform`, the
user agent, language and languages, the timezone, hardware concurrency, the user
agent data brands and high-entropy values, ICE candidates and gathering state,
and the font enumeration with layout widths.

`runtime::verify` then compares the reading with the profile:

- platform, user agent, hardware concurrency, language and languages (both
  checked against the first entry of `accept_language`), timezone;
- brand, only when the capability table claims the brand, against the brand
  names the engine reports (`brand_readback`);
- platform version, only when the capability supports it and the profile has
  one;
- the WebRTC leak claim, only under `DisableNonProxiedUdp`: gathering must
  complete with no `host` or `srflx` candidate;
- the font surface: can it be read at all, and do CJK and emoji have glyphs
  rather than the missing-glyph box.

A reading that was not reported is itself a `Discrepancy`, never a pass. The
canvas and audio fingerprints and the WebGL exclusion are comparison-only - one
reading cannot say whether it was perturbed - so `verify` does not assert them;
`ObservedFingerprint::canvas_signature` and `audio_signature` exist for the
acceptance suite, which compares sessions (see
[fingerprint-matrix.md](../fingerprint-matrix.md)). When the profile has a
proxy, the same job also reads where the browser's own traffic leaves from; that
egress read-back belongs to [traffic verification](traffic-verification.md), not
to this page.

## State and transitions

- Capability resolution is stateless: a `CoreCapabilities` value is derived from
  `BrowserCore.major` at each use - launch and verification both - and nothing
  about it is stored. A core whose version was never read has
  `capabilities() == None`: there is no generation to show, the launch refuses
  it, and `begin_verification` refuses it too.
- A verification runs through `Verification` in
  `crates/app/src/state/rows.rs`: `Running`, then `Confirmed` (no
  discrepancies), `Disagreements` (one or more) or `Unreadable` (no reading
  could be taken). The outcomes are also appended to the activity log (see
  [diagnostics.md](../diagnostics.md)).
- `complete_verification` discards an outcome that no longer describes the same
  browser: the job's `TaskId` must still be the current one, the snapshot must
  still be `Running`, the `acknowledged_start` must equal the job's session and
  the CDP port must match. Anything else is forgotten rather than shown.
- A stored reading is forgotten when the profile is stopped, restarted, edited
  or deleted (`forget_verification`; `AppState::stop`, `queue_opening` for a
  restart, `update_profile`, `delete_profile`) - a new browser has a new
  fingerprint.
- The compatibility warning is cleared with the other start diagnostics when the
  next start enters `Starting` (`RuntimeSupervisor::set_snapshot_state`) and is
  carried into the `Running` snapshot when the warning event lands.

## Constraints and failure behavior

- Unknown is not a generation. `major == 0` is refused by
  `DefaultCapabilityResolver` rather than resolved to an assumed table, because
  a fingerprint browser that cannot classify its engine must not launch with
  unverified spoofing switches. The same value yields `None` from
  `BrowserCore::capabilities()` and `suggested_name` calls such a core
  "`<binary>` (version unknown)".
- A legacy core (< 144) is still given the noise pair - the table says the
  engine honours it below the pivot - and is not given `--disable-spoofing`,
  which is reported as `UnverifiedGeneration` rather than as an unsupported
  switch.
- A brand the table does not carry (Opera, Vivaldi) never becomes a claim: the
  serializer omits the brand and its version, `compat` reports
  `UnsupportedBrand`, and `brand_readback` returns `None` so the read-back does
  not assert a brand the engine would never report.
- A browser that cannot be read at all is an error, not an empty discrepancy
  list; the window's verifier has a `VERIFICATION_TIMEOUT` of 25 seconds for a
  reading, and the probe's own default is 10 seconds. The verification is an
  explicit action because it opens a tab, runs an offline audio render and
  starts ICE gathering in the browser it reads.
- The probe must not touch the network:
  `the_probe_expression_never_touches_the_network` asserts that
  `PROBE_EXPRESSION` contains no `fetch`, `XMLHttpRequest`, `WebSocket`,
  `importScripts` or STUN URL.
- The browser accepts unknown arguments silently, so a switch name is not
  evidence that the browser honours it; that is why the subsystem reads values
  out of the page instead (see
  [fingerprint-matrix.md](../fingerprint-matrix.md)).

## Verification

Unit tests, run by the gate:

```sh
cargo test -p domain core::tests
cargo test -p domain test_launch_switch_vocabulary_matches_the_engine
cargo test -p runtime capability::tests
cargo test -p runtime compat::tests
cargo test -p runtime fingerprint_args::tests
cargo test -p runtime verify::tests
cargo test -p runtime test_supervisor_start_nonexistent_executable_fails
cargo test -p application a_new_profile_takes_its_seed_from_the_injected_source
cargo test -p application the_default_seed_source_does_not_repeat_itself
cargo test -p app verifier::tests
cargo test -p app state::tests::verification
```

Of these, `capability::tests::refuses_an_undetected_major` and
`compat::tests::a_legacy_core_reports_the_exclusions_it_ignores` pin the two
main failure behaviors, and
`test_supervisor_start_nonexistent_executable_fails` shows the compatibility
warning arriving from a real supervisor start.

Opt-in suites. `fingerprint_real` launches a real fingerprint-chromium over the
production supervisor path and reads every expectation back over CDP;
`the_capability_table_matches_this_build` checks the table against whatever
binary is given, and the other tests cover the seed, the exclusions, the
WebRTC policy and the font surface:

```sh
CHROMIUM_BIN=/path/to/chrome cargo test -p runtime --test fingerprint_real -- --ignored
```

The window's own path - `AppState` to a real browser - is exercised by the
app's ignored acceptance tests, including
`real_chromium_fingerprint_verification_through_the_verifier` and
`real_chromium_start_stop_through_app_state`:

```sh
CHROMIUM_BIN=/path/to/chrome cargo test -p app real_chromium -- --ignored
```

That module also holds the egress acceptance test
(`real_chromium_reports_the_address_its_own_traffic_leaves_from`), which needs
`ECHO_URL` and a networked machine in addition to `CHROMIUM_BIN`.

The runtime acceptance runs and the machines they were measured on are recorded
in [chromium-acceptance.md](../chromium-acceptance.md) (Linux) and
[windows-acceptance.md](../windows-acceptance.md) (Windows), and the switch
measurements themselves in
[fingerprint-matrix.md](../fingerprint-matrix.md).
