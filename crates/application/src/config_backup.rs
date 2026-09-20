//! The configuration exchange file: what an export writes and an import reads.
//!
//! This module owns the document, not the decision to take one. It builds a
//! document from the three lists a configuration is made of, applies the
//! credential choice, and reads a document back - refusing one it cannot be sure
//! it understands. Where a document meets the database - identifiers that are
//! already taken, paths that do not exist here, a profile whose core is missing -
//! is a separate question, and not this file's.
//!
//! Browser data is deliberately absent. A browser-data backup is a copy of
//! `profiles/<id>` directories and has no document at all; putting cookies in
//! here would make the common case - move my configuration to a new machine -
//! cost gigabytes.

use domain::{BrowserCore, BrowserProfile, ProxyOutbound, ProxyProfile};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What an export writes at the top of the file, so a reader can refuse it
/// before parsing anything else.
pub const FORMAT: &str = "fp-browser/config-backup";

/// The version of the *document*, not of the program.
///
/// It changes when the shape below changes, which is what lets an old file be
/// read by a newer build on purpose rather than by luck. A program version would
/// say nothing about whether the fields this build wants are in the file.
pub const VERSION: u32 = 1;

/// Whether an export wrote the credentials it could have left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Credentials {
    /// Written as they are. The file is then a plain-text document holding
    /// secrets, and the export is expected to say so.
    Included,
    /// Left out, by [`ProxyOutbound::without_credentials`].
    Excluded,
}

impl Credentials {
    /// What the export does to one outbound.
    pub fn apply(self, outbound: &ProxyOutbound) -> ProxyOutbound {
        match self {
            Self::Included => outbound.clone(),
            Self::Excluded => outbound.without_credentials(),
        }
    }
}

/// Where and when an export was taken, for the file's own header.
///
/// Both fields are informational - nothing in the program decides anything from
/// them. They are strings because this layer has no clock: the caller has one,
/// and formatting a time is the app layer's business.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportOrigin {
    /// A UTC timestamp, spelled the way the activity log spells one.
    pub exported_at: String,
    /// The data directory the export was read from, so an import can say where a
    /// path came from rather than only that it was re-pointed.
    pub source_data_dir: String,
}

/// What an export is made of, before the credential choice is applied.
///
/// The three lists travel together because every export writes all three and a
/// profile is unreadable without the core it names and the proxy it uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigSnapshot {
    pub cores: Vec<BrowserCore>,
    pub proxies: Vec<ProxyProfile>,
    pub profiles: Vec<BrowserProfile>,
}

impl ConfigSnapshot {
    /// Whether the installation this was read from holds nothing at all.
    ///
    /// This is restore's precondition: a configuration with no cores, proxies or
    /// profiles can be replaced without asking, because there is nothing to
    /// lose. It is asked of the snapshot rather than of a repository, which has
    /// no emptiness call of its own.
    pub fn is_empty(&self) -> bool {
        self.cores.is_empty() && self.proxies.is_empty() && self.profiles.is_empty()
    }

    /// How many proxies hold something an [`Credentials::Excluded`] export would
    /// leave out.
    ///
    /// Asked of the snapshot rather than of a built document, because once the
    /// document is stripped it no longer knows the answer - which is the whole
    /// point of stripping it. This is the number an export has to be able to
    /// report, so that "credentials were left out" and "there were none" cannot
    /// be confused for each other.
    pub fn proxies_carrying_credentials(&self) -> usize {
        self.proxies
            .iter()
            .filter(|proxy| proxy.outbound.carries_credentials())
            .count()
    }
}

/// A configuration backup, as it is written and read.
///
/// The fields are the domain types rather than a parallel set invented for
/// export, so a field cannot mean one thing in the database and another here.
/// [`deny_unknown_fields`](serde::Deserialize) is what keeps that promise in the
/// other direction: a field this build does not know is refused rather than
/// dropped in silence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigBackup {
    pub format: String,
    pub version: u32,
    pub credentials: Credentials,
    pub exported_at: String,
    pub source_data_dir: String,
    pub cores: Vec<BrowserCore>,
    pub proxies: Vec<ProxyProfile>,
    pub profiles: Vec<BrowserProfile>,
}

impl ConfigBackup {
    /// Builds the document an export would write.
    pub fn build(snapshot: ConfigSnapshot, credentials: Credentials, origin: ExportOrigin) -> Self {
        Self {
            format: FORMAT.to_string(),
            version: VERSION,
            credentials,
            exported_at: origin.exported_at,
            source_data_dir: origin.source_data_dir,
            cores: snapshot.cores,
            proxies: snapshot
                .proxies
                .into_iter()
                .map(|mut proxy| {
                    proxy.outbound = credentials.apply(&proxy.outbound);
                    proxy
                })
                .collect(),
            profiles: snapshot.profiles,
        }
    }

    /// The document, as it would be written to a file.
    pub fn to_json(&self) -> Result<String, BackupError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// How many proxies in this document hold credentials.
    ///
    /// Zero for an [`Credentials::Excluded`] document by construction, which is
    /// exactly why an import cannot ask this question of the file and gets its
    /// answer from [`ConfigBackup::credentials`] instead.
    pub fn proxies_carrying_credentials(&self) -> usize {
        self.proxies
            .iter()
            .filter(|proxy| proxy.outbound.carries_credentials())
            .count()
    }

    /// Reads a document, refusing one this build cannot be sure it understands.
    pub fn from_json(text: &str) -> Result<Self, BackupError> {
        // The header is read on its own, first, so a file that is not one of
        // ours is refused as "not ours" rather than as the list of missing
        // fields it was never going to have. That message is the difference
        // between a user picking the wrong file and a user picking a damaged
        // one, and those call for different next steps.
        #[derive(Deserialize)]
        struct Header {
            format: String,
            version: u32,
        }

        let header: Header = serde_json::from_str(text)?;
        if header.format != FORMAT {
            return Err(BackupError::UnknownFormat {
                found: header.format,
            });
        }
        if header.version != VERSION {
            return Err(BackupError::UnknownVersion {
                found: header.version,
                supported: VERSION,
            });
        }

        Ok(serde_json::from_str(text)?)
    }
}

/// Why a document could not be read.
///
/// The three are kept apart because they call for different next steps: the
/// first two mean the wrong file, or a file from a build that knows a shape this
/// one does not; the third means the file itself is damaged.
#[derive(Debug, Error)]
pub enum BackupError {
    #[error("this is not a fingerprint-browser configuration backup: its format is {found:?}")]
    UnknownFormat { found: String },

    #[error(
        "configuration backup version {found} is not readable by this build, which reads version {supported}"
    )]
    UnknownVersion { found: u32, supported: u32 },

    #[error("configuration backup is not readable: {source}")]
    Malformed {
        #[from]
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        CoreId, FingerprintProfile, ProfileId, ProxyId, ShadowsocksOutbound, Socks5Outbound,
        StartTarget, WindowProfile,
    };
    use std::path::PathBuf;

    /// A secret that would be recognisable in a serialised document, so a test
    /// that fails shows the value rather than merely a mismatch.
    const SECRET: &str = "correct horse battery staple";

    fn core() -> BrowserCore {
        BrowserCore {
            id: CoreId::new(),
            name: "Fingerprint Chromium 148".to_string(),
            executable: PathBuf::from("/opt/chromium-148/chrome"),
            version: "148.0.7778.215".to_string(),
            major: 148,
        }
    }

    fn secret_proxy() -> ProxyProfile {
        ProxyProfile {
            id: ProxyId::new(),
            name: "Zurich exit".to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "203.0.113.10".to_string(),
                port: 1080,
                username: Some("alice".to_string()),
                password: Some(SECRET.to_string()),
            }),
        }
    }

    fn open_proxy() -> ProxyProfile {
        ProxyProfile {
            id: ProxyId::new(),
            name: "Office".to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "10.0.0.1".to_string(),
                port: 1080,
                username: None,
                password: None,
            }),
        }
    }

    fn profile(core: &BrowserCore, proxy: &ProxyProfile) -> BrowserProfile {
        BrowserProfile {
            id: ProfileId::new(),
            name: "Work laptop".to_string(),
            core_id: core.id,
            user_data_dir: PathBuf::from("/home/alice/.local/share/FpBrowser/profiles/one"),
            fingerprint: FingerprintProfile::new_random(4242),
            proxy_id: Some(proxy.id),
            window: WindowProfile::new(1440, 900),
            start_target: StartTarget::Blank,
        }
    }

    fn snapshot() -> (ConfigSnapshot, BrowserCore, ProxyProfile) {
        let core = core();
        let proxy = secret_proxy();
        let snapshot = ConfigSnapshot {
            cores: vec![core.clone()],
            proxies: vec![proxy.clone()],
            profiles: vec![profile(&core, &proxy)],
        };
        (snapshot, core, proxy)
    }

    fn origin() -> ExportOrigin {
        ExportOrigin {
            exported_at: "2026-09-20T16:04:00.000Z".to_string(),
            source_data_dir: "/home/alice/.local/share/FpBrowser".to_string(),
        }
    }

    fn exported(credentials: Credentials) -> String {
        let (snapshot, _, _) = snapshot();
        ConfigBackup::build(snapshot, credentials, origin())
            .to_json()
            .expect("serialise")
    }

    #[test]
    fn an_included_export_writes_the_secret_into_the_file() {
        // The choice is honest in both directions: this is what "included" means,
        // and it is why the export says which one it was.
        assert!(
            exported(Credentials::Included).contains(SECRET),
            "an included export should carry the password"
        );
    }

    #[test]
    fn an_excluded_export_leaves_the_secret_out_of_the_file() {
        let text = exported(Credentials::Excluded);
        assert!(
            !text.contains(SECRET),
            "an excluded export still carried the password: {text}"
        );
        // Stripped, not dropped: the proxy is still there to be re-pointed at,
        // which is what makes the file worth keeping.
        let document = ConfigBackup::from_json(&text).expect("round trip");
        assert_eq!(document.proxies.len(), 1);
        assert_eq!(document.proxies[0].outbound.host(), "203.0.113.10");
        assert_eq!(document.proxies[0].outbound.port(), 1080);
        assert!(!document.proxies[0].outbound.carries_credentials());
    }

    #[test]
    fn the_document_records_which_choice_was_made() {
        assert!(exported(Credentials::Included).contains("\"credentials\": \"included\""));
        assert!(exported(Credentials::Excluded).contains("\"credentials\": \"excluded\""));
    }

    #[test]
    fn a_round_trip_returns_the_same_document() {
        let (snapshot, _, _) = snapshot();
        let document = ConfigBackup::build(snapshot, Credentials::Included, origin());

        let text = document.to_json().expect("serialise");
        assert_eq!(ConfigBackup::from_json(&text).expect("parse"), document);
    }

    #[test]
    fn a_round_trip_through_a_file_keeps_paths_and_seeds() {
        // `user_data_dir` is a `PathBuf` and the seed is the number that
        // identifies a profile whose name has stopped meaning anything; both
        // have to survive the trip or an import cannot line a profile up with
        // its browser data.
        let (snapshot, _, _) = snapshot();
        let document = ConfigBackup::build(snapshot, Credentials::Included, origin());

        let text = document.to_json().expect("serialise");
        let read = ConfigBackup::from_json(&text).expect("parse");

        assert_eq!(
            read.profiles[0].user_data_dir,
            document.profiles[0].user_data_dir
        );
        assert_eq!(read.profiles[0].fingerprint.seed, 4242);
        assert_eq!(read.cores[0].executable, document.cores[0].executable);
        assert_eq!(read.cores[0].major, 148);
    }

    #[test]
    fn the_header_is_written_first() {
        // Not decoration: it is what lets a reader refuse before parsing the
        // rest, so it has to be at the front of the text rather than anywhere
        // `serde` happens to put it.
        let text = exported(Credentials::Included);
        assert!(
            text.starts_with("{\n  \"format\": \"fp-browser/config-backup\""),
            "the header should lead the file: {text}"
        );
    }

    #[test]
    fn a_document_that_is_not_ours_is_refused_by_name() {
        let text = r#"{"format": "some-other-tool/v3", "version": 1}"#;
        match ConfigBackup::from_json(text) {
            Err(BackupError::UnknownFormat { found }) => assert_eq!(found, "some-other-tool/v3"),
            other => panic!("expected an unknown format, got {other:?}"),
        }
    }

    #[test]
    fn a_document_from_another_shape_is_refused_rather_than_read() {
        // The version names the document's shape, so a build that does not know
        // it must not guess at the fields. Reading a v2 file as a v1 one would
        // quietly drop whatever v2 added.
        let text = format!(r#"{{"format": "{FORMAT}", "version": 2}}"#);
        match ConfigBackup::from_json(&text) {
            Err(BackupError::UnknownVersion { found, supported }) => {
                assert_eq!(found, 2);
                assert_eq!(supported, VERSION);
            }
            other => panic!("expected an unknown version, got {other:?}"),
        }
    }

    #[test]
    fn something_that_is_not_json_is_refused_as_unreadable() {
        match ConfigBackup::from_json("not a document at all") {
            Err(BackupError::Malformed { .. }) => {}
            other => panic!("expected a malformed document, got {other:?}"),
        }
    }

    #[test]
    fn a_file_without_a_header_is_refused_as_unreadable_rather_than_as_foreign() {
        // Both are refusals, but only one says "this is someone else's file"; a
        // JSON object with no `format` is more likely to be a damaged backup
        // than a different tool's, and the message should not claim otherwise.
        match ConfigBackup::from_json(r#"{"cores": [], "proxies": [], "profiles": []}"#) {
            Err(BackupError::Malformed { .. }) => {}
            other => panic!("expected a malformed document, got {other:?}"),
        }
    }

    #[test]
    fn a_field_this_build_does_not_know_is_refused_rather_than_dropped() {
        // The other direction of "the fields are the domain types": a file that
        // carries something this build would ignore is not read as if it were
        // not there.
        let text = format!(
            r#"{{"format": "{FORMAT}", "version": 1, "credentials": "included",
                "exported_at": "", "source_data_dir": "",
                "cores": [], "proxies": [], "profiles": [], "telemetry": true}}"#
        );
        match ConfigBackup::from_json(&text) {
            Err(BackupError::Malformed { .. }) => {}
            other => panic!("expected a malformed document, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_configuration_is_a_document_too() {
        // A person with no profiles yet can still take a backup, and the file
        // has to be readable rather than treated as an empty or damaged one.
        let document = ConfigBackup::build(
            ConfigSnapshot::default(),
            Credentials::Excluded,
            ExportOrigin::default(),
        );
        let text = document.to_json().expect("serialise");
        assert_eq!(ConfigBackup::from_json(&text).expect("parse"), document);
    }

    #[test]
    fn an_export_reports_how_many_proxies_carried_credentials() {
        let (mut snapshot, core, _) = snapshot();
        snapshot.proxies.push(open_proxy());
        snapshot.proxies.push(ProxyProfile {
            id: ProxyId::new(),
            name: "Relay".to_string(),
            outbound: ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                host: "203.0.113.11".to_string(),
                port: 8388,
                password: SECRET.to_string(),
                method: "aes-256-gcm".to_string(),
                stream: Default::default(),
            }),
        });
        snapshot.profiles.push(profile(&core, &snapshot.proxies[0]));

        // Two of three, and the count has to be taken before stripping: a built
        // `Excluded` document holds no credentials at all, which is the whole
        // reason an export needs a number to report.
        assert_eq!(snapshot.proxies_carrying_credentials(), 2);

        let document = ConfigBackup::build(snapshot, Credentials::Excluded, origin());
        assert_eq!(document.proxies_carrying_credentials(), 0);
        assert_eq!(document.proxies.len(), 3);
    }
}
