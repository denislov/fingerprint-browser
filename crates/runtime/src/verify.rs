//! Reads a fingerprint back out of a running browser and compares it with what
//! the profile asked for.
//!
//! The supervisor can only report the switches it passed on the command line.
//! A switch the engine accepts and ignores is indistinguishable from one it
//! honours until something asks the page, which is what this module does: it
//! evaluates [`PROBE_EXPRESSION`] over CDP and turns the answer into
//! [`ObservedFingerprint`].
//!
//! [`verify`] then reports every profile claim the observation does not
//! support. It never reports success for something it could not observe: a
//! missing reading is itself a discrepancy. Two surfaces cannot be checked
//! against a single reading — canvas noise and the WebGL exclusion are only
//! visible by comparing two sessions, which is why the observed values are
//! exposed for comparison as well.
//!
//! The vocabulary this module asserts was measured against the verified
//! generation on Linux; see `docs/fingerprint-matrix.md`.

use crate::cdp::{CdpProbe as _, CdpSession, HttpCdpProbe};
use crate::error::CdpError;
use domain::{CoreCapabilities, FingerprintProfile, Platform};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Evaluates to a JSON string describing the fingerprint surface.
///
/// Kept to plain ES5 with no network access so it can run in any page state,
/// including `about:blank`.
pub const PROBE_EXPRESSION: &str = r#"(function(){
  function hash(text){var h=0;for(var i=0;i<text.length;i++){h=(h*31+text.charCodeAt(i))>>>0}return h}
  function hashBytes(bytes){var h=0;for(var i=0;i<bytes.length;i++){h=(h*31+bytes[i])>>>0}return h}
  var out={};
  var canvas=document.createElement('canvas');canvas.width=200;canvas.height=50;
  var context=canvas.getContext('2d');
  context.textBaseline='top';context.font='14px Arial';
  context.fillStyle='#f60';context.fillRect(0,0,100,20);
  context.fillStyle='#069';context.fillText('fingerprint-probe',2,2);
  try{out.canvasDataUrl=hash(canvas.toDataURL())}catch(e){out.canvasDataUrl=null}
  try{out.canvasPixels=hashBytes(context.getImageData(0,0,200,50).data)}catch(e){out.canvasPixels=null}
  out.measureText=Number(context.measureText('fingerprint-probe').width.toFixed(8));
  var rects=[];
  for(var i=0;i<3;i++){
    var element=document.createElement('div');
    element.style.cssText='width:100px;height:20px';element.textContent='rect'+i;
    document.body.appendChild(element);
    var rect=element.getBoundingClientRect();
    rects.push([Number(rect.x.toFixed(6)),Number(rect.y.toFixed(6)),rect.width,rect.height]);
  }
  out.rects=rects;
  try{
    var gl=document.createElement('canvas').getContext('webgl');
    var debug=gl.getExtension('WEBGL_debug_renderer_info');
    out.webglVendor=debug?gl.getParameter(debug.UNMASKED_VENDOR_WEBGL):null;
    out.webglRenderer=debug?gl.getParameter(debug.UNMASKED_RENDERER_WEBGL):null;
  }catch(e){out.webglVendor=null;out.webglRenderer=null}
  out.hardwareConcurrency=navigator.hardwareConcurrency;
  out.platform=navigator.platform;
  out.userAgent=navigator.userAgent;
  out.language=navigator.language;
  out.languages=(navigator.languages||[]).join(',');
  out.timezone=Intl.DateTimeFormat().resolvedOptions().timeZone;
  out.userAgentDataBrands=(navigator.userAgentData&&navigator.userAgentData.brands)?
    navigator.userAgentData.brands.map(function(b){return b.brand+'/'+b.version}).join(','):null;
  if(navigator.userAgentData&&navigator.userAgentData.getHighEntropyValues){
    return navigator.userAgentData.getHighEntropyValues(['platformVersion','architecture','bitness','uaFullVersion','fullVersionList'])
      .then(function(high){
        out.platformVersion=high.platformVersion;
        out.architecture=high.architecture;
        out.bitness=high.bitness;
        out.userAgentFullVersion=high.uaFullVersion;
        out.userAgentFullBrands=(high.fullVersionList||[]).map(function(b){return b.brand+'/'+b.version}).join(',');
        return JSON.stringify(out);
      });
  }
  return JSON.stringify(out);
})()"#;

/// What the page reported, as read back over CDP.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ObservedFingerprint {
    pub canvas_data_url: Option<u64>,
    pub canvas_pixels: Option<u64>,
    pub measure_text: Option<f64>,
    pub rects: Vec<[f64; 4]>,
    pub webgl_vendor: Option<String>,
    pub webgl_renderer: Option<String>,
    pub hardware_concurrency: Option<u32>,
    pub platform: Option<String>,
    pub user_agent: Option<String>,
    pub language: Option<String>,
    pub languages: Option<String>,
    pub timezone: Option<String>,
    pub user_agent_data_brands: Option<String>,
    pub platform_version: Option<String>,
    pub architecture: Option<String>,
    pub bitness: Option<String>,
    pub user_agent_full_version: Option<String>,
    pub user_agent_full_brands: Option<String>,
}

impl ObservedFingerprint {
    /// Parses the JSON string the probe expression returns.
    pub fn parse(value: &serde_json::Value) -> Result<Self, CdpError> {
        let text = value
            .as_str()
            .ok_or_else(|| CdpError::Evaluation("probe did not return a JSON string".into()))?;
        serde_json::from_str(text).map_err(|e| CdpError::InvalidResponse(e.to_string()))
    }

    /// The values a canvas fingerprint is computed from.
    ///
    /// Two sessions must not agree on all of them: noise is only observable by
    /// comparison, since a single reading cannot say whether it was perturbed.
    pub fn canvas_signature(&self) -> (Option<u64>, Option<u64>, Option<f64>) {
        (self.canvas_data_url, self.canvas_pixels, self.measure_text)
    }

    /// Whether the ClientRects surface carries sub-pixel noise.
    pub fn has_rect_noise(&self) -> bool {
        self.rects.iter().any(|rect| {
            rect.iter()
                .any(|value| (value.fract()).abs() > f64::EPSILON && *value != 0.0)
        })
    }

    /// Whether the brand list claims a brand whose name contains `needle`.
    pub fn claims_brand(&self, needle: &str) -> bool {
        [&self.user_agent_data_brands, &self.user_agent_full_brands]
            .into_iter()
            .flatten()
            .any(|list| list.to_lowercase().contains(&needle.to_lowercase()))
    }
}

/// One profile claim the observation does not support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discrepancy {
    /// The surface that disagrees, e.g. `platform`.
    pub claim: &'static str,
    pub expected: String,
    pub observed: String,
}

impl std::fmt::Display for Discrepancy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: expected {}, observed {}",
            self.claim, self.expected, self.observed
        )
    }
}

/// `navigator.platform` and user agent fragments the engine produces for each
/// spoofable platform, measured on the verified generation.
fn platform_readback(platform: Platform) -> (&'static str, &'static str) {
    match platform {
        Platform::Windows => ("Win32", "Windows NT"),
        Platform::MacOs => ("MacIntel", "Macintosh"),
        Platform::Linux => ("Linux", "Linux"),
    }
}

/// The brand name the engine reports for a requested brand.
fn brand_readback(brand: domain::BrowserBrand) -> Option<&'static str> {
    match brand {
        domain::BrowserBrand::Chrome => Some("Google Chrome"),
        domain::BrowserBrand::Edge => Some("Microsoft Edge"),
        // Not honoured by the engine; the capability table does not claim them
        // and the compatibility layer reports the omission.
        domain::BrowserBrand::Opera | domain::BrowserBrand::Vivaldi => None,
    }
}

/// Reports every claim in `profile` that `observed` does not support.
pub fn verify(
    profile: &FingerprintProfile,
    capabilities: &CoreCapabilities,
    observed: &ObservedFingerprint,
) -> Vec<Discrepancy> {
    let mut found = Vec::new();

    let (platform_string, user_agent_fragment) = platform_readback(profile.platform);
    check(
        &mut found,
        "platform",
        platform_string.to_string(),
        observed.platform.clone(),
    );
    if capabilities.supports_brand(profile.brand)
        && let Some(expected) = brand_readback(profile.brand)
        && !observed.claims_brand(expected)
    {
        found.push(Discrepancy {
            claim: "brand",
            expected: expected.to_string(),
            observed: observed
                .user_agent_data_brands
                .clone()
                .unwrap_or_else(|| "not reported".to_string()),
        });
    }
    if let Some(user_agent) = &observed.user_agent {
        if !user_agent.contains(user_agent_fragment) {
            found.push(Discrepancy {
                claim: "user agent",
                expected: format!("contains {user_agent_fragment}"),
                observed: user_agent.clone(),
            });
        }
    } else {
        found.push(Discrepancy {
            claim: "user agent",
            expected: format!("contains {user_agent_fragment}"),
            observed: "not reported".to_string(),
        });
    }

    if let Some(concurrency) = profile.hardware_concurrency {
        check(
            &mut found,
            "hardware concurrency",
            concurrency.to_string(),
            observed.hardware_concurrency.map(|value| value.to_string()),
        );
    }

    // `navigator.language` follows the first entry of `--accept-lang`; `--lang`
    // alone does not move it (measured on the verified generation).
    let expected_language = profile
        .accept_language
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    check(
        &mut found,
        "language",
        expected_language.clone(),
        observed.language.clone(),
    );
    if let Some(languages) = &observed.languages
        && !languages.starts_with(&expected_language)
    {
        found.push(Discrepancy {
            claim: "languages",
            expected: format!("starts with {expected_language}"),
            observed: languages.clone(),
        });
    }

    check(
        &mut found,
        "timezone",
        profile.timezone.clone(),
        observed.timezone.clone(),
    );

    if capabilities.supports_platform_version
        && let Some(version) = profile
            .platform_version
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        check(
            &mut found,
            "platform version",
            version.to_string(),
            observed.platform_version.clone(),
        );
    }

    found
}

/// Records a claim the observation does not support.
fn check(
    found: &mut Vec<Discrepancy>,
    claim: &'static str,
    expected: String,
    observed: Option<String>,
) {
    match observed {
        Some(observed) if observed == expected => {}
        Some(observed) => found.push(Discrepancy {
            claim,
            expected,
            observed,
        }),
        None => found.push(Discrepancy {
            claim,
            expected,
            observed: "not reported".to_string(),
        }),
    }
}

/// A browser with a CDP endpoint, ready to be asked about its fingerprint.
pub struct FingerprintProbe {
    timeout: Duration,
}

impl Default for FingerprintProbe {
    fn default() -> Self {
        Self::new(Duration::from_secs(10))
    }
}

impl FingerprintProbe {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// Waits for the CDP endpoint, then reads the fingerprint out of the page.
    ///
    /// A reading taken this way is complete only if the page already is a real
    /// document; on `about:blank` the user agent data surface does not exist
    /// and the brand claims come back unverifiable. Use [`Self::read_at`] when
    /// the page cannot be chosen in advance.
    pub fn read(&self, port: u16) -> Result<ObservedFingerprint, CdpError> {
        self.read_with(port, None)
    }

    /// Navigates the page to `url` before reading.
    pub fn read_at(&self, port: u16, url: &str) -> Result<ObservedFingerprint, CdpError> {
        self.read_with(port, Some(url))
    }

    fn read_with(&self, port: u16, url: Option<&str>) -> Result<ObservedFingerprint, CdpError> {
        HttpCdpProbe::new().wait_ready(port, self.timeout)?;
        let mut session = CdpSession::open_page(port, self.timeout)?;
        if let Some(url) = url {
            session.navigate(url, self.timeout)?;
        }
        let value = session.evaluate(PROBE_EXPRESSION, self.timeout)?;
        ObservedFingerprint::parse(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::BrowserBrand;

    /// Real readings captured from the verified generation on Linux: this one
    /// comes from a session launched with the profile `profile()` asks for.
    const MATCHING_READING: &str = r#"{
      "canvasDataUrl": 368676017, "canvasPixels": 4160716610, "measureText": -0.00011179,
      "rects": [[8.000975,26.000078,100,20],[8.000975,46.00008,100,20],[8.000975,66.000076,100,20]],
      "webglVendor": "Google Inc. (Intel)",
      "webglRenderer": "ANGLE (Intel, Intel(R) Iris(R) Xe Graphics (0x00009A49) Direct3D11 vs_5_0 ps_5_0, D3D11)",
      "hardwareConcurrency": 8, "platform": "Win32",
      "userAgent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
      "language": "en-US", "languages": "en-US,en", "timezone": "America/New_York",
      "userAgentDataBrands": "Chromium/148,Google Chrome/148,Not/A)Brand/99",
      "platformVersion": "10.0.0", "architecture": "x86", "bitness": "64",
      "userAgentFullVersion": "148.0.7778.97",
      "userAgentFullBrands": "Chromium/148.0.7778.97,Google Chrome/148.0.7778.97,Not/A)Brand/99.0.0.0"
    }"#;

    fn observed(json: &str) -> ObservedFingerprint {
        serde_json::from_str(json).expect("reading parses")
    }

    fn profile() -> FingerprintProfile {
        FingerprintProfile {
            seed: 11111,
            brand: BrowserBrand::Chrome,
            brand_version: Some("148.0.7778.97".to_string()),
            platform: Platform::Windows,
            platform_version: Some("10.0.0".to_string()),
            language: "en-US".to_string(),
            accept_language: "en-US,en;q=0.9".to_string(),
            timezone: "America/New_York".to_string(),
            hardware_concurrency: Some(8),
            webrtc_policy: domain::WebRtcPolicy::DisableNonProxiedUdp,
            disabled_spoofing: Vec::new(),
        }
    }

    #[test]
    fn a_faithful_session_reports_nothing() {
        let capabilities = CoreCapabilities::for_major(148);
        let found = verify(&profile(), &capabilities, &observed(MATCHING_READING));

        assert!(found.is_empty(), "unexpected findings: {found:?}");
    }

    #[test]
    fn every_claim_is_checked_independently() {
        let capabilities = CoreCapabilities::for_major(148);
        let broken = observed(
            r#"{
              "hardwareConcurrency": 26, "platform": "Linux x86_64",
              "userAgent": "Mozilla/5.0 (X11; Linux x86_64) Chrome/148.0.0.0 Safari/537.36",
              "language": "zh-CN", "languages": "zh-CN,zh", "timezone": "Asia/Shanghai",
              "userAgentDataBrands": "Not/A)Brand/99,Chromium/148",
              "platformVersion": "6.14.0"
            }"#,
        );

        let found = verify(&profile(), &capabilities, &broken);
        let claims: Vec<&str> = found.iter().map(|f| f.claim).collect();

        assert_eq!(
            claims,
            vec![
                "platform",
                "brand",
                "user agent",
                "hardware concurrency",
                "language",
                "languages",
                "timezone",
                "platform version"
            ]
        );
        assert_eq!(found[3].expected, "8");
        assert_eq!(found[3].observed, "26");
    }

    #[test]
    fn a_missing_reading_is_a_discrepancy_not_a_pass() {
        let capabilities = CoreCapabilities::for_major(148);
        let found = verify(&profile(), &capabilities, &ObservedFingerprint::default());

        assert!(!found.is_empty());
        assert!(
            found
                .iter()
                .any(|f| f.claim == "hardware concurrency" && f.observed == "not reported")
        );
        assert!(found.iter().any(|f| f.claim == "timezone"));
    }

    #[test]
    fn an_unsupported_brand_is_not_asserted() {
        let capabilities = CoreCapabilities::for_major(148);
        let mut fp = profile();
        fp.brand = BrowserBrand::Opera;

        let found = verify(&fp, &capabilities, &observed(MATCHING_READING));

        assert!(
            !found.iter().any(|f| f.claim == "brand"),
            "the engine never reports Opera, so the claim is not made: {found:?}"
        );
    }

    #[test]
    fn noise_is_visible_by_comparison_only() {
        let noisy = observed(MATCHING_READING);
        let quiet = observed(
            r#"{
              "canvasDataUrl": 1521918225, "canvasPixels": 4164937777, "measureText": 102.72363281,
              "rects": [[8,26,100,20],[8,46,100,20],[8,66,100,20]]
            }"#,
        );

        assert_ne!(noisy.canvas_signature(), quiet.canvas_signature());
        assert!(noisy.has_rect_noise());
        assert!(!quiet.has_rect_noise());
    }

    #[test]
    fn the_probe_expression_never_touches_the_network() {
        for forbidden in ["fetch(", "XMLHttpRequest", "WebSocket", "importScripts"] {
            assert!(
                !PROBE_EXPRESSION.contains(forbidden),
                "the probe must stay self-contained: {forbidden}"
            );
        }
    }
}
