//! View-facing application state.
//!
//! [`AppState`] is intentionally free of GPUI types so it can be unit tested
//! without a window. The view layer renders it and forwards user actions back
//! into it. Runtime state is never owned here: every read goes through
//! [`RuntimeService::snapshot`], which is the documented reconciliation path.

use crate::browser_data::BrowserDataJob;
use crate::exit::ExitMode;
use crate::log_file::LogFile;
use crate::maintenance;
use crate::proxy_tester::ProxyTestJob;
use crate::settings::{SettingKey, SettingRow, Settings};
use crate::text::{Lang, Text, text};
use crate::theme::ThemeChoice;
use crate::verifier::{EgressJob, VerificationJob, VerificationReport};
use application::{
    AppError, BrowserDataReport, CoreService, Counts, Credentials, Direction, ExportOrigin,
    ExportReport, Held, ImportNotes, ImportReport, NewProfile, NewProxy, Operation, Operations,
    ProfileService, ProxyService, RestoreMode, RestoreReport, RuntimeService,
};
use domain::{
    BrowserCore, BrowserProfile, CoreId, ProfileId, ProxyId, ProxyOutbound, ProxyProfile,
    RuntimeState,
};
use runtime::{Diagnosis, Discrepancy, Fault, RuntimeComponent, RuntimeEvent, RuntimeSnapshot};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use storage::ConfigurationRepository;

/// A user-facing message shown in the window banner.
///
/// The banner is for problems: it stays until it is dismissed. Anything that
/// succeeded is a [`Toast`], which is transient and does not need an
/// acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub error: bool,
    pub message: String,
}

impl Notice {
    fn error(message: impl Into<String>) -> Self {
        Self {
            error: true,
            message: message.into(),
        }
    }

    fn info(message: impl Into<String>) -> Self {
        Self {
            error: false,
            message: message.into(),
        }
    }
}

/// How a [`Toast`] should read at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Warning,
    Error,
}

/// A transient message the window shows and then forgets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub kind: ToastKind,
    pub message: String,
}

/// Severity of one line in the activity log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

impl LogLevel {
    pub fn label(self, t: &Text) -> &'static str {
        match self {
            Self::Info => t.log_level_info,
            Self::Warning => t.log_level_warning,
            Self::Error => t.log_level_error,
        }
    }
}

/// One line of the activity log, as it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub at: SystemTime,
    pub level: LogLevel,
    /// The profile the line is about, when it is about one.
    pub profile_id: Option<ProfileId>,
    pub message: String,
}

/// One line of the activity log with the profile name resolved, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRow {
    pub at: SystemTime,
    pub level: LogLevel,
    /// A profile name, or `app` for a window-level line.
    pub who: String,
    pub message: String,
}

/// How many log lines are kept. The window is a session tool, not an audit
/// system, and an unbounded vector would grow with every restart.
const LOG_CAPACITY: usize = 500;

/// How long a start may hold its profile before the window assumes it will never
/// be answered.
///
/// A start's lease is normally given back within a tick, when the runtime's
/// snapshot stops saying the profile is stopped. This is the valve for a runtime
/// that says nothing at all - generous, because a start that is merely slow has
/// already published its state by then, and holding a profile for longer than this
/// would refuse every copy of it for no reason.
const START_LEASE: Duration = Duration::from_secs(30);

/// How long a proxy check may hold its profile before the window assumes no
/// answer is coming.
///
/// Longer than [`START_LEASE`] on purpose: the check itself is bounded by the
/// engine's readiness deadline and the request's timeout, and a lease that
/// expired underneath a check that is still running would hand the profile out
/// while its proxy is still being asked. This is the outer bound for a worker
/// that never reported at all.
const CHECK_LEASE: Duration = Duration::from_secs(60);

/// Which of the activity log's lines the page is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFilter {
    #[default]
    All,
    /// Warnings and errors: the lines that are not routine.
    Warnings,
    Errors,
}

impl LogFilter {
    pub const ALL: [LogFilter; 3] = [Self::All, Self::Warnings, Self::Errors];

    pub fn label(self, t: &Text) -> &'static str {
        match self {
            Self::All => t.log_filter_all,
            Self::Warnings => t.log_filter_warnings,
            Self::Errors => t.log_filter_errors,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::All => "log-filter-all",
            Self::Warnings => "log-filter-warnings",
            Self::Errors => "log-filter-errors",
        }
    }

    /// What the filter keeps, for the empty line that says what is hidden.
    pub fn noun(self, t: &Text) -> &'static str {
        match self {
            Self::All => t.log_filter_noun_all,
            Self::Warnings => t.log_filter_noun_warnings,
            Self::Errors => t.log_filter_noun_errors,
        }
    }

    fn admits(self, level: LogLevel) -> bool {
        match self {
            Self::All => true,
            Self::Warnings => level != LogLevel::Info,
            Self::Errors => level == LogLevel::Error,
        }
    }
}

/// Which view of the selected profile the Runtime Details panel is showing.
///
/// The panel used to be one long scroll: identity, diagnostics, findings and the
/// whole launch line at once. The three views are the three questions asked of a
/// running profile - what is it, what was it started with, what has it done -
/// and only one of them is usually being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailsTab {
    #[default]
    Details,
    Args,
    Log,
}

impl DetailsTab {
    pub const ALL: [DetailsTab; 3] = [Self::Details, Self::Args, Self::Log];

    pub fn label(self, t: &Text) -> &'static str {
        match self {
            Self::Details => t.details_tab_details,
            Self::Args => t.details_tab_args,
            Self::Log => t.details_tab_log,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Details => "details-tab-details",
            Self::Args => "details-tab-args",
            Self::Log => "details-tab-log",
        }
    }
}

/// How many of a profile's log lines the panel's Log view shows.
const PANEL_LOG_LINES: usize = 20;

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
fn answers_to(row: &ProfileRow, needle: &str) -> bool {
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
fn generation_line(core: &BrowserCore) -> Option<String> {
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

pub struct AppState {
    profiles: Arc<dyn ProfileService>,
    runtime: Arc<RuntimeService>,
    cores: Arc<dyn CoreService>,
    proxies: Arc<dyn ProxyService>,
    /// The whole configuration as one thing, for the one operation that replaces
    /// it: a restore that went through the three services above would remove the
    /// old rows and write the new ones one commit at a time.
    configuration: Arc<dyn ConfigurationRepository>,
    settings: Settings,
    page: Page,
    rows: Vec<ProfileRow>,
    selected: Option<ProfileId>,
    /// What the Profiles page is narrowing its list by. Empty means no filter.
    profile_filter: String,
    notice: Option<Notice>,
    verifications: HashMap<ProfileId, Verification>,
    /// What the last test of each proxy found.
    proxy_tests: HashMap<ProxyId, ProxyTest>,
    /// Toasts the window has not shown yet.
    toasts: Vec<Toast>,
    /// What happened this session, oldest first.
    log: Vec<LogEntry>,
    /// Which of those lines the page is showing.
    log_filter: LogFilter,
    /// Which view the Runtime Details panel is showing.
    details_tab: DetailsTab,
    /// Where the lines are also written down, when a file could be opened.
    log_file: Option<LogFile>,
    /// Why there is no file, or the first write that failed.
    log_file_error: Option<String>,
    /// Where the next configuration export writes, as the user typed it.
    ///
    /// Empty means `wherever the default is`, so the field can start empty and
    /// the path is resolved when it is used. That is also what keeps the
    /// default's timestamp current instead of frozen at the moment the window
    /// opened.
    export_path: String,
    /// Whether the next export keeps the credentials it could leave out.
    export_includes_credentials: bool,
    /// Where the next configuration import reads from, as the user typed it.
    ///
    /// Empty means nowhere. An export has a sensible default because the program
    /// chooses where its own output goes; an import does not, because a default
    /// file to read from would be a file the user never named.
    import_path: String,
    /// Where the next configuration restore reads from, as the user typed it.
    ///
    /// Its own field rather than sharing the import's: the two are different
    /// verbs with different consequences, and a path left over from an import
    /// must not become a path a restore acts on by itself.
    restore_path: String,
    /// The backup directory for browser data, as the user typed it. Shared by
    /// both directions, because it is one place: a copy out writes there and a
    /// copy back in reads from it.
    browser_data_path: String,
    /// Which profiles are busy, and with what.
    ///
    /// The runtime's snapshot cannot answer that on its own: a start returns when
    /// its command is queued, and the state it produces is published a tick
    /// later. Everything that must not overlap with a start or a copy asks here.
    operations: Arc<Operations>,
    /// The starts that are waiting for a proxy to answer, by profile, and which
    /// opening each of them is: a start or a restart.
    ///
    /// The command is not queued until the answer is in, so this is what the
    /// answer is matched against - one entry per profile, because a profile is
    /// exactly as busy as the lease above says it is.
    pending_starts: BTreeMap<ProfileId, PendingStart>,
    /// The maintenance task a worker is running, if one is.
    ///
    /// One at a time, because the four of them read and write the same file and
    /// the same rows, and the window has one button each rather than a queue. It
    /// is cleared by whichever `finish_` the answer goes to - the one that a
    /// synchronous caller reaches in a row, and the one the view reaches through
    /// [`AppState::finish_maintenance`].
    maintenance: Option<maintenance::Kind>,
}

/// A start that is waiting for its proxy to answer.
#[derive(Debug, Clone, Copy)]
struct PendingStart {
    proxy: ProxyId,
    how: Opening,
    /// When the check was asked for.
    ///
    /// Every wait has to end somewhere: the answer resolves it, a proxy that is
    /// edited cancels it, and this is what calls it off if neither happens - a
    /// worker that died, or an answer that was dropped. Without it a start that
    /// nobody ever answers would hold its profile's lease for the rest of the
    /// session.
    asked: Instant,
}

/// Which way a profile is about to be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    Start,
    /// Stop it and start it again, in one command.
    Restart,
}

/// What opening a profile still needs before its command is queued.
#[derive(Debug, Clone)]
pub enum StartGate {
    /// The command is queued: nothing had to be asked first.
    Queued,
    /// A test of this profile's proxy is already in flight, so this opening waits
    /// for that answer rather than asking a second time.
    Awaiting,
    /// The proxy has to be asked first, and this is the job that asks it.
    ///
    /// Boxed because the job carries a whole proxy profile and the other two
    /// variants carry nothing: an enum that is 464 bytes for the two answers that
    /// need no data at all is a cost paid on every one of them.
    Checking(Box<ProxyTestJob>),
}

/// What the window's state writes through.
///
/// One value rather than five parameters, because every constructor here needs
/// all of them and a caller that passes them one by one is a caller that can pass
/// them in the wrong order - which, for five `Arc<dyn Trait>`s, is a mistake the
/// compiler cannot catch.
pub struct Services {
    pub profiles: Arc<dyn ProfileService>,
    pub runtime: Arc<RuntimeService>,
    pub cores: Arc<dyn CoreService>,
    pub proxies: Arc<dyn ProxyService>,
    /// The whole configuration as one thing, for the one operation that replaces
    /// it.
    pub configuration: Arc<dyn ConfigurationRepository>,
}

impl AppState {
    pub fn new(services: Services, settings: Settings) -> Self {
        let (log_file, log_file_error) =
            match LogFile::open(&settings.data_dir().join(crate::log_file::LOG_DIR)) {
                Ok(file) => (Some(file), None),
                Err(error) => (None, Some(error)),
            };
        Self::assemble(services, settings, log_file, log_file_error)
    }

    /// The same state with a specific activity log, or none at all.
    ///
    /// A test must not write into the data directory of the machine it runs on;
    /// a test that drives the sink's failure path builds its own [`LogFile`].
    #[cfg(test)]
    pub(crate) fn with_log(
        services: Services,
        settings: Settings,
        log_file: Option<LogFile>,
        log_file_error: Option<String>,
    ) -> Self {
        Self::assemble(services, settings, log_file, log_file_error)
    }

    /// The window's state with no activity log on disk.
    #[cfg(test)]
    pub(crate) fn for_test(services: Services, settings: Settings) -> Self {
        Self::with_log(services, settings, None, None)
    }

    fn assemble(
        services: Services,
        settings: Settings,
        log_file: Option<LogFile>,
        log_file_error: Option<String>,
    ) -> Self {
        let Services {
            profiles,
            runtime,
            cores,
            proxies,
            configuration,
        } = services;
        Self {
            profiles,
            runtime,
            cores,
            proxies,
            configuration,
            settings,
            page: Page::Profiles,
            rows: Vec::new(),
            selected: None,
            profile_filter: String::new(),
            notice: None,
            verifications: HashMap::new(),
            proxy_tests: HashMap::new(),
            toasts: Vec::new(),
            log: Vec::new(),
            log_filter: LogFilter::default(),
            details_tab: DetailsTab::default(),
            log_file,
            log_file_error,
            export_path: String::new(),
            // Left out by default. A file that carries plain-text passwords is
            // the one to reach for deliberately, not the one to get by not
            // looking at a checkbox.
            export_includes_credentials: false,
            import_path: String::new(),
            restore_path: String::new(),
            browser_data_path: String::new(),
            operations: Arc::new(Operations::default()),
            pending_starts: BTreeMap::new(),
            maintenance: None,
        }
    }

    /// Reload profiles from storage and reconcile every runtime snapshot.
    ///
    /// Failures are recorded in the banner as well as returned, because every
    /// caller in the UI wants them shown.
    pub fn load(&mut self) -> Result<(), AppError> {
        let result = self.load_rows();
        if let Err(error) = &result {
            self.set_notice(Notice::error(error.to_string()));
        }
        result
    }

    fn load_rows(&mut self) -> Result<(), AppError> {
        let t = self.text();
        let runtime = Arc::clone(&self.runtime);
        let cores: HashMap<CoreId, String> = self
            .cores
            .list()?
            .into_iter()
            .map(|core| (core.id, core.name))
            .collect();
        let proxies = self.proxy_names()?;

        let mut profiles = self.profiles.list()?;
        profiles.sort_by_key(|profile| profile.name.to_lowercase());

        self.rows = profiles
            .into_iter()
            .map(|profile| {
                let core_name = cores
                    .get(&profile.core_id)
                    .cloned()
                    .unwrap_or_else(|| t.core_missing_marker.to_string());
                let proxy_name = profile.proxy_id.and_then(|id| proxies.get(&id).cloned());
                let snapshot = runtime.snapshot(profile.id);
                ProfileRow {
                    checking_proxy: self.pending_starts.contains_key(&profile.id),
                    profile,
                    core_name,
                    proxy_name,
                    snapshot,
                }
            })
            .collect();

        if let Some(selected) = self.selected
            && !self.rows.iter().any(|row| row.profile.id == selected)
        {
            self.selected = None;
        }

        Ok(())
    }

    /// Reconcile snapshots only; used by the periodic tick and after commands.
    pub fn refresh_runtime(&mut self) {
        let runtime = Arc::clone(&self.runtime);
        for row in &mut self.rows {
            row.checking_proxy = self.pending_starts.contains_key(&row.profile.id);
            row.snapshot = runtime.snapshot(row.profile.id);
            // A start lease is held only until the runtime has said something
            // about it. The snapshot is what says it: once the profile is no
            // longer stopped, the state itself refuses a copy - a profile that is
            // starting is active - so the lease has done its job.
            //
            // Nothing is released for a profile the snapshot still says is
            // stopped, which is the window this lease exists for, and
            // `Operations::expire` is what stops a start that is never answered
            // from holding its profile for as long as the window is open.
            if row
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.state != RuntimeState::Stopped)
            {
                self.operations.free(row.profile.id, Operation::Starting);
            }
        }

        // A check nobody ever answered is called off here rather than left to
        // hold its profile: the worker may have died, or the answer may have been
        // dropped with the proxy it was for.
        let unanswered: Vec<ProfileId> = self
            .pending_starts
            .iter()
            .filter(|(_, pending)| pending.asked.elapsed() >= CHECK_LEASE)
            .map(|(profile, _)| *profile)
            .collect();
        for profile in unanswered {
            let reason = self.text().proxy_check_timed_out();
            self.cancel_pending_start(profile, reason);
        }

        for profile in self.operations.expire(START_LEASE) {
            if self.pending_starts.contains_key(&profile) {
                // A proxy is still being asked about this profile, and the lease
                // is what keeps it to itself while that happens. Taking it again
                // restarts the clock rather than handing the profile out from
                // under a check that is still running - and `CHECK_LEASE` above is
                // what stops that from being forever.
                let _ = self.operations.take(profile, Operation::Starting);
                continue;
            }
            tracing::warn!(
                "no answer for the start of {} within {}s; the profile is free again",
                self.profile_name(profile),
                START_LEASE.as_secs()
            );
        }
    }

    /// The row of one profile, for the places that ask about it by identifier.
    pub fn row(&self, id: ProfileId) -> Option<&ProfileRow> {
        self.rows.iter().find(|row| row.profile.id == id)
    }

    pub fn rows(&self) -> &[ProfileRow] {
        &self.rows
    }

    pub fn selected_id(&self) -> Option<ProfileId> {
        self.selected
    }

    pub fn selected(&self) -> Option<&ProfileRow> {
        let selected = self.selected?;
        self.rows.iter().find(|row| row.profile.id == selected)
    }

    pub fn select(&mut self, id: ProfileId) {
        self.selected = Some(id);
    }

    /// What the Profiles page is narrowing its list by.
    pub fn profile_filter(&self) -> &str {
        &self.profile_filter
    }

    pub fn set_profile_filter(&mut self, filter: impl Into<String>) {
        self.profile_filter = filter.into();
    }

    /// The rows the Profiles page lists.
    ///
    /// The filter narrows the list and nothing else. The focused profile stays
    /// focused when it stops matching, because the details panel answers for
    /// what was chosen rather than for what is currently on screen; and a
    /// profile that is running keeps running. Both are why filtering can never
    /// be the thing that changes a profile's state.
    ///
    /// Cloned rather than borrowed: the view builds its tree from owned
    /// values, which is what the render path did before there was a filter.
    pub fn visible_rows(&self) -> Vec<ProfileRow> {
        let needle = self.profile_filter.trim();
        if needle.is_empty() {
            return self.rows.clone();
        }
        self.rows
            .iter()
            .filter(|row| answers_to(row, needle))
            .cloned()
            .collect()
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// Surface a bootstrap-level message (core discovery, version detection).
    pub fn push_notice(&mut self, message: impl Into<String>, error: bool) {
        self.set_notice(if error {
            Notice::error(message)
        } else {
            Notice::info(message)
        });
    }

    /// Put a line in the activity log without showing it.
    ///
    /// The log's own first line goes in this way: what build this run is and
    /// where it keeps its files belongs in the file that outlives the window,
    /// and it is not news the user has to acknowledge with a toast - which is
    /// the whole difference between this and [`AppState::push_notice`].
    pub fn note_startup(&mut self, message: impl Into<String>) {
        self.append_log(LogLevel::Info, None, message);
    }

    pub fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    /// Show a message and remember it.
    ///
    /// A problem owns the banner until it is dismissed or a later action
    /// succeeds; a success clears the banner and is only a toast. Either way
    /// the line is kept in the [`AppState::log_rows`] history, which is what
    /// survives for as long as the window is open.
    fn set_notice(&mut self, notice: Notice) {
        let level = if notice.error {
            LogLevel::Error
        } else {
            LogLevel::Info
        };
        let kind = if notice.error {
            ToastKind::Error
        } else {
            ToastKind::Success
        };
        self.append_log(level, None, notice.message.clone());
        self.toast(kind, notice.message.clone());
        self.notice = notice.error.then_some(notice);
    }

    /// Queue a toast the window has not shown yet.
    fn toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.toasts.push(Toast {
            kind,
            message: message.into(),
        });
    }

    /// Toasts the window has not shown yet, oldest first.
    pub fn drain_toasts(&mut self) -> Vec<Toast> {
        std::mem::take(&mut self.toasts)
    }

    #[cfg(test)]
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// The activity log, oldest first.
    #[cfg(test)]
    pub fn log_entries(&self) -> &[LogEntry] {
        &self.log
    }

    /// The activity log with profile names resolved and the newest line first,
    /// which is the order the Log page reads in. The page's filter is applied.
    pub fn log_rows(&self) -> Vec<LogRow> {
        self.log
            .iter()
            .rev()
            .filter(|entry| self.log_filter.admits(entry.level))
            .map(|entry| LogRow {
                at: entry.at,
                level: entry.level,
                who: self.who(entry.profile_id),
                message: entry.message.clone(),
            })
            .collect()
    }

    /// How many lines were recorded, before the page's filter.
    pub fn log_len(&self) -> usize {
        self.log.len()
    }

    pub fn log_filter(&self) -> LogFilter {
        self.log_filter
    }

    pub fn set_log_filter(&mut self, filter: LogFilter) {
        self.log_filter = filter;
    }

    pub fn details_tab(&self) -> DetailsTab {
        self.details_tab
    }

    pub fn set_details_tab(&mut self, tab: DetailsTab) {
        self.details_tab = tab;
    }

    /// The newest log lines for one profile, for the panel's Log view.
    ///
    /// Only this profile's own lines: the Log page is where everything is, and
    /// the panel is where one profile is being looked at.
    pub fn log_tail(&self, profile_id: ProfileId) -> Vec<LogRow> {
        self.log
            .iter()
            .rev()
            .filter(|entry| entry.profile_id == Some(profile_id))
            .take(PANEL_LOG_LINES)
            .map(|entry| LogRow {
                at: entry.at,
                level: entry.level,
                who: self.who(entry.profile_id),
                message: entry.message.clone(),
            })
            .collect()
    }

    /// Where the log is also written down: the path, or the reason it is not.
    ///
    /// A write that failed outranks the path: the file is there, but it is no
    /// longer being appended to.
    pub fn log_file_status(&self) -> Result<&std::path::Path, &str> {
        let t = self.text();
        if let Some(error) = &self.log_file_error {
            return Err(error.as_str());
        }
        match &self.log_file {
            Some(file) => Ok(file.path()),
            None => Err(t.no_activity_log),
        }
    }

    /// The name a line's profile is shown under.
    fn who(&self, profile_id: Option<ProfileId>) -> String {
        let t = self.text();
        let Some(id) = profile_id else {
            return "app".to_string();
        };
        self.rows
            .iter()
            .find(|row| row.profile.id == id)
            .map(|row| row.profile.name.clone())
            .unwrap_or_else(|| t.a_removed_profile.to_string())
    }

    pub fn clear_log(&mut self) {
        self.log.clear();
    }

    /// Record one runtime notification.
    ///
    /// Events are best-effort and never replayed as state, but they are the
    /// only place a crash, a warning or the pids a run got are ever written
    /// down; the snapshot keeps only the latest of each.
    pub fn record_event(&mut self, event: &RuntimeEvent) {
        let t = self.text();
        let entry = match event {
            // Starting, stopping and a refused start are only visible as a
            // state change; running and stopped have their own event, and are
            // not repeated here.
            RuntimeEvent::StateChanged { profile_id, state } => match state {
                RuntimeState::Starting => (LogLevel::Info, *profile_id, "starting".to_string()),
                RuntimeState::Stopping => (LogLevel::Info, *profile_id, "stopping".to_string()),
                RuntimeState::Failed { message } => {
                    (LogLevel::Error, *profile_id, t.log_failed(message))
                }
                RuntimeState::Running | RuntimeState::Stopped | RuntimeState::Crashed { .. } => {
                    return;
                }
            },
            RuntimeEvent::EffectiveLaunchArgs { profile_id, args } => {
                (LogLevel::Info, *profile_id, t.log_launching(args.len()))
            }
            RuntimeEvent::Started {
                profile_id,
                browser_pid,
                xray_pid,
                cdp_port,
                socks_port,
            } => (
                LogLevel::Info,
                *profile_id,
                t.log_browser_started(*browser_pid, *cdp_port, *socks_port, *xray_pid),
            ),
            RuntimeEvent::Stopped { profile_id } => (
                LogLevel::Info,
                *profile_id,
                t.log_browser_stopped.to_string(),
            ),
            RuntimeEvent::Crashed {
                profile_id,
                component,
                message,
            } => (
                LogLevel::Error,
                *profile_id,
                t.log_crashed(component_label(*component), message),
            ),
            RuntimeEvent::Warning {
                profile_id,
                message,
            } => {
                // A warning is exactly the kind of thing the row shows but the
                // user does not necessarily go looking for.
                self.toast(ToastKind::Warning, message.clone());
                (LogLevel::Warning, *profile_id, message.clone())
            }
        };
        self.append_log(entry.0, Some(entry.1), entry.2);
    }

    fn append_log(
        &mut self,
        level: LogLevel,
        profile_id: Option<ProfileId>,
        message: impl Into<String>,
    ) {
        self.log.push(LogEntry {
            at: SystemTime::now(),
            level,
            profile_id,
            message: message.into(),
        });
        if self.log.len() > LOG_CAPACITY {
            let excess = self.log.len() - LOG_CAPACITY;
            self.log.drain(..excess);
        }

        // The file gets the same line, including the profile's name rather than
        // its id: a log read a week later has no profile list next to it.
        let Some(entry) = self.log.last() else {
            return;
        };
        let t = self.text();
        let line = format!(
            "{} {:<7} {}: {}\n",
            crate::log_file::timestamp(entry.at),
            entry.level.label(t),
            self.who(entry.profile_id),
            entry.message
        );
        let Some(file) = self.log_file.as_mut() else {
            return;
        };
        if let Err(error) = file.append(&line) {
            self.report_log_failure(error);
        }
    }

    /// Says so once when the log cannot be written, and never recurses: the
    /// report is a toast, and a toast does not go through the log.
    fn report_log_failure(&mut self, error: String) {
        let t = self.text();
        if self.log_file_error.is_none() {
            self.log_file_error = Some(error.clone());
            self.toast(ToastKind::Error, t.log_write_failed(&error));
        }
    }

    /// Placeholder naming until the Profile Editor page exists.
    pub fn next_profile_name(&self) -> String {
        let t = self.text();
        t.next_profile_name(self.rows.len() + 1)
    }

    /// The page the sidebar is showing.
    pub fn page(&self) -> Page {
        self.page
    }

    pub fn set_page(&mut self, page: Page) {
        self.page = page;
        // The usage lists on the proxy and core pages are derived from the
        // profiles, so the rows are re-read when either page is opened.
        if matches!(page, Page::Proxies | Page::Cores) {
            let _ = self.load_rows();
        }
    }

    fn proxy_names(&self) -> Result<HashMap<ProxyId, String>, AppError> {
        Ok(self
            .proxies
            .list()?
            .into_iter()
            .map(|proxy| (proxy.id, proxy.name))
            .collect())
    }

    /// Every proxy with the profiles assigned to it, for the Proxies page.
    pub fn proxy_rows(&self) -> Result<Vec<ProxyRow>, AppError> {
        let usage = self.proxies.usage()?;
        Ok(self
            .proxies
            .list()?
            .into_iter()
            .map(|proxy| ProxyRow {
                used_by: usage.get(&proxy.id).cloned().unwrap_or_default(),
                proxy,
            })
            .collect())
    }

    /// `(id, name)` for every proxy, for the profile editor's picker.
    pub fn proxy_choices(&self) -> Vec<(ProxyId, String)> {
        self.proxies
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|proxy| (proxy.id, proxy.name))
            .collect()
    }

    pub fn proxy(&self, id: ProxyId) -> Option<ProxyProfile> {
        self.proxies.get(id).ok().flatten()
    }

    pub fn create_proxy(
        &mut self,
        name: &str,
        outbound: ProxyOutbound,
    ) -> Result<ProxyId, AppError> {
        let t = self.text();
        let created = self.record(self.proxies.create(NewProxy {
            name: name.trim().to_string(),
            outbound,
        }))?;
        let id = created.id;
        self.set_notice(Notice::info(t.proxy_created(&created.name)));
        Ok(id)
    }

    pub fn update_proxy(&mut self, proxy: ProxyProfile) -> Result<(), AppError> {
        let t = self.text();
        self.record(self.proxies.update(proxy.clone()))?;
        self.set_notice(Notice::info(t.proxy_saved(&proxy.name)));
        // The old result was about the old upstream, and would read as a claim
        // about the new one. Last, so that a start this edit called off is what
        // the banner says: a profile left waiting would otherwise read as one
        // that was never asked to start.
        self.forget_proxy_test(proxy.id);
        Ok(())
    }

    /// Removes a proxy. Refused while a profile still points at it, because the
    /// alternative is that profile quietly going direct.
    pub fn delete_proxy(&mut self, id: ProxyId) -> Result<(), AppError> {
        let t = self.text();
        let name = self
            .proxy(id)
            .map(|proxy| proxy.name)
            .unwrap_or_else(|| t.a_removed_profile.to_string());
        self.record(self.proxies.delete(id))?;
        self.set_notice(Notice::info(t.proxy_deleted(&name)));
        // Nothing points at this id any more, so a result kept for it could
        // only ever be shown against a different proxy.
        self.forget_proxy_test(id);
        Ok(())
    }

    /// The table for the language in force.
    pub fn text(&self) -> &'static Text {
        text(self.settings.language())
    }

    /// Every setting with the value in force and where it came from.
    pub fn setting_rows(&self) -> Vec<SettingRow> {
        self.settings.rows(self.text())
    }

    /// The language the window is shown in.
    pub fn language(&self) -> Lang {
        self.settings.language()
    }

    /// Stores the chosen language. The caller repaints afterwards, because the
    /// whole window's text changes; nothing else here is reloaded, since the
    /// language is presentation and no row's value depends on it.
    pub fn set_language(&mut self, lang: Lang) -> Result<(), AppError> {
        let result = self.settings.set_language(lang).map_err(AppError::Conflict);
        match &result {
            Ok(()) => {
                // The sentence is written in the language being switched *to*,
                // which is the one the reader is about to be reading.
                let t = text(lang);
                self.set_notice(Notice::info(t.language_chosen(lang.label())));
            }
            Err(error) => self.set_notice(Notice::error(error.to_string())),
        }
        result
    }

    /// Stores an editable setting for the next start.
    ///
    /// The value is not always live: the ones that decide what a process does
    /// are read when it starts, and the window says so on the row. The one that
    /// is live says that instead, because a saved value that looks like it
    /// applies and does not is the trap this page exists to avoid.
    pub fn update_setting(&mut self, key: SettingKey, value: &str) -> Result<(), AppError> {
        let result = self.settings.set(key, value).map_err(AppError::Conflict);
        let t = self.text();
        match &result {
            Ok(()) => self.set_notice(Notice::info(t.setting_saved(key.label(t), key.effect()))),
            Err(error) => {
                self.set_notice(Notice::error(error.to_string()));
            }
        }
        result
    }

    /// Where an export writes with no path typed.
    ///
    /// Recomputed on every call rather than stored, so a window left open
    /// overnight still proposes the current second - and so two exports in a row
    /// do not both aim at the same file.
    pub fn export_default_path(&self) -> PathBuf {
        crate::paths::default_export_file(self.settings.data_dir(), SystemTime::now())
    }

    /// The appearance the window is shown in.
    pub fn theme(&self) -> ThemeChoice {
        self.settings.theme()
    }

    /// Ends this run with every running session left running.
    ///
    /// The browsers and the tunnels stay, and the runtime marks their records so
    /// the next start adopts them. Nothing to report back: the program is
    /// leaving, and the banner would go with it.
    pub fn release_runtime(&self) {
        if let Err(error) = self.runtime.release_all() {
            tracing::warn!("could not leave the running sessions running: {error}");
        }
    }

    /// Whether any profile is in a state the exit choice is about.
    ///
    /// Read when the dialog opens, so the sentence above the three choices can
    /// say whether there is anything to decide about - the choices are the same
    /// either way, but a sentence claiming browsers are running when none are
    /// would be the window guessing.
    pub fn any_profile_active(&self) -> bool {
        self.rows().iter().any(|row| row.state().is_active())
    }

    /// What closing the window does: a stored answer, or the question.
    ///
    /// Read when the window is closed, which is why a caller must read it again at
    /// that moment rather than hold onto a value from when the page was drawn.
    pub fn exit_mode(&self) -> ExitMode {
        self.settings.exit_mode()
    }

    /// Stores what closing the window does.
    ///
    /// Like the appearance and the language, the file is written before the choice
    /// is accepted: a config file that cannot be written refuses the change rather
    /// than accepting one that would be gone by the next start. Unlike them it
    /// changes nothing on screen, so a caller that repaints does so only to move
    /// the chip.
    pub fn set_exit_mode(&mut self, mode: ExitMode) -> Result<(), AppError> {
        let result = self
            .settings
            .set_exit_mode(mode)
            .map_err(AppError::Conflict);
        match &result {
            Ok(()) => self.set_notice(Notice::info(
                self.text().exit_mode_chosen(mode.label(self.text())),
            )),
            Err(error) => self.set_notice(Notice::error(error.to_string())),
        }
        result
    }

    /// Stores the chosen appearance.
    ///
    /// The one setting whose effect is immediate, so the caller repaints rather
    /// than waiting for a restart. It is still a setting, so a config file that
    /// cannot be written refuses the change instead of accepting a choice that
    /// would be gone by the next start - and the caller must therefore switch
    /// only after this returns `Ok`, not before.
    pub fn set_theme(&mut self, choice: ThemeChoice) -> Result<(), AppError> {
        let result = self.settings.set_theme(choice).map_err(AppError::Conflict);
        match &result {
            Ok(()) => self.set_notice(Notice::info(
                self.text()
                    .theme_chosen(&choice.label(self.text()).to_lowercase()),
            )),
            Err(error) => self.set_notice(Notice::error(error.to_string())),
        }
        result
    }

    /// The export path field's contents. Empty means the default.
    ///
    /// Only the tests read this back. The window keeps the text in the field
    /// itself and hands it over when the button is pressed, so nothing in the
    /// program asks what was typed without also being the thing that typed it -
    /// and an ungated method would be dead code in the binary build, which
    /// `clippy --all-targets -D warnings` refuses.
    #[cfg(test)]
    pub fn export_path(&self) -> &str {
        &self.export_path
    }

    pub fn set_export_path(&mut self, path: impl Into<String>) {
        self.export_path = path.into();
    }

    /// Where an export would write now, typed or defaulted.
    pub fn export_destination(&self) -> PathBuf {
        let typed = self.export_path.trim();
        if typed.is_empty() {
            self.export_default_path()
        } else {
            PathBuf::from(typed)
        }
    }

    /// Whether the next export keeps the proxy credentials it could leave out.
    pub fn export_includes_credentials(&self) -> bool {
        self.export_includes_credentials
    }

    pub fn set_export_includes_credentials(&mut self, include: bool) {
        self.export_includes_credentials = include;
    }

    /// Writes a configuration backup and says what it did.
    ///
    /// Reading the three lists and writing one small file are both local and
    /// quick, so this runs on the calling thread - the same reason
    /// [`AppState::start`] does, and unlike a proxy test, which waits on a far
    /// end.
    ///
    /// Nothing here checks whether the destination is already taken. The default
    /// names a new file each second, so it cannot land on an older backup by
    /// accident; a path typed on purpose is meant, and refusing it would break
    /// updating a backup kept at one path.
    /// The export, on the calling thread.
    ///
    /// For tests, and only for tests: the window uses [`AppState::begin_export`]
    /// and applies the answer through [`AppState::finish_maintenance`], so the
    /// read and the write happen on a worker rather than on the thread that draws
    /// the window. This is the same three steps in a row, which is what makes the
    /// whole task testable without a window.
    #[cfg(test)]
    pub(crate) fn export_configuration(&mut self) -> Result<ExportReport, String> {
        let job = self.begin_export()?;
        let destination = job.destination.clone();
        let result = maintenance::run_export(job);
        self.finish_export(&destination, &result);
        result
    }

    /// The export as a value, for a caller that will run it somewhere else.
    ///
    /// Refused while another maintenance task is in flight: these read and write
    /// the same file and the same rows, and the window has one button rather than
    /// two. The marker is cleared by whichever `finish_` the answer goes to.
    pub fn begin_export(&mut self) -> Result<maintenance::ExportJob, String> {
        self.begin_maintenance(maintenance::Kind::Export)?;
        let credentials = if self.export_includes_credentials {
            Credentials::Included
        } else {
            Credentials::Excluded
        };
        Ok(maintenance::ExportJob {
            text: self.text(),
            profiles: Arc::clone(&self.profiles),
            cores: Arc::clone(&self.cores),
            proxies: Arc::clone(&self.proxies),
            credentials,
            origin: ExportOrigin {
                exported_at: crate::log_file::timestamp(SystemTime::now()),
                source_data_dir: self.settings.data_dir().display().to_string(),
            },
            destination: self.export_destination(),
        })
    }

    /// What an export did, said to the reader, and the marker let go.
    pub fn finish_export(
        &mut self,
        destination: &std::path::Path,
        result: &Result<ExportReport, String>,
    ) {
        self.maintenance = None;
        let t = self.text();
        match result {
            Ok(report) => self.set_notice(Notice::info(export_summary(report, t))),
            // The report names the file it wrote; a failure has no report, so the
            // sentence is the only place the destination appears.
            Err(message) => {
                let _ = destination;
                self.set_notice(Notice::error(message.clone()));
            }
        }
    }

    /// Records that a maintenance task has started, refusing a second one.
    fn begin_maintenance(&mut self, kind: maintenance::Kind) -> Result<(), String> {
        if let Some(running) = self.maintenance {
            let message = self.text().maintenance_busy();
            self.set_notice(Notice::error(message.clone()));
            tracing::debug!("refused a {kind:?} while a {running:?} is running");
            return Err(message);
        }
        self.maintenance = Some(kind);
        Ok(())
    }

    /// Applies whatever a maintenance task reported.
    ///
    /// The one door the view's worker answers come through, so a new maintenance
    /// task cannot report its result anywhere the window does not hear it. Each
    /// `finish_` clears the marker, which is what lets the next task run: a task
    /// that never let go would refuse every task after it.
    pub fn finish_maintenance(&mut self, outcome: maintenance::Outcome) {
        match outcome {
            maintenance::Outcome::Exported {
                destination,
                result,
            } => self.finish_export(&destination, &result),
            maintenance::Outcome::Imported { source, result } => {
                self.finish_import(&source, &result)
            }
            maintenance::Outcome::Restored { source, result } => {
                self.finish_restore(&source, &result)
            }
            maintenance::Outcome::Redetected { id, result } => self.finish_redetect(id, &result),
        }
    }

    pub fn set_import_path(&mut self, path: impl Into<String>) {
        self.import_path = path.into();
    }

    /// Where a diagnostics report would write now.
    ///
    /// Recomputed like [`AppState::export_default_path`] and for the same
    /// reason: a window left open overnight still proposes the current second,
    /// and two reports in a row do not aim at the same file.
    pub fn diagnostics_destination(&self) -> PathBuf {
        crate::paths::default_diagnostics_file(self.settings.data_dir(), SystemTime::now())
    }

    /// Writes a report about this installation and says where it went.
    ///
    /// What it collects is in [`crate::diagnostics`]: the build, the settings in
    /// force with the source each came from, the state of every file this
    /// installation keeps, and the end of the activity log. It reads files and
    /// writes one, both locally and quickly, so it runs on the calling thread.
    ///
    /// The database is deliberately not among the files it reads: the proxy
    /// credentials are in it, and a report is meant to be sent to someone.
    pub fn write_diagnostics(&mut self) -> Result<PathBuf, String> {
        let t = self.text();
        let destination = self.diagnostics_destination();
        let result = crate::diagnostics::write_for(&self.settings, &destination, t);
        match &result {
            Ok(()) => self.set_notice(Notice::info(
                t.diag_written(&destination.display().to_string()),
            )),
            Err(message) => self.set_notice(Notice::error(message.clone())),
        }
        result.map(|()| destination)
    }

    /// Where an import would read from, or `None` when nothing was typed.
    pub fn import_source(&self) -> Option<PathBuf> {
        let typed = self.import_path.trim();
        (!typed.is_empty()).then(|| PathBuf::from(typed))
    }

    /// Reads a configuration backup into this installation and says what it did.
    ///
    /// Nothing is confirmed first, and that is the point of import being a
    /// separate verb from restore. Import never overwrites: the worst a mistaken
    /// one can do is add records the user did not want, which the sentence below
    /// says and which is undone one row at a time. Restore replaces, so restore
    /// is the one that will ask.
    ///
    /// Runs on the calling thread for the same reason an export does: one small
    /// file and a handful of rows are local and quick, and nothing here waits on
    /// a far end.
    ///
    /// The rows are reloaded afterwards, because a Profiles page still showing
    /// the list from before the import would contradict the sentence above it.
    /// The import, on the calling thread. See
    /// [`AppState::export_configuration`] for why this exists.
    #[cfg(test)]
    pub(crate) fn import_configuration(&mut self) -> Result<ImportReport, String> {
        let job = self.begin_import()?;
        let source = job.source.clone();
        let result = maintenance::run_import(job);
        self.finish_import(&source, &result);
        result
    }

    /// The import as a value, for a caller that will run it somewhere else.
    ///
    /// Every refusal that can be made before a thread is spawned is made here: no
    /// path typed, another maintenance task in flight.
    pub fn begin_import(&mut self) -> Result<maintenance::ImportJob, String> {
        let Some(source) = self.import_source() else {
            let message = "Type the path of a configuration backup to import.".to_string();
            self.set_notice(Notice::error(message.clone()));
            return Err(message);
        };
        self.begin_maintenance(maintenance::Kind::Import)?;
        Ok(maintenance::ImportJob {
            text: self.text(),
            source,
            data_dir: self.settings.data_dir().to_path_buf(),
            profiles: Arc::clone(&self.profiles),
            cores: Arc::clone(&self.cores),
            proxies: Arc::clone(&self.proxies),
        })
    }

    /// What an import did, said to the reader, with the rows reloaded.
    pub fn finish_import(
        &mut self,
        source: &std::path::Path,
        result: &Result<ImportReport, String>,
    ) {
        self.maintenance = None;
        let t = self.text();
        match result {
            Ok(report) => {
                let summary = import_summary(report, source, t);
                // An import that did less than the file asked for gets the
                // banner, which stays until it is dismissed; one that did
                // exactly what it asked gets a toast. That is what makes the
                // banner mean something when it appears.
                self.set_notice(if report.needs_attention() {
                    Notice::error(summary)
                } else {
                    Notice::info(summary)
                });
                let _ = self.load();
            }
            Err(message) => self.set_notice(Notice::error(message.clone())),
        }
    }

    pub fn set_restore_path(&mut self, path: impl Into<String>) {
        self.restore_path = path.into();
    }

    /// Where a restore would read from, or `None` when nothing was typed.
    pub fn restore_source(&self) -> Option<PathBuf> {
        let typed = self.restore_path.trim();
        (!typed.is_empty()).then(|| PathBuf::from(typed))
    }

    /// Whether this installation holds no cores, proxies or profiles.
    ///
    /// The question a restore's precondition is made of, asked before the
    /// window decides whether to confirm. A read failure is reported rather than
    /// treated as "empty", because guessing here would be the difference between
    /// asking and silently replacing.
    pub fn is_configuration_empty(&self) -> Result<bool, AppError> {
        Ok(
            application::read_configuration(&*self.profiles, &*self.cores, &*self.proxies)?
                .is_empty(),
        )
    }

    /// The names of the profiles whose browsers are active right now.
    ///
    /// Read from freshly reconciled snapshots rather than from a cached row, so
    /// the guard is against what is running at this moment. Restore and the
    /// browser-data copy both refuse while this is non-empty: the first would
    /// delete a row out from under a live process, the second would copy a
    /// directory Chromium is writing.
    fn active_profile_names(&mut self) -> Vec<String> {
        self.refresh_runtime();
        self.rows
            .iter()
            .filter(|row| row.state().is_active())
            .map(|row| row.profile.name.clone())
            .collect()
    }

    /// Makes this installation be the configuration backup at the typed path.
    ///
    /// The mode is the confirmation: [`RestoreMode::OnlyWhenEmpty`] refuses a
    /// populated installation, and [`RestoreMode::Replace`] carries out the
    /// replacement the window has already asked about. Running profiles block it
    /// either way, because deleting the rows of live processes would leave
    /// browsers this window can no longer stop.
    ///
    /// Runs on the calling thread for the same reason an import does - one small
    /// file and a handful of rows - and reloads the rows afterwards, because a
    /// list still showing what was replaced would contradict the sentence above
    /// it.
    /// The restore, on the calling thread. See
    /// [`AppState::export_configuration`] for why this exists.
    #[cfg(test)]
    pub(crate) fn restore_configuration(
        &mut self,
        mode: RestoreMode,
    ) -> Result<RestoreReport, String> {
        let job = self.begin_restore(mode)?;
        let source = job.source.clone();
        let result = maintenance::run_restore(job);
        self.finish_restore(&source, &result);
        result
    }

    /// The restore as a value, for a caller that will run it somewhere else.
    ///
    /// Every refusal the installation can make is made here, before a thread is
    /// spawned: nothing typed, a running browser, a profile another operation is
    /// holding, another maintenance task in flight. What is left for the worker is
    /// what only the file can be refused for.
    pub fn begin_restore(&mut self, mode: RestoreMode) -> Result<maintenance::RestoreJob, String> {
        let t = self.text();
        let Some(source) = self.restore_source() else {
            let message = t.restore_needs_path.to_string();
            self.set_notice(Notice::error(message.clone()));
            return Err(message);
        };

        let running = self.active_profile_names();
        if !running.is_empty() {
            let message = t.restore_running(&running.join(", "));
            self.set_notice(Notice::error(message.clone()));
            return Err(message);
        }

        // A profile that is starting, or that a copy is holding, is not in the
        // snapshot yet or is not in it at all - and the replacement is about to
        // delete the rows both of them are working from. Refused for the same
        // reason a running profile is, which is the case this generalises.
        if let Some((profile, operation)) = self.operations.any() {
            let message = t.install_busy(&self.profile_name(profile), self.busy_phrase(operation));
            self.set_notice(Notice::error(message.clone()));
            return Err(message);
        }

        self.begin_maintenance(maintenance::Kind::Restore)?;
        Ok(maintenance::RestoreJob {
            text: t,
            source,
            data_dir: self.settings.data_dir().to_path_buf(),
            mode,
            profiles: Arc::clone(&self.profiles),
            cores: Arc::clone(&self.cores),
            proxies: Arc::clone(&self.proxies),
            configuration: Arc::clone(&self.configuration),
        })
    }

    /// What a restore did, said to the reader, with the rows reloaded.
    pub fn finish_restore(
        &mut self,
        source: &std::path::Path,
        result: &Result<RestoreReport, String>,
    ) {
        self.maintenance = None;
        let t = self.text();
        match result {
            Ok(report) => {
                let summary = restore_summary(report, source, t);
                self.set_notice(if report.needs_attention() {
                    Notice::error(summary)
                } else {
                    Notice::info(summary)
                });
                let _ = self.load();
            }
            Err(message) => self.set_notice(Notice::error(message.clone())),
        }
    }

    pub fn set_browser_data_path(&mut self, path: impl Into<String>) {
        self.browser_data_path = path.into();
    }

    /// The browser-data backup directory, or `None` when nothing was typed.
    pub fn browser_data_directory(&self) -> Option<PathBuf> {
        let typed = self.browser_data_path.trim();
        (!typed.is_empty()).then(|| PathBuf::from(typed))
    }

    /// Builds one browser-data copy job, or says why it cannot be started.
    ///
    /// The run happens on a worker, so this only gathers: the profiles as
    /// storage holds them, the identifiers that are active, and the directory.
    /// Every refusal that can be made before a thread is spawned is made here -
    /// an empty path, an empty installation, a running profile, a profile
    /// something else is already doing something with - so the worker is never
    /// started for a copy that could not run.
    ///
    /// The second half of the answer is the lease on those profiles: it is taken
    /// here, and it is the worker that gives it back, because the worker is what
    /// knows the copy is over.
    pub fn browser_data_job(
        &mut self,
        direction: Direction,
    ) -> Result<(BrowserDataJob, Held), String> {
        let t = self.text();
        let directory = self
            .browser_data_directory()
            .ok_or_else(|| "Type the directory to keep browser data in.".to_string())?;

        if self
            .profiles
            .list()
            .map_err(|error| error.to_string())?
            .is_empty()
        {
            return Err("There are no profiles whose browser data could be copied.".to_string());
        }

        let running = self.active_profile_names();
        if !running.is_empty() {
            return Err(t.copy_running(&running.join(", ")));
        }

        let profiles = self.profiles.list().map_err(|error| error.to_string())?;
        let active: Vec<ProfileId> = self
            .rows
            .iter()
            .filter(|row| row.state().is_active())
            .map(|row| row.profile.id)
            .collect();

        // Held from here, not from the worker's first line: the profiles a copy
        // will write are taken before the thread exists, so nothing can start one
        // of them in the moment between the click and the copy. Every profile is
        // in the job - a copy covers the installation - so a copy is what makes
        // the whole installation busy.
        let ids: Vec<ProfileId> = profiles.iter().map(|profile| profile.id).collect();
        let lease = self
            .operations
            .lease(&ids, Operation::Copying)
            .map_err(|busy| {
                t.profile_busy(
                    &self.profile_name(busy.profile),
                    self.busy_phrase(busy.operation),
                )
            })?;

        let job = BrowserDataJob {
            direction,
            profiles,
            running: active,
            directory,
        };
        Ok((job, lease))
    }

    /// Reports the outcome of a browser-data copy the worker finished.
    ///
    /// A copy changes no stored row, so nothing is reloaded: the notice is the
    /// whole of what the window has to do.
    pub fn finish_browser_data(
        &mut self,
        direction: Direction,
        outcome: Result<BrowserDataReport, String>,
    ) {
        let t = self.text();
        let notice = match outcome {
            Ok(report) => Notice::info(browser_data_summary(direction, &report, t)),
            Err(reason) => Notice::error(reason),
        };
        self.set_notice(notice);
    }

    /// Every core with the profiles that use it, for the Browser Cores page.
    pub fn core_rows(&self) -> Result<Vec<CoreRow>, AppError> {
        let usage = self.cores.usage()?;
        Ok(self
            .cores
            .list()?
            .into_iter()
            .map(|core| CoreRow {
                present: core.executable.is_file(),
                used_by: usage.get(&core.id).cloned().unwrap_or_default(),
                core,
            })
            .collect())
    }

    pub fn core(&self, id: CoreId) -> Option<BrowserCore> {
        self.cores.get(id).ok().flatten()
    }

    /// Registers a browser binary, reading its version rather than trusting one.
    pub fn add_core(&mut self, name: Option<String>, path: PathBuf) -> Result<CoreId, AppError> {
        let t = self.text();
        let core = self.record(self.cores.add(name, path))?;
        let id = core.id;
        self.set_notice(Notice::info(t.core_added(
            &core.name,
            &core.version,
            core.major,
        )));
        Ok(id)
    }

    /// Saves a core, re-reading the version when its executable changed.
    pub fn update_core(&mut self, core: BrowserCore) -> Result<(), AppError> {
        let t = self.text();
        let saved = self.record(self.cores.update(core))?;
        self.set_notice(Notice::info(t.core_saved(
            &saved.name,
            &saved.version,
            saved.major,
        )));
        // The rows carry display names that came from this core.
        self.load_rows()?;
        Ok(())
    }

    /// Re-reads a core's version, for a binary that was replaced in place.
    ///
    /// The synchronous half of this is for tests, for the same reason the other
    /// three are: the window runs the probe on a worker.
    #[cfg(test)]
    pub(crate) fn redetect_core(&mut self, id: CoreId) -> Result<(), AppError> {
        // Reading a version starts the core's binary and waits for it to answer,
        // which is why the window runs this on a worker; a caller that wants the
        // answer before returning gets the same three steps in a row.
        let job = self.begin_redetect(id).map_err(AppError::Other)?;
        let result = maintenance::run_redetect(job);
        self.finish_redetect(id, &result);
        Ok(())
    }

    /// The version probe as a value, for a caller that will run it somewhere else.
    pub fn begin_redetect(&mut self, id: CoreId) -> Result<maintenance::RedetectJob, String> {
        self.begin_maintenance(maintenance::Kind::Redetect)?;
        Ok(maintenance::RedetectJob {
            id,
            cores: Arc::clone(&self.cores),
        })
    }

    /// What a version probe found, said to the reader, with the rows reloaded.
    pub fn finish_redetect(&mut self, id: CoreId, result: &Result<BrowserCore, String>) {
        self.maintenance = None;
        let t = self.text();
        match result {
            Ok(refreshed) => {
                self.set_notice(Notice::info(t.core_refreshed(
                    &refreshed.name,
                    &refreshed.version,
                    refreshed.major,
                )));
                let _ = self.load_rows();
            }
            // The probe is the failure: a missing binary, a version that cannot be
            // read, a program that never answers.
            Err(message) => {
                let _ = id;
                self.set_notice(Notice::error(message.clone()));
            }
        }
    }

    /// Removes a core. Refused while a profile still launches with it.
    pub fn delete_core(&mut self, id: CoreId) -> Result<(), AppError> {
        let t = self.text();
        let name = self
            .core(id)
            .map(|core| core.name)
            .unwrap_or_else(|| id.to_string());
        self.record(self.cores.delete(id))?;
        self.set_notice(Notice::info(t.core_deleted(&name)));
        Ok(())
    }

    pub fn has_core(&self) -> bool {
        self.cores
            .list()
            .map(|cores| !cores.is_empty())
            .unwrap_or(false)
    }

    #[cfg(test)]
    fn default_core_id(&self) -> Result<CoreId, AppError> {
        let t = self.text();
        self.cores
            .list()?
            .first()
            .map(|core| core.id)
            .ok_or_else(|| AppError::Conflict(t.no_core_registered.to_string()))
    }

    /// The cores a profile can be put on, for the profile form.
    ///
    /// A core with no detected version is listed too: it is a real choice that
    /// will be refused at launch, and hiding it would leave the user with a
    /// form that cannot explain where their core went.
    pub fn core_choices(&self) -> Vec<CoreChoice> {
        self.cores
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|core| {
                let capabilities = core.capabilities();
                CoreChoice {
                    generation: generation_line(&core),
                    exclusions_honoured: capabilities
                        .as_ref()
                        .is_some_and(|capabilities| capabilities.supports_disable_spoofing),
                    major: core.major,
                    name: core.name,
                    id: core.id,
                }
            })
            .collect()
    }

    /// Creates a profile with the default fingerprint on the first core.
    ///
    /// The window's New Profile form goes through [`Self::create_profile_from`]
    /// instead, so this is only the shortcut a caller without a form in front of
    /// it uses - the acceptance harness that needs a working row, and the tests.
    #[cfg(test)]
    pub fn create_profile(&mut self, name: &str) -> Result<ProfileId, AppError> {
        let core_id = self.core_id()?;
        self.create_profile_from(NewProfile {
            name: name.trim().to_string(),
            core_id,
            user_data_dir: None,
            fingerprint: None,
            proxy_id: None,
            window: None,
            start_target: None,
        })
    }

    /// Creates the profile a filled-in form asked for.
    ///
    /// The draft carries the whole profile the user configured, so nothing is
    /// defaulted behind their back: the seed, the core and the fingerprint are
    /// the ones on the form. Only what the form does not ask about (id, data
    /// directory, start target) is left to the service.
    pub fn create_profile_from(&mut self, draft: NewProfile) -> Result<ProfileId, AppError> {
        let t = self.text();
        let profile = self.record(self.profiles.create(draft))?;
        let id = profile.id;
        self.load()?;
        self.selected = Some(id);
        self.set_notice(Notice::info(t.profile_created(&profile.name)));
        Ok(id)
    }

    /// Opens a profile, once its proxy has said it can carry the traffic.
    ///
    /// The gate is the whole point of this method. A profile that leaves through a
    /// proxy is only useful while that proxy carries traffic, and a launch that
    /// goes ahead without it produces the worst of both outcomes: a browser that
    /// looks healthy while its traffic leaks, or one whose first page never
    /// loads. So the command is not queued until one request has come back
    /// through the same engine the profile would use - [`crate::proxy_tester`],
    /// the same question the Proxies page asks by hand.
    ///
    /// The lease is taken here, before the check, and that is what makes the two
    /// halves one operation: while the proxy is being asked, the profile is
    /// already busy - a second start, a restart and a browser-data copy are all
    /// refused - and a check that fails gives the lease back, so the profile can
    /// be tried again as soon as the proxy is fixed.
    ///
    /// A profile with no proxy has nothing to ask, so it is queued here and the
    /// answer is [`StartGate::Queued`].
    pub fn begin_opening(&mut self, id: ProfileId, how: Opening) -> Result<StartGate, AppError> {
        self.begin_starting(id)?;

        let Some(proxy_id) = self.profile(id).and_then(|profile| profile.proxy_id) else {
            // Nothing to ask: the request a check would send is the one the
            // browser makes for its own first page anyway.
            return match self.queue_opening(id, how) {
                Ok(()) => Ok(StartGate::Queued),
                Err(error) => {
                    // The command never reached the runtime, so no snapshot will
                    // ever answer for it: give the profile back here rather than
                    // at the tick.
                    self.operations.free(id, Operation::Starting);
                    Err(error)
                }
            };
        };

        // A test of this proxy is already in flight - the Proxies page's own
        // button, or another profile's start. Its answer is the answer this
        // opening needs, so it waits for it instead of refusing and making the
        // reader press Start again a few seconds later.
        let pending = PendingStart {
            proxy: proxy_id,
            how,
            asked: Instant::now(),
        };
        if self
            .proxy_tests
            .get(&proxy_id)
            .is_some_and(ProxyTest::is_running)
        {
            self.pending_starts.insert(id, pending);
            self.set_checking_proxy(id, true);
            return Ok(StartGate::Awaiting);
        }

        match self.begin_proxy_test(proxy_id) {
            Ok(job) => {
                self.pending_starts.insert(id, pending);
                self.set_checking_proxy(id, true);
                Ok(StartGate::Checking(Box::new(job)))
            }
            Err(error) => {
                // No such proxy, or one that cannot be tested at all. Nothing is
                // going to answer for this profile, so it is refused now rather
                // than left waiting for an answer that was never asked for.
                self.operations.free(id, Operation::Starting);
                let message = {
                    let t = self.text();
                    t.start_not_checked(&self.profile_name(id), &error.to_string())
                };
                self.set_notice(Notice::error(message));
                Err(error)
            }
        }
    }

    /// Queues the command an opening was holding, for a profile that holds its
    /// lease.
    ///
    /// The second half of [`AppState::begin_opening`]: either nothing had to be
    /// asked, or the proxy has just answered. Fails only when the command itself
    /// does, and the caller is what gives the lease back in that case.
    fn queue_opening(&mut self, id: ProfileId, how: Opening) -> Result<(), AppError> {
        let result = match how {
            Opening::Start => self.runtime.start(id),
            Opening::Restart => self.runtime.restart(id),
        };
        self.record(result)?;
        if how == Opening::Restart {
            // A restarted browser is a new browser: the old reading is stale.
            self.forget_verification(id);
        }
        self.refresh_runtime();
        Ok(())
    }

    /// Starts a profile without asking its proxy first.
    ///
    /// The command half of [`AppState::begin_opening`], for a caller that has
    /// already decided: a test, or the acceptance harness that drives the state
    /// directly. The window never uses it - `begin_opening` is what makes a start
    /// wait for the proxy, and a second way in would be a way around that.
    #[cfg(test)]
    pub(crate) fn start(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.begin_starting(id)?;
        if let Err(error) = self.queue_opening(id, Opening::Start) {
            self.operations.free(id, Operation::Starting);
            return Err(error);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn restart(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.begin_starting(id)?;
        if let Err(error) = self.queue_opening(id, Opening::Restart) {
            self.operations.free(id, Operation::Starting);
            return Err(error);
        }
        Ok(())
    }

    pub fn stop(&mut self, id: ProfileId) -> Result<(), AppError> {
        // A stop pressed while the proxy is still being asked calls the start
        // off rather than stopping a browser that was never launched: the command
        // would reach a runtime with nothing to stop, and the answer to the check
        // would then start the profile the reader just called off.
        if self.pending_starts.remove(&id).is_some() {
            self.operations.free(id, Operation::Starting);
            self.set_checking_proxy(id, false);
            self.refresh_runtime();
            return Ok(());
        }

        self.record(self.runtime.stop(id))?;
        // The reading described a browser that no longer exists.
        self.forget_verification(id);
        self.refresh_runtime();
        Ok(())
    }

    /// Marks a row as waiting for its proxy, now rather than at the next tick.
    fn set_checking_proxy(&mut self, id: ProfileId, checking: bool) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.profile.id == id) {
            row.checking_proxy = checking;
        }
    }

    /// Calls off a start that is waiting for a proxy, and says why.
    ///
    /// The lease goes back first: whatever went wrong with the check, the profile
    /// is free again, and a wait that nobody is going to answer must not be what
    /// keeps it that way.
    fn cancel_pending_start(&mut self, profile: ProfileId, reason: &str) {
        self.pending_starts.remove(&profile);
        self.operations.free(profile, Operation::Starting);
        self.set_checking_proxy(profile, false);
        let message = {
            let t = self.text();
            t.start_not_checked(&self.profile_name(profile), reason)
        };
        self.set_notice(Notice::error(message.clone()));
        self.toast(ToastKind::Error, message);
    }

    /// Calls off every start that is waiting on one proxy.
    ///
    /// A proxy that was edited or removed cannot answer the question that was
    /// asked about it - the old reading is a claim about the old upstream - so the
    /// checks waiting on it are called off rather than left waiting for an answer
    /// that will never be matched to them.
    fn cancel_pending_starts(&mut self, proxy: ProxyId, reason: &str) {
        let waiting: Vec<ProfileId> = self
            .pending_starts
            .iter()
            .filter(|(_, pending)| pending.proxy == proxy)
            .map(|(profile, _)| *profile)
            .collect();
        for profile in waiting {
            self.cancel_pending_start(profile, reason);
        }
    }

    /// Takes the profile for a start that is about to be queued.
    ///
    /// The lease outlives this call on purpose: `RuntimeService::start` returns as
    /// soon as the command is queued, so releasing here would leave exactly the
    /// window this is for - a copy that could begin while the browser is coming
    /// up. [`AppState::refresh_runtime`] gives it back when the runtime's snapshot
    /// stops saying the profile is stopped, and [`Operations::expire`] gives it
    /// back if the runtime never says anything at all.
    fn begin_starting(&self, id: ProfileId) -> Result<(), AppError> {
        let t = self.text();
        self.operations
            .take(id, Operation::Starting)
            .map_err(|busy| {
                AppError::Other(t.profile_busy(
                    &self.profile_name(busy.profile),
                    self.busy_phrase(busy.operation),
                ))
            })
    }

    /// What an operation is, in the words a refusal uses.
    fn busy_phrase(&self, operation: Operation) -> &'static str {
        let t = self.text();
        match operation {
            Operation::Starting => t.busy_starting(),
            Operation::Copying => t.busy_copying(),
        }
    }

    /// The name of a profile, or its identifier when storage no longer has it.
    fn profile_name(&self, id: ProfileId) -> String {
        self.row(id)
            .map(|row| row.profile.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    /// Writes an edited profile back, keeping the row list in step.
    pub fn update_profile(&mut self, profile: BrowserProfile) -> Result<(), AppError> {
        self.record(self.profiles.update(profile))?;
        self.load_rows()?;
        self.refresh_runtime();
        Ok(())
    }

    /// Copies a profile under a new name, id, seed and data directory.
    pub fn duplicate_profile(&mut self, id: ProfileId) -> Result<ProfileId, AppError> {
        let t = self.text();
        let name = t.profile_copy_name(&self.next_profile_name());
        let duplicated = self.record(self.profiles.duplicate(id, name))?;
        self.load_rows()?;
        self.refresh_runtime();
        self.selected = Some(duplicated.id);
        Ok(duplicated.id)
    }

    /// Removes a profile. Its browser data is kept on disk: deleting a profile
    /// should not be the same decision as destroying its sessions.
    pub fn delete_profile(&mut self, id: ProfileId) -> Result<(), AppError> {
        // A profile that is being checked has no browser yet; leaving the check
        // pending would start a profile that no longer exists, so it is called
        // off with the record.
        if self.pending_starts.remove(&id).is_some() {
            self.operations.free(id, Operation::Starting);
        }
        self.record(self.profiles.delete(id))?;
        self.forget_verification(id);
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.load_rows()?;
        self.refresh_runtime();
        Ok(())
    }

    /// The stored profile behind a row, for the editor to start from.
    pub fn profile(&self, id: ProfileId) -> Option<BrowserProfile> {
        self.rows
            .iter()
            .find(|row| row.profile.id == id)
            .map(|row| row.profile.clone())
    }

    /// Claims the verification slot for a profile and assembles the job.
    ///
    /// Refuses when the profile is not running, because a fingerprint can only
    /// be read out of a live browser, and while another verification is in
    /// flight for the same profile.
    pub fn begin_verification(&mut self, id: ProfileId) -> Result<VerificationJob, AppError> {
        let t = self.text();
        let existing = self.verifications.get(&id);
        if existing.is_some_and(Verification::is_running) {
            return Err(AppError::Other(t.verification_busy(&id.to_string())));
        }
        let job = self.verification_job(id)?;
        self.verifications.insert(id, Verification::Running);
        Ok(job)
    }

    /// Records the outcome of a verification the view ran on a worker.
    pub fn finish_verification(
        &mut self,
        id: ProfileId,
        outcome: Result<VerificationReport, String>,
    ) {
        let t = self.text();
        let verification = match outcome {
            Ok(report) if report.discrepancies.is_empty() => Verification::Confirmed(report),
            Ok(report) => Verification::Disagreements(report),
            Err(reason) => Verification::Unreadable(reason),
        };
        // A reading is the answer to a question the user asked, so it belongs
        // in the history as well as on the row.
        match &verification {
            Verification::Confirmed(report) => {
                self.append_log(LogLevel::Info, Some(id), confirmed_line(report, t));
                self.toast(ToastKind::Success, t.fingerprint_confirmed_toast);
            }
            Verification::Disagreements(report) => {
                let message = t.verification_claims_toast(report.discrepancies.len());
                self.toast(
                    ToastKind::Warning,
                    t.verification_claims_details(report.discrepancies.len()),
                );
                self.append_log(LogLevel::Warning, Some(id), message);
            }
            Verification::Unreadable(reason) => {
                self.toast(ToastKind::Error, t.fingerprint_read_failed(reason));
                self.append_log(LogLevel::Error, Some(id), t.fingerprint_read_failed(reason));
            }
            Verification::Running => {}
        }
        self.verifications.insert(id, verification);
    }

    /// Clears a verification result, e.g. after the profile restarted: a new
    /// browser has a new fingerprint.
    pub fn forget_verification(&mut self, id: ProfileId) {
        self.verifications.remove(&id);
    }

    pub fn verification(&self, id: ProfileId) -> Option<&Verification> {
        self.verifications.get(&id)
    }

    fn verification_job(&mut self, id: ProfileId) -> Result<VerificationJob, AppError> {
        let t = self.text();
        let row = self
            .rows
            .iter()
            .find(|row| row.profile.id == id)
            .ok_or_else(|| AppError::Other(t.profile_not_found(&id.to_string())))?;
        let port = row
            .cdp_port()
            .ok_or_else(|| AppError::Other(t.verify_needs_browser()))?;
        let core = self
            .cores
            .get(row.profile.core_id)?
            .ok_or_else(|| AppError::Other(t.core_not_found(&row.profile.core_id.to_string())))?;
        // A core whose version was never read has no capability table, and
        // asking for one would be asking what the engine may claim. This is the
        // same refusal the launch path makes.
        let capabilities = core
            .capabilities()
            .ok_or_else(|| AppError::Conflict(t.core_has_no_version(&core.name)))?;
        Ok(VerificationJob {
            profile_id: id,
            port,
            profile: row.profile.fingerprint.clone(),
            capabilities,
            egress: self.egress_job(row.profile.proxy_id),
        })
    }

    /// The address question for a profile, when it has one to be asked.
    ///
    /// A profile with no proxy claims nothing about an address, so it is not
    /// asked: the endpoint learns the address it was asked from, and there is
    /// no reason to spend that on a browser that was never meant to leave by
    /// anywhere else.
    ///
    /// The expectation is the address the proxy was *tested* at, and it is
    /// absent until a test has run. An untested proxy leaves nothing to
    /// disagree with, and a reading is still worth having: it says where the
    /// traffic went, which is the first thing anyone wants to know.
    fn egress_job(&self, proxy_id: Option<ProxyId>) -> Option<EgressJob> {
        let proxy_id = proxy_id?;
        Some(EgressJob {
            echo_url: self.settings.echo_url().to_string(),
            expected: self
                .proxy_tests
                .get(&proxy_id)
                .and_then(ProxyTest::exit_ip)
                .map(str::to_string),
        })
    }

    /// Claims the test slot for a proxy and assembles the job.
    ///
    /// A proxy can be tested whether or not anything is running, which is the
    /// point: a proxy has to be judged before a profile is launched with it.
    /// Refused only while another test of the same proxy is in flight, which
    /// would start a second engine for an answer already on its way.
    pub fn begin_proxy_test(&mut self, id: ProxyId) -> Result<ProxyTestJob, AppError> {
        let t = self.text();
        if self.proxy_tests.get(&id).is_some_and(ProxyTest::is_running) {
            return Err(AppError::Other(t.proxy_test_busy(&id.to_string())));
        }
        let proxy = self
            .proxies
            .get(id)?
            .ok_or_else(|| AppError::NotFound(t.proxy_not_found(&id.to_string())))?;
        let job = ProxyTestJob {
            proxy_id: id,
            proxy,
            echo_url: self.settings.echo_url().to_string(),
            live_port: self.live_port_for(id),
        };
        self.proxy_tests.insert(id, ProxyTest::Running);
        Ok(job)
    }

    /// Records the outcome of a test the view ran on a worker.
    ///
    /// `live` says whether the request went through an engine that was already
    /// up, because the two answers mean different things: one is a rehearsal,
    /// the other is what is happening to a profile's traffic right now.
    pub fn finish_proxy_test(
        &mut self,
        id: ProxyId,
        live: bool,
        outcome: Result<Diagnosis, Fault>,
    ) {
        let t = self.text();
        let test = match outcome {
            Ok(diagnosis) => ProxyTest::Passed(ProxyReading {
                exit_ip: diagnosis.exit_ip,
                elapsed: diagnosis.elapsed,
                live,
            }),
            Err(fault) => ProxyTest::Failed(fault),
        };
        let name = self
            .proxy(id)
            .map(|proxy| proxy.name)
            .unwrap_or_else(|| t.a_removed_profile.to_string());

        // The openings that were waiting for this answer, taken out first: the
        // answer is what decides them, and a test that is only a reading has
        // none.
        let waiting: Vec<(ProfileId, Opening)> = self
            .pending_starts
            .iter()
            .filter(|(_, pending)| pending.proxy == id)
            .map(|(profile, pending)| (*profile, pending.how))
            .collect();
        for (profile, _) in &waiting {
            self.pending_starts.remove(profile);
            self.set_checking_proxy(*profile, false);
        }

        match &test {
            ProxyTest::Passed(reading) => {
                if waiting.is_empty() {
                    self.toast(
                        ToastKind::Success,
                        t.proxy_traffic_from(&name, &reading.exit_ip),
                    );
                }
                // A start that waited for this reading is the reason the reading
                // was taken, so its sentence is the one the reader is waiting
                // for. The profile's lease is already held, and queuing is what
                // the answer buys.
                for (profile, how) in waiting {
                    let profile_name = self.profile_name(profile);
                    match self.queue_opening(profile, how) {
                        Ok(()) => self.toast(
                            ToastKind::Success,
                            t.started_through_proxy(&profile_name, &name, &reading.exit_ip),
                        ),
                        Err(_) => {
                            // `queue_opening` recorded the reason; nothing may be
                            // left holding the profile.
                            self.operations.free(profile, Operation::Starting);
                        }
                    }
                }
            }
            ProxyTest::Failed(fault) => {
                if waiting.is_empty() {
                    // The class is what to act on, so it goes first; the evidence
                    // follows, because a dashboard line is too short to hold it
                    // and this is the one moment the user is thinking about this
                    // proxy.
                    self.toast(
                        ToastKind::Error,
                        t.proxy_no_traffic_reached(&name, &fault.to_string()),
                    );
                }
                for (profile, _) in waiting {
                    // The refusal, and the profile is free again: a proxy that is
                    // down now may be up in a minute, and the reader has to be
                    // able to press Start then.
                    self.operations.free(profile, Operation::Starting);
                    let message = t.proxy_blocks_start(
                        &self.profile_name(profile),
                        &name,
                        &fault.to_string(),
                    );
                    self.set_notice(Notice::error(message.clone()));
                    self.toast(ToastKind::Error, message);
                }
            }
            ProxyTest::Running => {}
        }
        // The path is the half of the answer the row cannot show, and the class
        // is what to act on. Both go in the record, which outlives the row.
        if let Some(detail) = test.detail(self.text()) {
            let level = if test.fault().is_some() {
                LogLevel::Error
            } else {
                LogLevel::Info
            };
            self.append_log(level, None, t.proxy_test_log(&name, &detail));
        }
        self.proxy_tests.insert(id, test);
    }

    /// Clears a test result. Editing a proxy is enough: a different upstream is
    /// a different answer, and the old one would read as a claim about the new.
    pub fn forget_proxy_test(&mut self, id: ProxyId) {
        self.proxy_tests.remove(&id);
        let reason = self.text().proxy_check_dropped();
        self.cancel_pending_starts(id, reason);
    }

    /// What the last test of this proxy found, if there was one.
    pub fn proxy_test(&self, id: ProxyId) -> Option<&ProxyTest> {
        self.proxy_tests.get(&id)
    }

    /// The SOCKS port of an engine already up for a profile using this proxy.
    ///
    /// An engine that is up is the one that would have to forward, so probing
    /// it asks about the live path instead of a rehearsal.
    ///
    /// The port is only considered when the profile says it is running. The
    /// supervisor clears the port when a profile stops, but this decision is
    /// worth its own gate: probing a port nothing is listening on would report
    /// a timeout, and a timeout here reads as "this proxy is broken" when the
    /// truth is that nothing was asked in the first place.
    fn live_port_for(&self, id: ProxyId) -> Option<u16> {
        self.rows
            .iter()
            .filter(|row| row.profile.proxy_id == Some(id))
            .filter(|row| matches!(row.state(), RuntimeState::Running))
            .find_map(|row| row.socks_port())
    }

    #[cfg(test)]
    fn core_id(&mut self) -> Result<CoreId, AppError> {
        let resolved = self.default_core_id();
        self.record(resolved)
    }

    /// Record a failure in the banner, and hand it back to the caller.
    fn record<T>(&mut self, result: Result<T, AppError>) -> Result<T, AppError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.set_notice(Notice::error(error.to_string()));
                Err(error)
            }
        }
    }
}

/// The log line for a verification that found nothing wrong.
///
/// The address belongs here rather than on the row: the row has no space for
/// it, and this is the half of the answer that says the traffic took the path
/// the user intended. A reading that was not taken says so, because "confirmed"
/// on its own would read as though the address had been checked too.
fn confirmed_line(report: &VerificationReport, t: &Text) -> String {
    let mut line = t.egress_confirmed();
    if let Some(exit_ip) = &report.exit_ip {
        line.push_str(&t.egress_left_from(exit_ip));
    }
    if let Some(reason) = &report.exit_unreadable {
        line.push_str(&t.egress_unreadable(reason));
    }
    line
}

/// How a crashed component is named in the log.
fn component_label(component: RuntimeComponent) -> &'static str {
    match component {
        RuntimeComponent::Browser => "browser",
        RuntimeComponent::Xray => "xray",
    }
}

#[cfg(test)]
pub(crate) mod testing;
/// What an export did, in one sentence.
///
/// The three cases are kept apart on purpose. "Credentials were left out" and
/// "there were none to leave out" are different sentences, and only one of them
/// tells the reader that the file is not the whole configuration. Saying which
/// file and how many of each is the difference between a backup someone trusts
/// and a backup someone assumes.
fn export_summary(report: &ExportReport, t: &Text) -> String {
    let counts = counts_phrase(
        &Counts {
            cores: report.cores,
            proxies: report.proxies,
            profiles: report.profiles,
        },
        t,
    );
    let where_it_went = t.export_wrote(&counts, &report.path.display().to_string());

    let clause = match (report.credentials, report.credentials_removed) {
        (Credentials::Included, _) => t.export_carries_credentials(),
        (Credentials::Excluded, 0) => t.export_had_no_credentials(),
        (Credentials::Excluded, removed) => t.export_left_out(removed),
    };
    t.join_sentences(&[where_it_went, clause])
}

/// `1 core, 2 proxies and 3 profiles`, the phrase every report sentence opens
/// with. One place, so an export, an import and a restore cannot count the same
/// three lists three different ways.
fn counts_phrase(counts: &Counts, t: &Text) -> String {
    t.counts_phrase(counts.cores, counts.proxies, counts.profiles)
}

/// The clauses an import and a restore share, in the order they are read.
///
/// The two verbs report the same shortfalls because they write through the same
/// rules; what differs is the sentence they are clauses of.
fn note_clauses(notes: &ImportNotes, t: &Text) -> Vec<String> {
    let mut clauses: Vec<String> = Vec::new();
    if !notes.differing.is_empty() {
        clauses.push(t.notes_differing(&listed(&notes.differing, t)));
    }
    if !notes.missing_core.is_empty() {
        clauses.push(t.notes_missing_core(&listed(&notes.missing_core, t)));
    }
    if !notes.missing_proxy.is_empty() {
        clauses.push(t.notes_missing_proxy(&listed(&notes.missing_proxy, t)));
    }
    if !notes.repointed.is_empty() {
        let moved: Vec<String> = notes
            .repointed
            .iter()
            .map(|moved| moved.profile.clone())
            .collect();
        clauses.push(t.notes_repointed(&listed(&moved, t)));
    }
    clauses
}

/// The clause for records the database refused, or nothing when none were.
fn failed_clause(failed: &[String], t: &Text) -> Option<String> {
    (!failed.is_empty()).then(|| t.refused_not_stored(&listed(failed, t)))
}

/// The clause that explains a file written without credentials, when one of its
/// proxies landed.
fn credential_clause(notes: &ImportNotes, proxies_landed: usize, t: &Text) -> Option<String> {
    (notes.credentials_excluded && proxies_landed > 0).then(|| t.credentials_left_out_clause())
}

/// What an import did, in one sentence.
///
/// Five things can be true at once and every one of them is something the reader
/// has to hear: what arrived, what was left alone because the identifier was
/// taken, what was skipped, what came in without its proxy, and whose browser
/// data is about to start from this machine's directory instead of the recorded
/// one. They are clauses of one sentence rather than separate sentences because
/// they all describe the same event, and any one of them alone would be a
/// misleading account of the rest.
///
/// Names are capped by [`listed`]: this is a line in a toast and a line in the
/// activity log, not the place for a list of twenty profiles. Naming every one
/// of them is the presentation question the design leaves open.
fn import_summary(report: &ImportReport, source: &std::path::Path, t: &Text) -> String {
    let source = source.display().to_string();
    let base = if report.added.total() == 0 {
        t.read_nothing_added(&source)
    } else {
        t.read_added(&source, &counts_phrase(&report.added, t))
    };

    let notes = &report.notes;
    let mut clauses = note_clauses(notes, t);
    if let Some(clause) = failed_clause(&report.failed, t) {
        clauses.push(clause);
    }
    if let Some(clause) = credential_clause(notes, report.added.proxies + notes.kept.proxies, t) {
        clauses.push(clause);
    }

    if clauses.is_empty() {
        base
    } else {
        clauses.insert(0, base);
        t.join_sentences(&clauses)
    }
}

/// What a restore did, in one sentence.
///
/// The import's sentence plus what was replaced: a restore that removed three
/// and added three is a different event from one that added three to nothing,
/// and the reader has to be able to tell which happened.
fn restore_summary(report: &RestoreReport, source: &std::path::Path, t: &Text) -> String {
    let source = source.display().to_string();
    let mut parts = vec![if report.added.total() == 0 {
        t.read_nothing_added(&source)
    } else {
        t.read_added(&source, &counts_phrase(&report.added, t))
    }];
    if report.removed.total() > 0 {
        parts.push(t.replaced(&counts_phrase(&report.removed, t)));
    }

    let mut clauses = note_clauses(&report.notes, t);
    if let Some(clause) = failed_clause(&report.failed, t) {
        clauses.push(clause);
    }
    // The same clause an import adds, for the same event: a file written without
    // credentials restores proxies that no longer carry them. A restore plans
    // against an empty snapshot, so `kept` is normally zero - it is added in
    // because a removal the database refused can leave a record for the import
    // half to keep, and the sentence must not depend on that having worked.
    if let Some(clause) = credential_clause(
        &report.notes,
        report.added.proxies + report.notes.kept.proxies,
        t,
    ) {
        clauses.push(clause);
    }

    let mut all = parts;
    all.extend(clauses);
    t.join_sentences(&all)
}

/// What a browser-data copy did, in one sentence.
///
/// The direction changes the preposition and nothing else, which is the point:
/// the two are the same copy read from opposite ends. A profile that has no
/// directory yet is named rather than counted, because "why is my profile not in
/// the backup" is the question the sentence has to answer.
///
/// The empty sentence also turns on the direction, because the two directions
/// are empty for opposite reasons: copying out finds nothing because no profile
/// has run yet, and copying in finds nothing because the backup directory does
/// not hold these profiles. Saying "no browser data in <backup>" for a copy out
/// would blame the destination for what the source never had.
fn browser_data_summary(direction: Direction, report: &BrowserDataReport, t: &Text) -> String {
    let directory = report.directory.display().to_string();
    let sentence = if report.copied.is_empty() {
        match direction {
            Direction::ToBackup => t.copy_nothing_out(),
            Direction::FromBackup => t.copy_nothing_in(&directory),
        }
    } else {
        t.copy_copied(
            direction == Direction::ToBackup,
            report.copied.len(),
            &directory,
            &human_bytes(report.bytes),
        )
    };

    if report.skipped.is_empty() {
        sentence
    } else {
        t.join_sentences(&[sentence, t.copy_skipped(&listed(&report.skipped, t))])
    }
}

/// A byte count a person can read at a glance.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [
        ("GiB", 1 << 30),
        ("MiB", 1 << 20),
        ("KiB", 1 << 10),
        ("B", 1),
    ];
    for (unit, size) in UNITS {
        if bytes >= size {
            return if size == 1 {
                format!("{bytes} B")
            } else {
                format!("{:.1} {unit}", bytes as f64 / size as f64)
            };
        }
    }
    "0 B".to_string()
}

/// Up to three names, then how many were left off.
///
/// A count rather than silence, so a reader knows the sentence is a summary
/// rather than the whole of it.
fn listed(names: &[String], t: &Text) -> String {
    const SHOWN: usize = 3;
    if names.len() <= SHOWN {
        names.join(", ")
    } else {
        t.listed_more(&names[..SHOWN].join(", "), names.len() - SHOWN)
    }
}

#[cfg(test)]
mod tests;
