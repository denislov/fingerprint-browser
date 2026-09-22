//! User-facing summaries of completed operations.
use super::*;

/// What an export did, in one sentence.
///
/// The three cases are kept apart on purpose. "Credentials were left out" and
/// "there were none to leave out" are different sentences, and only one of them
/// tells the reader that the file is not the whole configuration. Saying which
/// file and how many of each is the difference between a backup someone trusts
/// and a backup someone assumes.
pub(super) fn export_summary(report: &ExportReport, t: &Text) -> String {
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
pub(super) fn counts_phrase(counts: &Counts, t: &Text) -> String {
    t.counts_phrase(counts.cores, counts.proxies, counts.profiles)
}

/// The clauses an import and a restore share, in the order they are read.
///
/// The two verbs report the same shortfalls because they write through the same
/// rules; what differs is the sentence they are clauses of.
pub(super) fn note_clauses(notes: &ImportNotes, t: &Text) -> Vec<String> {
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
pub(super) fn failed_clause(failed: &[String], t: &Text) -> Option<String> {
    (!failed.is_empty()).then(|| t.refused_not_stored(&listed(failed, t)))
}

/// The clause that explains a file written without credentials, when one of its
/// proxies landed.
pub(super) fn credential_clause(
    notes: &ImportNotes,
    proxies_landed: usize,
    t: &Text,
) -> Option<String> {
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
pub(super) fn import_summary(report: &ImportReport, source: &std::path::Path, t: &Text) -> String {
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
pub(super) fn restore_summary(
    report: &RestoreReport,
    source: &std::path::Path,
    t: &Text,
) -> String {
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
pub(super) fn browser_data_summary(
    direction: Direction,
    report: &BrowserDataReport,
    t: &Text,
) -> String {
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
pub(super) fn human_bytes(bytes: u64) -> String {
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
pub(super) fn listed(names: &[String], t: &Text) -> String {
    const SHOWN: usize = 3;
    if names.len() <= SHOWN {
        names.join(", ")
    } else {
        t.listed_more(&names[..SHOWN].join(", "), names.len() - SHOWN)
    }
}
