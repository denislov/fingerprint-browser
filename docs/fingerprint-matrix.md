# Fingerprint switch matrix

Which fingerprint-chromium switches this project is willing to pass, for which
core major, and on what evidence.

## The pivot: major 144

`FingerprintGeneration::PIVOT_MAJOR` is 144. Below it the core is `Legacy`,
from it upwards the core is `Chrome144Plus`.

Evidence:

- The sibling Go product (`Ant-Browser`) verified its switch set against a local
  Chromium 144 and encodes that in `backend/app_browser_fingerprint_matrix.go`:
  the noise switches it emits at runtime are
  `--fingerprinting-canvas-image-data-noise` and
  `--fingerprinting-client-rects-noise`, and a list of older switches was
  measured to have no effect on that build.
- This repository certifies the runtime path against a fingerprint-chromium 148
  build (`docs/chromium-acceptance.md`), and now certifies the fingerprint
  surface itself by reading it back out of the page
  (`crates/runtime/tests/fingerprint_real.rs`).
- Nothing below 144 has been verified here, so nothing below 144 is claimed.

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
| spoofing exclusions | yes | yes | `--disable-spoofing=<feature,...>` |
| canvas and client-rects noise | **no** | yes | `--fingerprinting-canvas-image-data-noise`, `--fingerprinting-client-rects-noise` |

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

- **Cross-platform font handling.** When the spoofed platform differs from the
  host, `Ant-Browser` appends `font` to `--disable-spoofing` so the host's real
  fonts are used and CJK text does not render as missing glyphs. The Rust model
  has no host-platform policy yet, so this is not done.
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
