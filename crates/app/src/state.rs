//! View-facing application state.
//!
//! [`AppState`] is intentionally free of GPUI types so it can be unit tested
//! without a window. The view layer renders it and forwards user actions back
//! into it. Runtime state is never owned here: every read goes through
//! [`RuntimeService::snapshot`], which is the documented reconciliation path.

use crate::log_file::LogFile;
use crate::settings::{SettingKey, SettingRow, Settings};
use application::{
    AppError, CoreService, DeleteMode, NewProfile, NewProxy, ProfileService, ProxyService,
    RuntimeService,
};
use domain::{
    BrowserCore, BrowserProfile, CoreCapabilities, CoreId, FingerprintProfile, ProfileId, ProxyId,
    ProxyOutbound, ProxyProfile, RuntimeState,
};
use runtime::{Discrepancy, RuntimeComponent, RuntimeEvent, RuntimeSnapshot};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

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
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
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

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Warnings => "Warnings",
            Self::Errors => "Errors",
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
    pub fn noun(self) -> &'static str {
        match self {
            Self::All => "anything",
            Self::Warnings => "warning or error",
            Self::Errors => "error",
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

    pub fn state_label(&self) -> &'static str {
        match self.state() {
            RuntimeState::Stopped => "Stopped",
            RuntimeState::Starting => "Starting",
            RuntimeState::Running => "Running",
            RuntimeState::Stopping => "Stopping",
            RuntimeState::Failed { .. } => "Failed",
            RuntimeState::Crashed { .. } => "Crashed",
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

/// What a verification of one profile produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Verification {
    /// A reading is in flight.
    Running,
    /// Every claim the profile makes was confirmed by the reading.
    Confirmed,
    /// The reading disagreed with the profile.
    Disagreements(Vec<Discrepancy>),
    /// No reading could be taken, so nothing was confirmed.
    Unreadable(String),
}

impl Verification {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Short label for the profile row and the details panel.
    pub fn label(&self) -> String {
        match self {
            Self::Running => "verifying...".to_string(),
            Self::Confirmed => "fingerprint confirmed".to_string(),
            Self::Disagreements(found) => match found.len() {
                1 => "1 claim not confirmed".to_string(),
                count => format!("{count} claims not confirmed"),
            },
            Self::Unreadable(_) => "fingerprint unreadable".to_string(),
        }
    }

    pub fn disagreements(&self) -> &[Discrepancy] {
        match self {
            Self::Disagreements(found) => found,
            _ => &[],
        }
    }

    pub fn failure(&self) -> Option<&str> {
        match self {
            Self::Unreadable(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Everything a worker needs to verify one profile without touching the view.
#[derive(Debug, Clone)]
pub struct VerificationJob {
    pub profile_id: ProfileId,
    pub port: u16,
    pub profile: FingerprintProfile,
    pub capabilities: CoreCapabilities,
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
    pub fn label(self) -> &'static str {
        match self {
            Self::Profiles => "Profiles",
            Self::Proxies => "Proxies",
            Self::Cores => "Browser Cores",
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

    pub fn usage_label(&self) -> String {
        match self.used_by.len() {
            0 => "not assigned".to_string(),
            1 => format!("used by {}", self.used_by[0]),
            count => format!("used by {count} profiles"),
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

    pub fn usage_label(&self) -> String {
        match self.used_by.len() {
            0 => "not used".to_string(),
            1 => format!("used by {}", self.used_by[0]),
            count => format!("used by {count} profiles"),
        }
    }

    /// Which switch generation this core belongs to, as the page shows it.
    ///
    /// `None` for a core whose version was never read: there is no generation,
    /// which is exactly what the launch path refuses on.
    pub fn generation_label(&self) -> Option<String> {
        self.core.capabilities().map(|capabilities| {
            format!(
                "{} · {}",
                capabilities.generation_label(),
                capabilities.exclusion_label()
            )
        })
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
    notice: Option<Notice>,
    verifications: HashMap<ProfileId, Verification>,
    /// Toasts the window has not shown yet.
    toasts: Vec<Toast>,
    /// What happened this session, oldest first.
    log: Vec<LogEntry>,
    /// Which of those lines the page is showing.
    log_filter: LogFilter,
    /// Where the lines are also written down, when a file could be opened.
    log_file: Option<LogFile>,
    /// Why there is no file, or the first write that failed.
    log_file_error: Option<String>,
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
            notice: None,
            verifications: HashMap::new(),
            toasts: Vec::new(),
            log: Vec::new(),
            log_filter: LogFilter::default(),
            log_file,
            log_file_error,
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
                    .unwrap_or_else(|| "(missing core)".to_string());
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

    /// Where the log is also written down: the path, or the reason it is not.
    ///
    /// A write that failed outranks the path: the file is there, but it is no
    /// longer being appended to.
    pub fn log_file_status(&self) -> Result<&std::path::Path, &str> {
        if let Some(error) = &self.log_file_error {
            return Err(error.as_str());
        }
        match &self.log_file {
            Some(file) => Ok(file.path()),
            None => Err("no activity log is being kept"),
        }
    }

    /// The name a line's profile is shown under.
    fn who(&self, profile_id: Option<ProfileId>) -> String {
        let Some(id) = profile_id else {
            return "app".to_string();
        };
        self.rows
            .iter()
            .find(|row| row.profile.id == id)
            .map(|row| row.profile.name.clone())
            .unwrap_or_else(|| "a removed profile".to_string())
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
        let entry = match event {
            // Starting, stopping and a refused start are only visible as a
            // state change; running and stopped have their own event, and are
            // not repeated here.
            RuntimeEvent::StateChanged { profile_id, state } => match state {
                RuntimeState::Starting => (LogLevel::Info, *profile_id, "starting".to_string()),
                RuntimeState::Stopping => (LogLevel::Info, *profile_id, "stopping".to_string()),
                RuntimeState::Failed { message } => {
                    (LogLevel::Error, *profile_id, format!("failed: {message}"))
                }
                RuntimeState::Running | RuntimeState::Stopped | RuntimeState::Crashed { .. } => {
                    return;
                }
            },
            RuntimeEvent::EffectiveLaunchArgs { profile_id, args } => (
                LogLevel::Info,
                *profile_id,
                format!("launching with {} arguments", args.len()),
            ),
            RuntimeEvent::Started {
                profile_id,
                browser_pid,
                xray_pid,
                cdp_port,
                socks_port,
            } => {
                let mut message =
                    format!("browser started (pid {browser_pid}, cdp port {cdp_port}");
                if let Some(port) = socks_port {
                    message.push_str(&format!(", socks port {port}"));
                }
                if let Some(pid) = xray_pid {
                    message.push_str(&format!(", xray pid {pid}"));
                }
                message.push(')');
                (LogLevel::Info, *profile_id, message)
            }
            RuntimeEvent::Stopped { profile_id } => {
                (LogLevel::Info, *profile_id, "browser stopped".to_string())
            }
            RuntimeEvent::Crashed {
                profile_id,
                component,
                message,
            } => (
                LogLevel::Error,
                *profile_id,
                format!("{} crashed: {message}", component_label(*component)),
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
        let line = format!(
            "{} {:<7} {}: {}\n",
            crate::log_file::timestamp(entry.at),
            entry.level.label(),
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
        if self.log_file_error.is_none() {
            self.log_file_error = Some(error.clone());
            self.toast(
                ToastKind::Error,
                format!("the activity log could not be written: {error}"),
            );
        }
    }

    /// Placeholder naming until the Profile Editor page exists.
    pub fn next_profile_name(&self) -> String {
        format!("Profile {}", self.rows.len() + 1)
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
        let created = self.record(self.proxies.create(NewProxy {
            name: name.trim().to_string(),
            outbound,
        }))?;
        let id = created.id;
        self.set_notice(Notice::info(format!("Created proxy {}", created.name)));
        Ok(id)
    }

    pub fn update_proxy(&mut self, proxy: ProxyProfile) -> Result<(), AppError> {
        self.record(self.proxies.update(proxy.clone()))?;
        self.set_notice(Notice::info(format!(
            "Saved {}. Running profiles keep the proxy they started with.",
            proxy.name
        )));
        Ok(())
    }

    /// Removes a proxy. Refused while a profile still points at it, because the
    /// alternative is that profile quietly going direct.
    pub fn delete_proxy(&mut self, id: ProxyId) -> Result<(), AppError> {
        let name = self
            .proxy(id)
            .map(|proxy| proxy.name)
            .unwrap_or_else(|| id.to_string());
        self.record(self.proxies.delete(id))?;
        self.set_notice(Notice::info(format!("Deleted proxy {name}")));
        Ok(())
    }

    /// Every setting with the value in force and where it came from.
    pub fn setting_rows(&self) -> Vec<SettingRow> {
        self.settings.rows()
    }

    /// Stores an editable setting for the next start.
    ///
    /// The value is not live: it decides what the next process does, and the
    /// window says so on the row.
    pub fn update_setting(&mut self, key: SettingKey, value: &str) -> Result<(), AppError> {
        let result = self.settings.set(key, value).map_err(AppError::Conflict);
        match &result {
            Ok(()) => {
                self.set_notice(Notice::info(format!(
                    "Saved {}. It takes effect at the next start.",
                    key.label()
                )));
            }
            Err(error) => {
                self.set_notice(Notice::error(error.to_string()));
            }
        }
        result
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
        let core = self.record(self.cores.add(name, path))?;
        let id = core.id;
        self.set_notice(Notice::info(format!(
            "Added {} ({}, major {})",
            core.name, core.version, core.major
        )));
        Ok(id)
    }

    /// Saves a core, re-reading the version when its executable changed.
    pub fn update_core(&mut self, core: BrowserCore) -> Result<(), AppError> {
        let saved = self.record(self.cores.update(core))?;
        self.set_notice(Notice::info(format!(
            "Saved {} ({}, major {})",
            saved.name, saved.version, saved.major
        )));
        // The rows carry display names that came from this core.
        self.load_rows()?;
        Ok(())
    }

    /// Re-reads a core's version, for a binary that was replaced in place.
    pub fn redetect_core(&mut self, id: CoreId) -> Result<(), AppError> {
        let refreshed = self.record(self.cores.redetect(id))?;
        self.set_notice(Notice::info(format!(
            "{} is {} (major {})",
            refreshed.name, refreshed.version, refreshed.major
        )));
        self.load_rows()?;
        Ok(())
    }

    /// Removes a core. Refused while a profile still launches with it.
    pub fn delete_core(&mut self, id: CoreId) -> Result<(), AppError> {
        let name = self
            .core(id)
            .map(|core| core.name)
            .unwrap_or_else(|| id.to_string());
        self.record(self.cores.delete(id))?;
        self.set_notice(Notice::info(format!("Deleted core {name}")));
        Ok(())
    }

    pub fn has_core(&self) -> bool {
        self.cores
            .list()
            .map(|cores| !cores.is_empty())
            .unwrap_or(false)
    }

    fn default_core_id(&self) -> Result<CoreId, AppError> {
        self.cores
            .list()?
            .first()
            .map(|core| core.id)
            .ok_or_else(|| {
                AppError::Conflict(
                    "no browser core configured; set FP_BROWSER_CHROMIUM_BIN and restart"
                        .to_string(),
                )
            })
    }

    pub fn create_profile(&mut self, name: &str) -> Result<ProfileId, AppError> {
        let core_id = self.core_id()?;
        let draft = NewProfile {
            name: name.trim().to_string(),
            core_id,
            user_data_dir: None,
            fingerprint: None,
            proxy_id: None,
            window: None,
            start_target: None,
        };

        let profile = self.record(self.profiles.create(draft))?;
        let id = profile.id;
        self.load()?;
        self.selected = Some(id);
        self.set_notice(Notice::info(format!("Created {}", profile.name)));
        Ok(id)
    }

    pub fn start(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.record(self.runtime.start(id))?;
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
        self.record(self.runtime.restart(id))?;
        // A restarted browser is a new browser: the old reading is stale.
        self.forget_verification(id);
        self.refresh_runtime();
        Ok(())
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
        let name = format!("{} copy", self.next_profile_name());
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
        let existing = self.verifications.get(&id);
        if existing.is_some_and(Verification::is_running) {
            return Err(AppError::Other(format!(
                "profile {id} is already being verified"
            )));
        }
        let job = self.verification_job(id)?;
        self.verifications.insert(id, Verification::Running);
        Ok(job)
    }

    /// Records the outcome of a verification the view ran on a worker.
    pub fn finish_verification(
        &mut self,
        id: ProfileId,
        outcome: Result<Vec<Discrepancy>, String>,
    ) {
        let verification = match outcome {
            Ok(found) if found.is_empty() => Verification::Confirmed,
            Ok(found) => Verification::Disagreements(found),
            Err(reason) => Verification::Unreadable(reason),
        };
        // A reading is the answer to a question the user asked, so it belongs
        // in the history as well as on the row.
        match &verification {
            Verification::Confirmed => {
                self.append_log(
                    LogLevel::Info,
                    Some(id),
                    "fingerprint confirmed by reading the running browser",
                );
                self.toast(ToastKind::Success, "Fingerprint confirmed.");
            }
            Verification::Disagreements(found) => {
                let message = format!(
                    "fingerprint read back with {} claim(s) not confirmed",
                    found.len()
                );
                self.toast(
                    ToastKind::Warning,
                    format!(
                        "{} claim(s) the browser did not reproduce; see Runtime Details",
                        found.len()
                    ),
                );
                self.append_log(LogLevel::Warning, Some(id), message);
            }
            Verification::Unreadable(reason) => {
                self.toast(
                    ToastKind::Error,
                    format!("Fingerprint could not be read: {reason}"),
                );
                self.append_log(
                    LogLevel::Error,
                    Some(id),
                    format!("fingerprint could not be read: {reason}"),
                );
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
        let row = self
            .rows
            .iter()
            .find(|row| row.profile.id == id)
            .ok_or_else(|| AppError::Other(format!("profile {id} not found")))?;
        let port = row.cdp_port().ok_or_else(|| {
            AppError::Other("the browser must be running before it can be verified".to_string())
        })?;
        let core = self
            .cores
            .get(row.profile.core_id)?
            .ok_or_else(|| AppError::Other(format!("core {} not found", row.profile.core_id)))?;
        // A core whose version was never read has no capability table, and
        // asking for one would be asking what the engine may claim. This is the
        // same refusal the launch path makes.
        let capabilities = core.capabilities().ok_or_else(|| {
            AppError::Conflict(format!(
                "{} has no detected version, so there are no switches to check against; \
                 give it a version before verifying",
                core.name
            ))
        })?;
        Ok(VerificationJob {
            profile_id: id,
            port,
            profile: row.profile.fingerprint.clone(),
            capabilities,
        })
    }

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
    }

    impl FakeRuntime {
        pub fn new() -> Self {
            Self::default()
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
            self.set_state(id, RuntimeState::Running);
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
            self.set_state(id, RuntimeState::Running);
            self.record(&format!("restart:{id}"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::{CoreBinary, FakeRuntime, core, core_service};
    use application::{DefaultProfileService, DefaultProxyService};
    use domain::CoreId;
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
        fixture_full(config, None, None)
    }

    /// The same fixture, with an activity log on disk in `log_dir`.
    fn fixture_with_log(log_dir: &std::path::Path) -> Fixture {
        let file = crate::log_file::LogFile::open(log_dir).expect("open the log file");
        fixture_full(
            &std::env::temp_dir()
                .join("fp-app-settings-fixture")
                .join("config.json"),
            Some(file),
            None,
        )
    }

    fn fixture_full(
        config: &std::path::Path,
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
        fixture.state.finish_verification(id, Ok(Vec::new()));

        fixture.state.delete_profile(id).expect("delete");

        assert!(fixture.state.rows().is_empty(), "the row is gone");
        assert!(fixture.state.profile(id).is_none());
        assert!(
            fixture.state.verification(id).is_none(),
            "a deleted profile keeps no verification result"
        );
        assert_eq!(fixture.state.selected_id(), None);
    }

    fn socks5(host: &str, port: u16) -> ProxyOutbound {
        ProxyOutbound::Socks5(domain::Socks5Outbound {
            host: host.to_string(),
            port,
            username: None,
            password: None,
        })
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
        assert_eq!(rows[0].usage_label(), "not assigned");
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
        assert_eq!(rows[0].usage_label(), "used by Proxied");
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
        assert_eq!(rows[0].usage_label(), "not used");
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

        assert_eq!(rows.len(), 6, "every setting is listed");
        let data_dir = rows
            .iter()
            .find(|row| row.key == SettingKey::DataDir)
            .expect("the data directory row");
        assert_eq!(data_dir.source, crate::settings::Source::Default);
        assert_eq!(data_dir.source_label(), "from the default");
        assert!(data_dir.key.editable());
        assert_eq!(data_dir.key.effect(), "next start");

        let chromium = rows
            .iter()
            .find(|row| row.key == SettingKey::ChromiumBin)
            .expect("the chromium row");
        assert!(
            !chromium.key.editable(),
            "the binary is chosen by the environment and shown on the cores page"
        );
    }

    #[test]
    fn saving_a_setting_stores_it_and_says_when_it_applies() {
        let dir = std::env::temp_dir().join(format!("fp-app-settings-{}", CoreId::new()));
        let config = dir.join("config.json");
        let mut fixture = fixture_with_config(&config);

        fixture
            .state
            .update_setting(SettingKey::DataDir, "/srv/fp")
            .expect("save");

        let stored = std::fs::read_to_string(&config).expect("the config file was written");
        assert!(stored.contains("/srv/fp"), "{stored}");
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
                .find(|row| row.key == SettingKey::DataDir)
                .expect("the row")
                .value,
            "/srv/fp",
            "the page shows what the next start will use"
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

        fixture.state.finish_verification(id, Ok(Vec::new()));
        assert_eq!(
            fixture.state.verification(id),
            Some(&Verification::Confirmed)
        );

        let disagreement = Discrepancy {
            claim: "platform",
            expected: "Win32".to_string(),
            observed: "Linux x86_64".to_string(),
        };
        fixture
            .state
            .finish_verification(id, Ok(vec![disagreement.clone()]));
        let recorded = fixture.state.verification(id).expect("recorded");
        assert_eq!(
            recorded.disagreements(),
            std::slice::from_ref(&disagreement)
        );
        assert_eq!(recorded.label(), "1 claim not confirmed");

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
        fixture.state.finish_verification(id, Ok(Vec::new()));
        assert!(fixture.state.verification(id).is_some());

        fixture.state.restart(id).expect("restart");
        assert!(
            fixture.state.verification(id).is_none(),
            "a restarted browser has a new fingerprint"
        );

        fixture.state.finish_verification(id, Ok(Vec::new()));
        fixture.state.stop(id).expect("stop");
        assert!(fixture.state.verification(id).is_none());
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
        assert_eq!(row.state_label(), "Stopped");
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
        assert_eq!(row.state_label(), "Crashed");
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
            .map(|entry| (entry.level.label(), entry.message.clone()))
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
            .map(|row| row.level.label())
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
}
