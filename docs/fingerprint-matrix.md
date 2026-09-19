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
- This repository's Linux acceptance certifies the runtime path with
  ungoogled-chromium 148 (`docs/chromium-acceptance.md`).
- Nothing below 144 has been verified here, so nothing below 144 is claimed.

## Capability table

`CoreCapabilities::for_major` is the single source of truth; the serializer and
the compatibility layer both read it.

| Capability | Legacy (< 144) | Chrome144Plus (>= 144) | Switch |
| --- | --- | --- | --- |
| seed | yes | yes | `--fingerprint=` |
| brand | yes | yes | `--fingerprint-brand=` |
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

Known gaps, all inherited from the sibling product's matrix:

- **Cross-platform font handling.** When the spoofed platform differs from the
  host, `Ant-Browser` appends `font` to `--disable-spoofing` so the host's real
  fonts are used and CJK text does not render as missing glyphs. The Rust model
  has no host-platform policy yet, so this is not done.
- **Legacy switch names.** Pre-144 cores used `--fingerprint-canvas-noise` /
  `--fingerprint-client-rects-noise`. This model does not carry them; those
  cores simply do not get canvas noise.
- **No-effect switches for 144+.** `--fingerprint-device-memory`,
  `--fingerprint-color-depth`, `--fingerprint-touch-points`,
  `--fingerprint-do-not-track`, `--fingerprint-media-devices`,
  `--fingerprint-audio-noise`, font and WebGL vendor/renderer switches, screen
  size, DPR, geolocation and canvas measure-text noise were measured as having
  no effect on Chromium 144. The typed profile never emits them, so there is
  nothing to clean up yet; adding any of those fields requires re-measuring.
- **GPU switch migration.** `--disable-gpu-fingerprint` maps to
  `--disable-spoofing=gpu`, and `--fingerprint-gpu-vendor`/`-renderer` are gone
  in 144+. The Rust model exposes only `SpoofingFeature::Gpu`, so the migration
  is implicit.
- **Runtime verification.** `Ant-Browser` re-reads the resulting JS fingerprint
  values from the browser to prove the switches took effect. The Rust runtime
  only certifies that the process started; reading back fingerprints is a
  separate acceptance step and the only way to certify a new major.
