//! Notifications and activity history.
use super::*;

impl AppState {
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
    pub(super) fn set_notice(&mut self, notice: Notice) {
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
    pub(super) fn toast(&mut self, kind: ToastKind, message: impl Into<String>) {
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
    pub(super) fn who(&self, profile_id: Option<ProfileId>) -> String {
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

    pub(super) fn append_log(
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
    pub(super) fn report_log_failure(&mut self, error: String) {
        let t = self.text();
        if self.log_file_error.is_none() {
            self.log_file_error = Some(error.clone());
            self.toast(ToastKind::Error, t.log_write_failed(&error));
        }
    }
}
