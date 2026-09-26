//! The consumable dose lifecycle, owned by the tracker.
//!
//! A dose starts from a consumable's hotbar key, a manual start, or a paid
//! heal of a tool with an on-use buff. It is written as it starts, with an
//! absolute expiry, so the lifecycle survives any restart: the running doses
//! are read back when the tracker starts and after a correction elsewhere.
//! The tracker is the only writer, so starts, re-doses, removals, restores,
//! and expiries are serialised with the shots and heals they reprice.
//!
//! Expiry is a comparison with the injected clock, made before every message
//! the tracker handles; a dose that ended since the last message is closed at
//! its own expiry, not at the time it was noticed. A wake-up at the next
//! expiry only nudges that same comparison, so readouts and the context
//! interval close on time with no event arriving; the clock, never the
//! wake-up, decides.
//!
//! A dose of a cost-tracked item books its cost once, in the context it was
//! taken in, to the session running then; none books outside a session. A
//! timed dose opens a stacking `Consumable` interval in the running session
//! (a session starting while a dose runs opens one for it), so later events
//! carry the context of the doses in force. Whenever the running doses
//! change, the reload speed in effect is recomputed and, when it moved, the
//! carried weapons and the held healer are re-priced under it.

use std::time::Duration;

use crate::consumables::{
    adjust_session_consumable_cost, effects_reload_speed_percent, insert_dose, read_dose,
    read_running_doses, set_interval, set_removed, set_superseded, ConsumableProfile, DoseRecord,
    DoseRemoval, DoseSource,
};
use crate::db::DbError;
use crate::passive_effects::{
    equipped_reload_magnitudes, reload_seconds_under, reload_speed_in_effect,
};
use crate::ped::Ped;

use super::actor::{TrackerActor, TrackerMsg};
use super::intervals::{IntervalKind, IntervalSpec};
use super::time::{instant_to_epoch, resolve_local, to_iso_utc};

/// How long after a dose's expiry the wake-up fires, so the clock reads
/// past the expiry when it does.
const WAKE_GRACE_SECONDS: f64 = 0.05;

/// The longest a single wake-up sleeps; a longer wait re-arms on waking.
const WAKE_MAX_SECONDS: f64 = 3600.0;

/// A dose to start: the item and one dose of it as it stands now.
#[derive(Debug, Clone, PartialEq)]
pub struct DoseStart {
    pub equipment_id: i64,
    pub item_name: String,
    pub profile: ConsumableProfile,
}

/// Why a dose command changed nothing.
#[derive(Debug, thiserror::Error)]
pub enum DoseError {
    #[error("No such dose")]
    NotFound,
    #[error("{0}")]
    Refused(&'static str),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// The tracker's dose state: the doses still running, and the pending
/// wake-up at the earliest expiry.
#[derive(Default)]
pub(super) struct DoseRuntime {
    pub(super) running: Vec<DoseRecord>,
    wake: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for DoseRuntime {
    fn drop(&mut self) {
        if let Some(wake) = self.wake.take() {
            wake.abort();
        }
    }
}

/// The context interval a timed dose stands for in the running session.
/// A heal's on-use buff lasts seconds and belongs to the heal's own
/// provenance, so it opens none.
fn interval_spec(dose: &DoseRecord) -> Option<IntervalSpec> {
    if dose.source == DoseSource::OnUse || dose.ends_at() <= dose.started_at {
        return None;
    }
    let reload = effects_reload_speed_percent(&dose.effects);
    Some(
        IntervalSpec::new(IntervalKind::Consumable)
            .label(Some(dose.item_name.clone()))
            .ref_id(dose.equipment_id)
            .magnitude((reload != 0.0).then_some(reload))
            .stacking(),
    )
}

impl TrackerActor {
    pub(super) fn epoch_now(&self) -> f64 {
        instant_to_epoch(resolve_local(self.clock.now()))
    }

    /// The reload speed in effect at `now`: the declared equipped sources
    /// and the doses running then, under the game's limits.
    pub(super) fn reload_speed_at(&self, now: f64) -> f64 {
        let sources = self.providers.config.passive_effect_sources();
        reload_speed_in_effect(
            equipped_reload_magnitudes(&sources),
            self.doses
                .running
                .iter()
                .filter(|dose| dose.in_effect_at(now))
                .map(|dose| effects_reload_speed_percent(&dose.effects))
                .filter(|percent| *percent != 0.0),
        )
    }

    /// Adopt the persisted running doses: the tracker's start, and the
    /// re-read after a correction elsewhere moved one. A failed read keeps
    /// what memory holds.
    pub(super) async fn restore_doses(&mut self) {
        let now = self.epoch_now();
        match self
            .db
            .with_reader(move |conn| read_running_doses(conn, now))
            .await
        {
            Ok(running) => self.doses.running = running,
            Err(error) => tracing::warn!(
                target: "eo::tracker",
                %error,
                "running doses could not be read; the readout keeps what it had",
            ),
        }
        self.doses_changed(now, false).await;
    }

    /// End every dose whose expiry has passed, each at its own expiry.
    /// Runs before every message, so nothing is priced or stamped under a
    /// dose that has already ended.
    pub(super) async fn sweep_doses(&mut self) {
        let now = self.epoch_now();
        if !self.doses.running.iter().any(|dose| dose.ends_at() <= now) {
            return;
        }
        let (mut ended, running): (Vec<_>, Vec<_>) = std::mem::take(&mut self.doses.running)
            .into_iter()
            .partition(|dose| dose.ends_at() <= now);
        self.doses.running = running;
        ended.sort_by(|a, b| a.ends_at().total_cmp(&b.ends_at()));
        let db = self.db.clone();
        if let Some(active) = self.session.active_mut() {
            let session_id = active.session.id.clone();
            for dose in &ended {
                if let Some(interval_id) =
                    dose.interval_id.filter(|id| active.intervals.is_open(*id))
                {
                    let _ = active
                        .intervals
                        .close_ids(&db, &session_id, dose.ends_at(), &[interval_id])
                        .await;
                }
            }
        }
        self.doses_changed(now, true).await;
    }

    /// Publish the running doses, re-price under the reload speed they put
    /// in effect, re-arm the wake-up, and (for a change the readouts have
    /// not caused themselves) announce it.
    async fn doses_changed(&mut self, now: f64, announce: bool) {
        self.providers
            .doses
            .publish(self.doses.running.iter().map(DoseRecord::live).collect());
        self.reprice_for_reload(now);
        self.arm_dose_wake(now);
        if announce {
            self.announce_doses(now);
        }
    }

    /// Re-price the session's weapons and held healer when the reload speed
    /// in effect moved.
    pub(super) fn reprice_for_reload(&mut self, now: f64) {
        let speed = self.reload_speed_at(now);
        let Self {
            session, providers, ..
        } = self;
        let Some(active) = session.active_mut() else {
            return;
        };
        if (active.reload_speed_percent - speed).abs() < 1e-9 {
            return;
        }
        active.reload_speed_percent = speed;
        let equipment = providers.equipment.clone();
        active.weapons.reprice(equipment.carried_weapons(), |name| {
            equipment
                .weapon_profile(name)
                .filter(|profile| !profile.is_empty())
                .map(|profile| std::sync::Arc::new(serde_json::Value::Object(profile)))
        });
        if let Some(intent) = active.healing.intent.as_mut() {
            intent.apply_reload_speed(speed);
        }
        active.dirty = true;
    }

    fn arm_dose_wake(&mut self, now: f64) {
        if let Some(wake) = self.doses.wake.take() {
            wake.abort();
        }
        let Some(next) = self
            .doses
            .running
            .iter()
            .map(DoseRecord::ends_at)
            .filter(|ends_at| *ends_at > now)
            .reduce(f64::min)
        else {
            return;
        };
        let delay = ((next - now) + WAKE_GRACE_SECONDS).clamp(0.0, WAKE_MAX_SECONDS);
        let sender = self.sender.clone();
        self.doses.wake = Some(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(delay)).await;
            let _ = sender.send(TrackerMsg::DoseWake);
        }));
    }

    fn announce_doses(&self, now: f64) {
        use eo_wire::domain_events::{
            ConsumablesUpdated, ConsumablesUpdatedPayload, ConsumablesUpdatedTag,
        };
        self.bus
            .publish(&crate::bus_events::BusEvent::ConsumablesUpdated(
                ConsumablesUpdated {
                    topic: ConsumablesUpdatedTag,
                    event_version: 1,
                    occurred_at: to_iso_utc(now),
                    payload: ConsumablesUpdatedPayload {},
                },
            ));
    }

    /// Start a dose now. A running dose of the same item ends where this one
    /// starts (the two never stack); a cost-tracked item books its dose cost
    /// to the running session. `activation_id` names the paid heal that
    /// opened an on-use buff.
    pub(super) async fn start_dose(
        &mut self,
        start: DoseStart,
        source: DoseSource,
        activation_id: Option<String>,
    ) -> Result<DoseRecord, DoseError> {
        let now = self.epoch_now();
        let prior = self
            .doses
            .running
            .iter()
            .find(|dose| dose.equipment_id == Some(start.equipment_id) && dose.in_effect_at(now))
            .cloned();
        let session = self
            .session
            .active()
            .map(|active| (active.session.id.clone(), active.intervals.context_id()));
        let cost_ped = if session.is_some() {
            start.profile.booked_cost_ped()
        } else {
            0.0
        };
        let mut dose = DoseRecord {
            id: uuid::Uuid::new_v4().to_string(),
            equipment_id: Some(start.equipment_id),
            item_name: start.item_name,
            source,
            session_id: session.as_ref().map(|(id, _)| id.clone()),
            context_id: session.as_ref().and_then(|(_, context)| *context),
            interval_id: None,
            started_at: now,
            expires_at: now + start.profile.duration_seconds,
            cost_ped,
            cost_tracked: start.profile.track_cost,
            effects: start.profile.effects,
            healing_activation_id: activation_id,
            supersedes_dose_id: prior.as_ref().map(|prior| prior.id.clone()),
            superseded_at: None,
            removed_at: None,
            removed_by: None,
        };
        let prior_id = prior.as_ref().map(|prior| prior.id.clone());
        let close: Vec<i64> = prior.iter().filter_map(|prior| prior.interval_id).collect();
        let open = interval_spec(&dose);
        let write = dose.clone();
        let writes = move |tx: &rusqlite::Transaction<'_>, interval_id: Option<i64>| {
            if let Some(prior) = &prior_id {
                set_superseded(tx, prior, Some(write.started_at))?;
            }
            let mut write = write;
            write.interval_id = interval_id;
            insert_dose(tx, &write)?;
            if let (Some(session_id), true) = (&write.session_id, write.cost_ped > 0.0) {
                adjust_session_consumable_cost(tx, session_id, write.cost_ped)?;
            }
            Ok(())
        };
        dose.interval_id = self.write_dose_change(now, &close, open, writes).await?;
        if let Some(active) = self.session.active_mut() {
            active.consumable_cost += Ped(cost_ped);
            active.dirty = true;
        }
        if let Some(prior) = prior {
            self.doses.running.retain(|running| running.id != prior.id);
        }
        if dose.ends_at() > now {
            self.doses.running.push(dose.clone());
        }
        self.doses_changed(now, true).await;
        Ok(dose)
    }

    /// Remove a dose: its effect stops counting, any cost it booked comes
    /// back off its session, and the earlier dose of the item it ended runs
    /// on in its place: again until its own expiry, or, when a later dose
    /// had already replaced this one, until that later dose started.
    /// Removing a removed dose changes nothing.
    pub(super) async fn remove_dose(
        &mut self,
        id: &str,
        by: DoseRemoval,
    ) -> Result<DoseRecord, DoseError> {
        let now = self.epoch_now();
        let dose = self.read_dose(id).await?;
        if dose.removed_at.is_some() {
            return Ok(dose);
        }
        if by == DoseRemoval::Player && dose.source == DoseSource::OnUse {
            return Err(DoseError::Refused(
                "A heal's buff goes with the heal; correct the heal instead",
            ));
        }
        let prior = self
            .standing_prior(&dose)
            .await?
            .filter(|prior| prior.superseded_at == Some(dose.started_at));
        let mut prior_back = prior.map(|mut prior| {
            prior.superseded_at = dose.superseded_at;
            prior
        });
        let close: Vec<i64> = dose.interval_id.into_iter().collect();
        let open = prior_back
            .as_ref()
            .filter(|prior| prior.in_effect_at(now))
            .and_then(interval_spec);
        let write = dose.clone();
        let prior_write = prior_back
            .as_ref()
            .map(|prior| (prior.id.clone(), prior.superseded_at));
        let writes = move |tx: &rusqlite::Transaction<'_>, interval_id: Option<i64>| {
            set_removed(tx, &write.id, Some((now, by)))?;
            if let (Some(session_id), true) = (&write.session_id, write.cost_ped > 0.0) {
                adjust_session_consumable_cost(tx, session_id, -write.cost_ped)?;
            }
            if let Some((prior, ends)) = &prior_write {
                set_superseded(tx, prior, *ends)?;
                if interval_id.is_some() {
                    set_interval(tx, prior, interval_id)?;
                }
            }
            Ok(())
        };
        let interval_id = self.write_dose_change(now, &close, open, writes).await?;
        if let Some(prior) = prior_back.as_mut() {
            if interval_id.is_some() {
                prior.interval_id = interval_id;
            }
        }
        self.adjust_live_cost(&dose, -dose.cost_ped);
        self.doses.running.retain(|running| running.id != dose.id);
        if let Some(prior) = prior_back.filter(|prior| prior.ends_at() > now) {
            self.doses.running.push(prior);
        }
        self.doses_changed(now, true).await;
        let mut removed = dose;
        removed.removed_at = Some(now);
        removed.removed_by = Some(by);
        Ok(removed)
    }

    /// Give a removed dose back exactly: its cost returns to its session and
    /// its effect counts again for whatever is left of its run, ending the
    /// earlier dose of its item where it started, as before. A dose a heal
    /// correction removed comes back only when that correction is undone.
    pub(super) async fn restore_dose(
        &mut self,
        id: &str,
        by: DoseRemoval,
    ) -> Result<DoseRecord, DoseError> {
        let now = self.epoch_now();
        let mut dose = self.read_dose(id).await?;
        if dose.removed_at.is_none() {
            return Ok(dose);
        }
        if dose.removed_by != Some(by) {
            return Err(DoseError::Refused(
                "This buff was removed with its heal; undo the heal's correction to restore it",
            ));
        }
        dose.removed_at = None;
        dose.removed_by = None;
        let running_now = dose.in_effect_at(now);
        // The earlier dose runs where this one would: to the same end. This
        // dose takes that stretch back from it, as it did when it started.
        let prior = self.standing_prior(&dose).await?.filter(|prior| {
            prior.started_at <= dose.started_at && prior.superseded_at == dose.superseded_at
        });
        // Any other running dose of the item would stack with it.
        if running_now
            && self.doses.running.iter().any(|running| {
                running.equipment_id == dose.equipment_id
                    && running.id != dose.id
                    && prior.as_ref().is_none_or(|prior| prior.id != running.id)
            })
        {
            return Err(DoseError::Refused(
                "A later dose of this item is running; remove it first",
            ));
        }
        let close: Vec<i64> = prior.iter().filter_map(|prior| prior.interval_id).collect();
        let open = running_now.then(|| interval_spec(&dose)).flatten();
        let write = dose.clone();
        let prior_write = prior.as_ref().map(|prior| prior.id.clone());
        let writes = move |tx: &rusqlite::Transaction<'_>, interval_id: Option<i64>| {
            set_removed(tx, &write.id, None)?;
            if let (Some(session_id), true) = (&write.session_id, write.cost_ped > 0.0) {
                adjust_session_consumable_cost(tx, session_id, write.cost_ped)?;
            }
            if let Some(prior) = &prior_write {
                set_superseded(tx, prior, Some(write.started_at))?;
            }
            if interval_id.is_some() {
                set_interval(tx, &write.id, interval_id)?;
            }
            Ok(())
        };
        if let Some(interval_id) = self.write_dose_change(now, &close, open, writes).await? {
            dose.interval_id = Some(interval_id);
        }
        self.adjust_live_cost(&dose, dose.cost_ped);
        if let Some(prior) = &prior {
            self.doses.running.retain(|running| running.id != prior.id);
        }
        if running_now {
            self.doses.running.push(dose.clone());
        }
        self.doses_changed(now, true).await;
        Ok(dose)
    }

    /// Open a context interval for every running dose that has none standing
    /// in the session that just started, so its play carries their context.
    pub(super) async fn open_running_dose_intervals(&mut self) {
        let now = self.epoch_now();
        let db = self.db.clone();
        let Some(active) = self.session.active_mut() else {
            return;
        };
        let session_id = active.session.id.clone();
        for dose in &mut self.doses.running {
            if !dose.in_effect_at(now) {
                continue;
            }
            let Some(spec) = interval_spec(dose) else {
                continue;
            };
            let dose_id = dose.id.clone();
            let opened = active
                .intervals
                .transition_with(
                    &db,
                    &session_id,
                    now,
                    &[],
                    Some(spec),
                    move |tx, interval_id, _| set_interval(tx, &dose_id, interval_id),
                )
                .await;
            if let Ok((interval_id, ())) = opened {
                dose.interval_id = interval_id;
            }
        }
    }

    /// Commit one dose change: through the interval engine when a context
    /// interval opens or closes in the running session (so the rows and the
    /// context move in one transaction), else as a plain write. Returns the
    /// interval opened, if any.
    async fn write_dose_change(
        &mut self,
        now: f64,
        close: &[i64],
        open: Option<IntervalSpec>,
        writes: impl FnOnce(&rusqlite::Transaction<'_>, Option<i64>) -> Result<(), DbError>
            + Send
            + 'static,
    ) -> Result<Option<i64>, DoseError> {
        let db = self.db.clone();
        if let Some(active) = self.session.active_mut() {
            let close: Vec<i64> = close
                .iter()
                .copied()
                .filter(|id| active.intervals.is_open(*id))
                .collect();
            if !close.is_empty() || open.is_some() {
                let session_id = active.session.id.clone();
                let (interval_id, ()) = active
                    .intervals
                    .transition_with(&db, &session_id, now, &close, open, move |tx, id, _| {
                        writes(tx, id)
                    })
                    .await?;
                return Ok(interval_id);
            }
        }
        db.with_writer(move |conn| {
            let tx = conn.transaction()?;
            writes(&tx, None)?;
            tx.commit()?;
            Ok(())
        })
        .await?;
        Ok(None)
    }

    /// The dose this one replaced, looking past any since removed: the
    /// earliest link of the item's re-dose chain still standing.
    async fn standing_prior(&self, dose: &DoseRecord) -> Result<Option<DoseRecord>, DoseError> {
        let mut link = dose.supersedes_dose_id.clone();
        let mut seen = vec![dose.id.clone()];
        while let Some(id) = link {
            if seen.contains(&id) {
                break;
            }
            let prior = self.read_dose(&id).await?;
            if prior.removed_at.is_none() {
                return Ok(Some(prior));
            }
            seen.push(id);
            link = prior.supersedes_dose_id.clone();
        }
        Ok(None)
    }

    async fn read_dose(&self, id: &str) -> Result<DoseRecord, DoseError> {
        let id = id.to_string();
        self.db
            .with_reader(move |conn| read_dose(conn, &id))
            .await?
            .ok_or(DoseError::NotFound)
    }

    /// Mirror a cost moved on the running session's row in its live total.
    fn adjust_live_cost(&mut self, dose: &DoseRecord, delta: f64) {
        if let Some(active) = self.session.active_mut() {
            if dose.session_id.as_deref() == Some(active.session.id.as_str()) {
                active.consumable_cost += Ped(delta);
                active.dirty = true;
            }
        }
    }
}

impl super::healing::HealingIntent {
    /// Re-time the held healer's reload under a new reload speed in effect.
    pub(super) fn apply_reload_speed(&mut self, speed_percent: f64) {
        let Some(base) = self.profile.base_reload_seconds else {
            return;
        };
        let effective = reload_seconds_under(base, speed_percent);
        self.reload_seconds = effective;
        self.profile.reload_speed_percent = Some(speed_percent);
        self.profile.effective_reload_seconds = Some(effective);
    }
}
