//! Read models and display decisions; no service writes.
use super::*;

/// One row of the profiles list with the display names already resolved.
#[derive(Clone)]
pub struct ProfileRow {
    pub profile: BrowserProfile,
    pub core_name: String,
    pub proxy_name: Option<String>,
    pub snapshot: Option<RuntimeSnapshot>,
    /// True while this profile's start is waiting for its proxy to answer.
    ///
    /// A start that has to check its proxy has asked for a browser and not been
    /// given one yet, and the runtime has heard nothing about it: the snapshot
    /// still says stopped. `Starting` is exactly what that is, and it is what
    /// keeps the row honest - the profile is on its way, and the Stop button
    /// beside it is what calls it off.
    pub checking_proxy: bool,
}

impl ProfileRow {
    /// Snapshot state, or `Stopped` when the profile was never started.
    pub fn state(&self) -> RuntimeState {
        if self.checking_proxy {
            return RuntimeState::Starting;
        }
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.state.clone())
            .unwrap_or(RuntimeState::Stopped)
    }

    pub fn state_label(&self, t: &Text) -> &'static str {
        match self.state() {
            RuntimeState::Stopped => t.state_stopped,
            RuntimeState::Starting => t.state_starting,
            RuntimeState::Running => t.state_running,
            RuntimeState::Stopping => t.state_stopping,
            RuntimeState::Failed { .. } => t.state_failed,
            RuntimeState::Crashed { .. } => t.state_crashed,
        }
    }

    /// A failure message carried by the state itself, if any.
    pub fn state_message(&self) -> Option<&str> {
        match self.snapshot.as_ref().map(|snapshot| &snapshot.state) {
            Some(RuntimeState::Failed { message } | RuntimeState::Crashed { message }) => {
                Some(message.as_str())
            }
            _ => None,
        }
    }

    pub fn can_start(&self) -> bool {
        matches!(
            self.state(),
            RuntimeState::Stopped | RuntimeState::Failed { .. } | RuntimeState::Crashed { .. }
        )
    }

    pub fn can_stop(&self) -> bool {
        self.state().is_active()
    }

    /// Restart is rejected by the supervisor only while another transition owns
    /// the profile, so mirror exactly that rule here.
    pub fn can_restart(&self) -> bool {
        !matches!(
            self.state(),
            RuntimeState::Starting | RuntimeState::Stopping
        )
    }

    pub fn last_error(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.last_error.as_deref())
    }

    pub fn last_warning(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.last_warning.as_deref())
    }

    pub fn browser_pid(&self) -> Option<u32> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.browser_pid)
    }

    pub fn xray_pid(&self) -> Option<u32> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.xray_pid)
    }

    pub fn cdp_port(&self) -> Option<u16> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.cdp_port)
    }

    pub fn socks_port(&self) -> Option<u16> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.socks_port)
    }

    pub fn effective_args(&self) -> &[String] {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.effective_args.as_slice())
            .unwrap_or(&[])
    }

    pub fn dropped_events(&self) -> u64 {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.dropped_events)
            .unwrap_or(0)
    }
}

/// Whether a profile answers to a filter term.
///
/// The term is matched, case-insensitively, against everything the row shows
/// plus the seed it carries. The seed is there because it is the one number
/// that identifies a profile whose name no longer means anything, and the core
/// and proxy are there because they are what someone scanning a list of
/// similar-looking profiles is actually looking for.
///
/// A profile that has no proxy contributes an empty string, so a term can
/// never match on the absence of one.
pub(super) fn answers_to(row: &ProfileRow, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    let fingerprint = &row.profile.fingerprint;
    let seed = fingerprint.seed.to_string();
    [
        row.profile.name.as_str(),
        seed.as_str(),
        fingerprint.brand.as_arg_value(),
        fingerprint.platform.as_arg_value(),
        row.core_name.as_str(),
        row.proxy_name.as_deref().unwrap_or_default(),
    ]
    .iter()
    .any(|field| field.to_lowercase().contains(&needle))
}

/// What a verification of one profile produced.
///
/// The terminal variants carry the whole report rather than only the findings,
/// because a pass has something to say as well: the address the traffic left
/// from, or why it could not be read. A finding is what the user must act on,
/// but an address is what tells them the path is the one they intended.
#[derive(Debug, Clone, PartialEq)]
pub enum Verification {
    /// A reading is in flight.
    Running,
    /// Every claim the profile makes was confirmed by the reading.
    Confirmed(VerificationReport),
    /// The reading disagreed with the profile.
    Disagreements(VerificationReport),
    /// No reading could be taken, so nothing was confirmed.
    Unreadable(String),
}

impl Verification {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Short label for the profile row and the details panel.
    pub fn label(&self, t: &Text) -> String {
        match self {
            Self::Running => t.verification_running.to_string(),
            Self::Confirmed(_) => t.verification_short(None),
            Self::Disagreements(report) => t.verification_short(Some(report.discrepancies.len())),
            Self::Unreadable(_) => t.fingerprint_unreadable_short.to_string(),
        }
    }

    /// The report, for the two outcomes that produced one.
    pub fn report(&self) -> Option<&VerificationReport> {
        match self {
            Self::Confirmed(report) | Self::Disagreements(report) => Some(report),
            _ => None,
        }
    }

    pub fn disagreements(&self) -> &[Discrepancy] {
        self.report()
            .map(|report| report.discrepancies.as_slice())
            .unwrap_or(&[])
    }

    pub fn failure(&self) -> Option<&str> {
        match self {
            Self::Unreadable(reason) => Some(reason),
            _ => None,
        }
    }
}

/// What a test of one proxy produced.
///
/// A success is the address the traffic left from, which is the whole point of
/// asking. The launch path only ever learns that the engine's loopback inbound
/// is open, and a port that accepts a connection but carries nothing looks
/// exactly like a working proxy - so this is the only thing that tells the two
/// apart, and the difference is whether the profile leaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyTest {
    /// A request is in flight.
    Running,
    /// Traffic left, and this is what the endpoint saw.
    Passed(ProxyReading),
    /// Nothing arrived, and this is where it stopped.
    Failed(Fault),
}

/// A proxy test that reached the endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyReading {
    pub exit_ip: String,
    pub elapsed: Duration,
    /// True when the engine was already up for a running profile, so this is
    /// the path that profile's traffic is taking right now rather than a
    /// rehearsal on a second engine.
    pub live: bool,
}

impl ProxyTest {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Short label for the proxy row.
    pub fn label(&self, t: &Text) -> String {
        match self {
            Self::Running => t.proxy_testing(),
            Self::Passed(reading) => t.proxy_exit(&reading.exit_ip),
            Self::Failed(fault) => t.proxy_no_traffic(&t.fault_class(fault.class)),
        }
    }

    /// How to read the result: which engine was probed, and how it went.
    pub fn detail(&self, t: &Text) -> Option<String> {
        match self {
            Self::Running => None,
            Self::Passed(reading) => Some(t.proxy_left_from(
                &reading.exit_ip,
                reading.elapsed.as_millis(),
                if reading.live {
                    t.engine_running_profile
                } else {
                    t.engine_temporary
                },
            )),
            Self::Failed(fault) => Some(t.proxy_no_traffic_detail(&fault.to_string())),
        }
    }

    /// The address this test established, when it established one.
    ///
    /// This is what a *proxy* was measured at, and it is the only expectation
    /// the product has for where a profile's traffic should leave from. Reading
    /// a running browser back is what says whether the traffic took that path.
    pub fn exit_ip(&self) -> Option<&str> {
        self.reading().map(|reading| reading.exit_ip.as_str())
    }

    /// The reading, when traffic left. The window matches on the variant
    /// instead, because it also wants the colour.
    pub(crate) fn reading(&self) -> Option<&ProxyReading> {
        match self {
            Self::Passed(reading) => Some(reading),
            _ => None,
        }
    }

    pub fn fault(&self) -> Option<&Fault> {
        match self {
            Self::Failed(fault) => Some(fault),
            _ => None,
        }
    }
}

/// Which page the sidebar has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Profiles,
    Proxies,
    Cores,
    Log,
    Settings,
}

impl Page {
    pub fn label(self, t: &Text) -> &'static str {
        match self {
            Self::Profiles => t.nav_profiles,
            Self::Proxies => t.nav_proxies,
            Self::Cores => t.nav_cores,
            Self::Log => t.nav_log,
            Self::Settings => t.nav_settings,
        }
    }

    /// The stable id the window and the tests use.
    ///
    /// Deliberately *not* the label: an element id that changes with the
    /// language would break every test that names it, and every user's muscle
    /// memory along with the automation built on top. These are the English
    /// names, frozen.
    pub fn id(self) -> &'static str {
        match self {
            Self::Profiles => "Profiles",
            Self::Proxies => "Proxies",
            Self::Cores => "Cores",
            Self::Log => "Log",
            Self::Settings => "Settings",
        }
    }

    /// Whether the page has been built yet.
    pub fn is_ready(self) -> bool {
        true
    }
}

/// One row of the proxies list, with who is using it.
#[derive(Clone)]
pub struct ProxyRow {
    pub proxy: ProxyProfile,
    pub used_by: Vec<String>,
}

impl ProxyRow {
    /// `name` plus what it dials, for the row's second line.
    pub fn endpoint(&self) -> String {
        self.proxy.endpoint()
    }

    pub fn is_used(&self) -> bool {
        !self.used_by.is_empty()
    }

    pub fn usage_label(&self, t: &Text) -> String {
        match self.used_by.len() {
            0 => t.proxy_not_assigned.to_string(),
            count => t.used_by(count, &self.used_by[0]),
        }
    }
}

/// One row of the browser-core list, with who is using it.
#[derive(Clone)]
pub struct CoreRow {
    pub core: BrowserCore,
    pub used_by: Vec<String>,
    /// False when the executable is no longer on disk.
    pub present: bool,
}

impl CoreRow {
    pub fn is_used(&self) -> bool {
        !self.used_by.is_empty()
    }

    pub fn usage_label(&self, t: &Text) -> String {
        match self.used_by.len() {
            0 => t.proxy_not_used.to_string(),
            count => t.used_by(count, &self.used_by[0]),
        }
    }

    /// Which switch generation this core belongs to, as the page shows it.
    ///
    /// `None` for a core whose version was never read: there is no generation,
    /// which is exactly what the launch path refuses on.
    pub fn generation_label(&self) -> Option<String> {
        generation_line(&self.core)
    }
}

/// The generation line a core is shown with, in one place.
///
/// `None` when the version was never read: "unknown" is not a generation, and
/// a core that has none cannot be launched with.
pub(super) fn generation_line(core: &BrowserCore) -> Option<String> {
    core.capabilities().map(|capabilities| {
        format!(
            "{} · {}",
            capabilities.generation_label(),
            capabilities.exclusion_label()
        )
    })
}

/// One browser core the profile form can put a profile on.
///
/// The name, the major and the generation are read from the core that was
/// detected rather than typed: which engine runs a profile decides which
/// switches the engine may be asked to spoof, so the form shows what the binary
/// answered instead of a version somebody remembered.
#[derive(Clone)]
pub struct CoreChoice {
    pub id: CoreId,
    pub name: String,
    /// The detected major, or `0` when the version was never read.
    pub major: u32,
    /// The same generation line the Browser Cores page shows.
    pub generation: Option<String>,
    /// Whether this engine honours `--disable-spoofing` (measured: not before
    /// major 144). The form says so rather than letting a checkbox be silently
    /// ignored at launch.
    pub exclusions_honoured: bool,
}

impl CoreChoice {
    /// The chip label: the core's own name and the major it answered with.
    ///
    /// A core still called what the binary and its major suggest (the name the
    /// Cores page generates) already carries the number, and repeating it would
    /// put "chrome 148 (Chrome 148)" in the window.
    pub fn label(&self, t: &Text) -> String {
        let major = self.major.to_string();
        if self.major == 0 {
            t.core_choice_unknown_version(&self.name)
        } else if self.name.contains(&major) {
            self.name.clone()
        } else {
            t.core_choice_major(&self.name, &major)
        }
    }
}
