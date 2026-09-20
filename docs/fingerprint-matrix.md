# Fingerprint switch matrix

Which fingerprint-chromium switches this project is willing to pass, for which
core major, and on what evidence.

## The pivot: major 144

`FingerprintGeneration::PIVOT_MAJOR` is 144. Below it the core is `Legacy`,
from it upwards the core is `Chrome144Plus`.

The generation label is a coarse split, not a claim that every switch changes at
144: each capability is its own measured claim, and the two unstable ones turned
out to sit on **opposite sides** of the pivot.

Evidence:

- The sibling Go product (`Ant-Browser`) verified its switch set against a local
  Chromium 144 and encodes that in `backend/app_browser_fingerprint_matrix.go`.
  That is where the pivot came from, and it is inherited evidence: nothing in
  this repository had measured a major below 144.
- This repository certifies the runtime path against a fingerprint-chromium 148
  build (`docs/chromium-acceptance.md`), and reads the fingerprint surface back
  out of the page (`crates/runtime/tests/fingerprint_real.rs`).
- **A fingerprint-chromium 142 build (major 142, the last release below the
  pivot) was measured here**, and it contradicts the inherited claim:

  | Measured on 142 and 148 | 142 | 148 |
  | --- | --- | --- |
  | `--fingerprinting-canvas-image-data-noise` changes `toDataURL` | **yes** | yes |
  | the same switch changes `getImageData` | no | no |
  | `--disable-spoofing=canvas\|clientrects\|audio` is honoured | **no** | yes |

  So the noise switch is *not* a 144 feature - a major below the pivot was being
  denied a switch the engine honours - and the exclusions *are*. Both the table
  and the tests were corrected (`the_capability_table_matches_this_build` now
  measures this on whatever binary `CHROMIUM_BIN` points at).

- Majors 144 to 147 remain inherited: exclusions are claimed for them on the
  strength of the sibling product's 144 measurement, and the noise switch is
  claimed for every major on the strength of 142 and 148.
- Nothing below 142 has been verified here.

## Capability table

`CoreCapabilities::for_major` is the single source of truth; the serializer and
the compatibility layer both read it.

| Capability | Legacy (< 144) | Chrome144Plus (>= 144) | Switch |
| --- | --- | --- | --- |
| seed | yes | yes | `--fingerprint=` |
| brand | Chrome, Edge | Chrome, Edge | `--fingerprint-brand=` |
| brand version | yes | yes | `--fingerprint-brand-version=` |
| platform | yes | yes | `--fingerprint-platform=` |
| platform version | yes | yes | `--fingerprint-platform-version=` |
| language, timezone | yes | yes | `--lang=`, `--accept-lang=`, `--timezone=` |
| hardware concurrency | yes | yes | `--fingerprint-hardware-concurrency=` |
| WebRTC policy | yes | yes | `--disable-non-proxied-udp` |
| spoofing exclusions | **no** (measured on 142) | yes (measured on 148) | `--disable-spoofing=<feature,...>` |
| canvas and client-rects noise | yes (measured on 142) | yes | `--fingerprinting-canvas-image-data-noise`, `--fingerprinting-client-rects-noise` |

"yes" means the switch is passed. "no" means the serializer omits it **and** the
compatibility layer reports the omission, so the window shows why a profile is
not getting the fingerprint it asks for.

## Policy: generate, then report

`Ant-Browser` preserved user-typed command lines, so an unrecognised switch was
passed through ("避免误删按原样传递"). This project generates a command line from
a typed profile instead, which inverts the risk: the failure mode is not losing
a user's switch, it is claiming a fingerprint the engine silently ignores.

So the rule here is:

1. A switch is emitted only when the capability table says the core honours it.
2. Every omission is reported: `runtime::compat::check` turns it into a
   `RuntimeEvent::Warning`, which lands in `RuntimeSnapshot::last_warning` and on
   the profile row.
3. An undetected version (major `0`) is refused by the capability resolver
   rather than resolved to an assumed capability set.
4. A claim that has not been read back is not certified. A core is only
   "verified" for the generation whose claims this project has measured.

## How the switches were measured

A switch name is not evidence: the engine accepts unknown arguments silently, so
every row below is a value read out of the page. The method:

1. Launch the browser with `--remote-debugging-port` on loopback.
2. Connect over CDP, navigate the target to a real document (`file:` — the user
   agent data surface does not exist on `about:blank` or `data:` URLs), and
   evaluate a probe that hashes the canvas surfaces and reads `navigator`,
   `Intl` and `navigator.userAgentData`.
3. Compare against a baseline run (no switches) and against a run with only
   `--fingerprint=<seed>`.
4. For anything a headless run fixes on its own (`screen.*`,
   `devicePixelRatio`), repeat on a real X11 display.

The probe and the comparison live in `crates/runtime/src/verify.rs`; the
end-to-end run is:

```sh
CHROMIUM_BIN=/path/to/chrome cargo test -p runtime --test fingerprint_real -- --ignored
```

## Measured on a legacy generation (fingerprint-chromium 142, Linux)

The last release below the pivot, added to test the inherited claim rather than
to assume it. Same method, same probe, same seed as the 148 rows below.

| Switch | Observable | 142 |
| --- | --- | --- |
| `--fingerprint=<seed>` | canvas `toDataURL`/`getImageData`, `measureText`, sub-pixel rects, UA-CH version | same as 148; the same seed gives the **same** canvas reading as 148 |
| `--fingerprinting-canvas-image-data-noise` | canvas `toDataURL` | changes it, exactly as on 148; `getImageData` untouched |
| `--fingerprinting-client-rects-noise` | client rects | no change beyond what the seed applies (same as 148) |
| `--disable-spoofing=canvas` | canvas returns to the engine's own value | **no**: the seed still reaches the canvas |
| `--disable-spoofing=clientrects` | client rects become integral | **no** |
| `--disable-spoofing=audio` | audio fingerprint returns to the engine's own value | **no** |
| `--fingerprint-platform=macos` | `navigator.platform`, user agent | honoured |
| `--disable-non-proxied-udp` | ICE gathering completes with no candidates | honoured |

All ten integration tests run against both builds:

```sh
CHROMIUM_BIN=/path/to/chrome-142 cargo test -p runtime --test fingerprint_real -- --ignored
CHROMIUM_BIN=/path/to/chrome-148 cargo test -p runtime --test fingerprint_real -- --ignored
```

The suite resolves the major from the binary it is given and checks the table's
claims for *that* major against the engine, so a wrong table fails the run
instead of hiding behind a hard-coded number.

## Measured on the verified generation (fingerprint-chromium 148, Linux)

Effective:

| Switch | Observable that moved |
| --- | --- |
| `--fingerprint=<seed>` | canvas `toDataURL` **and** `getImageData`, `measureText`, sub-pixel client rects, `hardwareConcurrency`, `deviceMemory`, WebGL vendor/renderer, UA-CH full version. Deterministic: the same seed twice gives byte-identical readings. |
| `--fingerprint-brand=Chrome` | UA-CH brands gain `Google Chrome/148` (default is `Chromium/148` only) |
| `--fingerprint-brand=Edge` | UA-CH brands gain `Microsoft Edge/148`, user agent gains `Edg/148.0.0.0` |
| `--fingerprint-brand-version=120.0.0.0` | with a brand: brands and UA-CH full version report `120`; without a brand: no effect |
| `--fingerprint-platform=windows` | `navigator.platform` `Win32`, user agent `Windows NT 10.0; Win64; x64`, UA-CH platform version default `19.0.0` |
| `--fingerprint-platform=macos` | `navigator.platform` `MacIntel`, user agent `Macintosh; Intel Mac OS X 10_15_7`, UA-CH platform version default `15.6.0` |
| `--fingerprint-platform-version=11.0.0` | with a platform: UA-CH platform version `11.0.0`; without a platform: no effect |
| `--accept-lang=en-US,en` | `navigator.language` `en-US`, `navigator.languages` `en-US,en` |
| `--timezone=America/New_York` | `Intl.DateTimeFormat().resolvedOptions().timeZone`, UTC offset `240` |
| `--fingerprint-hardware-concurrency=4` | `navigator.hardwareConcurrency` |
| `--disable-spoofing=canvas` | canvas returns to the engine's own values (the seed stops reaching it) |
| `--disable-spoofing=clientrects` | client rects become integral again |
| `--disable-spoofing=gpu` | WebGL vendor/renderer return to the host values |
| `--disable-spoofing=font` | not isolatable with this probe: needs a font-listing reading |
| `--fingerprinting-canvas-image-data-noise` | canvas `toDataURL` hash changes; `getImageData` is **not** affected on this build |
| `--disable-spoofing=audio` | the audio fingerprint returns to the engine's own value (the seed stops reaching it) |
| `--disable-non-proxied-udp` | ICE gathering completes with **no candidates at all**; removing the switch exposes the machine's own addresses |
| `--disable-spoofing=font` | no observable change to the font enumeration or the layout metrics; it is emitted for the host-font case, and CJK glyphs stay present either way |

Accepted and ignored (no observable moved):

| Switch | Result |
| --- | --- |
| `--lang=en-US` | `navigator.language` unchanged; only `--accept-lang` moves it. Both are emitted: `--lang` aligns the browser's own locale, `--accept-lang` is what a page sees. |
| `--fingerprint-screen-width` / `--fingerprint-screen-height` | on a real 1440x900 display with a 2x scale factor, `screen.width/height` stayed 1440x900 |
| `--fingerprint-device-scale-factor=1` / `2` / `3` | on the same display `devicePixelRatio` stayed 2 |
| `--fingerprint-platform=mac` | platform stayed `Linux x86_64`: the token must be `macos` |
| `--fingerprint-brand=Opera` / `Vivaldi` / `brave` / `google chrome` / `microsoft edge` | brands unchanged: only the tokens `Chrome` and `Edge` are honoured |
| `--disable-spoofing=all` / `rects` / `client-rects` / `webrtc` / `timezone` / `screen` / `hardware` / `language` / `navigator` / `webgl` | no observable moved |
| `--fingerprinting-client-rects-noise` | no change beyond what the seed already applies |
| `--fingerprinting-canvas-measuretext-noise` | no change beyond what the seed already applies |
| `--fingerprint-font-list`, `--fingerprint-fonts` | not emitted by this model; the font enumeration measured 13 families and no switch moved it |

Surfaces that are off by default in this build:

| Surface | Measurement |
| --- | --- |
| WebRTC | With no policy switch the browser gathers **zero** ICE candidates, so a local or public address never reaches the page even though nothing asks for that. Re-opening the policy (`--webrtc-ip-handling-policy=default`) leaks the LAN address `192.168.31.222` as a `host` candidate and the public address as an `srflx` candidate, which is how the check is proven to be able to see a leak at all. |

What the probe reads (`PROBE_EXPRESSION`): canvas `toDataURL` and `getImageData`
hashes, text measurement, client rects, WebGL vendor and renderer,
`navigator.platform`, user agent, language and languages, timezone, hardware
concurrency, the user agent data brand list and high-entropy values, an
`OfflineAudioContext` audio fingerprint, ICE candidates and gathering state, and
the font enumeration with layout widths for ASCII, Latin, CJK, emoji and a
codepoint that has no glyph anywhere.

Not verified, and why: geolocation (this model has no location field, so no
switch is emitted — `--fingerprint-location` also produced no coordinates in
any test here, which cannot be separated from "this headless browser has no
location provider"), and anything that needs a network peer.

## Defects this measurement found

Three of them were in this repository's own switch vocabulary, and all three
failed silently — the engine accepted the argument and the profile shipped with
a fingerprint nobody asked for:

| Defect | Before | Evidence | Fix |
| --- | --- | --- | --- |
| macOS platform never applied | `--fingerprint-platform=mac` | `navigator.platform` stayed `Linux x86_64`; `macos` switches it to `MacIntel` | `Platform::MacOs` emits `macos` |
| ClientRects exclusion never applied | `--disable-spoofing=client-rects` | client rects kept their sub-pixel noise; `clientrects` removes it | `SpoofingFeature::ClientRects` emits `clientrects` |
| Brands claimed that the engine ignores | Opera and Vivaldi in `supported_brands` | `--fingerprint-brand=Opera` leaves the default Chromium brand list | table carries only `Chrome` and `Edge`; the compatibility layer reports the rest |

Each fix has a regression test in `crates/runtime/tests/fingerprint_real.rs`
that reads the value back, not just the command line.

## Fonts: what `--disable-spoofing=font` does, and why the product does not add it

`Ant-Browser` appends `font` to `--disable-spoofing` when the profile's platform
differs from the host's, so the host's real fonts are used instead of a spoofed
list. That rule was inherited, not measured, and measuring it on a real 148 (and
142) changed the answer.

Same seed, same host (Linux), read back with `FingerprintProbe`; the switch was
added as a raw argument so the measurement does not depend on the capability
table under test (`the_font_exclusion_changes_what_a_spoofed_platform_enumerates`):

| Claimed platform | `--disable-spoofing=font` | Fonts the page enumerates |
| --- | --- | --- |
| windows | off | `Georgia`, `Verdana`, `Tahoma`, `Comic Sans MS`, `Cambria`, `Arial Black`, ... |
| windows | on | `DejaVu Sans`, `Liberation Sans`, `Noto Sans CJK SC`, `Noto Color Emoji`, `Ubuntu`, `Cantarell`, ... |
| macos | off | `Georgia`, `Verdana`, `Tahoma`, `Comic Sans MS`, ... |
| macos | on | the same host list as above |
| linux (host) | either | the host list; the switch changes nothing |

The widths (`mmmmmmmmmmlli`, latin, CJK, emoji and the missing-glyph box) are
**identical in all six readings**, and CJK and emoji have glyphs in all six.
Two conclusions follow, and both are the opposite of the inherited rule:

1. The switch changes the claim, not the rendering. The glyphs are the host's
   whatever the engine reports, so excluding font spoofing **cannot fix a
   missing glyph** - that was the stated reason for the rule. What renders is
   decided by which fonts the host has installed, in both cases.
2. With the switch off, a Windows- or macOS-claiming profile enumerates fonts
   this host does not have (Tahoma, Cambria, Comic Sans MS, ...), which is what
   the font spoof is for. Excluding it would replace a fabricated list with the
   host's own and admit the host platform to any page that enumerates fonts.

So the product keeps the engine's font spoof and **does not add `font` to
`--disable-spoofing`**. `SpoofingFeature::Font` is still offered per profile in
the editor, so a user who would rather have the host's list than a consistent
claim can ask for exactly that.

The measurement also settled two things about the exclusion itself:

- It is honoured on **148** and ignored on **142** (the two lists are identical
  with and without it), which is the same split as `canvas`, `clientrects` and
  `audio` - so one `supports_disable_spoofing` flag covers all of them, and
  `the_font_exclusion_changes_what_a_spoofed_platform_enumerates` now checks the
  table against the engine for this feature too.
- The font surface is a **host** artefact, so what is worth reading back is
  what the host can render. `verify` now settles all three single-reading font
  questions - the surface is readable at all, CJK has glyphs, emoji has glyphs -
  instead of only CJK. The emoji reading is the one that catches a host with no
  emoji font, which is visible to any page and has nothing to do with the
  platform the profile claims.

## Version detection

`BrowserCore.major` comes from the binary: `runtime::version::VersionReport`
runs `<executable> --version` under a deadline (5s) and parses the first digit
run (`Chromium 148.0.7778.215` -> 148). `FP_BROWSER_CHROMIUM_MAJOR` overrides it
for a binary that does not answer usefully.

The catalogue is reconciled on every start (`app::core_detect::maintain`):
a replaced binary is re-detected and the stored major/name are refreshed, so the
capability table cannot go stale. A missing executable is reported; a core whose
version cannot be re-read keeps its stored major instead of being silently
downgraded to "unknown".

## Not implemented yet

- **Legacy switch names.** Pre-144 cores used `--fingerprint-canvas-noise` /
  `--fingerprint-client-rects-noise`. This model does not carry them; those
  cores simply do not get canvas noise.
- **Profile fields the engine cannot honour.** Screen size, device scale
  factor, geolocation and the no-effect switches listed above are not modelled
  at all, so there is nothing to clean up yet; adding any of those fields
  requires re-measuring first. In particular, `--fingerprint-screen-width/height`
  and `--fingerprint-device-scale-factor` were measured to do nothing on a real
  display, and `--fingerprint-location` could not be made to produce a
  coordinate here.
- **GPU switch migration.** `--disable-gpu-fingerprint` maps to
  `--disable-spoofing=gpu`, and `--fingerprint-gpu-vendor`/`-renderer` are gone
  in 144+. The Rust model exposes only `SpoofingFeature::Gpu`, so the migration
  is implicit.
- **Surfaces a single reading cannot settle.** The audio and canvas
  fingerprints and the WebGL exclusion are only observable by comparing
  sessions, so `verify` does not assert them; the acceptance tests compare a
  spoofed session against an excluded one instead.
- **Verification has a cost and a footprint.** The probe appends and removes
  nodes, runs an offline audio render and opens an ICE gathering session in the
  page it is asked about, so it belongs to an explicit verification action, not
  to every profile start.
