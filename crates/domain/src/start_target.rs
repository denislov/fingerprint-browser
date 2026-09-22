use crate::error::ValidationError;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", content = "url", rename_all = "snake_case")]
pub enum StartTarget {
    #[default]
    Blank,
    Url(String),
}

impl StartTarget {
    /// The page a profile with no target opens, and the one `Url` value that is
    /// accepted without one of [`StartTarget::SCHEMES`].
    pub const BLANK_URL: &'static str = "about:blank";

    /// The schemes a start page may use.
    ///
    /// `file` is here because a local page is a legitimate thing to open in a
    /// browser; a scheme outside this list is refused rather than passed through
    /// on the grounds that the browser probably knows what to do with it.
    pub const SCHEMES: [&'static str; 3] = ["http", "https", "file"];

    /// Checks that this target is a page rather than a browser switch.
    ///
    /// The target is appended to the browser's command line as an argument of its
    /// own, and Chromium reads an argument that begins with `-` as a switch
    /// wherever it sits in that command line. A profile whose start page is
    /// `--no-proxy-server` is therefore not a profile that opens an odd page: it
    /// is a profile with one more switch, and the proxy its fingerprint depends on
    /// is off. Nothing above this layer would notice - the value is a `String`, it
    /// serializes, it deserializes, and the planner appends it - which is why the
    /// rule lives on the type and is asked for by the domain validation, by the
    /// service that stores a profile, and by the planner that would use it.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let Self::Url(url) = self else {
            return Ok(());
        };

        if url.trim().is_empty() {
            return Err(ValidationError::EmptyStartUrl);
        }
        if url.chars().any(char::is_control) {
            return Err(ValidationError::UnusableStartUrl { url: url.clone() });
        }
        // Leading whitespace is not a switch to Chromium, but it is a value that
        // says one thing and does another, and nothing here should have it.
        if url.trim_start().starts_with('-') {
            return Err(ValidationError::StartUrlIsSwitch { url: url.clone() });
        }

        let parsed =
            Url::parse(url).map_err(|_| ValidationError::UnusableStartUrl { url: url.clone() })?;
        if self.is_blank_url(&parsed) || Self::SCHEMES.contains(&parsed.scheme()) {
            return Ok(());
        }
        Err(ValidationError::UnsupportedStartScheme {
            scheme: parsed.scheme().to_string(),
        })
    }

    /// Whether a parsed URL is the blank page [`StartTarget::Blank`] already says.
    fn is_blank_url(&self, parsed: &Url) -> bool {
        parsed.scheme() == "about" && parsed.path().eq_ignore_ascii_case("blank")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> StartTarget {
        StartTarget::Url(value.to_string())
    }

    /// The pages a profile may open. `about:blank` is the one [`StartTarget::Blank`]
    /// already means, so it has to be accepted as a URL too or a round trip through
    /// one representation would change the target.
    #[test]
    fn pages_with_a_supported_scheme_are_accepted() {
        assert!(StartTarget::Blank.validate().is_ok());
        for value in [
            "http://example.com",
            "https://example.com/path?q=1#fragment",
            "file:///home/alice/fixture.html",
            StartTarget::BLANK_URL,
        ] {
            assert!(url(value).validate().is_ok(), "{value}");
        }
    }

    /// The case this exists for: a value that is a switch on the browser's command
    /// line. It is refused as a switch rather than as a malformed URL, because the
    /// reason is what the reader has to understand.
    #[test]
    fn a_switch_spelled_as_a_start_page_is_refused() {
        for value in [
            "--no-proxy-server",
            "--user-data-dir=/tmp/other",
            "-incognito",
            // Leading whitespace does not hide it from Chromium's parser, and it
            // must not hide it from this one.
            "  --no-proxy-server",
        ] {
            assert!(
                matches!(
                    url(value).validate(),
                    Err(ValidationError::StartUrlIsSwitch { .. })
                ),
                "{value}"
            );
        }
    }

    /// Everything else a browser might understand and this program does not.
    #[test]
    fn a_scheme_this_program_does_not_open_is_refused_by_name() {
        for (value, scheme) in [
            ("chrome://newtab", "chrome"),
            ("javascript:alert(1)", "javascript"),
            ("data:text/html,<p>hi</p>", "data"),
            ("view-source:https://example.com", "view-source"),
        ] {
            match url(value).validate() {
                Err(ValidationError::UnsupportedStartScheme { scheme: named }) => {
                    assert_eq!(named, scheme, "{value}");
                }
                other => panic!("expected an unsupported scheme for {value}, got {other:?}"),
            }
        }
    }

    /// Values that are not a page at all, and the control characters that would
    /// make one value two on some command line.
    #[test]
    fn a_start_page_that_is_not_a_url_is_refused() {
        assert!(matches!(
            url("").validate(),
            Err(ValidationError::EmptyStartUrl)
        ));
        assert!(matches!(
            url("   ").validate(),
            Err(ValidationError::EmptyStartUrl)
        ));
        for value in ["not a url", "example.com", "http://exa mple.com\u{0}"] {
            assert!(
                matches!(
                    url(value).validate(),
                    Err(ValidationError::UnusableStartUrl { .. })
                ),
                "{value:?}"
            );
        }
    }

    /// A stored target survives a round trip, which is what lets the rule be on
    /// the type without changing the shape of what is stored.
    #[test]
    fn a_target_round_trips_through_json() {
        for target in [StartTarget::Blank, url("https://example.com")] {
            let json = serde_json::to_string(&target).expect("serialize");
            assert_eq!(
                serde_json::from_str::<StartTarget>(&json).expect("deserialize"),
                target
            );
        }
    }
}
