//! What to tell the user about the sessions a previous run left behind.
//!
//! The banner shows one line and does not wrap, so this produces one sentence
//! naming the profiles by name where the caller can resolve them.

use domain::ProfileId;
use runtime::{ReclaimReport, Reclaimed};

/// A notice for what `RuntimeSupervisor::reclaim_orphans` did, or `None` when
/// there was nothing to do. The flag is the banner's error colour: something
/// that could not be reclaimed is an error, something that was cleaned up is
/// information.
pub fn reclaim_notice(
    report: &ReclaimReport,
    name_of: impl Fn(ProfileId) -> Option<String>,
) -> Option<(String, bool)> {
    if report.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    if !report.reclaimed.is_empty() {
        let sessions: Vec<String> = report
            .reclaimed
            .iter()
            .map(|session| describe(session, &name_of))
            .collect();
        parts.push(format!(
            "a previous run left {} running; {}",
            plural(sessions.len(), "browser session"),
            list(&sessions)
        ));
        if report.reclaimed.iter().any(|session| session.forced) {
            parts.push("had to be killed after ignoring the request to exit".to_string());
        }
    }
    if !report.unresolved.is_empty() {
        let reasons: Vec<String> = report
            .unresolved
            .iter()
            .map(|unresolved| {
                let who = unresolved
                    .profile_id
                    .and_then(&name_of)
                    .unwrap_or_else(|| "an unnamed profile".to_string());
                format!("{who}: {}", unresolved.reason)
            })
            .collect();
        parts.push(format!(
            "{} could not be reclaimed ({})",
            plural(reasons.len(), "session record"),
            list(&reasons)
        ));
    }
    if !report.stale.is_empty() {
        parts.push(format!(
            "removed {} whose processes had already exited",
            plural(report.stale.len(), "stale session record")
        ));
    }

    Some((parts.join("; "), !report.unresolved.is_empty()))
}

fn describe(session: &Reclaimed, name_of: &impl Fn(ProfileId) -> Option<String>) -> String {
    let who = name_of(session.profile_id).unwrap_or_else(|| session.profile_id.to_string());
    match session.xray_pid {
        Some(xray_pid) => format!(
            "{who} (browser pid {}, xray pid {xray_pid})",
            session.browser_pid
        ),
        None => format!("{who} (browser pid {})", session.browser_pid),
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Names up to two entries, then counts the rest: the banner is one line.
fn list(entries: &[String]) -> String {
    match entries {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [first, rest @ ..] => format!("{first} and {} more", rest.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Reclaimed, UnresolvedSession};
    use std::path::PathBuf;

    fn reclaimed(forced: bool, xray_pid: Option<u32>) -> Reclaimed {
        Reclaimed {
            profile_id: ProfileId::new(),
            browser_pid: 4321,
            xray_pid,
            forced,
        }
    }

    #[test]
    fn nothing_to_report_is_no_notice() {
        assert!(reclaim_notice(&ReclaimReport::default(), |_| None).is_none());
    }

    #[test]
    fn a_reclaimed_session_names_the_profile_and_its_pids() {
        let report = ReclaimReport {
            reclaimed: vec![reclaimed(false, Some(4322))],
            ..Default::default()
        };
        let (message, error) = reclaim_notice(&report, |_| Some("Profile 1".into())).unwrap();
        assert_eq!(
            message,
            "a previous run left 1 browser session running; Profile 1 (browser pid 4321, xray pid 4322)"
        );
        assert!(!error, "a clean reclaim is information, not an error");
    }

    #[test]
    fn a_session_that_had_to_be_killed_says_so() {
        let report = ReclaimReport {
            reclaimed: vec![reclaimed(true, None)],
            ..Default::default()
        };
        let (message, error) = reclaim_notice(&report, |_| Some("Profile 1".into())).unwrap();
        assert!(message.contains("had to be killed"), "{message}");
        assert!(!error, "{message}");
    }

    #[test]
    fn a_profile_that_cannot_be_named_is_identified_by_its_id() {
        let session = reclaimed(false, None);
        let report = ReclaimReport {
            reclaimed: vec![session],
            ..Default::default()
        };
        let (message, _) = reclaim_notice(&report, |_| None).unwrap();
        assert!(message.contains(&report.reclaimed[0].profile_id.to_string()));
    }

    #[test]
    fn a_record_that_could_not_be_honoured_is_an_error() {
        let report = ReclaimReport {
            unresolved: vec![UnresolvedSession {
                profile_id: Some(ProfileId::new()),
                path: PathBuf::from("/tmp/session.json"),
                reason: "nothing was killed: browser pid 4321 now runs \"other\"".into(),
            }],
            ..Default::default()
        };
        let (message, error) = reclaim_notice(&report, |_| Some("Profile 2".into())).unwrap();
        assert!(
            message.contains("1 session record could not be reclaimed"),
            "{message}"
        );
        assert!(
            message.contains("Profile 2: nothing was killed"),
            "{message}"
        );
        assert!(error);
    }

    #[test]
    fn more_than_two_entries_are_counted_rather_than_listed() {
        let report = ReclaimReport {
            reclaimed: vec![
                reclaimed(false, None),
                reclaimed(false, None),
                reclaimed(false, None),
            ],
            stale: vec![PathBuf::from("a"), PathBuf::from("b")],
            ..Default::default()
        };
        let (message, _) = reclaim_notice(&report, |_| Some("Profile".into())).unwrap();
        assert!(message.contains("3 browser sessions"), "{message}");
        assert!(message.contains("and 2 more"), "{message}");
        assert!(message.contains("2 stale session records"), "{message}");
    }
}
