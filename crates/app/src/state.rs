//! View-facing application state.
//!
//! [`AppState`] is intentionally free of GPUI types so it can be unit tested
//! without a window. The view layer renders it and forwards user actions back
//! into it. Runtime state is never owned here: every read goes through
//! [`RuntimeService::snapshot`], which is the documented reconciliation path.

use crate::browser_data::BrowserDataJob;
use crate::exit::ExitMode;
use crate::log_file::LogFile;
use crate::proxy_tester::ProxyTestJob;
use crate::settings::{SettingKey, SettingRow, Settings};
use crate::text::{Lang, Text, text};
use crate::theme::ThemeChoice;
use crate::verifier::{EgressJob, VerificationJob, VerificationReport};
use application::{
    AppError, BrowserDataReport, CoreService, Counts, Credentials, DeleteMode, Direction,
    ExportOrigin, ExportReport, Held, ImportNotes, ImportReport, NewProfile, NewProxy, Operation,
    Operations, ProfileService, ProxyService, RestoreError, RestoreMode, RestoreReport,
    RuntimeService,
};
use domain::{
    BrowserCore, BrowserProfile, CoreId, ProfileId, ProxyId, ProxyOutbound, ProxyProfile,
    RuntimeState,
};
use runtime::{Diagnosis, Discrepancy, Fault, RuntimeComponent, RuntimeEvent, RuntimeSnapshot};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

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
}

impl ProfileRow {
    /// Snapshot state, or `Stopped` when the profile was never started.
    pub fn state(&self) -> RuntimeState {
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
}

impl AppState {
    pub fn new(
        profiles: Arc<dyn ProfileService>,
        runtime: Arc<RuntimeService>,
        cores: Arc<dyn CoreService>,
        proxies: Arc<dyn ProxyService>,
        settings: Settings,
    ) -> Self {
        let (log_file, log_file_error) =
            match LogFile::open(&settings.data_dir().join(crate::log_file::LOG_DIR)) {
                Ok(file) => (Some(file), None),
                Err(error) => (None, Some(error)),
            };
        Self::assemble(
            profiles,
            runtime,
            cores,
            proxies,
            settings,
            log_file,
            log_file_error,
        )
    }

    /// The same state with a specific activity log, or none at all.
    ///
    /// A test must not write into the data directory of the machine it runs on;
    /// a test that drives the sink's failure path builds its own [`LogFile`].
    #[cfg(test)]
    pub(crate) fn with_log(
        profiles: Arc<dyn ProfileService>,
        runtime: Arc<RuntimeService>,
        cores: Arc<dyn CoreService>,
        proxies: Arc<dyn ProxyService>,
        settings: Settings,
        log_file: Option<LogFile>,
        log_file_error: Option<String>,
    ) -> Self {
        Self::assemble(
            profiles,
            runtime,
            cores,
            proxies,
            settings,
            log_file,
            log_file_error,
        )
    }

    /// The window's state with no activity log on disk.
    #[cfg(test)]
    pub(crate) fn for_test(
        profiles: Arc<dyn ProfileService>,
        runtime: Arc<RuntimeService>,
        cores: Arc<dyn CoreService>,
        proxies: Arc<dyn ProxyService>,
        settings: Settings,
    ) -> Self {
        Self::with_log(profiles, runtime, cores, proxies, settings, None, None)
    }

    fn assemble(
        profiles: Arc<dyn ProfileService>,
        runtime: Arc<RuntimeService>,
        cores: Arc<dyn CoreService>,
        proxies: Arc<dyn ProxyService>,
        settings: Settings,
        log_file: Option<LogFile>,
        log_file_error: Option<String>,
    ) -> Self {
        Self {
            profiles,
            runtime,
            cores,
            proxies,
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

        for profile in self.operations.expire(START_LEASE) {
            tracing::warn!(
                "no answer for the start of {} within {}s; the profile is free again",
                self.profile_name(profile),
                START_LEASE.as_secs()
            );
        }
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
        // The old result was about the old upstream, and would read as a claim
        // about the new one.
        self.forget_proxy_test(proxy.id);
        self.set_notice(Notice::info(t.proxy_saved(&proxy.name)));
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
        // Nothing points at this id any more, so a result kept for it could
        // only ever be shown against a different proxy.
        self.forget_proxy_test(id);
        self.set_notice(Notice::info(t.proxy_deleted(&name)));
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
    pub fn export_configuration(&mut self) -> Result<ExportReport, String> {
        let t = self.text();
        let credentials = if self.export_includes_credentials {
            Credentials::Included
        } else {
            Credentials::Excluded
        };
        let origin = ExportOrigin {
            exported_at: crate::log_file::timestamp(SystemTime::now()),
            source_data_dir: self.settings.data_dir().display().to_string(),
        };
        let destination = self.export_destination();

        let result = application::read_configuration(&*self.profiles, &*self.cores, &*self.proxies)
            .map_err(|error| t.export_read_failed(&error.to_string()))
            .and_then(|snapshot| {
                application::write_config_backup(snapshot, credentials, origin, &destination)
                    .map_err(|error| t.export_write_failed(&error.to_string()))
            });

        match &result {
            Ok(report) => self.set_notice(Notice::info(export_summary(report, t))),
            Err(message) => self.set_notice(Notice::error(message.clone())),
        }
        result
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
    pub fn import_configuration(&mut self) -> Result<ImportReport, String> {
        let t = self.text();
        let Some(source) = self.import_source() else {
            let message = "Type the path of a configuration backup to import.".to_string();
            self.set_notice(Notice::error(message.clone()));
            return Err(message);
        };
        let data_dir = self.settings.data_dir().to_path_buf();

        let result = application::read_config_backup(&source)
            .map_err(|error| t.file_read_failed(&error.to_string()))
            .and_then(|document| {
                application::read_configuration(&*self.profiles, &*self.cores, &*self.proxies)
                    .map_err(|error| t.export_read_failed(&error.to_string()))
                    .and_then(|present| {
                        let plan = application::plan_import(&document, &present, &data_dir);
                        application::apply_import(
                            plan,
                            &*self.cores,
                            &*self.proxies,
                            &*self.profiles,
                        )
                        .map_err(|error| t.config_write_failed(&error.to_string()))
                    })
            });

        match &result {
            Ok(report) => {
                let summary = import_summary(report, &source, t);
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
        result
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
    pub fn restore_configuration(&mut self, mode: RestoreMode) -> Result<RestoreReport, String> {
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

        let data_dir = self.settings.data_dir().to_path_buf();
        let result = application::read_config_backup(&source)
            .map_err(|error| t.file_read_failed(&error.to_string()))
            .and_then(|document| {
                application::read_configuration(&*self.profiles, &*self.cores, &*self.proxies)
                    .map_err(|error| t.export_read_failed(&error.to_string()))
                    .and_then(|present| {
                        let plan = application::plan_restore(&document, &present, &data_dir, mode)
                            .map_err(|error| match error {
                                RestoreError::NotEmpty { present } => {
                                    t.restore_not_empty(&t.counts_phrase(
                                        present.cores,
                                        present.proxies,
                                        present.profiles,
                                    ))
                                }
                            })?;
                        application::apply_restore(
                            plan,
                            &present,
                            &*self.cores,
                            &*self.proxies,
                            &*self.profiles,
                        )
                        .map_err(|error| t.config_write_failed(&error.to_string()))
                    })
            });

        match &result {
            Ok(report) => {
                let summary = restore_summary(report, &source, t);
                self.set_notice(if report.needs_attention() {
                    Notice::error(summary)
                } else {
                    Notice::info(summary)
                });
                let _ = self.load();
            }
            Err(message) => self.set_notice(Notice::error(message.clone())),
        }
        result
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
    pub fn redetect_core(&mut self, id: CoreId) -> Result<(), AppError> {
        let t = self.text();
        let refreshed = self.record(self.cores.redetect(id))?;
        self.set_notice(Notice::info(t.core_refreshed(
            &refreshed.name,
            &refreshed.version,
            refreshed.major,
        )));
        self.load_rows()?;
        Ok(())
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

    pub fn start(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.begin_starting(id)?;
        if let Err(error) = self.record(self.runtime.start(id)) {
            // The command never reached the runtime, so no snapshot will ever
            // answer for it: give the profile back here rather than at the tick.
            self.operations.free(id, Operation::Starting);
            return Err(error);
        }
        self.refresh_runtime();
        Ok(())
    }

    pub fn stop(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.record(self.runtime.stop(id))?;
        // The reading described a browser that no longer exists.
        self.forget_verification(id);
        self.refresh_runtime();
        Ok(())
    }

    pub fn restart(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.begin_starting(id)?;
        if let Err(error) = self.record(self.runtime.restart(id)) {
            self.operations.free(id, Operation::Starting);
            return Err(error);
        }
        // A restarted browser is a new browser: the old reading is stale.
        self.forget_verification(id);
        self.refresh_runtime();
        Ok(())
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
        self.rows
            .iter()
            .find(|row| row.profile.id == id)
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
        self.record(self.profiles.delete(id, DeleteMode::KeepUserData))?;
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

        match &test {
            ProxyTest::Passed(reading) => {
                self.toast(
                    ToastKind::Success,
                    t.proxy_traffic_from(&name, &reading.exit_ip),
                );
            }
            ProxyTest::Failed(fault) => {
                // The class is what to act on, so it goes first; the evidence
                // follows, because a dashboard line is too short to hold it and
                // this is the one moment the user is thinking about this proxy.
                self.toast(
                    ToastKind::Error,
                    t.proxy_no_traffic_reached(&name, &fault.to_string()),
                );
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
pub(crate) mod testing {
    use domain::{ProfileId, RuntimeState};
    use runtime::{RuntimeCommandError, RuntimeFacade, RuntimeSnapshot, StartParams};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, RwLock};
    use storage::{MemCoreRepository, MemProfileRepository};

    /// Synchronous stand-in for the supervisor channel façade.
    ///
    /// Commands take effect immediately so tests assert the UI contract without
    /// depending on process startup.
    #[derive(Default)]
    pub struct FakeRuntime {
        snapshots: RwLock<HashMap<ProfileId, RuntimeSnapshot>>,
        pub commands: Mutex<Vec<String>>,
        /// Whether a start publishes the state it produces.
        ///
        /// A real runtime answers a start a tick after the command is queued, and
        /// the ticket's window between the two is what a start's lease covers. A
        /// test that needs that window turns the answer off and leaves the
        /// snapshot stopped.
        silent: std::sync::atomic::AtomicBool,
    }

    impl FakeRuntime {
        pub fn new() -> Self {
            Self::default()
        }

        /// Stops publishing the state of a start, so the snapshot stays stopped:
        /// the command is queued and nothing has answered.
        pub fn answer_nothing(&self) {
            self.silent.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        pub fn set_state(&self, id: ProfileId, state: RuntimeState) {
            let mut snapshots = self.snapshots.write().expect("snapshot lock");
            let snapshot = snapshots
                .entry(id)
                .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
            snapshot.state = state;
        }

        /// Publishes the debug port a running browser exposes.
        pub fn set_cdp_port(&self, id: ProfileId, port: u16) {
            let mut snapshots = self.snapshots.write().expect("snapshot lock");
            let snapshot = snapshots
                .entry(id)
                .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
            snapshot.cdp_port = Some(port);
        }

        /// Publishes the loopback SOCKS port a running engine listens on.
        pub fn set_socks_port(&self, id: ProfileId, port: u16) {
            let mut snapshots = self.snapshots.write().expect("snapshot lock");
            let snapshot = snapshots
                .entry(id)
                .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
            snapshot.socks_port = Some(port);
        }

        /// Publishes the launch line a running browser was started with.
        #[cfg(test)]
        pub fn set_args(&self, id: ProfileId, args: Vec<String>) {
            let mut snapshots = self.snapshots.write().expect("snapshot lock");
            let snapshot = snapshots
                .entry(id)
                .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
            snapshot.effective_args = args;
        }

        /// Publishes a diagnostic the way the supervisor's warning event does.
        pub fn set_warning(&self, id: ProfileId, message: &str) {
            let mut snapshots = self.snapshots.write().expect("snapshot lock");
            let snapshot = snapshots
                .entry(id)
                .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
            snapshot.last_warning = Some(message.to_string());
        }

        /// The snapshot a test wants to inspect without going through the UI.
        #[cfg(test)]
        pub fn snapshot_of(&self, id: ProfileId) -> Option<RuntimeSnapshot> {
            self.snapshot(id)
        }

        fn record(&self, command: &str) {
            self.commands
                .lock()
                .expect("command lock")
                .push(command.to_string());
        }
    }

    fn snapshot(id: ProfileId, state: RuntimeState) -> RuntimeSnapshot {
        RuntimeSnapshot {
            profile_id: id,
            state,
            browser_pid: None,
            xray_pid: None,
            cdp_port: None,
            socks_port: None,
            started_at: None,
            effective_args: Vec::new(),
            last_error: None,
            last_warning: None,
            dropped_events: 0,
        }
    }

    impl RuntimeFacade for FakeRuntime {
        fn start(&self, params: StartParams) -> Result<(), RuntimeCommandError> {
            let id = params.profile_id();
            if !self.silent.load(std::sync::atomic::Ordering::SeqCst) {
                self.set_state(id, RuntimeState::Running);
            }
            self.record(&format!("start:{id}"));
            Ok(())
        }

        fn stop(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError> {
            self.set_state(profile_id, RuntimeState::Stopped);
            self.record(&format!("stop:{profile_id}"));
            Ok(())
        }

        fn restart(&self, params: StartParams) -> Result<(), RuntimeCommandError> {
            let id = params.profile_id();
            if !self.silent.load(std::sync::atomic::Ordering::SeqCst) {
                self.set_state(id, RuntimeState::Running);
            }
            self.record(&format!("restart:{id}"));
            Ok(())
        }

        fn release_all(&self) -> Result<(), RuntimeCommandError> {
            self.record("release-all");
            Ok(())
        }

        fn snapshot(&self, profile_id: ProfileId) -> Option<RuntimeSnapshot> {
            self.snapshots
                .read()
                .expect("snapshot lock")
                .get(&profile_id)
                .cloned()
        }
    }

    /// The core service over in-memory storage, with a probe that reads the
    /// banner out of the file instead of spawning anything.
    ///
    /// Tests that need a specific version write the banner into the file they
    /// pass to `add`.
    pub fn core_service(
        cores: Arc<MemCoreRepository>,
        profiles: Arc<MemProfileRepository>,
    ) -> Arc<dyn application::CoreService> {
        Arc::new(application::DefaultCoreService::with_probe(
            cores,
            profiles,
            Box::new(|path: &std::path::Path| {
                let banner = std::fs::read_to_string(path).unwrap_or_default();
                let banner = banner.trim();
                runtime::version::VersionReport::from_banner(
                    (!banner.is_empty()).then(|| banner.to_string()),
                )
            }),
        ))
    }

    /// Settings for a test: nothing set, in a config file under the temp dir.
    pub fn settings() -> crate::settings::Settings {
        settings_at(
            &std::env::temp_dir()
                .join("fp-app-settings-test")
                .join("config.json"),
        )
    }

    /// Settings read from a specific config file, for a test that saves one.
    pub fn settings_at(config: &std::path::Path) -> crate::settings::Settings {
        let (settings, _) = crate::settings::Settings::load(crate::settings::Environment {
            config: Some(config.to_string_lossy().to_string()),
            ..crate::settings::Environment::default()
        });
        settings
    }

    /// A browser binary on disk: the file holds the version it reports.
    ///
    /// The directory goes away with the guard, so a test run leaves nothing
    /// behind.
    pub struct CoreBinary {
        dir: std::path::PathBuf,
        path: std::path::PathBuf,
    }

    impl CoreBinary {
        pub fn new(dir: &str, banner: Option<&str>) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-app-core-{dir}"));
            std::fs::create_dir_all(&dir).expect("temp dir");
            let path = dir.join("chrome");
            Self::write(&path, banner);
            Self { dir, path }
        }

        pub fn path_buf(&self) -> std::path::PathBuf {
            self.path.clone()
        }

        /// Replaces the file behind the same path, as a reinstall would.
        pub fn replace(&self, banner: Option<&str>) {
            Self::write(&self.path, banner);
        }

        fn write(path: &std::path::Path, banner: Option<&str>) {
            std::fs::write(path, banner.unwrap_or("")).expect("write binary");
        }
    }

    impl Drop for CoreBinary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    pub fn core(id: domain::CoreId) -> domain::BrowserCore {
        domain::BrowserCore {
            id,
            name: "Test Core 144".to_string(),
            executable: std::path::PathBuf::from("chrome"),
            version: "144.0.0.0".to_string(),
            major: 144,
        }
    }
}

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
mod tests {
    use crate::text::en;

    use super::*;
    use crate::state::testing::{CoreBinary, FakeRuntime, core, core_service};
    use application::{DefaultProfileService, DefaultProxyService};
    use domain::CoreId;
    use runtime::FaultClass;
    use std::path::PathBuf;
    use storage::{
        CoreRepository as _, MemCoreRepository, MemProfileRepository, MemProxyRepository,
        ProfileRepository as _, ProxyRepository as _,
    };

    struct Fixture {
        state: AppState,
        runtime: Arc<FakeRuntime>,
        cores: Arc<MemCoreRepository>,
        profiles: Arc<MemProfileRepository>,
        proxies: Arc<MemProxyRepository>,
    }

    fn fixture() -> Fixture {
        fixture_with_config(
            &std::env::temp_dir()
                .join("fp-app-settings-fixture")
                .join("config.json"),
        )
    }

    /// The same fixture, with the settings file somewhere the test can read.
    fn fixture_with_config(config: &std::path::Path) -> Fixture {
        fixture_full(config, None, None, None)
    }

    /// The same fixture, with its data directory somewhere the test can name.
    ///
    /// An import re-points a profile's browser data at the local default under
    /// the data directory, so a test of that has to know which directory the
    /// settings believe in.
    fn fixture_with_data_dir(data_dir: &std::path::Path) -> Fixture {
        fixture_full(
            &std::env::temp_dir()
                .join("fp-app-settings-fixture")
                .join("config.json"),
            Some(data_dir),
            None,
            None,
        )
    }

    /// The same fixture, with an activity log on disk in `log_dir`.
    fn fixture_with_log(log_dir: &std::path::Path) -> Fixture {
        let file = crate::log_file::LogFile::open(log_dir).expect("open the log file");
        fixture_full(
            &std::env::temp_dir()
                .join("fp-app-settings-fixture")
                .join("config.json"),
            None,
            Some(file),
            None,
        )
    }

    fn fixture_full(
        config: &std::path::Path,
        data_dir: Option<&std::path::Path>,
        log_file: Option<crate::log_file::LogFile>,
        log_file_error: Option<String>,
    ) -> Fixture {
        let profile_repo: Arc<MemProfileRepository> = Arc::new(MemProfileRepository::new());
        let core_repo: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let proxy_repo: Arc<MemProxyRepository> = Arc::new(MemProxyRepository::new());
        let runtime = Arc::new(FakeRuntime::new());

        let service = Arc::new(DefaultProfileService::new(
            profile_repo.clone(),
            PathBuf::from("data"),
        ));
        let runtime_service = Arc::new(RuntimeService::new(
            profile_repo.clone(),
            core_repo.clone(),
            proxy_repo.clone(),
            runtime.clone(),
        ));

        let proxy_service: Arc<dyn ProxyService> = Arc::new(DefaultProxyService::new(
            proxy_repo.clone(),
            profile_repo.clone(),
        ));

        let core_service = core_service(core_repo.clone(), profile_repo.clone());
        let (settings, _) = Settings::load(crate::settings::Environment {
            config: Some(config.to_string_lossy().to_string()),
            data_dir: data_dir.map(|dir| dir.to_string_lossy().to_string()),
            ..crate::settings::Environment::default()
        });
        let state = AppState::with_log(
            service,
            runtime_service,
            core_service,
            proxy_service,
            settings,
            log_file,
            log_file_error,
        );

        Fixture {
            state,
            runtime,
            cores: core_repo,
            profiles: profile_repo,
            proxies: proxy_repo,
        }
    }

    fn seed_core(fixture: &Fixture) -> CoreId {
        let core = core(CoreId::new());
        fixture.cores.save(&core).expect("save core");
        core.id
    }

    #[test]
    fn editing_a_profile_writes_it_and_keeps_the_row_in_step() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Before").expect("create");

        let mut edited = fixture.state.profile(id).expect("the profile is loaded");
        edited.name = "After".to_string();
        edited.fingerprint.seed = 999;
        edited.window.width = 1600;
        fixture.state.update_profile(edited).expect("save");

        let row = fixture
            .state
            .rows()
            .iter()
            .find(|row| row.profile.id == id)
            .expect("the row is still listed");
        assert_eq!(row.profile.name, "After");
        assert_eq!(row.profile.fingerprint.seed, 999);
        assert_eq!(row.profile.window.width, 1600);
        assert_eq!(
            fixture.profiles.get(id).expect("stored").unwrap().name,
            "After",
            "the edit reached storage, not just the view"
        );
    }

    #[test]
    fn a_refused_edit_leaves_the_stored_profile_alone() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Kept").expect("create");

        let mut broken = fixture.state.profile(id).expect("loaded");
        broken.name = "   ".to_string();

        assert!(fixture.state.update_profile(broken).is_err());
        assert!(
            fixture.state.notice().is_some_and(|notice| notice.error),
            "the refusal is shown in the banner"
        );
        assert_eq!(
            fixture.profiles.get(id).expect("stored").unwrap().name,
            "Kept"
        );
    }

    #[test]
    fn a_duplicate_gets_its_own_identity_and_is_selected() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Source").expect("create");
        fixture.state.select(id);

        let copy = fixture.state.duplicate_profile(id).expect("duplicate");

        assert_ne!(copy, id);
        assert_eq!(fixture.state.selected_id(), Some(copy));
        let source = fixture.state.profile(id).expect("source");
        let duplicated = fixture.state.profile(copy).expect("copy");
        assert_ne!(duplicated.fingerprint.seed, source.fingerprint.seed);
        assert_ne!(duplicated.user_data_dir, source.user_data_dir);
        assert!(duplicated.name.contains("Source") || duplicated.name.contains("copy"));
    }

    #[test]
    fn deleting_a_profile_removes_it_and_forgets_its_reading() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);
        fixture
            .state
            .finish_verification(id, Ok(VerificationReport::default()));

        fixture.state.delete_profile(id).expect("delete");

        assert!(fixture.state.rows().is_empty(), "the row is gone");
        assert!(fixture.state.profile(id).is_none());
        assert!(
            fixture.state.verification(id).is_none(),
            "a deleted profile keeps no verification result"
        );
        assert_eq!(fixture.state.selected_id(), None);
    }

    /// A loaded profile with a name and a seed the test chose, so a filter has
    /// something predictable to match on.
    fn named_profile(fixture: &mut Fixture, name: &str, seed: u32) -> ProfileId {
        let id = fixture.state.create_profile(name).expect("create");
        let mut profile = fixture.state.profile(id).expect("loaded");
        profile.fingerprint.seed = seed;
        fixture.state.update_profile(profile).expect("save");
        id
    }

    /// A fixture with a core and three profiles to filter between.
    fn filtered_fixture() -> (Fixture, ProfileId, ProfileId, ProfileId) {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let work = named_profile(&mut fixture, "Work laptop", 4242);
        let shopping = named_profile(&mut fixture, "Shopping", 7777);
        let staging = named_profile(&mut fixture, "Staging box", 1234);
        (fixture, work, shopping, staging)
    }

    #[test]
    fn an_empty_filter_lists_every_profile() {
        let (mut fixture, _, _, _) = filtered_fixture();

        assert_eq!(fixture.state.visible_rows().len(), 3);

        // Whitespace is not a filter: it is a field someone tabbed through.
        fixture.state.set_profile_filter("   ");
        assert_eq!(fixture.state.visible_rows().len(), 3);
    }

    #[test]
    fn a_filter_narrows_the_list_to_the_profiles_that_match() {
        let (mut fixture, work, _, _) = filtered_fixture();

        fixture.state.set_profile_filter("work");

        let visible = fixture.state.visible_rows();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].profile.id, work);
        assert_eq!(
            fixture.state.profile_filter(),
            "work",
            "the field holds what was typed, filter or not"
        );
    }

    #[test]
    fn a_filter_ignores_case_and_the_space_around_it() {
        let (mut fixture, work, _, _) = filtered_fixture();

        fixture.state.set_profile_filter("  WORK  ");

        let visible = fixture.state.visible_rows();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].profile.id, work);
    }

    #[test]
    fn a_filter_matches_the_seed_as_well_as_the_name() {
        let (mut fixture, _, _, staging) = filtered_fixture();

        fixture.state.set_profile_filter("1234");

        let visible = fixture.state.visible_rows();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].profile.id, staging);
    }

    #[test]
    fn a_filter_matches_the_core_and_the_proxy_a_profile_runs_on() {
        let (mut fixture, _, shopping, _) = filtered_fixture();
        let zurich = fixture
            .state
            .create_proxy("Zurich exit", socks5("127.0.0.1", 1080))
            .expect("create a proxy");
        let mut with_proxy = fixture.state.profile(shopping).expect("loaded");
        with_proxy.proxy_id = Some(zurich);
        fixture.state.update_profile(with_proxy).expect("save");

        // The core is shared, so a term only it carries matches all three.
        fixture.state.set_profile_filter("test core");
        assert_eq!(fixture.state.visible_rows().len(), 3);

        // The proxy belongs to one profile and is in none of their names.
        fixture.state.set_profile_filter("zurich");
        let visible = fixture.state.visible_rows();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].profile.id, shopping);

        // The label the row prints is not a field the filter reads, so a
        // profile with no proxy cannot answer to the word for having none.
        fixture.state.set_profile_filter("direct");
        assert!(
            fixture.state.visible_rows().is_empty(),
            "the filter matches what is stored, not what the row renders"
        );
    }

    #[test]
    fn a_filter_that_matches_nothing_hides_the_list_without_touching_a_profile() {
        let (mut fixture, _, _, _) = filtered_fixture();

        fixture.state.set_profile_filter("nothing is called this");

        assert!(fixture.state.visible_rows().is_empty());
        assert_eq!(
            fixture.state.rows().len(),
            3,
            "a filter is a view of the list, not a way to lose profiles"
        );
    }

    #[test]
    fn a_filtered_out_profile_keeps_its_focus_and_keeps_running() {
        let (mut fixture, work, _, _) = filtered_fixture();
        fixture.state.select(work);
        fixture.state.start(work).expect("start");
        let started = fixture
            .state
            .rows()
            .iter()
            .find(|row| row.profile.id == work)
            .expect("the row is listed")
            .state();
        assert!(
            matches!(started, RuntimeState::Running),
            "the fixture really starts it"
        );

        fixture.state.set_profile_filter("shopping");

        assert_eq!(
            fixture.state.selected_id(),
            Some(work),
            "the panel answers for what was chosen, not for what is listed"
        );
        let visible = fixture.state.visible_rows();
        assert_eq!(visible.len(), 1);
        assert_ne!(visible[0].profile.id, work);
        let after = fixture
            .state
            .rows()
            .iter()
            .find(|row| row.profile.id == work)
            .expect("still listed")
            .state();
        assert!(
            matches!(after, RuntimeState::Running),
            "hiding a row cannot stop the browser behind it"
        );
    }

    fn socks5(host: &str, port: u16) -> ProxyOutbound {
        ProxyOutbound::Socks5(domain::Socks5Outbound {
            host: host.to_string(),
            port,
            username: None,
            password: None,
        })
    }

    /// A switch reaches the words the window *writes* afterwards, not only the
    /// ones it looks up while rendering: a notice and a log line are built the
    /// moment the event happens, so they have to be built in the language in
    /// force then - and the file has to agree with the window, because the log
    /// outlives it.
    #[test]
    fn switching_the_language_reaches_what_the_window_writes_next() {
        let dir = std::env::temp_dir().join(format!("fp-app-language-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        // The log file is handed in rather than opened by the fixture, so the
        // test can read the bytes the window wrote.
        let log = crate::log_file::LogFile::open(&dir.join("logs")).expect("log file");
        let mut fixture = fixture_full(&dir.join("config.json"), None, Some(log), None);

        fixture
            .state
            .set_language(Lang::Zh)
            .expect("the choice is stored");
        assert_eq!(fixture.state.language(), Lang::Zh);

        fixture
            .state
            .set_theme(ThemeChoice::Light)
            .expect("the window repaints");
        let toast = fixture
            .state
            .toasts()
            .last()
            .expect("a toast")
            .message
            .clone();
        assert!(toast.contains("主题"), "{toast}");

        fixture
            .state
            .append_log(LogLevel::Warning, None, "a warning".to_string());
        let line = std::fs::read_to_string(dir.join("logs").join("activity.log"))
            .expect("the line reached the file");
        assert!(line.contains("警告"), "{line}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_created_proxy_is_listed_and_stored() {
        let mut fixture = fixture();
        let id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");

        let rows = fixture.state.proxy_rows().expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].proxy.name, "Office");
        assert_eq!(rows[0].endpoint(), "socks5://10.0.0.1:1080");
        assert_eq!(rows[0].usage_label(en()), "not assigned");
        assert!(fixture.proxies.get(id).expect("stored").is_some());
    }

    #[test]
    fn a_broken_proxy_is_refused_and_reported() {
        let mut fixture = fixture();
        assert!(
            fixture
                .state
                .create_proxy("Broken", socks5("", 1080))
                .is_err()
        );
        assert!(fixture.state.proxy_rows().expect("rows").is_empty());
        assert!(
            fixture.state.notice().is_some_and(|notice| notice.error),
            "the refusal is shown in the banner"
        );
    }

    #[test]
    fn the_picker_offers_every_stored_proxy() {
        let mut fixture = fixture();
        assert!(fixture.state.proxy_choices().is_empty());
        let id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create");
        assert_eq!(
            fixture.state.proxy_choices(),
            vec![(id, "Office".to_string())]
        );
    }

    #[test]
    fn assigning_a_proxy_reaches_storage_and_the_usage_list() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let profile_id = fixture.state.create_profile("Proxied").expect("create");
        let proxy_id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");

        let mut profile = fixture.state.profile(profile_id).expect("loaded");
        profile.proxy_id = Some(proxy_id);
        fixture.state.update_profile(profile).expect("assign");

        assert_eq!(
            fixture.proxies.get(proxy_id).expect("stored").map(|p| p.id),
            Some(proxy_id)
        );
        let rows = fixture.state.proxy_rows().expect("rows");
        assert_eq!(rows[0].used_by, vec!["Proxied".to_string()]);
        assert_eq!(rows[0].usage_label(en()), "used by Proxied");
        assert!(rows[0].is_used());
        assert_eq!(
            fixture
                .profiles
                .get(profile_id)
                .expect("stored")
                .and_then(|profile| profile.proxy_id),
            Some(proxy_id),
            "the assignment reached storage"
        );
        assert_eq!(
            fixture.state.rows()[0].proxy_name.as_deref(),
            Some("Office"),
            "the row shows the assigned proxy"
        );
    }

    /// Refused rather than quietly rewritten to Direct: the assignment would
    /// otherwise be nulled out and that profile's traffic would leave unproxied.
    #[test]
    fn a_proxy_in_use_cannot_be_deleted() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let profile_id = fixture.state.create_profile("Proxied").expect("create");
        let proxy_id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        let mut profile = fixture.state.profile(profile_id).expect("loaded");
        profile.proxy_id = Some(proxy_id);
        fixture.state.update_profile(profile).expect("assign");

        let error = fixture.state.delete_proxy(proxy_id).unwrap_err();
        assert!(error.to_string().contains("Proxied"), "{error}");
        assert_eq!(fixture.state.proxy_rows().expect("rows").len(), 1);
        assert_eq!(
            fixture
                .profiles
                .get(profile_id)
                .expect("stored")
                .and_then(|profile| profile.proxy_id),
            Some(proxy_id),
            "the assignment survived the refused delete"
        );
    }

    #[test]
    fn a_proxy_is_deleted_once_nothing_uses_it() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let profile_id = fixture.state.create_profile("Proxied").expect("create");
        let proxy_id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        let mut profile = fixture.state.profile(profile_id).expect("loaded");
        profile.proxy_id = Some(proxy_id);
        fixture.state.update_profile(profile).expect("assign");

        let mut unassigned = fixture.state.profile(profile_id).expect("loaded");
        unassigned.proxy_id = None;
        fixture.state.update_profile(unassigned).expect("unassign");
        fixture.state.delete_proxy(proxy_id).expect("delete");

        assert!(fixture.state.proxy_rows().expect("rows").is_empty());
        assert_eq!(
            fixture.state.rows()[0].proxy_name,
            None,
            "the row falls back to direct"
        );
    }

    #[test]
    fn saving_a_proxy_keeps_a_running_profile_on_what_it_started_with() {
        let mut fixture = fixture();
        let profile_id = running_profile(&mut fixture);
        let proxy_id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");

        let mut proxy = fixture.state.proxy(proxy_id).expect("stored");
        proxy.outbound = socks5("10.0.0.2", 1081);
        fixture.state.update_proxy(proxy).expect("save");

        assert_eq!(
            fixture.runtime.snapshot_of(profile_id).map(|s| s.state),
            Some(RuntimeState::Running),
            "editing a proxy does not disturb a running profile"
        );
        assert!(
            fixture.state.toasts().iter().any(|toast| toast
                .message
                .contains("next start")
                || toast.message.contains("keep")),
            "the toast says when the change takes effect"
        );
        assert!(
            fixture.state.notice().is_none(),
            "a success is not a banner: it is a toast and a log line"
        );
    }

    #[test]
    fn an_added_core_carries_the_version_its_binary_reported() {
        let mut fixture = fixture();
        let binary = CoreBinary::new("add", Some("Chromium 148.0.7778.215"));

        let id = fixture
            .state
            .add_core(None, binary.path_buf())
            .expect("add");

        let rows = fixture.state.core_rows().expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].core.id, id);
        assert_eq!(rows[0].core.version, "Chromium 148.0.7778.215");
        assert_eq!(rows[0].core.major, 148);
        assert!(rows[0].present, "the binary is on disk");
        assert_eq!(rows[0].usage_label(en()), "not used");
        assert_eq!(
            rows[0].generation_label().as_deref(),
            Some("Chrome 144+ · spoofing exclusions honoured")
        );
    }

    /// The pivot made visible: a core below it is labelled as not offering the
    /// switches this project only verified from 144.
    #[test]
    fn a_legacy_core_says_the_noise_switches_are_not_offered() {
        let mut fixture = fixture();
        let binary = CoreBinary::new("legacy", Some("Chromium 128.0.0.0"));
        fixture
            .state
            .add_core(Some("Old".to_string()), binary.path_buf())
            .expect("add");

        let rows = fixture.state.core_rows().expect("rows");
        assert_eq!(rows[0].core.name, "Old");
        assert_eq!(rows[0].core.major, 128);
        assert_eq!(
            rows[0].generation_label().as_deref(),
            Some("Chrome 143 and older · spoofing exclusions not honoured")
        );
    }

    #[test]
    fn a_binary_without_a_usable_version_is_refused_and_reported() {
        let mut fixture = fixture();
        let binary = CoreBinary::new("silent", None);

        let error = fixture.state.add_core(None, binary.path_buf()).unwrap_err();

        assert!(error.to_string().contains("--version"), "{error}");
        assert!(fixture.state.core_rows().expect("rows").is_empty());
        assert!(fixture.state.notice().is_some_and(|notice| notice.error));
    }

    /// A core whose version was never read has no capability table. Asking for
    /// one used to reach `CoreCapabilities::for_major(0)`, which is a debug
    /// assertion: in a debug build the window would have panicked.
    #[test]
    fn verifying_through_a_core_without_a_version_is_refused_not_asserted() {
        let mut fixture = fixture();
        let unknown = domain::BrowserCore {
            id: CoreId::new(),
            name: "unknown 0".to_string(),
            executable: PathBuf::from("/tmp/whatever/chrome"),
            version: "unknown".to_string(),
            major: 0,
        };
        fixture.cores.save(&unknown).expect("save core");
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("unreadable").expect("create");
        fixture.runtime.set_state(id, RuntimeState::Running);
        fixture.runtime.set_cdp_port(id, 9333);
        fixture.state.refresh_runtime();

        let error = fixture.state.begin_verification(id).unwrap_err();
        assert!(error.to_string().contains("no detected version"), "{error}");
        assert_eq!(
            fixture.state.core_rows().expect("rows")[0].generation_label(),
            None,
            "there is no generation to show"
        );
    }

    #[test]
    fn re_detecting_a_replaced_binary_updates_the_major_and_the_label() {
        let mut fixture = fixture();
        let binary = CoreBinary::new("redetect", Some("Chromium 148.0.7778.215"));
        let id = fixture
            .state
            .add_core(None, binary.path_buf())
            .expect("add");

        // The same path now answers with another build.
        binary.replace(Some("Chromium 128.0.0.0"));
        fixture.state.redetect_core(id).expect("redetect");

        let rows = fixture.state.core_rows().expect("rows");
        assert_eq!(rows[0].core.major, 128);
        assert_eq!(
            rows[0].generation_label().as_deref(),
            Some("Chrome 143 and older · spoofing exclusions not honoured")
        );
        assert!(
            fixture
                .state
                .toasts()
                .iter()
                .any(|toast| toast.message.contains("128")),
            "the toast says what it found"
        );
    }

    #[test]
    fn renaming_a_core_keeps_its_version() {
        let mut fixture = fixture();
        let binary = CoreBinary::new("rename", Some("Chromium 148.0.7778.215"));
        let id = fixture
            .state
            .add_core(None, binary.path_buf())
            .expect("add");

        let mut core = fixture.state.core(id).expect("stored");
        core.name = "Work browser".to_string();
        fixture.state.update_core(core).expect("update");

        let rows = fixture.state.core_rows().expect("rows");
        assert_eq!(rows[0].core.name, "Work browser");
        assert_eq!(rows[0].core.major, 148);
    }

    #[test]
    fn a_core_a_profile_launches_with_cannot_be_deleted() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let core_id = fixture.state.core_rows().expect("rows")[0].core.id;
        fixture.state.create_profile("Uses it").expect("create");

        let error = fixture.state.delete_core(core_id).unwrap_err();
        assert!(error.to_string().contains("Uses it"), "{error}");
        assert_eq!(fixture.state.core_rows().expect("rows").len(), 1);
    }

    #[test]
    fn an_unused_core_is_deleted() {
        let mut fixture = fixture();
        let binary = CoreBinary::new("unused", Some("Chromium 148.0.7778.215"));
        let id = fixture
            .state
            .add_core(None, binary.path_buf())
            .expect("add");
        fixture.state.delete_core(id).expect("delete");
        assert!(fixture.state.core_rows().expect("rows").is_empty());
        assert!(!fixture.state.has_core());
    }

    #[test]
    fn the_settings_rows_say_where_each_value_came_from() {
        let fixture = fixture();
        let rows = fixture.state.setting_rows();

        assert_eq!(
            rows.len(),
            SettingKey::ALL.len(),
            "every setting is listed, whatever the list grows to"
        );
        let data_dir = rows
            .iter()
            .find(|row| row.key == SettingKey::DataDir)
            .expect("the data directory row");
        assert_eq!(data_dir.source, crate::settings::Source::Default);
        assert_eq!(data_dir.source_label(en()), "from the default");
        assert!(
            !data_dir.key.editable(),
            "the config file lives in the data directory, so the directory is \
             chosen by the environment or the platform, not from inside it"
        );
        assert_eq!(data_dir.key.effect().label(en()), "next start");
        assert!(
            data_dir.note.is_some(),
            "the row says how to move it, because the row cannot do it"
        );

        let chromium = rows
            .iter()
            .find(|row| row.key == SettingKey::ChromiumBin)
            .expect("the chromium row");
        assert!(
            !chromium.key.editable(),
            "the binary is chosen by the environment and shown on the cores page"
        );

        let endpoint = rows
            .iter()
            .find(|row| row.key == SettingKey::EchoUrl)
            .expect("the proxy test endpoint row");
        assert!(
            endpoint.key.editable(),
            "the endpoint a proxy test asks is the user's to choose"
        );
        assert_eq!(
            endpoint.key.effect().label(en()),
            "now",
            "a test asks whatever the endpoint is at the time, not what it was at startup"
        );
        assert!(
            endpoint.source == crate::settings::Source::Default,
            "the fixture sets no endpoint, so the built-in one is shown"
        );
    }

    #[test]
    fn saving_a_setting_stores_it_and_says_when_it_applies() {
        let dir = std::env::temp_dir().join(format!("fp-app-settings-{}", CoreId::new()));
        let config = dir.join("config.json");
        let mut fixture = fixture_with_config(&config);

        fixture
            .state
            .update_setting(SettingKey::XrayExecutable, "/opt/xray")
            .expect("save");

        let stored = std::fs::read_to_string(&config).expect("the config file was written");
        assert!(stored.contains("/opt/xray"), "{stored}");
        let toast = fixture
            .state
            .toasts()
            .last()
            .expect("a toast saying what happened");
        assert!(
            toast.message.contains("next start"),
            "the toast says when it applies: {}",
            toast.message
        );
        assert!(
            fixture.state.notice().is_none(),
            "saving a setting is not a problem, so the banner stays clear"
        );
        assert_eq!(
            fixture
                .state
                .setting_rows()
                .iter()
                .find(|row| row.key == SettingKey::XrayExecutable)
                .expect("the row")
                .value,
            "/opt/xray",
            "the page shows what the next start will use"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The appearance is stored and reported. It is read back out of the state
    /// rather than only out of the file, because the state is what the window
    /// paints from: a change that reached the file alone would leave the window
    /// showing one thing while the file said another.
    #[test]
    fn choosing_an_appearance_stores_it_and_says_so() {
        let dir = std::env::temp_dir().join(format!("fp-app-theme-{}", CoreId::new()));
        let config = dir.join("config.json");
        let mut fixture = fixture_with_config(&config);
        assert_eq!(fixture.state.theme(), ThemeChoice::Dark, "the default");

        fixture.state.set_theme(ThemeChoice::Light).expect("save");

        assert_eq!(fixture.state.theme(), ThemeChoice::Light);
        let stored = std::fs::read_to_string(&config).expect("the config file was written");
        assert!(stored.contains("\"light\""), "{stored}");
        let toast = fixture.state.toasts().last().expect("a toast saying so");
        assert!(toast.message.contains("light theme"), "{}", toast.message);
        assert!(
            fixture.state.notice().is_none(),
            "choosing an appearance is not a problem, so the banner stays clear"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_setting_is_reported_and_changes_nothing() {
        let mut fixture = fixture();
        fixture
            .state
            .update_setting(SettingKey::DataDir, "   ")
            .expect_err("an empty value is refused");
        assert!(fixture.state.notice().is_some_and(|notice| notice.error));
        assert_eq!(
            fixture
                .state
                .setting_rows()
                .iter()
                .find(|row| row.key == SettingKey::DataDir)
                .expect("the row")
                .source,
            crate::settings::Source::Default,
            "nothing was stored"
        );
    }

    #[test]
    fn the_page_can_be_switched() {
        let mut fixture = fixture();
        assert_eq!(fixture.state.page(), Page::Profiles);
        fixture.state.set_page(Page::Proxies);
        assert_eq!(fixture.state.page(), Page::Proxies);
        assert!(Page::Proxies.is_ready());
        assert!(
            Page::Settings.is_ready(),
            "every page in the sidebar is built now"
        );
    }

    /// A running profile with a debug port, ready to be verified.
    fn running_profile(fixture: &mut Fixture) -> ProfileId {
        seed_core(fixture);
        fixture.state.load().expect("load");
        let id = fixture
            .state
            .create_profile("verify me")
            .expect("create profile");
        fixture.runtime.set_state(id, RuntimeState::Running);
        fixture.runtime.set_cdp_port(id, 9333);
        fixture.state.refresh_runtime();
        id
    }

    /// A running profile whose traffic is meant to leave by a proxy.
    fn proxied_profile(fixture: &mut Fixture) -> (ProfileId, ProxyId) {
        let id = running_profile(fixture);
        let proxy_id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        let mut profile = fixture.state.profile(id).expect("the profile is loaded");
        profile.proxy_id = Some(proxy_id);
        fixture.state.update_profile(profile).expect("assign");
        (id, proxy_id)
    }

    /// The address question is only put to a profile that makes a claim about
    /// one, and the expectation behind it is what the proxy was *measured* at.
    #[test]
    fn a_proxied_profile_is_asked_about_the_address_its_proxy_was_tested_at() {
        let mut fixture = fixture();
        let (id, proxy_id) = proxied_profile(&mut fixture);
        fixture.state.finish_proxy_test(
            proxy_id,
            false,
            Ok(Diagnosis {
                exit_ip: "203.0.113.7".to_string(),
                elapsed: Duration::from_millis(120),
            }),
        );

        let job = fixture.state.begin_verification(id).expect("begin");

        let egress = job.egress.expect("a proxied profile is asked");
        // The endpoint is the one the Settings page shows, so the reading can
        // never be taken against something the user cannot see.
        let shown = fixture
            .state
            .setting_rows()
            .iter()
            .find(|row| row.key == SettingKey::EchoUrl)
            .map(|row| row.value.clone())
            .expect("the endpoint row");
        assert_eq!(egress.echo_url, shown);
        assert_eq!(
            egress.expected.as_deref(),
            Some("203.0.113.7"),
            "the expectation is the address the proxy was tested at"
        );
    }

    /// A profile that was never meant to leave by a proxy claims nothing about
    /// an address, and the endpoint learns the address it is asked from: there
    /// is no reason to spend that on a browser with nothing to check.
    #[test]
    fn a_profile_without_a_proxy_is_not_asked_about_an_address() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);

        let job = fixture.state.begin_verification(id).expect("begin");

        assert!(job.egress.is_none());
    }

    /// A reading is still worth having before the proxy has been tested - it is
    /// the first thing anyone wants to know - but with nothing measured there
    /// is no claim for it to contradict.
    #[test]
    fn an_untested_proxy_leaves_nothing_to_disagree_with() {
        let mut fixture = fixture();
        let (id, _) = proxied_profile(&mut fixture);

        let job = fixture.state.begin_verification(id).expect("begin");

        let egress = job.egress.expect("a proxied profile is asked");
        assert_eq!(egress.expected, None);
    }

    /// The half of the answer the row cannot show: the row says the fingerprint
    /// was confirmed, and this is where the traffic went.
    #[test]
    fn a_confirmed_reading_is_logged_with_the_address_it_left_from() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);
        fixture.state.finish_verification(
            id,
            Ok(VerificationReport {
                exit_ip: Some("203.0.113.7".to_string()),
                ..VerificationReport::default()
            }),
        );

        let line = confirmed_line_in(&fixture);

        assert!(line.contains("203.0.113.7"), "{line}");
    }

    /// "Confirmed" on its own would read as though the address had been checked
    /// too, so a reading that was not taken says so in the same line.
    #[test]
    fn a_confirmed_reading_that_could_not_read_an_address_says_so() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);
        fixture.state.finish_verification(
            id,
            Ok(VerificationReport {
                exit_unreadable: Some("the browser's page never committed".to_string()),
                ..VerificationReport::default()
            }),
        );

        let line = confirmed_line_in(&fixture);

        assert!(line.contains("exit address was not read"), "{line}");
        assert!(line.contains("never committed"), "{line}");
    }

    /// The log line for a verification that found nothing wrong.
    fn confirmed_line_in(fixture: &Fixture) -> String {
        fixture
            .state
            .log_entries()
            .iter()
            .map(|entry| entry.message.clone())
            .find(|message| message.starts_with("fingerprint confirmed"))
            .unwrap_or_else(|| {
                panic!(
                    "no record of a confirmed reading: {:?}",
                    fixture
                        .state
                        .log_entries()
                        .iter()
                        .map(|entry| entry.message.clone())
                        .collect::<Vec<_>>()
                )
            })
    }

    /// A wrong address is a finding, not a footnote: the profile claims to
    /// leave by a proxy that was measured somewhere else.
    #[test]
    fn a_profile_that_leaves_from_elsewhere_is_not_confirmed() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);
        fixture.state.finish_verification(
            id,
            Ok(VerificationReport {
                discrepancies: vec![Discrepancy {
                    claim: "exit address",
                    expected: "203.0.113.7 (where the proxy was tested)".to_string(),
                    observed: "198.51.100.9".to_string(),
                }],
                exit_ip: Some("198.51.100.9".to_string()),
                exit_unreadable: None,
            }),
        );

        let verification = fixture.state.verification(id).expect("recorded");

        assert_eq!(verification.label(en()), "1 claim not confirmed");
        assert_eq!(
            verification
                .report()
                .and_then(|report| report.exit_label(en())),
            Some("traffic left from 198.51.100.9".to_string()),
            "the address is still worth showing: it is where the traffic went"
        );
    }

    #[test]
    fn a_verification_needs_a_running_browser_with_a_debug_port() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture
            .state
            .create_profile("stopped")
            .expect("create profile");

        assert!(fixture.state.begin_verification(id).is_err());
        assert!(fixture.state.verification(id).is_none());
    }

    #[test]
    fn a_verification_job_carries_the_profile_and_its_capabilities() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);

        let job = fixture.state.begin_verification(id).expect("begin");

        assert_eq!(job.port, 9333);
        assert_eq!(job.profile_id, id);
        assert_eq!(
            job.profile.seed,
            fixture.state.rows()[0].profile.fingerprint.seed
        );
        assert_eq!(
            job.capabilities.major, 144,
            "the capabilities come from the profile's own core"
        );
        assert!(
            fixture
                .state
                .verification(id)
                .is_some_and(Verification::is_running)
        );
    }

    #[test]
    fn a_second_verification_of_the_same_profile_is_refused() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);
        fixture.state.begin_verification(id).expect("first");

        assert!(fixture.state.begin_verification(id).is_err());
    }

    #[test]
    fn an_outcome_records_confirmation_disagreement_or_failure() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);

        fixture
            .state
            .finish_verification(id, Ok(VerificationReport::default()));
        assert!(matches!(
            fixture.state.verification(id),
            Some(Verification::Confirmed(_))
        ));

        let disagreement = Discrepancy {
            claim: "platform",
            expected: "Win32".to_string(),
            observed: "Linux x86_64".to_string(),
        };
        fixture.state.finish_verification(
            id,
            Ok(VerificationReport::from_discrepancies(vec![
                disagreement.clone(),
            ])),
        );
        let recorded = fixture.state.verification(id).expect("recorded");
        assert_eq!(
            recorded.disagreements(),
            std::slice::from_ref(&disagreement)
        );
        assert_eq!(recorded.label(en()), "1 claim not confirmed");

        fixture
            .state
            .finish_verification(id, Err("no debug port".to_string()));
        let failed = fixture.state.verification(id).expect("recorded");
        assert_eq!(failed.failure(), Some("no debug port"));
        assert!(
            !failed.is_running(),
            "a failed reading is not a reading in flight"
        );
    }

    #[test]
    fn stopping_or_restarting_a_profile_drops_a_stale_reading() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);
        fixture
            .state
            .finish_verification(id, Ok(VerificationReport::default()));
        assert!(fixture.state.verification(id).is_some());

        fixture.state.restart(id).expect("restart");
        assert!(
            fixture.state.verification(id).is_none(),
            "a restarted browser has a new fingerprint"
        );

        fixture
            .state
            .finish_verification(id, Ok(VerificationReport::default()));
        fixture.state.stop(id).expect("stop");
        assert!(fixture.state.verification(id).is_none());
    }

    /// A proxy has to be judgeable before anything is launched with it, so a
    /// test needs no running profile - it starts an engine of its own.
    #[test]
    fn a_proxy_can_be_tested_with_nothing_running() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");

        let job = fixture.state.begin_proxy_test(id).expect("begin");

        assert_eq!(job.proxy_id, id);
        assert_eq!(job.proxy.name, "Office");
        assert_eq!(
            job.live_port, None,
            "nothing is running, so the test starts its own engine"
        );
        assert!(!job.is_live());
        assert_eq!(
            job.echo_url,
            fixture.state.setting_rows()[2].value,
            "the endpoint is the configured one"
        );
        assert!(
            fixture
                .state
                .proxy_test(id)
                .is_some_and(ProxyTest::is_running)
        );
    }

    /// A test of a proxy that is not stored is a mistake, not a job.
    #[test]
    fn testing_a_proxy_that_is_not_there_is_refused() {
        let mut fixture = fixture();
        assert!(fixture.state.begin_proxy_test(ProxyId::new()).is_err());
    }

    #[test]
    fn a_second_test_of_the_same_proxy_is_refused() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");

        fixture.state.begin_proxy_test(id).expect("first");
        assert!(
            fixture.state.begin_proxy_test(id).is_err(),
            "a second engine for an answer already on its way"
        );

        fixture.state.finish_proxy_test(
            id,
            false,
            Err(Fault::new(FaultClass::Unreachable, "no route")),
        );
        assert!(
            fixture.state.begin_proxy_test(id).is_ok(),
            "the slot is free once the answer is in"
        );
    }

    /// When a profile is already up, its engine is the one that would have to
    /// forward, so the test asks that engine instead of starting a second one.
    #[test]
    fn a_running_profile_makes_the_test_probe_its_engine() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let proxy_id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        let profile_id = fixture.state.create_profile("Proxied").expect("create");
        let mut profile = fixture.state.profile(profile_id).expect("loaded");
        profile.proxy_id = Some(proxy_id);
        fixture.state.update_profile(profile).expect("assign");
        fixture.runtime.set_state(profile_id, RuntimeState::Running);
        fixture.runtime.set_socks_port(profile_id, 51234);
        fixture.state.refresh_runtime();

        let job = fixture.state.begin_proxy_test(proxy_id).expect("begin");

        assert_eq!(
            job.live_port,
            Some(51234),
            "the engine a profile is using is the one that has to carry the request"
        );
        assert!(job.is_live());
    }

    /// The port is cleared when a profile stops, so a test cannot probe a port
    /// that nothing is listening on and call the answer a proxy failure.
    #[test]
    fn a_stopped_profile_leaves_no_port_to_probe() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let proxy_id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        let profile_id = fixture.state.create_profile("Proxied").expect("create");
        let mut profile = fixture.state.profile(profile_id).expect("loaded");
        profile.proxy_id = Some(proxy_id);
        fixture.state.update_profile(profile).expect("assign");
        fixture.runtime.set_state(profile_id, RuntimeState::Running);
        fixture.runtime.set_socks_port(profile_id, 51234);
        fixture.state.refresh_runtime();

        fixture.runtime.set_state(profile_id, RuntimeState::Stopped);
        fixture.state.refresh_runtime();

        // The snapshot still holds the port, which is exactly the trap: it is
        // the profile's own state that decides, so a stale port is never
        // dialled and a dead socket is never reported as a broken proxy.
        assert_eq!(
            fixture
                .runtime
                .snapshot_of(profile_id)
                .expect("snapshot")
                .socks_port,
            Some(51234),
            "the port outlives the profile in the snapshot"
        );

        let job = fixture.state.begin_proxy_test(proxy_id).expect("begin");
        assert_eq!(job.live_port, None);
    }

    #[test]
    fn an_outcome_records_the_address_and_the_path_it_left_by() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        fixture.state.begin_proxy_test(id).expect("begin");

        fixture.state.finish_proxy_test(
            id,
            true,
            Ok(Diagnosis {
                exit_ip: "198.51.100.9".to_string(),
                elapsed: Duration::from_millis(431),
            }),
        );

        let test = fixture.state.proxy_test(id).expect("a result");
        let reading = test.reading().expect("a reading, not a fault");
        assert_eq!(reading.exit_ip, "198.51.100.9");
        assert_eq!(reading.elapsed, Duration::from_millis(431));
        assert!(reading.live, "the request went through the running engine");
        assert_eq!(test.label(en()), "exit 198.51.100.9");
        assert!(test.fault().is_none());
    }

    /// A fault is not a reading with an empty address: it says where the
    /// request stopped, because that is what decides the fix.
    #[test]
    fn a_failed_test_keeps_the_class_and_the_evidence() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        fixture.state.begin_proxy_test(id).expect("begin");

        fixture.state.finish_proxy_test(
            id,
            false,
            Err(Fault::new(
                FaultClass::Auth,
                "upstream refused the credentials it was offered",
            )),
        );

        let test = fixture.state.proxy_test(id).expect("a result");
        assert!(
            test.reading().is_none(),
            "nothing left, so nothing was read"
        );
        let fault = test.fault().expect("the fault");
        assert_eq!(fault.class, FaultClass::Auth);
        assert!(fault.detail.contains("credentials"), "{fault}");
        assert_eq!(test.label(en()), "no traffic (authentication)");
        assert!(
            test.detail(en())
                .expect("a detail line")
                .contains("no traffic reached the endpoint"),
            "the row and the log both have to say nothing arrived"
        );
    }

    /// The address and the path are the record: a row is replaced by the next
    /// test, and the log is not.
    #[test]
    fn the_activity_log_keeps_the_address_the_path_and_the_class() {
        let dir = std::env::temp_dir().join(format!("fp-proxy-test-log-{}", CoreId::new()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut fixture = fixture_with_log(&dir);
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let ok = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        let bad = fixture
            .state
            .create_proxy("Home", socks5("10.0.0.2", 1080))
            .expect("create proxy");

        fixture.state.begin_proxy_test(ok).expect("begin");
        fixture.state.finish_proxy_test(
            ok,
            false,
            Ok(Diagnosis {
                exit_ip: "198.51.100.9".to_string(),
                elapsed: Duration::from_millis(90),
            }),
        );
        fixture.state.begin_proxy_test(bad).expect("begin");
        fixture.state.finish_proxy_test(
            bad,
            false,
            Err(Fault::new(
                FaultClass::Unreachable,
                "no route to the upstream",
            )),
        );

        let lines: Vec<String> = fixture
            .state
            .log_entries()
            .iter()
            .map(|entry| entry.message.clone())
            .collect();
        // `Created proxy Office` is also a line about Office, so the search is
        // for the test's own record rather than for the name.
        let mention = |name: &str| {
            lines
                .iter()
                .find(|line| line.starts_with("proxy test:") && line.contains(name))
                .unwrap_or_else(|| panic!("no record of testing {name}: {lines:?}"))
        };
        let passed = mention("Office");
        assert!(passed.contains("198.51.100.9"), "{passed}");
        assert!(
            passed.contains("temporary engine"),
            "the path is the half of the answer the row cannot keep: {passed}"
        );
        let failed = mention("Home");
        assert!(failed.contains("unreachable"), "{failed}");
        assert!(failed.contains("no route"), "{failed}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The class is the whole point of keeping them apart, so it is what the
    /// immediate feedback leads with.
    #[test]
    fn a_failure_toasts_the_class_and_a_success_the_address() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");

        fixture.state.begin_proxy_test(id).expect("begin");
        fixture.state.finish_proxy_test(
            id,
            false,
            Err(Fault::new(
                FaultClass::Timeout,
                "the request ran out of time",
            )),
        );
        let toast = fixture.state.toasts().last().expect("a toast").clone();
        assert_eq!(toast.kind, ToastKind::Error);
        assert!(toast.message.contains("timeout"), "{}", toast.message);

        fixture.state.begin_proxy_test(id).expect("begin again");
        fixture.state.finish_proxy_test(
            id,
            false,
            Ok(Diagnosis {
                exit_ip: "198.51.100.9".to_string(),
                elapsed: Duration::from_millis(90),
            }),
        );
        let toast = fixture.state.toasts().last().expect("a toast").clone();
        assert_eq!(toast.kind, ToastKind::Success);
        assert!(toast.message.contains("198.51.100.9"), "{}", toast.message);
    }

    /// A result is about one upstream. Editing it leaves the old answer sitting
    /// under a new address, which reads as a claim about the new one.
    #[test]
    fn editing_a_proxy_forgets_what_the_old_one_answered() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        fixture.state.begin_proxy_test(id).expect("begin");
        fixture.state.finish_proxy_test(
            id,
            false,
            Ok(Diagnosis {
                exit_ip: "198.51.100.9".to_string(),
                elapsed: Duration::from_millis(90),
            }),
        );
        assert!(fixture.state.proxy_test(id).is_some());

        let mut proxy = fixture.state.proxy(id).expect("stored");
        proxy.outbound = socks5("10.0.0.2", 1080);
        fixture.state.update_proxy(proxy).expect("edit");

        assert!(
            fixture.state.proxy_test(id).is_none(),
            "the address it left from was the old upstream's"
        );
    }

    #[test]
    fn deleting_a_proxy_forgets_its_result() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");
        fixture.state.begin_proxy_test(id).expect("begin");
        fixture.state.finish_proxy_test(
            id,
            false,
            Ok(Diagnosis {
                exit_ip: "198.51.100.9".to_string(),
                elapsed: Duration::from_millis(90),
            }),
        );

        fixture.state.delete_proxy(id).expect("delete");
        assert!(fixture.state.proxy_test(id).is_none());
    }

    #[test]
    fn create_lists_the_profile_and_selects_it() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("initial load");

        let id = fixture.state.create_profile("Primary").expect("create");

        assert_eq!(fixture.state.rows().len(), 1);
        let row = &fixture.state.rows()[0];
        assert_eq!(row.profile.name, "Primary");
        assert_eq!(row.profile.id, id);
        assert_eq!(row.state(), RuntimeState::Stopped);
        assert_eq!(row.state_label(en()), "Stopped");
        assert_eq!(row.core_name, "Test Core 144");
        assert_eq!(fixture.state.selected_id(), Some(id));
        assert!(row.can_start());
        assert!(!row.can_stop());
    }

    #[test]
    fn create_without_a_core_records_an_error_notice() {
        let mut fixture = fixture();
        fixture.state.load().expect("initial load");

        let error = fixture
            .state
            .create_profile("Primary")
            .expect_err("must fail");

        assert!(matches!(error, AppError::Conflict(_)));
        assert!(fixture.state.rows().is_empty());
        let notice = fixture.state.notice().expect("notice");
        assert!(notice.error);
        assert!(notice.message.contains("no browser core"));
    }

    #[test]
    fn start_and_stop_update_the_row_from_the_snapshot() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Primary").expect("create");

        fixture.state.start(id).expect("start");
        let row = fixture.state.selected().expect("selected row");
        assert_eq!(row.state(), RuntimeState::Running);
        assert!(row.can_stop());
        assert!(!row.can_start());

        fixture.state.stop(id).expect("stop");
        assert_eq!(
            fixture.state.selected().expect("selected row").state(),
            RuntimeState::Stopped
        );

        let commands = fixture.runtime.commands.lock().expect("commands");
        assert_eq!(
            commands.as_slice(),
            [format!("start:{id}"), format!("stop:{id}")]
        );
    }

    #[test]
    fn starting_an_unknown_profile_records_an_error_notice() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");

        let unknown = ProfileId::new();
        let error = fixture.state.start(unknown).expect_err("must fail");

        assert!(matches!(error, AppError::NotFound(_)));
        assert!(fixture.state.notice().expect("notice").error);
    }

    #[test]
    fn refresh_runtime_picks_up_external_state_changes() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Primary").expect("create");

        // Simulate a supervisor-side crash that the UI never received as an event.
        fixture.runtime.set_state(
            id,
            RuntimeState::Crashed {
                message: "browser exited".to_string(),
            },
        );
        fixture.state.refresh_runtime();

        let row = fixture.state.selected().expect("selected row");
        assert_eq!(row.state_label(en()), "Crashed");
        assert_eq!(row.state_message(), Some("browser exited"));
        assert!(row.can_start());
    }

    #[test]
    fn load_drops_a_selection_whose_profile_was_deleted() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Primary").expect("create");

        fixture.profiles.delete(id).expect("delete");
        fixture.state.load().expect("reload");

        assert!(fixture.state.rows().is_empty());
        assert_eq!(fixture.state.selected_id(), None);
    }

    #[test]
    fn rows_are_sorted_by_name_and_renumbered_for_new_profiles() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");

        assert_eq!(fixture.state.next_profile_name(), "Profile 1");
        fixture.state.create_profile("Bravo").expect("create");
        assert_eq!(fixture.state.next_profile_name(), "Profile 2");
        fixture.state.create_profile("Alpha").expect("create");

        let names: Vec<&str> = fixture
            .state
            .rows()
            .iter()
            .map(|row| row.profile.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "Bravo"]);
    }

    #[test]
    fn a_success_is_a_toast_and_leaves_the_banner_clear() {
        let mut fixture = fixture();

        fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create proxy");

        let toast = fixture.state.toasts().last().expect("a toast");
        assert_eq!(toast.kind, ToastKind::Success);
        assert!(toast.message.contains("Office"), "{}", toast.message);
        assert!(
            fixture.state.notice().is_none(),
            "a success does not need an acknowledgement"
        );
    }

    #[test]
    fn a_problem_owns_the_banner_until_it_is_dismissed() {
        let mut fixture = fixture();

        fixture
            .state
            .create_proxy("Broken", socks5("", 1080))
            .expect_err("refused");

        let notice = fixture.state.notice().expect("the banner");
        assert!(notice.error);
        assert_eq!(
            fixture.state.toasts().last().map(|toast| toast.kind),
            Some(ToastKind::Error),
            "the problem is a toast as well, so it is seen while it happens"
        );

        // A later success clears the problem: the banner is the current
        // problem, not every problem ever seen.
        fixture.state.dismiss_notice();
        assert!(fixture.state.notice().is_none());
        fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create");
        assert!(fixture.state.notice().is_none());
    }

    #[test]
    fn draining_toasts_leaves_nothing_to_show_twice() {
        let mut fixture = fixture();
        fixture
            .state
            .create_proxy("Office", socks5("10.0.0.1", 1080))
            .expect("create");
        assert!(!fixture.state.toasts().is_empty());

        let drained = fixture.state.drain_toasts();

        assert_eq!(drained.len(), 1);
        assert!(drained[0].message.contains("Office"));
        assert!(fixture.state.toasts().is_empty());
        assert!(fixture.state.drain_toasts().is_empty());
    }

    #[test]
    fn every_notice_is_written_to_the_log() {
        let mut fixture = fixture();
        assert!(fixture.state.log_entries().is_empty());

        fixture.state.push_notice("Saved.", false);
        fixture.state.push_notice("it broke", true);

        let entries = fixture.state.log_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].level, LogLevel::Info);
        assert_eq!(entries[0].message, "Saved.");
        assert_eq!(entries[0].profile_id, None, "a window-level line");
        assert_eq!(entries[1].level, LogLevel::Error);
        assert_eq!(entries[1].message, "it broke");
    }

    #[test]
    fn runtime_events_are_written_to_the_log_once_each() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);
        let profile_id = id;
        // The profile creation is its own line; this test is about events.
        fixture.state.clear_log();

        for event in [
            RuntimeEvent::StateChanged {
                profile_id,
                state: RuntimeState::Starting,
            },
            RuntimeEvent::EffectiveLaunchArgs {
                profile_id,
                args: vec!["a".into(), "b".into(), "c".into()],
            },
            RuntimeEvent::Started {
                profile_id,
                browser_pid: 4242,
                xray_pid: Some(4343),
                cdp_port: 9222,
                socks_port: Some(1080),
            },
            RuntimeEvent::StateChanged {
                profile_id,
                state: RuntimeState::Running,
            },
            RuntimeEvent::Warning {
                profile_id,
                message: "legacy core".into(),
            },
            RuntimeEvent::Stopped { profile_id },
            RuntimeEvent::StateChanged {
                profile_id,
                state: RuntimeState::Stopped,
            },
        ] {
            fixture.state.record_event(&event);
        }

        let messages: Vec<(&str, String)> = fixture
            .state
            .log_entries()
            .iter()
            .map(|entry| (entry.level.label(en()), entry.message.clone()))
            .collect();
        assert_eq!(
            messages,
            [
                ("info", "starting".to_string()),
                ("info", "launching with 3 arguments".to_string()),
                (
                    "info",
                    "browser started (pid 4242, cdp port 9222, socks port 1080, xray pid 4343)"
                        .to_string()
                ),
                ("warning", "legacy core".to_string()),
                ("info", "browser stopped".to_string()),
            ],
            "running and stopped state changes are the events' own lines, not extra ones"
        );
        assert!(
            fixture
                .state
                .log_entries()
                .iter()
                .all(|entry| entry.profile_id == Some(profile_id))
        );
    }

    #[test]
    fn a_crash_and_a_refused_start_are_logged_as_errors() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Primary").expect("create");
        // Creating the profile is its own (informational) line.
        fixture.state.clear_log();

        fixture.state.record_event(&RuntimeEvent::Crashed {
            profile_id: id,
            component: RuntimeComponent::Xray,
            message: "process exited unexpectedly: signal: 11".into(),
        });
        fixture.state.record_event(&RuntimeEvent::StateChanged {
            profile_id: id,
            state: RuntimeState::Failed {
                message: "no browser core".into(),
            },
        });

        let entries = fixture.state.log_entries();
        assert_eq!(entries[0].level, LogLevel::Error);
        assert_eq!(
            entries[0].message,
            "xray crashed: process exited unexpectedly: signal: 11"
        );
        assert_eq!(entries[1].level, LogLevel::Error);
        assert_eq!(entries[1].message, "failed: no browser core");
    }

    #[test]
    fn a_failed_reading_is_logged_as_an_error() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);

        fixture
            .state
            .finish_verification(id, Err("no debug port".to_string()));

        let entry = fixture.state.log_entries().last().expect("a line");
        assert_eq!(entry.level, LogLevel::Error);
        assert!(entry.message.contains("no debug port"), "{}", entry.message);
    }

    #[test]
    fn the_log_is_capped_and_the_newest_line_survives() {
        let mut fixture = fixture();

        for index in 0..LOG_CAPACITY + 100 {
            fixture.state.push_notice(format!("line {index}"), false);
        }

        let entries = fixture.state.log_entries();
        assert_eq!(entries.len(), LOG_CAPACITY);
        assert_eq!(entries[0].message, "line 100", "the oldest lines fell off");
        assert_eq!(
            entries[LOG_CAPACITY - 1].message,
            format!("line {}", LOG_CAPACITY + 99)
        );
    }

    #[test]
    fn log_rows_name_the_profile_and_put_the_newest_line_first() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);
        fixture.state.clear_log();

        fixture
            .state
            .record_event(&RuntimeEvent::Stopped { profile_id: id });
        fixture.state.push_notice("Saved.", false);

        let rows = fixture.state.log_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].who, "app", "the newest line is a window-level one");
        assert_eq!(rows[0].message, "Saved.");
        assert_eq!(rows[1].who, "verify me", "the profile is named, not its id");
        assert_eq!(rows[1].message, "browser stopped");

        fixture.state.clear_log();
        assert!(fixture.state.log_rows().is_empty());
        assert!(
            fixture.state.notice().is_none(),
            "clearing the history does not silence a current problem"
        );
    }

    #[test]
    fn the_panel_log_tail_is_one_profile_and_newest_first() {
        let mut fixture = fixture();
        let id = running_profile(&mut fixture);
        fixture.state.clear_log();
        let other = ProfileId::new();

        fixture.state.push_notice("app level", false);
        fixture
            .state
            .record_event(&RuntimeEvent::Stopped { profile_id: id });
        fixture.state.record_event(&RuntimeEvent::Warning {
            profile_id: other,
            message: "another profile".into(),
        });
        fixture.state.record_event(&RuntimeEvent::Warning {
            profile_id: id,
            message: "this profile".into(),
        });

        let tail = fixture.state.log_tail(id);
        assert_eq!(tail.len(), 2, "{tail:?}");
        assert_eq!(tail[0].message, "this profile", "newest first");
        assert_eq!(tail[0].who, "verify me", "and named");
        assert_eq!(tail[1].message, "browser stopped");
        assert!(
            tail.iter().all(|row| row.message != "app level"),
            "the panel shows this profile only; the Log page has everything"
        );
    }

    #[test]
    fn the_details_panel_starts_on_details() {
        let mut fixture = fixture();
        assert_eq!(fixture.state.details_tab(), DetailsTab::Details);
        fixture.state.set_details_tab(DetailsTab::Args);
        assert_eq!(fixture.state.details_tab(), DetailsTab::Args);
    }

    #[test]
    fn the_log_page_filter_hides_lines_without_losing_them() {
        let mut fixture = fixture();
        fixture.state.push_notice("Saved.", false);
        fixture.state.push_notice("careful", true);
        let id = running_profile(&mut fixture);
        fixture.state.record_event(&RuntimeEvent::Warning {
            profile_id: id,
            message: "legacy core".into(),
        });

        assert_eq!(fixture.state.log_filter(), LogFilter::All);
        assert_eq!(fixture.state.log_rows().len(), fixture.state.log_len());

        fixture.state.set_log_filter(LogFilter::Warnings);
        let warnings: Vec<&str> = fixture
            .state
            .log_rows()
            .iter()
            .map(|row| row.level.label(en()))
            .collect();
        assert!(!warnings.contains(&"info"), "{warnings:?}");
        assert!(
            fixture.state.log_len() > fixture.state.log_rows().len(),
            "the filter hides lines, it does not drop them"
        );

        fixture.state.set_log_filter(LogFilter::Errors);
        assert!(
            fixture
                .state
                .log_rows()
                .iter()
                .all(|row| row.level == LogLevel::Error),
            "the narrowest filter keeps only errors"
        );
        assert!(fixture.state.log_len() >= 4, "nothing was cleared");
    }

    #[test]
    fn the_activity_log_is_written_to_the_file_as_well() {
        let dir = std::env::temp_dir().join(format!("fp-app-log-{}", CoreId::new()));
        let path = dir.join(crate::log_file::LOG_FILE);
        let mut fixture = fixture_with_log(&dir);
        let id = running_profile(&mut fixture);
        fixture.state.clear_log();

        fixture.state.record_event(&RuntimeEvent::Started {
            profile_id: id,
            browser_pid: 4242,
            xray_pid: None,
            cdp_port: 9222,
            socks_port: None,
        });
        fixture.state.push_notice("Saved.", false);
        fixture.state.push_notice("it broke", true);

        let text = std::fs::read_to_string(&path).expect("the log file was written");
        let lines: Vec<&str> = text.lines().collect();
        // The profile creation line is already in the file: `clear_log` empties
        // the window's history, not what was written down.
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].contains("app: Created verify me"), "{}", lines[0]);
        assert!(
            lines[1].contains("info")
                && lines[1].contains("verify me")
                && lines[1].contains("browser started (pid 4242, cdp port 9222)"),
            "a line names the profile and what happened: {}",
            lines[1]
        );
        assert!(
            lines[2].contains("info") && lines[2].ends_with("app: Saved."),
            "{}",
            lines[2]
        );
        assert!(
            lines[3].contains("error") && lines[3].ends_with("app: it broke"),
            "{}",
            lines[3]
        );
        assert!(
            lines
                .iter()
                .all(|line| line.starts_with("20") && line.contains('T')),
            "every line carries an absolute UTC time: {text}"
        );
        assert_eq!(
            fixture.state.log_file_status().ok(),
            Some(path.as_path()),
            "the page can say where the file is"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The window must keep working when the log cannot be written, and the
    /// failure must be said once rather than per line.
    #[test]
    fn a_log_file_that_cannot_be_written_is_reported_once() {
        let dir = std::env::temp_dir().join(format!("fp-app-log-bad-{}", CoreId::new()));
        let mut fixture = fixture_full(
            &dir.join("config.json"),
            None,
            Some(crate::log_file::LogFile::at(
                dir.join("missing").join("activity.log"),
            )),
            None,
        );

        fixture.state.push_notice("first", false);
        fixture.state.push_notice("second", false);
        fixture.state.push_notice("third", true);

        let error = fixture
            .state
            .log_file_status()
            .expect_err("the sink failed");
        assert!(error.contains("could not open"), "{error}");
        assert_eq!(
            fixture
                .state
                .toasts()
                .iter()
                .filter(|toast| toast.message.contains("activity log could not be written"))
                .count(),
            1,
            "the failure is reported once, not once per line"
        );
        assert_eq!(
            fixture.state.log_len(),
            3,
            "the in-memory log is unaffected"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory of the test's own to export into, removed when it ends.
    ///
    /// An export writes a file, and the export tests are the one place in this
    /// module that must, so they write here rather than anywhere the machine
    /// keeps its own data.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("fp-app-export-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self { dir }
        }

        fn join(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const EXPORT_SECRET: &str = "correct horse battery staple";

    /// The last thing the window was told.
    ///
    /// A success is a toast and a log line, not the banner: the banner is for
    /// problems, which stay until they are dismissed. A failed export therefore
    /// has to be read from `notice` and a successful one from here, and the
    /// tests below do exactly that.
    fn last_message(fixture: &Fixture) -> String {
        fixture
            .state
            .toasts()
            .last()
            .map(|toast| toast.message.clone())
            .expect("the window was told something")
    }

    fn seed_proxy_holding_a_password(fixture: &mut Fixture) -> ProxyId {
        fixture
            .state
            .create_proxy(
                "Zurich exit",
                ProxyOutbound::Socks5(domain::Socks5Outbound {
                    host: "203.0.113.10".to_string(),
                    port: 1080,
                    username: Some("alice".to_string()),
                    password: Some(EXPORT_SECRET.to_string()),
                }),
            )
            .expect("create proxy")
    }

    #[test]
    fn an_export_leaves_the_passwords_out_unless_asked_otherwise() {
        let scratch = Scratch::new("default-choice");
        let mut fixture = fixture();
        let core = seed_core(&fixture);
        let proxy = seed_proxy_holding_a_password(&mut fixture);
        fixture.state.load().expect("load");
        let profile = fixture.state.create_profile("Work laptop").expect("create");
        let mut draft = fixture.state.profile(profile).expect("the profile");
        draft.proxy_id = Some(proxy);
        draft.core_id = core;
        fixture
            .state
            .update_profile(draft)
            .expect("assign the proxy");

        let path = scratch.join("config.json");
        fixture
            .state
            .set_export_path(path.to_string_lossy().to_string());

        // The default is the safe one: a file carrying plain-text passwords is
        // the thing to reach for on purpose, not the thing to get by not reading
        // a checkbox.
        assert!(!fixture.state.export_includes_credentials());
        let report = fixture.state.export_configuration().expect("export");

        assert_eq!(report.profiles, 1);
        assert_eq!(report.credentials, Credentials::Excluded);
        assert_eq!(report.credentials_removed, 1);

        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(!text.contains(EXPORT_SECRET), "{text}");
        // And the profile's references still point at what travelled with it.
        let document = application::ConfigBackup::from_json(&text).expect("parse");
        assert_eq!(document.profiles[0].core_id, document.cores[0].id);
        assert_eq!(document.profiles[0].proxy_id, Some(document.proxies[0].id));
    }

    #[test]
    fn asking_for_the_credentials_writes_them_and_says_so() {
        let scratch = Scratch::new("asked-for");
        let mut fixture = fixture();
        seed_core(&fixture);
        seed_proxy_holding_a_password(&mut fixture);
        fixture.state.load().expect("load");

        let path = scratch.join("config.json");
        fixture
            .state
            .set_export_path(path.to_string_lossy().to_string());
        fixture.state.set_export_includes_credentials(true);
        let report = fixture.state.export_configuration().expect("export");

        assert_eq!(report.credentials, Credentials::Included);
        assert_eq!(report.credentials_removed, 0);
        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(text.contains(EXPORT_SECRET));

        // The warning is part of the answer, not decoration: a file holding
        // passwords in plain text has to say so where the user is looking.
        let message = last_message(&fixture);
        assert!(message.contains("plain text"), "{message}");
        assert!(message.contains("config.json"), "{message}");
    }

    #[test]
    fn an_export_with_nothing_to_leave_out_says_that_instead() {
        // The opposite sentence, and the reason the count is reported: "left
        // out" over a configuration that had nothing to leave out would describe
        // a file that is missing something it is not.
        let scratch = Scratch::new("nothing-to-leave-out");
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");

        let path = scratch.join("config.json");
        fixture
            .state
            .set_export_path(path.to_string_lossy().to_string());
        let report = fixture.state.export_configuration().expect("export");

        assert_eq!(report.credentials, Credentials::Excluded);
        assert_eq!(report.credentials_removed, 0);
        assert_eq!(report.proxies, 0);

        let message = last_message(&fixture);
        assert!(
            message.contains("no proxy credentials to leave out"),
            "{message}"
        );
    }

    #[test]
    fn a_failed_export_is_reported_as_an_error_and_writes_nothing() {
        let scratch = Scratch::new("failed");
        let blocker = scratch.join("not-a-directory");
        std::fs::write(&blocker, b"").expect("write the blocker");
        let mut fixture = fixture();
        fixture.state.load().expect("load");

        fixture
            .state
            .set_export_path(blocker.join("config.json").to_string_lossy().to_string());
        let error = fixture
            .state
            .export_configuration()
            .expect_err("cannot write");

        assert!(error.contains("could not be written"), "{error}");
        assert!(error.contains("not-a-directory"), "{error}");
        let notice = fixture.state.notice().expect("a banner");
        assert!(notice.error, "{}", notice.message);
    }

    #[test]
    fn an_empty_export_field_means_the_default_and_a_typed_one_means_itself() {
        let mut fixture = fixture();

        // Empty, and whitespace, both mean "the default": a field someone has
        // cleared is not a request to write a file called nothing.
        assert_eq!(
            fixture.state.export_destination(),
            fixture.state.export_default_path()
        );
        fixture.state.set_export_path("   ");
        assert_eq!(
            fixture.state.export_destination(),
            fixture.state.export_default_path()
        );

        fixture.state.set_export_path("  /tmp/my-backup.json  ");
        assert_eq!(
            fixture.state.export_destination(),
            PathBuf::from("/tmp/my-backup.json")
        );
    }

    #[test]
    fn the_default_export_lands_under_the_data_directory_and_names_a_new_file() {
        // Nothing is written here - the default is somewhere the machine keeps
        // real data, and this test only reads what it would be.
        let fixture = fixture();
        let data_dir = fixture.state.export_default_path();
        let parent = data_dir.parent().expect("a parent").to_path_buf();

        assert_eq!(
            parent.file_name().unwrap(),
            crate::paths::EXPORT_DIR,
            "{}",
            parent.display()
        );
        assert!(
            data_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("fp-browser-config-"),
            "{}",
            data_dir.display()
        );
        assert_eq!(
            data_dir.extension().unwrap(),
            "json",
            "{}",
            data_dir.display()
        );
    }

    #[test]
    fn the_export_path_field_survives_a_trip_to_another_page() {
        // The field itself lives in the view, which is not what is tested here;
        // what is testable without a window is that the state behind it keeps
        // what was typed rather than forgetting it between renders.
        let mut fixture = fixture();
        fixture.state.set_export_path("/tmp/kept.json");
        fixture.state.set_page(crate::state::Page::Proxies);
        assert_eq!(fixture.state.export_path(), "/tmp/kept.json");
        fixture.state.set_page(crate::state::Page::Settings);
        assert_eq!(fixture.state.export_path(), "/tmp/kept.json");
    }

    /// A configuration file to import, produced the honest way: by exporting
    /// from a populated installation. Building one by hand here would quietly
    /// decide what a real file looks like, and the export is the authority on
    /// that.
    fn write_backup_from(fixture: &mut Fixture, path: &std::path::Path) {
        fixture.state.load().expect("load");
        fixture
            .state
            .set_export_path(path.to_string_lossy().to_string());
        fixture.state.export_configuration().expect("export");
    }

    #[test]
    fn an_export_imports_back_into_an_empty_installation() {
        let scratch = Scratch::new("import-round-trip");
        let mut source = fixture();
        let core = seed_core(&source);
        let proxy = seed_proxy_holding_a_password(&mut source);
        let profile = source.state.create_profile("Work laptop").expect("create");
        let mut draft = source.state.profile(profile).expect("the profile");
        draft.proxy_id = Some(proxy);
        draft.core_id = core;
        source
            .state
            .update_profile(draft)
            .expect("assign the proxy");

        let backup = scratch.join("config.json");
        write_backup_from(&mut source, &backup);

        let data_dir = scratch.join("data");
        let mut arriving = fixture_with_data_dir(&data_dir);
        arriving
            .state
            .set_import_path(backup.to_string_lossy().to_string());
        let report = arriving.state.import_configuration().expect("import");

        assert_eq!(report.added.cores, 1);
        assert_eq!(report.added.proxies, 1);
        assert_eq!(report.added.profiles, 1);
        assert!(!report.needs_attention(), "{:?}", report.notes);

        // The message names the file and says what arrived.
        let message = last_message(&arriving);
        assert!(message.contains("config.json"), "{message}");
        assert!(message.contains("were added"), "{message}");

        // The rows are reloaded, so the page shows what just arrived.
        assert_eq!(arriving.state.rows().len(), 1);

        // The profile's directory moved to this machine's data directory:
        // the one the file recorded is not here, and a directory that is not
        // on this machine was never going to be right.
        let stored = arriving
            .profiles
            .get(profile)
            .expect("stored")
            .expect("the profile arrived");
        assert_eq!(
            stored.user_data_dir,
            data_dir.join("profiles").join(profile.to_string())
        );

        // And the proxy arrived without the password the export left out.
        let stored_proxy = arriving
            .proxies
            .get(proxy)
            .expect("stored")
            .expect("the proxy arrived");
        match &stored_proxy.outbound {
            ProxyOutbound::Socks5(socks5) => {
                assert_eq!(socks5.password, None, "{:?}", stored_proxy.outbound);
            }
            other => panic!("expected the socks5 proxy back, got {other:?}"),
        }
    }

    #[test]
    fn importing_the_same_file_twice_adds_nothing_the_second_time() {
        // The ordinary result of importing a file twice, and the reason the
        // report distinguishes "nothing was added" from a refusal: the second
        // import succeeded, it just had nothing to do.
        let scratch = Scratch::new("import-twice");
        let mut source = fixture();
        seed_core(&source);
        source.state.load().expect("load");
        source.state.create_profile("Work laptop").expect("create");

        let backup = scratch.join("config.json");
        write_backup_from(&mut source, &backup);

        let mut arriving = fixture();
        arriving
            .state
            .set_import_path(backup.to_string_lossy().to_string());
        let first = arriving.state.import_configuration().expect("first import");
        assert_eq!(first.added.total(), 2);

        let second = arriving
            .state
            .import_configuration()
            .expect("second import");
        assert_eq!(second.added.total(), 0);
        assert!(!second.needs_attention(), "{:?}", second.notes);
        let message = last_message(&arriving);
        assert!(message.contains("nothing was added"), "{message}");
        assert_eq!(arriving.state.rows().len(), 1);
    }

    #[test]
    fn an_import_whose_core_is_not_in_the_file_skips_its_profiles() {
        // A hand-edited file: the export always pairs a profile with its core,
        // but a file is a file a person can edit, and the rule has to hold
        // when the pairing is broken.
        let scratch = Scratch::new("import-missing-core");
        let mut source = fixture();
        seed_core(&source);
        source.state.load().expect("load");
        source.state.create_profile("Work laptop").expect("create");

        let backup = scratch.join("config.json");
        write_backup_from(&mut source, &backup);

        let mut document = application::ConfigBackup::from_json(
            &std::fs::read_to_string(&backup).expect("read the backup"),
        )
        .expect("parse");
        document.cores.clear();
        let edited = scratch.join("edited.json");
        std::fs::write(&edited, document.to_json().expect("serialise")).expect("write");

        let mut arriving = fixture();
        arriving
            .state
            .set_import_path(edited.to_string_lossy().to_string());
        let report = arriving.state.import_configuration().expect("import");

        assert_eq!(report.added.profiles, 0);
        assert_eq!(report.notes.missing_core, vec!["Work laptop".to_string()]);
        assert!(report.needs_attention());
        // A partial import is a problem the reader has to dismiss, not a
        // toast that drifts away: what was skipped is the thing they came to
        // find out.
        assert!(
            arriving.state.notice().is_some_and(|notice| notice.error),
            "the shortfall is in the banner"
        );
        assert_eq!(arriving.state.rows().len(), 0);
    }

    #[test]
    fn an_import_without_a_path_is_refused_where_the_field_is() {
        let mut arriving = fixture();
        let result = arriving.state.import_configuration();

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Type the path"));
        assert!(arriving.state.notice().is_some_and(|notice| notice.error));
    }

    #[test]
    fn a_file_that_is_not_a_backup_is_said_so() {
        let scratch = Scratch::new("import-foreign");
        let elsewhere = scratch.join("notes.txt");
        std::fs::write(&elsewhere, "not a configuration backup").expect("write");

        let mut arriving = fixture();
        arriving
            .state
            .set_import_path(elsewhere.to_string_lossy().to_string());
        let result = arriving.state.import_configuration();

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("could not be read"));
        assert_eq!(arriving.state.rows().len(), 0);
    }

    #[test]
    fn is_configuration_empty_says_what_is_here() {
        let mut fixture = fixture();
        assert!(fixture.state.is_configuration_empty().expect("read"));

        seed_core(&fixture);
        fixture.state.load().expect("load");
        fixture.state.create_profile("Work laptop").expect("create");

        assert!(!fixture.state.is_configuration_empty().expect("read"));
    }

    /// A file to restore, produced the honest way: by exporting from a populated
    /// installation. Nothing hand-builds what the export is the authority on.
    fn backup_with_one_profile(scratch: &Scratch) -> PathBuf {
        let mut source = fixture();
        seed_core(&source);
        source.state.load().expect("load");
        source.state.create_profile("Work laptop").expect("create");
        let backup = scratch.join("config.json");
        write_backup_from(&mut source, &backup);
        backup
    }

    #[test]
    fn a_restore_onto_an_empty_installation_reads_the_file() {
        let scratch = Scratch::new("restore-empty");
        let backup = backup_with_one_profile(&scratch);

        let data_dir = scratch.join("data");
        let mut arriving = fixture_with_data_dir(&data_dir);
        arriving
            .state
            .set_restore_path(backup.to_string_lossy().to_string());
        let report = arriving
            .state
            .restore_configuration(RestoreMode::OnlyWhenEmpty)
            .expect("restore");

        assert_eq!(report.added.cores, 1);
        assert_eq!(report.added.profiles, 1);
        assert_eq!(report.removed.total(), 0, "there was nothing to replace");
        assert!(
            arriving.state.notice().is_none(),
            "a clean restore is a toast, not a banner"
        );
        let message = last_message(&arriving);
        assert!(message.contains("were added"), "{message}");
        assert_eq!(arriving.state.rows().len(), 1);
    }

    /// The precondition, and the whole difference from an import: a populated
    /// installation is never replaced without being asked.
    #[test]
    fn a_restore_without_confirmation_refuses_a_populated_installation() {
        let scratch = Scratch::new("restore-refuses");
        let backup = backup_with_one_profile(&scratch);

        let mut arriving = fixture();
        seed_core(&arriving);
        arriving.state.load().expect("load");
        let kept = arriving.state.create_profile("Keep me").expect("create");
        arriving
            .state
            .set_restore_path(backup.to_string_lossy().to_string());

        let error = arriving
            .state
            .restore_configuration(RestoreMode::OnlyWhenEmpty)
            .expect_err("a populated installation cannot be quietly replaced");

        assert!(error.contains("already holds"), "{error}");
        assert!(
            arriving.state.notice().is_some_and(|notice| notice.error),
            "the refusal owns the banner"
        );
        assert_eq!(arriving.state.rows().len(), 1);
        assert_eq!(arriving.state.rows()[0].profile.name, "Keep me");
        assert!(
            arriving.state.profile(kept).is_some(),
            "nothing was removed"
        );
    }

    #[test]
    fn a_confirmed_restore_replaces_what_is_here() {
        let scratch = Scratch::new("restore-replaces");
        let backup = backup_with_one_profile(&scratch);

        let mut arriving = fixture();
        seed_core(&arriving);
        arriving.state.load().expect("load");
        arriving.state.create_profile("Replace me").expect("create");
        arriving
            .state
            .set_restore_path(backup.to_string_lossy().to_string());

        let report = arriving
            .state
            .restore_configuration(RestoreMode::Replace)
            .expect("restore");

        assert_eq!(
            report.removed.total(),
            2,
            "the core and profile that were here"
        );
        assert_eq!(report.added.total(), 2);
        assert_eq!(arriving.state.rows().len(), 1);
        assert_eq!(arriving.state.rows()[0].profile.name, "Work laptop");
    }

    #[test]
    fn a_restore_without_a_path_is_refused_where_the_field_is() {
        let mut arriving = fixture();
        let result = arriving
            .state
            .restore_configuration(RestoreMode::OnlyWhenEmpty);

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Type the path"));
        assert!(arriving.state.notice().is_some_and(|notice| notice.error));
    }

    /// Restoring would delete the row of a live browser, leaving a process the
    /// window can no longer stop; the refusal names what to stop.
    #[test]
    fn a_restore_is_blocked_while_a_profile_is_running() {
        let scratch = Scratch::new("restore-running");
        let backup = backup_with_one_profile(&scratch);

        let mut arriving = fixture();
        running_profile(&mut arriving);
        arriving
            .state
            .set_restore_path(backup.to_string_lossy().to_string());

        let error = arriving
            .state
            .restore_configuration(RestoreMode::Replace)
            .expect_err("a running profile blocks the restore");

        assert!(error.contains("Stop these profiles"), "{error}");
        assert!(error.contains("verify me"), "{error}");
        assert_eq!(arriving.state.rows().len(), 1, "nothing was replaced");
    }

    /// The restore and import fields are separate: a path left over from one verb
    /// must not become a path the other acts on.
    #[test]
    fn the_restore_and_import_paths_are_separate_fields() {
        let mut fixture = fixture();
        fixture.state.set_import_path("/tmp/import.json");
        fixture.state.set_restore_path("/tmp/restore.json");

        assert_eq!(
            fixture.state.import_source(),
            Some(PathBuf::from("/tmp/import.json"))
        );
        assert_eq!(
            fixture.state.restore_source(),
            Some(PathBuf::from("/tmp/restore.json"))
        );
    }

    #[test]
    fn a_browser_data_job_needs_a_directory_and_gathers_every_profile() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        fixture.state.create_profile("Work laptop").expect("create");

        assert!(
            fixture.state.browser_data_job(Direction::ToBackup).is_err(),
            "a copy needs somewhere to go"
        );

        fixture.state.set_browser_data_path("  /backups/fp  ");
        let (job, lease) = fixture
            .state
            .browser_data_job(Direction::ToBackup)
            .expect("a job");
        assert_eq!(job.directory, PathBuf::from("/backups/fp"), "trimmed");
        assert_eq!(job.profiles.len(), 1);
        assert!(job.running.is_empty());
        assert_eq!(job.direction, Direction::ToBackup);
        drop(lease);
    }

    #[test]
    fn a_browser_data_job_is_refused_while_a_profile_is_running() {
        let mut fixture = fixture();
        running_profile(&mut fixture);
        fixture.state.set_browser_data_path("/backups/fp");

        let error = fixture
            .state
            .browser_data_job(Direction::ToBackup)
            .expect_err("a running profile blocks the copy");

        assert!(error.contains("Stop these profiles"), "{error}");
        assert!(error.contains("verify me"), "{error}");
    }

    #[test]
    fn a_browser_data_job_with_no_profiles_is_refused() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        fixture.state.set_browser_data_path("/backups/fp");

        let error = fixture
            .state
            .browser_data_job(Direction::FromBackup)
            .expect_err("nothing to copy");

        assert!(error.contains("no profiles"), "{error}");
    }

    /// The whole point of the lease: while a copy owns these profiles, nothing
    /// else may touch them - not a second copy, not a start, not a configuration
    /// replacement - and when the copy is over they are free again.
    ///
    /// The window this closes is the one the snapshot cannot: `start` returns when
    /// its command is queued, so a profile whose browser is coming up still reads
    /// as stopped. The worker is what holds the copy's lease, so the test holds it
    /// the same way - by keeping the value alive.
    #[test]
    fn a_copy_holds_its_profiles_until_the_worker_gives_them_back() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Work laptop").expect("create");
        fixture.state.set_browser_data_path("/backups/fp");

        let (job, lease) = fixture
            .state
            .browser_data_job(Direction::ToBackup)
            .expect("the first copy");
        assert_eq!(job.profiles.len(), 1);

        let error = fixture
            .state
            .browser_data_job(Direction::FromBackup)
            .expect_err("a second copy is refused while the first runs");
        assert!(error.contains("Work laptop"), "{error}");
        assert!(error.contains("copied"), "{error}");

        let error = fixture
            .state
            .start(id)
            .expect_err("starting one of the profiles is refused");
        assert!(error.to_string().contains("Work laptop"), "{error}");

        // A replacement reads its file first, so the path has to be there for the
        // refusal to be about the busy profile rather than about the field.
        fixture.state.set_restore_path("/backups/fp/config.json");
        let error = fixture
            .state
            .restore_configuration(RestoreMode::Replace)
            .expect_err("replacing the configuration is refused");
        assert!(error.contains("Work laptop"), "{error}");

        // The worker finishes: everything it held is free again.
        drop(lease);
        let (job, lease) = fixture
            .state
            .browser_data_job(Direction::FromBackup)
            .expect("the copy after it finishes");
        assert_eq!(job.profiles.len(), 1);
        drop(lease);
    }

    /// A start holds its profile from the command until the runtime has answered
    /// for it. A stopped snapshot is the window between the two, and a copy is
    /// refused for its whole width - which the snapshot alone could not do, since
    /// it is stopped for the whole of it.
    #[test]
    fn a_queued_start_holds_its_profile_until_the_snapshot_answers() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Work laptop").expect("create");
        fixture.state.set_browser_data_path("/backups/fp");

        // The command is queued and nothing has answered: the profile reads as
        // stopped, which is exactly the window the lease is for.
        fixture.runtime.answer_nothing();
        fixture.state.start(id).expect("the command is queued");
        assert_eq!(
            fixture.state.operations.held(id),
            Some(Operation::Starting),
            "a start that has not been answered holds its profile"
        );

        let error = fixture
            .state
            .browser_data_job(Direction::ToBackup)
            .expect_err("the queued start holds the profile");
        assert!(error.contains("starting up"), "{error}");

        // The runtime answers. The snapshot refuses a copy from here on, and the
        // lease is gone - which is what lets the profile be restarted at all.
        fixture.runtime.set_state(id, RuntimeState::Running);
        fixture.state.refresh_runtime();
        assert_eq!(
            fixture.state.operations.held(id),
            None,
            "an answered start gives its profile back"
        );
        fixture
            .state
            .restart(id)
            .expect("a restart is not refused by the start before it");
    }

    #[test]
    fn finishing_a_browser_data_copy_says_what_happened() {
        let mut fixture = fixture();
        fixture.state.finish_browser_data(
            Direction::ToBackup,
            Ok(BrowserDataReport {
                directory: PathBuf::from("/backups/fp"),
                copied: vec!["Work laptop".to_string()],
                skipped: vec!["Fresh".to_string()],
                bytes: 2 * 1024 * 1024,
            }),
        );

        let message = last_message(&fixture);
        assert!(
            message.contains("Copied the browser data of 1 profile"),
            "{message}"
        );
        assert!(message.contains("/backups/fp"), "{message}");
        assert!(message.contains("2.0 MiB"), "{message}");
        assert!(
            message.contains("Fresh"),
            "a skipped profile is named: {message}"
        );
    }

    #[test]
    fn a_failed_browser_data_copy_is_a_banner() {
        let mut fixture = fixture();
        fixture.state.finish_browser_data(
            Direction::FromBackup,
            Err("Stop these profiles first.".to_string()),
        );

        let notice = fixture.state.notice().expect("a banner");
        assert!(notice.error, "{}", notice.message);
        assert!(
            notice.message.contains("Stop these profiles"),
            "{}",
            notice.message
        );
    }

    /// The two directions are empty for opposite reasons, and the sentence has to
    /// say which one happened: blaming the backup directory for profiles that
    /// have never been started would send the reader to the wrong place.
    #[test]
    fn an_empty_browser_data_copy_blames_the_end_that_was_empty() {
        let empty = BrowserDataReport {
            directory: PathBuf::from("/backups/fp"),
            copied: Vec::new(),
            skipped: vec!["Fresh".to_string()],
            bytes: 0,
        };

        let out = browser_data_summary(Direction::ToBackup, &empty, en());
        assert!(out.contains("no profile has browser data yet"), "{out}");
        assert!(
            !out.contains("/backups/fp"),
            "the source is at fault: {out}"
        );

        let back = browser_data_summary(Direction::FromBackup, &empty, en());
        assert!(back.contains("/backups/fp"), "{back}");
        assert!(back.contains("holds no browser data"), "{back}");
    }

    /// A file written without credentials restores proxies that no longer carry
    /// them. An import already says so; a restore that stayed silent would leave
    /// the reader with a proxy that fails authentication and no explanation.
    #[test]
    fn a_restore_from_a_credential_free_file_says_a_proxy_may_need_them_again() {
        let mut report = RestoreReport::default();
        report.added.proxies = 1;
        report.notes.credentials_excluded = true;

        let sentence = restore_summary(&report, std::path::Path::new("/tmp/config.json"), en());

        assert!(sentence.contains("were added"), "{sentence}");
        assert!(sentence.contains("without proxy credentials"), "{sentence}");
        assert!(
            sentence.contains("may need them typed in again"),
            "{sentence}"
        );
    }
}
