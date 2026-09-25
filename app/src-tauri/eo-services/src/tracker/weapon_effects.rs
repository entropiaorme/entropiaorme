//! Damage-over-time effect windows.
//!
//! A paid hit of a weapon with a declared damage-over-time effect opens a
//! window for the effect's duration, measured from the moment the hit was
//! observed through the injected clock. The window goes into attribution at
//! once (the very next line may be its first tick) and into the database
//! straight after, so it outlives the process: every session start reads
//! back each window whose absolute expiry is still ahead, whichever session
//! paid for it, and a restart mid-effect or a new session keeps treating
//! its ticks as the outcomes they are. Expiry is a comparison with the
//! clock, never a timer, so a window closes once however often the state
//! is read.
//!
//! The window carries no cost: the activation's price lives with its shot,
//! in the kill the shot settles into. A failed write keeps the window in
//! memory (its ticks in this process are still ticks) and says once that a
//! restart would lose it.

use crate::db::DbError;
use crate::ped::Ped;
use crate::weapon_effect::WeaponEffectProfile;

use super::actor::TrackerActor;
use super::attribution::{Observation, OffensiveEffectWindow, SWITCH_TAIL_SECONDS};
use super::session::ActiveSession;

/// One paid hit that opened an effect, as its window row keeps it.
#[derive(Debug, Clone)]
pub(super) struct EffectActivation {
    pub(super) window: OffensiveEffectWindow,
    pub(super) session_id: String,
    pub(super) context_id: Option<i64>,
    pub(super) hit_amount: f64,
    pub(super) critical: bool,
    pub(super) cost_per_shot: Ped,
    pub(super) profile: WeaponEffectProfile,
}

impl EffectActivation {
    /// The effect a priced shot opens: a hit (a jam, dodge, evade, or miss lands
    /// nothing, so starts nothing) of a carried weapon that declares one.
    pub(super) fn for_shot(
        active: &ActiveSession,
        tool: &str,
        observation: Observation,
        observed_at: f64,
        cost_per_shot: Ped,
    ) -> Option<Self> {
        let Observation::Hit { amount, critical } = observation else {
            return None;
        };
        let (weapon, profile) = active.weapons.attribution.effect_of(tool)?;
        Some(Self {
            window: OffensiveEffectWindow {
                id: uuid::Uuid::new_v4().to_string(),
                tool: weapon.name.clone(),
                equipment_id: Some(weapon.equipment_id),
                started_at: observed_at,
                expires_at: observed_at + profile.duration_seconds,
                tick: profile.tick_band(),
            },
            session_id: active.session.id.clone(),
            context_id: active.intervals.context_id(),
            hit_amount: amount,
            critical,
            cost_per_shot,
            profile: profile.clone(),
        })
    }

    fn insert(&self, conn: &rusqlite::Connection) -> Result<(), DbError> {
        let profile = serde_json::to_string(&self.profile).map_err(|source| DbError::Decode {
            context: "weapon effect profile encode",
            source,
        })?;
        conn.execute(
            "INSERT INTO weapon_effect_windows \
             (id, session_id, equipment_id, tool_name, context_id, started_at, expires_at, \
              hit_amount, critical, cost_per_shot, tick_min, tick_max, profile_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                self.window.id,
                self.session_id,
                self.window.equipment_id,
                self.window.tool,
                self.context_id,
                self.window.started_at,
                self.window.expires_at,
                self.hit_amount,
                i64::from(self.critical),
                self.cost_per_shot.value(),
                self.window.tick.min,
                self.window.tick.max,
                profile,
            ],
        )?;
        Ok(())
    }
}

/// Every window still able to explain a tick at `now`: its expiry, plus the
/// delivery tail, is not yet past, and no decision took it back.
pub(super) fn read_live_effect_windows(
    conn: &rusqlite::Connection,
    now: f64,
) -> Result<Vec<OffensiveEffectWindow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, tool_name, equipment_id, started_at, expires_at, tick_min, tick_max \
         FROM weapon_effect_windows WHERE expires_at >= ?1 AND withdrawn_at IS NULL \
         ORDER BY started_at, id",
    )?;
    let rows = stmt.query_map([now - SWITCH_TAIL_SECONDS], |row| {
        Ok(OffensiveEffectWindow {
            id: row.get(0)?,
            tool: row.get(1)?,
            equipment_id: row.get(2)?,
            started_at: row.get(3)?,
            expires_at: row.get(4)?,
            tick: crate::tracker::DamageBand {
                min: row.get(5)?,
                max: row.get(6)?,
            },
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

impl TrackerActor {
    /// Adopt the persisted windows as of `now`: an effect paid for before
    /// this session (in the previous one, or before a restart) keeps
    /// ticking into it. A failed read leaves attribution without them; the
    /// session keeps tracking, and their ticks are then judged by band.
    pub(super) async fn restore_persisted_weapon_effects(&mut self, now: f64) {
        if self.session.active().is_none() {
            return;
        }
        match self
            .db
            .with_reader(move |conn| read_live_effect_windows(conn, now))
            .await
        {
            Ok(windows) => {
                if let Some(active) = self.session.active_mut() {
                    active.weapons.attribution.set_effect_windows(windows);
                }
            }
            Err(error) => tracing::warn!(
                target: "eo::tracker",
                %error,
                "persisted weapon effects could not be read; earlier effects are not carried over",
            ),
        }
    }

    /// Persist a window the tracker already opened in memory.
    pub(super) async fn persist_effect_activation(&mut self, activation: EffectActivation) {
        let stored = self
            .db
            .with_writer(move |conn| activation.insert(conn))
            .await;
        if let Err(error) = stored {
            tracing::error!(target: "eo::tracker", %error, "weapon effect window write failed");
            if let Some(active) = self.session.active_mut() {
                let message =
                    "A weapon effect could not be saved; after a restart its ticks are judged by damage alone";
                if !active.warnings.iter().any(|warning| warning == message) {
                    active.warnings.push(message.to_string());
                }
                active.dirty = true;
            }
        }
    }
}
