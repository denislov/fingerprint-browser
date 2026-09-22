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

mod configuration;
mod cores;
mod profiles;
mod proxies;
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

/// How long a proxy check may hold its profile before the window assumes no
/// answer is coming.
///
/// The check itself is bounded by the engine's readiness deadline and the
/// request's timeout, and a lease that
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
    installation: Arc<application::coordination::Installation>,
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
    queued_starts: HashMap<ProfileId, u64>,
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
        let installation = Arc::new(application::coordination::Installation::default());
        let profiles = Arc::new(application::coordination::Coordinated {
            service: profiles,
            installation: Arc::clone(&installation),
        }) as Arc<dyn ProfileService>;
        let cores = Arc::new(application::coordination::Coordinated {
            service: cores,
            installation: Arc::clone(&installation),
        }) as Arc<dyn CoreService>;
        let proxies = Arc::new(application::coordination::Coordinated {
            service: proxies,
            installation: Arc::clone(&installation),
        }) as Arc<dyn ProxyService>;
        Self {
            installation,
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
            queued_starts: HashMap::new(),
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
            // Old failure/running snapshots do not answer a newly queued start.
            if self
                .queued_starts
                .get(&row.profile.id)
                .is_some_and(|request| {
                    row.snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.acknowledged_start >= *request)
                })
            {
                self.queued_starts.remove(&row.profile.id);
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
