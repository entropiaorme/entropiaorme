//! A session whose readings stop before it ends (a sign-in dropped, the
//! app closed) or start after it began, measured across the gap from the
//! nearest reading on its far side.
//!
//! The step from the session's last reading to the one the gap ends at
//! holds everything that changed in between: the session's own play, and
//! whatever the player did after it. It is the session's only when the
//! time it spends outside the session is shown clean, and then the
//! session's costs are billed from the readings either side of the gap.
//! Shown clean means all of:
//!
//! - the game's chat log covers that time, and holds none of the lines
//!   play leaves ([`PLAY_LINES`]) in it. A hole in the log (the game
//!   closed, so nothing was written) reads as clean: no line shows play.
//!   What guards that case is the next rule, which never lets a cost of a
//!   kind the session did not play into its bill;
//! - the bracketing steps hold no repair (any repair refuses, because the
//!   step nets the wear the repair restored away, so the decay it shows is
//!   not the session's whole wear), and no cost in a Cycled bucket the
//!   session's own tracked play never touched, so every cost in them is
//!   the session's kind of play;
//! - overhead (travel, field logistics, what the app cannot place) that
//!   changed in a bracketing step is never billed to the session: the
//!   player may have moved after it, and no chat line shows a teleport.
//!   Those buckets stay unmeasured.
//!
//! Otherwise every bucket keeps its tracked figure, with the reason.
//!
//! What the chat log showed is a verdict taken once per span (the readings
//! either side of the gap and the session's start and end): it is kept
//! apart from the projection, so a rebuild over the same span (a rule's
//! version moving, an item placed differently) takes the kept verdict and
//! never reads the log again, and a log that changes later changes
//! nothing. A new span takes a new verdict. Nothing is kept, and the
//! session is weighed again on a later read, while the gap cannot be
//! weighed yet: the reading the gap ends at has not come, the log's time
//! base is not known, or the log has not been written past the gap.

use std::collections::BTreeSet;

use super::costs::{
    verdict_over, whole_milliseconds, Basis, BucketCost, Charge, CostBucket, GapBound, GapBounds,
    GapSpan, GapVerdict, LineReason, LogVerdict, SessionCosts, Unmeasured,
};
use super::measure::SessionWindow;
use super::roles::KnownRoles;
use super::store::{compute_gap_measure, StoredAcrossGap, StoredGapEnd, StoredGapMeasure};
use super::AccountError;
use crate::chatlog_parser::EventType;
use crate::chatlog_span::{ChatLogSource, SpanCheck, Unchecked};
use crate::session_costs::Bucket;

/// The chat log lines that are the player's own play, read from the
/// parser's event kinds: shots and their outcomes, hits taken and avoided,
/// heals, a failed harvest, and an enhancer breaking. Loot is left out:
/// recycling, a Trade Terminal conversion and a strongbox opening may each
/// print a line of what arrived, and a gap bills only costs, never loot,
/// so an arrival alone is no play. Skill gains are left out too (a
/// teleport chip gains skill), as are globals (anyone's) and missions (a
/// hand-in is not play that costs).
pub const PLAY_LINES: [EventType; 15] = [
    EventType::DamageDealt,
    EventType::CriticalHit,
    EventType::TargetDodge,
    EventType::TargetEvade,
    EventType::TargetJam,
    EventType::TargetMiss,
    EventType::DamageReceived,
    EventType::PlayerDodge,
    EventType::PlayerEvade,
    EventType::PlayerJam,
    EventType::MobMiss,
    EventType::Deflect,
    EventType::SelfHeal,
    EventType::HarvestFail,
    EventType::EnhancerBreak,
];

/// The costs of a sealed session whose measure (`partial`'s) leaves an end
/// unread: billed across the gap when it is shown clean, otherwise
/// `partial` with the reason the gap could not be billed. A read: run it
/// on a reader, off any runtime worker (it may read the chat log).
pub(super) fn across_gap(
    conn: &rusqlite::Connection,
    session_id: &str,
    window: &SessionWindow,
    roles: &KnownRoles,
    chat_log: Option<&ChatLogSource>,
    partial: SessionCosts,
) -> Result<SessionCosts, AccountError> {
    let Some(ended) = window.ended_at else {
        return Ok(partial);
    };
    let gap = match compute_gap_measure(conn, session_id, window, roles)? {
        StoredGapMeasure::NotOffered => return Ok(partial),
        StoredGapMeasure::Awaiting => {
            return Ok(partial.refused(Unmeasured::AwaitingGapReading));
        }
        StoredGapMeasure::Offered(gap) => *gap,
    };
    let span = GapSpan {
        opening_reading_id: gap.opening.as_ref().map(|end| end.reading.0),
        closing_reading_id: gap.closing.as_ref().map(|end| end.reading.0),
        started_ms: whole_milliseconds(window.started_at),
        ended_ms: whole_milliseconds(ended),
    };
    let verdict = match verdict_over(conn, session_id, span)? {
        Some(kept) => Some(kept),
        None => {
            let mut spans = Vec::with_capacity(2);
            if let Some(opening) = &gap.opening {
                spans.push((opening.taken_at, window.started_at));
            }
            if let Some(closing) = &gap.closing {
                spans.push((ended, closing.taken_at));
            }
            match read_the_log(chat_log, &spans) {
                Ok(verdict) => Some(verdict),
                Err(Passing::NoLog) => None,
                Err(Passing::Pending(reason)) => return Ok(partial.refused(reason)),
            }
        }
    };
    let kept = verdict.map(|verdict| GapVerdict { span, verdict });
    let refused = |reason: Unmeasured| -> Result<SessionCosts, AccountError> {
        Ok(partial.clone().refused(reason).with_verdict(kept))
    };
    match verdict {
        None => return refused(Unmeasured::ChatLogUnchecked),
        Some(LogVerdict::PlayInGap) => return refused(Unmeasured::PlayInGap),
        Some(LogVerdict::MissesGap) => return refused(Unmeasured::ChatLogMissesGap),
        Some(LogVerdict::Unchecked) => return refused(Unmeasured::ChatLogUnchecked),
        Some(LogVerdict::Clean) => {}
    }
    let in_steps = step_buckets(&gap, roles, window.started_at)?;
    if !accounted_for(conn, session_id, &gap, &in_steps)? {
        return refused(Unmeasured::GapUnaccounted);
    }
    let bounds = GapBounds {
        first_reading_id: gap.first_reading.0,
        last_reading_id: gap.last_reading.0,
        opening: gap.opening.as_ref().map(bound),
        closing: gap.closing.as_ref().map(bound),
    };
    let mut costs =
        SessionCosts::measured_across_gap(&gap.gap.measure, roles, bounds, window.started_at)?;
    // Overhead a bracketing step holds may be the player's after the
    // session: it is not billed to it.
    for cost in &mut costs.buckets {
        if !cost.bucket.is_cycled() && in_steps.contains(&cost.bucket) {
            *cost = BucketCost {
                bucket: cost.bucket,
                basis: Basis::Unmeasured(Unmeasured::OverheadInGap),
            };
        }
    }
    Ok(costs.with_verdict(kept))
}

fn bound(end: &StoredGapEnd) -> GapBound {
    GapBound {
        reading_id: end.reading.0,
        outside_seconds: whole_seconds(end.end.outside_seconds),
        gap_seconds: whole_seconds(end.end.gap_seconds),
    }
}

fn whole_seconds(seconds: f64) -> i64 {
    // A span between two readings' times: finite and far inside i64.
    seconds.max(0.0).round() as i64
}

/// Why the log gave no verdict.
enum Passing {
    /// There is no log to read: nothing is kept.
    NoLog,
    /// The gap cannot be weighed yet, for this reason: nothing is kept,
    /// and a later read tries again.
    Pending(Unmeasured),
}

/// What the chat log shows over the spans.
fn read_the_log(
    chat_log: Option<&ChatLogSource>,
    spans: &[(f64, f64)],
) -> Result<LogVerdict, Passing> {
    let Some(chat_log) = chat_log else {
        return Err(Passing::NoLog);
    };
    let mut checks = Vec::with_capacity(spans.len());
    for &(from, to) in spans {
        let check = chat_log.check(from, to, &PLAY_LINES);
        if matches!(check, SpanCheck::Holds(_)) {
            return Ok(LogVerdict::PlayInGap);
        }
        checks.push(check);
    }
    if checks.contains(&SpanCheck::StartsLater) {
        Ok(LogVerdict::MissesGap)
    } else if checks.contains(&SpanCheck::Unchecked(Unchecked::TimeBaseUnknown)) {
        Err(Passing::Pending(Unmeasured::GapCheckPending))
    } else if checks.contains(&SpanCheck::NotYetPast) {
        Err(Passing::Pending(Unmeasured::ChatLogNotPastGap))
    } else if checks
        .iter()
        .any(|check| matches!(check, SpanCheck::Unchecked(_)))
    {
        Ok(LogVerdict::Unchecked)
    } else {
        Ok(LogVerdict::Clean)
    }
}

/// The buckets the bracketing steps hold any cost in, measured alone.
fn step_buckets(
    gap: &StoredAcrossGap,
    roles: &KnownRoles,
    started_at: f64,
) -> Result<BTreeSet<CostBucket>, AccountError> {
    let mut buckets = BTreeSet::new();
    for step in &gap.gap.steps {
        let costs = SessionCosts::running(step, roles, started_at)?;
        // A stack nothing names that left is no cost until it is placed.
        buckets.extend(
            costs
                .lines
                .iter()
                .filter(|line| line.charge != Charge::Nothing(LineReason::NotPlaced))
                .map(|line| line.bucket),
        );
    }
    Ok(buckets)
}

/// Whether everything the bracketing steps hold (`in_steps`, the buckets
/// they hold any cost in) is the session's kind of play: no repair, and no
/// cost in a Cycled bucket the session's own tracked play never touched.
fn accounted_for(
    conn: &rusqlite::Connection,
    session_id: &str,
    gap: &StoredAcrossGap,
    in_steps: &BTreeSet<CostBucket>,
) -> Result<bool, AccountError> {
    if gap.gap.steps.iter().any(|step| !step.repairs.is_empty()) {
        return Ok(false);
    }
    let played = crate::session_costs::played(conn, session_id)?;
    Ok(in_steps
        .iter()
        .filter(|bucket| bucket.is_cycled())
        .all(|bucket| cycled(*bucket).is_some_and(|bucket| played.contains(&bucket))))
}

/// A Cycled bucket as the resolver names it.
fn cycled(bucket: CostBucket) -> Option<Bucket> {
    Some(match bucket {
        CostBucket::Weapon => Bucket::Weapon,
        CostBucket::Enhancer => Bucket::Enhancer,
        CostBucket::Harvesting => Bucket::Harvesting,
        CostBucket::Healing => Bucket::Healing,
        CostBucket::Armour => Bucket::Armour,
        CostBucket::Consumables => Bucket::Consumables,
        CostBucket::Travel | CostBucket::FieldLogistics | CostBucket::Unclassified => return None,
    })
}
