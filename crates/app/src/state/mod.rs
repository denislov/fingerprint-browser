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

mod activity;
mod rows;
mod summaries;
pub use rows::*;
use summaries::*;
mod configuration;
mod cores;
mod profiles;
mod proxies;
use crate::proxy_tester::ProxyTestJob;
use crate::settings::{SettingGroup, SettingKey, SettingRow, Settings};
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
use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// Whether the Runtime Details panel is on screen.
    ///
    /// Separate from the selection, and false until something asks for it: the
    /// list is what the page is for, and a panel that is always there takes the
    /// height it needs whether or not it is being read. Choosing a profile is
    /// the ask; closing the panel is the other answer, and it leaves the profile
    /// chosen so the row stays the one the window is talking about.
    details_open: bool,
    /// What the Profiles page is narrowing its list by. Empty means no filter.
    profile_filter: String,
    notice: Option<Notice>,
    verifications: HashMap<ProfileId, Verification>,
    verification_tasks: HashMap<ProfileId, crate::task::TaskId>,
    proxy_tasks: HashMap<ProxyId, crate::task::TaskId>,
    /// What the last test of each proxy found.
    proxy_tests: HashMap<ProxyId, ProxyTest>,
    /// Toasts the window has not shown yet.
    toasts: Vec<Toast>,
    /// What happened this session, oldest first.
    log: Vec<LogEntry>,
    /// Which of those lines the page is showing.
    log_filter: LogFilter,
    /// The log lines the reader has opened in full, by the instant they were
    /// written.
    ///
    /// The time is the line's identity: it is the one field that is theirs alone,
    /// and it does not move when newer lines are prepended above them - which is
    /// exactly what a row index would do.
    expanded_logs: HashSet<SystemTime>,
    /// Which of the Settings page's four groups is on screen.
    settings_group: SettingGroup,
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
            verification_tasks: HashMap::new(),
            proxy_tasks: HashMap::new(),
            proxy_tests: HashMap::new(),
            toasts: Vec::new(),
            log: Vec::new(),
            log_filter: LogFilter::default(),
            expanded_logs: HashSet::new(),
            settings_group: SettingGroup::default(),
            details_tab: DetailsTab::default(),
            details_open: false,
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
                    queued_start: self.queued_starts.contains_key(&profile.id),
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
            row.queued_start = self.queued_starts.contains_key(&row.profile.id);
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
        self.details_open = true;
    }

    /// Whether the Runtime Details panel is showing.
    ///
    /// True once a profile has been chosen and until the panel is closed. It
    /// stays true with nothing selected - after the profile it was describing
    /// was deleted - because the panel is what says so.
    pub fn details_open(&self) -> bool {
        self.details_open
    }

    /// Closes the panel. The chosen profile stays chosen, and keeps its row lit.
    pub fn close_details(&mut self) {
        self.details_open = false;
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
    /// How many profiles are running, starting or stopping right now.
    ///
    /// The number the close dialog's three answers are about: "leave them
    /// running" and "stop everything" read very differently with six beside them.
    pub fn active_profile_count(&self) -> usize {
        self.rows()
            .iter()
            .filter(|row| row.state().is_active())
            .count()
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
#[cfg(test)]
mod tests;
