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
//! missing reading is itself a discrepancy.
//!
//! Two kinds of surface are read:
//!
//! - Claims a single reading can settle: the platform and user agent, the
//!   language, the timezone, the hardware concurrency, the platform version,
//!   that no ICE candidate leaks a local or public address, and that CJK text
//!   still has glyphs.
//! - Surfaces only a second session can settle, because the reading is
//!   perturbed rather than replaced: the canvas ([`ObservedFingerprint::canvas_signature`])
//!   and audio ([`ObservedFingerprint::audio_signature`]) fingerprints. A
//!   reading cannot say whether it was perturbed; two sessions with different
//!   seeds must disagree, and two sessions that exclude the same surface must
//!   agree. The WebGL exclusion is in the same group.
//!
//! The vocabulary this module asserts was measured against the verified
//! generation on Linux; see `docs/fingerprint-matrix.md`.

use crate::cdp::{CdpProbe as _, CdpSession, HttpCdpProbe};
use crate::error::CdpError;
use domain::{CoreCapabilities, FingerprintProfile, Platform, WebRtcPolicy};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Evaluates to a JSON string describing the fingerprint surface.
///
/// Kept to plain ES5 with no network access so it can run in any page state,
/// including `about:blank`.
pub const PROBE_EXPRESSION: &str = r#"(function(){
  function hash(text){var h=0;for(var i=0;i<text.length;i++){h=(h*31+text.charCodeAt(i))>>>0}return h}
  function hashBytes(bytes){var h=0;for(var i=0;i<bytes.length;i++){h=(h*31+bytes[i])>>>0}return h}
  function hashFloats(values){var h=0;for(var i=0;i<values.length;i++){h=(h*31+(Math.round(values[i]*1e7)&0xffff))>>>0}return h}
  // A document that is still parsing has no body yet; documentElement always
  // exists, and appending there is enough for a measurement.
  function host(){return document.body||document.documentElement}
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
    host().appendChild(element);
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
  function highEntropy(){
    return new Promise(function(resolve){
      if(!navigator.userAgentData||!navigator.userAgentData.getHighEntropyValues)return resolve();
      navigator.userAgentData.getHighEntropyValues(['platformVersion','architecture','bitness','uaFullVersion','fullVersionList'])
        .then(function(high){
          out.platformVersion=high.platformVersion;
          out.architecture=high.architecture;
          out.bitness=high.bitness;
          out.userAgentFullVersion=high.uaFullVersion;
          out.userAgentFullBrands=(high.fullVersionList||[]).map(function(b){return b.brand+'/'+b.version}).join(',');
          resolve();
        }).catch(function(){resolve()});
    });
  }
  // Text metrics measured through layout: canvas text metrics are perturbed by
  // the seed, so they cannot be used to read fonts back.
  function domWidth(family,text){
    var span=document.createElement('span');
    span.style.cssText='position:absolute;left:-9999px;top:-9999px;font-size:48px;white-space:nowrap';
    if(family)span.style.fontFamily=family;
    span.textContent=text;
    host().appendChild(span);
    var width=span.offsetWidth;
    span.remove();
    return width;
  }
  function fontReading(){
    return new Promise(function(resolve){
      var measure=function(){
        try{
          var bases=['monospace','sans-serif','serif'];
          var span=document.createElement('span');
          span.style.cssText='position:absolute;left:-9999px;top:-9999px;font-size:72px;white-space:nowrap';
          span.textContent='mmmmmmmmmmlli';
          host().appendChild(span);
          var base={};
          bases.forEach(function(b){span.style.fontFamily=b;base[b]=span.offsetWidth+','+span.offsetHeight});
          var found=[];
          ['Arial','Helvetica','Times New Roman','Courier New','Georgia','Verdana','Tahoma','Trebuchet MS',
           'Impact','Comic Sans MS','Segoe UI','Calibri','Cambria','Consolas','Arial Black','DejaVu Sans',
           'DejaVu Serif','Liberation Sans','Liberation Serif','Noto Sans CJK SC','Noto Color Emoji',
           'WenQuanYi Micro Hei','Ubuntu','Cantarell','Roboto','DejaVu Sans Mono'].forEach(function(family){
            var present=bases.some(function(b){
              span.style.fontFamily="'"+family+"',"+b;
              return (span.offsetWidth+','+span.offsetHeight)!==base[b];
            });
            if(present)found.push(family);
          });
          span.remove();
          out.fonts=found;
          out.fontAsciiWidth=domWidth(null,'mmmmmmmmmmlli');
          out.fontLatinWidth=domWidth(null,'abcdefghij');
          out.fontCjkWidth=domWidth(null,'\u4e2d\u6587\u6d4b\u8bd5');
          out.fontEmojiWidth=domWidth(null,'\ud83d\ude00');
          // Codepoints with no glyph anywhere: the width of a missing glyph box.
          out.fontTofuWidth=domWidth(null,'\uffff\ufffe\ue000\ue001');
        }catch(e){out.fontError=String(e)}
        resolve();
      };
      try{document.fonts.ready.then(measure).catch(measure)}catch(e){out.fontError=String(e);measure()}
    });
  }
  // OfflineAudioContext needs no gesture and no output device.
  function audioReading(){
    return new Promise(function(resolve){
      try{
        var context=new OfflineAudioContext(1,44100,44100);
        var oscillator=context.createOscillator();oscillator.type='triangle';oscillator.frequency.value=10000;
        var compressor=context.createDynamicsCompressor();
        compressor.threshold.value=-50;compressor.knee.value=40;compressor.ratio.value=12;
        compressor.attack.value=0;compressor.release.value=0.25;
        oscillator.connect(compressor);compressor.connect(context.destination);oscillator.start(0);
        context.startRendering().then(function(buffer){
          var data=buffer.getChannelData(0),sum=0;
          for(var i=0;i<data.length;i++){sum+=Math.abs(data[i])}
          out.audioHash=hashFloats(data);
          out.audioSum=Number(sum.toFixed(6));
          resolve();
        }).catch(function(e){out.audioError=String(e);resolve()});
      }catch(e){out.audioError=String(e);resolve()}
    });
  }
  // No STUN server: a host candidate is already a leak, and it needs no
  // network. The wait is generous because a proxied browser can take longer to
  // finish gathering, and "still gathering" is reported rather than assumed
  // empty.
  function webrtcReading(){
    return new Promise(function(resolve){
      var candidates=[];
      var finished=false;
      var finish=function(state){
        if(finished)return;finished=true;
        out.webrtcCandidates=candidates;
        out.webrtcGathering=state;
        resolve();
      };
      try{
        var peer=new RTCPeerConnection({iceServers:[]});
        peer.onicecandidate=function(event){
          if(event.candidate)candidates.push(event.candidate.candidate);
          else finish(peer.iceGatheringState);
        };
        peer.createDataChannel('probe');
        peer.createOffer().then(function(offer){return peer.setLocalDescription(offer)})
          .catch(function(e){out.webrtcError=String(e)});
        setTimeout(function(){finish(peer.iceGatheringState);try{peer.close()}catch(e){}},5000);
      }catch(e){out.webrtcError=String(e);resolve()}
    });
  }
  return Promise.all([highEntropy(),fontReading(),audioReading(),webrtcReading()])
    .then(function(){return JSON.stringify(out)});
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
    /// The audio fingerprint the page produced, and the sum it was hashed from.
    pub audio_hash: Option<u64>,
    pub audio_sum: Option<f64>,
    /// ICE candidates the page gathered, verbatim.
    pub webrtc_candidates: Vec<String>,
    /// `RTCPeerConnection.iceGatheringState` once gathering stopped.
    pub webrtc_gathering: Option<String>,
    /// Font families the page can see, and the layout widths they produce.
    pub fonts: Vec<String>,
    pub font_ascii_width: Option<f64>,
    pub font_latin_width: Option<f64>,
    pub font_cjk_width: Option<f64>,
    pub font_emoji_width: Option<f64>,
    /// Width of codepoints that have no glyph anywhere.
    pub font_tofu_width: Option<f64>,
    /// Why a reading is missing, when the probe could say.
    pub audio_error: Option<String>,
    pub webrtc_error: Option<String>,
    pub font_error: Option<String>,
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

    /// The values an audio fingerprint is computed from.
    ///
    /// Like the canvas, an audio reading cannot say whether it was perturbed:
    /// noise is only visible by comparing sessions.
    pub fn audio_signature(&self) -> (Option<u64>, Option<f64>) {
        (self.audio_hash, self.audio_sum)
    }

    /// Candidates that expose an address without going through the proxy.
    ///
    /// A `host` candidate is a local interface address and an `srflx` candidate
    /// is the public address a STUN server saw; both are what a WebRTC leak
    /// check is looking for.
    pub fn leaking_candidates(&self) -> Vec<&str> {
        self.webrtc_candidates
            .iter()
            .filter(|candidate| candidate.contains(" typ host") || candidate.contains(" typ srflx"))
            .map(String::as_str)
            .collect()
    }

    /// Whether CJK text renders as missing-glyph boxes.
    ///
    /// Comparing against a codepoint that has no glyph anywhere avoids
    /// hard-coding a font size: the box width is whatever this renderer uses.
    pub fn has_missing_cjk(&self) -> Option<bool> {
        match (self.font_cjk_width, self.font_tofu_width) {
            (Some(cjk), Some(tofu)) => Some((cjk - tofu).abs() < 1.0),
            _ => None,
        }
    }

    /// Whether the font surface could be read at all.
    pub fn fonts_readable(&self) -> bool {
        !self.fonts.is_empty() && self.font_ascii_width.is_some()
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

    // Only the strict policy makes a claim about leaking: the browser may still
    // gather relay candidates through a proxy, but a local or public address
    // must never appear.
    if profile.webrtc_policy == WebRtcPolicy::DisableNonProxiedUdp {
        match observed.webrtc_gathering.as_deref() {
            Some("complete") => {
                let leaking = observed.leaking_candidates();
                if !leaking.is_empty() {
                    found.push(Discrepancy {
                        claim: "webrtc leak",
                        expected: "no host or srflx candidate".to_string(),
                        observed: leaking.join(" "),
                    });
                }
            }
            // Without the end-of-gathering signal an empty list proves nothing:
            // it is also what a probe that never ran would report.
            Some(state) => found.push(Discrepancy {
                claim: "webrtc leak",
                expected: "ice gathering completes".to_string(),
                observed: state.to_string(),
            }),
            None => found.push(Discrepancy {
                claim: "webrtc leak",
                expected: "ice gathering completes".to_string(),
                observed: unreadable("not reported", observed.webrtc_error.as_deref()),
            }),
        }
    }

    // Missing glyphs are what a platform spoof that forgets the host's fonts
    // produces, and they are visible in a single reading.
    match observed.has_missing_cjk() {
        Some(true) => found.push(Discrepancy {
            claim: "fonts",
            expected: "CJK text has glyphs".to_string(),
            observed: "renders as missing glyphs".to_string(),
        }),
        Some(false) => {}
        None => found.push(Discrepancy {
            claim: "fonts",
            expected: "CJK text has glyphs".to_string(),
            observed: unreadable("not reported", observed.font_error.as_deref()),
        }),
    }

    found
}

/// Says a surface was not read, and why when the probe recorded a reason.
fn unreadable(fallback: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("{fallback}: {reason}"),
        None => fallback.to_string(),
    }
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

    /// Reads the fingerprint on a page of the probe's own.
    ///
    /// A fresh target is opened for the reading and closed afterwards, so
    /// verifying a running profile does not navigate, reload or otherwise touch
    /// the page the user is looking at. The document is a local file because
    /// the user agent data surface is absent on `about:blank` and `data:` URLs.
    pub fn read_fresh(&self, port: u16) -> Result<ObservedFingerprint, CdpError> {
        let probe = HttpCdpProbe::new();
        probe.wait_ready(port, self.timeout)?;
        let document = write_probe_document()?;
        let url = format!("file://{}", document.display());
        let target = probe.create_page(port, &url, self.timeout)?;

        let reading = (|| {
            let mut session = CdpSession::connect_page(port, &target, self.timeout)?;
            session.wait_for_document("file:", self.timeout)?;
            let value = session.evaluate(PROBE_EXPRESSION, self.timeout)?;
            ObservedFingerprint::parse(&value)
        })();

        // Closing is best effort: a target left behind must not turn a good
        // reading into an error, and the reading is what the caller asked for.
        let _ = probe.close_page(port, target.target_id(), self.timeout);
        reading
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

/// Writes the document the probe runs in and returns its path.
///
/// The content is deliberately empty: the probe brings its own DOM.
fn write_probe_document() -> Result<std::path::PathBuf, CdpError> {
    let path = std::env::temp_dir().join("fp-browser-probe.html");
    std::fs::write(
        &path,
        "<!doctype html><meta charset=\"utf-8\"><title>fingerprint probe</title><body>",
    )
    .map_err(|error| CdpError::Http(error.to_string()))?;
    Ok(path)
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
      "userAgentFullBrands": "Chromium/148.0.7778.97,Google Chrome/148.0.7778.97,Not/A)Brand/99.0.0.0",
      "audioHash": 1171369572, "audioSum": 10749.610675,
      "webrtcCandidates": [], "webrtcGathering": "complete",
      "fonts": ["Arial","Helvetica","Times New Roman","Courier New","DejaVu Sans","DejaVu Serif",
                "Liberation Sans","Liberation Serif","Noto Sans CJK SC","Noto Color Emoji","Ubuntu",
                "Cantarell","DejaVu Sans Mono"],
      "fontAsciiWidth": 413, "fontLatinWidth": 203, "fontCjkWidth": 192,
      "fontEmojiWidth": 60, "fontTofuWidth": 149
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
                "platform version",
                "webrtc leak",
                "fonts"
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
    fn a_leaking_candidate_is_reported() {
        let capabilities = CoreCapabilities::for_major(148);
        let leaking = observed(
            r#"{
              "webrtcCandidates": [
                "candidate:1 1 udp 2113937151 192.168.1.10 54492 typ host generation 0 ufrag a",
                "candidate:2 1 udp 1677729535 203.0.113.7 53224 typ srflx raddr 192.168.1.10"
              ],
              "webrtcGathering": "complete"
            }"#,
        );

        let found = verify(&profile(), &capabilities, &leaking);

        let leak = found
            .iter()
            .find(|f| f.claim == "webrtc leak")
            .expect("a host and a srflx candidate are both leaking");
        assert!(leak.observed.contains("typ host"));
        assert!(leak.observed.contains("typ srflx"));
    }

    #[test]
    fn candidates_through_a_proxy_are_not_a_leak() {
        let capabilities = CoreCapabilities::for_major(148);
        let relayed = observed(
            r#"{
              "webrtcCandidates": ["candidate:3 1 tcp 1518280447 127.0.0.1 9 typ relay raddr 0.0.0.0"],
              "webrtcGathering": "complete"
            }"#,
        );

        let found = verify(&profile(), &capabilities, &relayed);

        assert!(
            !found.iter().any(|f| f.claim == "webrtc leak"),
            "a relay candidate is the proxied path, not a leak: {found:?}"
        );
    }

    #[test]
    fn gathering_that_never_finished_is_not_a_pass() {
        let capabilities = CoreCapabilities::for_major(148);
        let unfinished = observed(r#"{"webrtcCandidates": [], "webrtcGathering": "gathering"}"#);

        let found = verify(&profile(), &capabilities, &unfinished);

        let leak = found
            .iter()
            .find(|f| f.claim == "webrtc leak")
            .expect("an empty candidate list only counts once gathering stopped");
        assert_eq!(leak.observed, "gathering");
    }

    #[test]
    fn a_relaxed_policy_makes_no_leak_claim() {
        let capabilities = CoreCapabilities::for_major(148);
        let mut fp = profile();
        fp.webrtc_policy = WebRtcPolicy::DefaultPublicAndPrivateInterfaces;
        let leaking = observed(
            r#"{
              "webrtcCandidates": ["candidate:1 1 udp 2113937151 192.168.1.10 54492 typ host"],
              "webrtcGathering": "complete"
            }"#,
        );

        let found = verify(&fp, &capabilities, &leaking);

        assert!(
            !found.iter().any(|f| f.claim == "webrtc leak"),
            "the profile did not ask for the strict policy: {found:?}"
        );
    }

    #[test]
    fn missing_cjk_glyphs_are_reported() {
        let capabilities = CoreCapabilities::for_major(148);
        // Width equal to the missing-glyph box means every CJK char is a box.
        let boxed = observed(r#"{"fontCjkWidth": 149, "fontTofuWidth": 149, "fonts": ["Arial"]}"#);
        let readable =
            observed(r#"{"fontCjkWidth": 192, "fontTofuWidth": 149, "fonts": ["Arial"]}"#);

        assert!(
            verify(&profile(), &capabilities, &boxed)
                .iter()
                .any(|f| f.claim == "fonts" && f.observed.contains("missing glyphs"))
        );
        assert!(
            !verify(&profile(), &capabilities, &readable)
                .iter()
                .any(|f| f.claim == "fonts")
        );
        assert_eq!(boxed.has_missing_cjk(), Some(true));
        assert_eq!(readable.has_missing_cjk(), Some(false));
    }

    #[test]
    fn audio_is_read_and_compared_between_sessions() {
        let capabilities = CoreCapabilities::for_major(148);
        let first = observed(MATCHING_READING);
        let second = observed(r#"{"audioHash": 2544522210, "audioSum": 10748.964798}"#);

        assert_ne!(first.audio_signature(), second.audio_signature());
        assert!(
            !verify(&profile(), &capabilities, &second)
                .iter()
                .any(|f| f.claim == "audio"),
            "audio is comparison-only: one reading cannot say whether it was perturbed"
        );
    }

    #[test]
    fn the_font_claim_is_settled_by_the_widths_not_the_enumeration() {
        let capabilities = CoreCapabilities::for_major(148);
        // Widths present: the glyph question is answered whether or not the
        // enumeration found anything.
        let widths_only = observed(r#"{"fonts": [], "fontCjkWidth": 192, "fontTofuWidth": 149}"#);
        // Widths absent: nothing was measured, so nothing is certified.
        let blind = observed(r#"{"fonts": ["Arial"]}"#);

        assert!(!widths_only.fonts_readable(), "no family was enumerated");
        assert!(
            !verify(&profile(), &capabilities, &widths_only)
                .iter()
                .any(|f| f.claim == "fonts")
        );
        assert!(
            verify(&profile(), &capabilities, &blind)
                .iter()
                .any(|f| f.claim == "fonts" && f.observed == "not reported")
        );
    }

    #[test]
    fn the_probe_expression_never_touches_the_network() {
        for forbidden in [
            "fetch(",
            "XMLHttpRequest",
            "WebSocket",
            "importScripts",
            "stun:",
        ] {
            assert!(
                !PROBE_EXPRESSION.contains(forbidden),
                "the probe must stay self-contained: {forbidden}"
            );
        }
    }
}
