// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! What a session SUMMARY means: the protocol's status bitset read as
//! activity, the stamp recency orders on, and the text a row carries.
//! The glyphs and colors that dress these belong to whatever shell
//! shows them — what the bits MEAN is the session's own business.

use ahp_types::state::{SessionStatus, SessionSummary};

/// A session's activity, decoded from the status bitset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionActivity {
    /// A turn is running but blocked on the user — an answer or a
    /// tool confirmation.
    Blocked,

    /// A turn is streaming.
    Working,

    /// The last turn ended in an error.
    Failed,

    /// Nothing is running, and the client has not viewed the session
    /// since it last changed.
    Unviewed,

    /// Nothing is running and nothing is owed.
    Quiet,
}

/// Read the summary's activity. The test ORDER is the protocol's
/// precedence, not a preference: `InputNeeded` carries the
/// `InProgress` bit, so a blocked turn must be recognized before a
/// merely running one.
pub fn activity(summary: &SessionSummary) -> SessionActivity {
    let status = SessionStatus::from_bits(summary.status).bits();
    let any = |flag: SessionStatus| status & flag.bits() != 0;
    let all = |flag: SessionStatus| status & flag.bits() == flag.bits();

    if all(SessionStatus::InputNeeded) {
        SessionActivity::Blocked
    } else if any(SessionStatus::InProgress) {
        SessionActivity::Working
    } else if any(SessionStatus::Error) {
        SessionActivity::Failed
    } else if !any(SessionStatus::IsRead) {
        SessionActivity::Unviewed
    } else {
        SessionActivity::Quiet
    }
}

/// Whether a turn is live — the bit that decides if the summary's
/// `activity` line is worth showing.
pub fn is_running(summary: &SessionSummary) -> bool {
    matches!(
        activity(summary),
        SessionActivity::Blocked | SessionActivity::Working
    )
}

/// The stamp the recency order runs on — `modified_at` moves on every
/// message, ours or the agent's. Unparseable stamps sink to the
/// epoch, so fresh sessions never hide below them.
pub fn modified_stamp(summary: &SessionSummary) -> std::time::SystemTime {
    humantime::parse_rfc3339_weak(&summary.modified_at).unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}

/// How long ago the session last moved, in the compact form a row
/// carries ("now", "5m", "3h", "2d"). `None` when the stamp is
/// unreadable or ahead of `now` — a trail nobody can trust is worse
/// than none.
pub fn age(now: std::time::SystemTime, summary: &SessionSummary) -> Option<String> {
    let stamp = humantime::parse_rfc3339_weak(&summary.modified_at).ok()?;
    let seconds = now.duration_since(stamp).ok()?.as_secs();
    Some(match seconds {
        0..=59 => "now".to_owned(),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    })
}

/// The text a session row carries: its title, what it reports doing
/// while a turn runs, and its change counts.
pub fn label(summary: &SessionSummary) -> String {
    let mut label = summary.title.clone();
    if let Some(activity) = summary.activity.as_ref().filter(|_| is_running(summary)) {
        label.push_str(" · ");
        label.push_str(activity);
    }
    if let Some(changes) = &summary.changes {
        if let (Some(additions), Some(deletions)) = (changes.additions, changes.deletions) {
            if additions > 0 || deletions > 0 {
                label.push_str(&format!("  +{additions} -{deletions}"));
            }
        }
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(status: u32) -> SessionSummary {
        SessionSummary {
            origin: None,
            provider: "test".to_owned(),
            title: "a session".to_owned(),
            status,
            activity: None,
            project: None,
            working_directories: None,
            annotations: None,
            resource: "test-session:/1".to_owned(),
            created_at: String::new(),
            modified_at: "2026-09-22T10:00:00Z".to_owned(),
            changes: None,
            meta: None,
        }
    }

    /// `InputNeeded` is `InProgress` PLUS the blocked bit, so reading
    /// the bits in the wrong order reports a blocked turn as merely
    /// running — and the row that wants the user looks busy instead.
    #[test]
    fn a_blocked_turn_outranks_the_running_bit_it_carries() {
        assert_eq!(
            activity(&summary(SessionStatus::InputNeeded.bits())),
            SessionActivity::Blocked
        );
        assert_eq!(
            activity(&summary(SessionStatus::InProgress.bits())),
            SessionActivity::Working
        );
        assert!(is_running(&summary(SessionStatus::InputNeeded.bits())));
    }

    /// A session nobody has looked at since it changed owes the user
    /// a mark; one that has been read owes nothing.
    #[test]
    fn the_unread_bit_is_what_marks_a_quiet_session() {
        assert_eq!(
            activity(&summary(SessionStatus::Idle.bits())),
            SessionActivity::Unviewed
        );
        assert_eq!(
            activity(&summary(SessionStatus::Idle.bits() | SessionStatus::IsRead.bits())),
            SessionActivity::Quiet
        );
    }

    /// An error outranks the unread mark: the failure is the news.
    #[test]
    fn a_failed_turn_reads_as_failed() {
        assert_eq!(
            activity(&summary(SessionStatus::Error.bits())),
            SessionActivity::Failed
        );
    }

    /// Unknown bits are not activity: a forward-compat flag must not
    /// turn a read, idle session into a report.
    #[test]
    fn an_unknown_bit_changes_nothing() {
        let known = SessionStatus::Idle.bits() | SessionStatus::IsRead.bits();
        assert_eq!(activity(&summary(known | 1 << 20)), SessionActivity::Quiet);
    }

    /// A stamp ahead of `now` yields no trail — a negative age is
    /// worse than a missing one.
    #[test]
    fn an_unreadable_or_future_stamp_has_no_age() {
        let mut ahead = summary(0);
        ahead.modified_at = "2099-01-01 00:00:00".to_owned();
        assert_eq!(age(std::time::SystemTime::now(), &ahead), None);

        let mut nonsense = summary(0);
        nonsense.modified_at = "not a stamp".to_owned();
        assert_eq!(age(std::time::SystemTime::now(), &nonsense), None);
        assert_eq!(
            modified_stamp(&nonsense),
            std::time::SystemTime::UNIX_EPOCH,
            "an unreadable stamp sinks, it does not float"
        );
    }
}
