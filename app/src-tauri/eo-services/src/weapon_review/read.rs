//! Review reads: a session's stored shots of one group, the weapons an
//! unpriced shot could be assigned to, and the session-detail summary with
//! the damage-over-time effects that ticked in the session.

use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use super::{
    BulkAssignOption, CorrectionWeapon, EffectCandidate, ReviewShot, ReviewShotPage, ShotCandidate,
    ShotGroup, UnpricedExposure, WeaponCorrectionKind, WeaponReviewError, UNDECLARED_WARNING_SHOTS,
};
use crate::attack_rate::WeaponPricing;
use crate::cost_engine::cost_per_shot_from_props;
use crate::db::DbError;
use crate::tracker::{damage_band_from_props, DamageBand, HitMargin};

/// How a weapon's stored props are prepared for pricing, when configured.
pub(super) type Pricing<'a> = Option<&'a WeaponPricing>;

/// A weapon as an assignment would book it now.
#[derive(Debug, Clone)]
pub(super) struct PricedWeapon {
    pub(super) name: String,
    /// Its per-shot cost in PED.
    pub(super) cost: f64,
    /// The damage band a hit is checked against, under the same preparation.
    pub(super) band: Option<DamageBand>,
}

/// A weapon as it is configured now, its props prepared by `pricing` under
/// the reload speed the shot was charged at (`reload_speed_percent`; a shot
/// stored before that was kept takes the reload speed in effect now), or
/// None when the item is gone or is not a weapon.
pub(super) fn weapon_price(
    conn: &rusqlite::Connection,
    equipment_id: i64,
    pricing: Pricing<'_>,
    reload_speed_percent: Option<f64>,
) -> Result<Option<PricedWeapon>, WeaponReviewError> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT name, properties_json FROM equipment_library \
             WHERE id = ?1 AND item_type = 'weapon'",
            [equipment_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((name, properties)) = row else {
        return Ok(None);
    };
    let props: Value = serde_json::from_str(&properties).map_err(|source| DbError::Decode {
        context: "weapon properties parse",
        source,
    })?;
    let props = match (pricing, reload_speed_percent) {
        (Some(pricing), Some(speed)) => pricing.prepare_at(&props, speed),
        (Some(pricing), None) => pricing.prepare(&props),
        (None, _) => props,
    };
    let cost = cost_per_shot_from_props(&props, None)
        .get("totalCostPerUse")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        / 100.0;
    let band = damage_band_from_props(&props);
    Ok(Some(PricedWeapon { name, cost, band }))
}

/// Whether a stored classification is a hit priced to the held weapon from
/// outside its band.
pub(super) fn is_margin(attribution: &str) -> bool {
    matches!(attribution, "boosted" | "reduced")
}

/// The margin holding a stored shot its candidate weapon did not fit when
/// it landed, judged against the weapon as it is configured now. A weapon
/// that fitted the shot keeps that verdict.
pub(super) fn stored_margin(
    candidate: &ShotCandidate,
    amount: Option<f64>,
    critical: bool,
    band: Option<DamageBand>,
) -> Option<HitMargin> {
    if candidate.fits {
        return None;
    }
    band?.margin(amount?, critical)
}

pub(super) fn parse_candidates(raw: &str) -> Result<Vec<ShotCandidate>, WeaponReviewError> {
    serde_json::from_str(raw).map_err(|_| WeaponReviewError::Stored("unreadable shot candidates"))
}

/// Whether an effect window still stands: it exists and no decision took
/// it back.
pub(super) fn window_stands(
    conn: &rusqlite::Connection,
    window_id: &str,
) -> Result<bool, WeaponReviewError> {
    Ok(conn
        .query_row(
            "SELECT withdrawn_at IS NULL FROM weapon_effect_windows WHERE id = ?1",
            [window_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

pub(super) fn parse_effect_candidates(
    raw: &str,
) -> Result<Vec<EffectCandidate>, WeaponReviewError> {
    serde_json::from_str(raw).map_err(|_| WeaponReviewError::Stored("unreadable effect candidates"))
}

pub(super) fn session_shots(
    conn: &rusqlite::Connection,
    session_id: &str,
    group: ShotGroup,
    offset: i64,
    limit: i64,
) -> Result<ReviewShotPage, WeaponReviewError> {
    let ended: bool = conn
        .query_row(
            "SELECT is_active = 0 FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false);
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM weapon_shot_evidence \
         WHERE session_id = ?1 AND attribution = ?2 AND (NOT ?3 OR review_id IS NULL)",
        rusqlite::params![session_id, group.attribution(), group.drops_decided()],
        |row| row.get(0),
    )?;
    let mut stmt = conn.prepare(
        "SELECT e.id, e.observed_at, e.amount, e.critical, e.reason, e.hotbar_tool, \
                e.tool_name, e.cost_per_shot, e.candidates_json, c.id, r.decision, \
                e.effect_candidates_json, e.effect_window_id, c.kind, c.effect_window_id \
         FROM weapon_shot_evidence e \
         LEFT JOIN weapon_attribution_corrections c \
                ON c.id = e.correction_id AND c.undone_at IS NULL \
         LEFT JOIN weapon_attribution_reviews r ON r.id = e.review_id \
         WHERE e.session_id = ?1 AND e.attribution = ?2 AND (NOT ?5 OR e.review_id IS NULL) \
         ORDER BY e.observed_at, e.id \
         LIMIT ?3 OFFSET ?4",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            session_id,
            group.attribution(),
            limit,
            offset,
            group.drops_decided()
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
            ))
        },
    )?;
    let mut shots = Vec::new();
    for row in rows {
        let (
            id,
            observed_at,
            amount,
            critical,
            reason,
            hotbar_tool,
            tool_name,
            cost_per_shot,
            candidates,
            correction_id,
            review_decision,
            effect_candidates,
            effect_window_id,
            correction_kind,
            correction_window_id,
        ) = row?;
        let correction_kind = match correction_kind.as_deref() {
            None => None,
            Some(kind) => Some(
                WeaponCorrectionKind::parse(kind)
                    .ok_or(WeaponReviewError::Stored("unknown correction kind"))?,
            ),
        };
        let mut effect_candidates = parse_effect_candidates(&effect_candidates)?;
        for candidate in &mut effect_candidates {
            candidate.standing = window_stands(conn, &candidate.window_id)?;
        }
        shots.push(ReviewShot {
            correctable: ended
                && correction_id.is_none()
                && match group {
                    ShotGroup::Evidence => false,
                    ShotGroup::Boosted | ShotGroup::Reduced => true,
                    ShotGroup::Unresolved | ShotGroup::EffectTick => tool_name.is_none(),
                },
            group,
            effect_candidates,
            effect_window_id,
            correction_kind,
            correction_window_id,
            id,
            observed_at,
            amount,
            critical,
            reason,
            hotbar_tool,
            tool_name,
            cost_per_shot,
            candidates: parse_candidates(&candidates)?,
            correction_id,
            review_decision,
        });
    }
    Ok(ReviewShotPage { shots, total })
}

pub(super) fn correction_weapons(
    conn: &rusqlite::Connection,
    evidence_id: &str,
    pricing: Pricing<'_>,
) -> Result<Vec<CorrectionWeapon>, WeaponReviewError> {
    type StoredOffer = (
        String,
        Option<f64>,
        Option<f64>,
        bool,
        Option<String>,
        String,
    );
    let stored: Option<StoredOffer> = conn
        .query_row(
            "SELECT candidates_json, reload_speed_percent, amount, critical, tool_name, \
                    attribution \
             FROM weapon_shot_evidence WHERE id = ?1",
            [evidence_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((candidates, reload_speed_percent, amount, critical, priced_to, attribution)) = stored
    else {
        return Err(WeaponReviewError::NotFound("Shot not found"));
    };
    let mut weapons = Vec::new();
    for candidate in parse_candidates(&candidates)? {
        // A margin hit is already priced to its held weapon: moving it there
        // would change nothing.
        if is_margin(&attribution) && priced_to.as_deref() == Some(candidate.name.as_str()) {
            continue;
        }
        if let Some(weapon) =
            weapon_price(conn, candidate.equipment_id, pricing, reload_speed_percent)?
        {
            let margin = stored_margin(&candidate, amount, critical, weapon.band);
            weapons.push(CorrectionWeapon {
                equipment_id: candidate.equipment_id,
                name: weapon.name,
                cost_per_shot_ped: weapon.cost,
                fits: candidate.fits,
                margin,
            });
        }
    }
    // Stable: fitting weapons first, then those holding the shot in their
    // margin, then the carried order.
    weapons.sort_by_key(|weapon| match (weapon.fits, weapon.margin) {
        (true, _) => 0,
        (false, Some(_)) => 1,
        (false, None) => 2,
    });
    Ok(weapons)
}

/// Shots of one session left without a price that share everything pricing
/// them needs: the carried weapons they remember, the reload speed they were
/// charged under, and the magnitude the weapons' margins are judged on.
pub(super) struct UnpricedGroup {
    pub(super) session_id: String,
    pub(super) candidates: Vec<ShotCandidate>,
    pub(super) amount: Option<f64>,
    pub(super) critical: bool,
    pub(super) reload_speed_percent: Option<f64>,
    pub(super) shots: i64,
}

/// The uncorrected unresolved shots of `session_ids`, grouped. A play
/// session's thousands of such shots fall into a handful of groups, since
/// the carried set and the reload speed rarely change within one.
pub(super) fn unpriced_groups(
    conn: &rusqlite::Connection,
    session_ids: &[String],
) -> Result<Vec<UnpricedGroup>, WeaponReviewError> {
    let mut groups = Vec::new();
    // Bounded batches keep the statement under SQLite's parameter limit.
    for chunk in session_ids.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT session_id, candidates_json, reload_speed_percent, amount, critical, \
                    COUNT(*) \
             FROM weapon_shot_evidence \
             WHERE attribution = 'unresolved' AND tool_name IS NULL \
               AND correction_id IS NULL AND session_id IN ({placeholders}) \
             GROUP BY session_id, candidates_json, reload_speed_percent, amount, critical"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        for row in rows {
            let (session_id, candidates, reload_speed_percent, amount, critical, shots) = row?;
            groups.push(UnpricedGroup {
                session_id,
                candidates: parse_candidates(&candidates)?,
                amount,
                critical,
                reload_speed_percent,
                shots,
            });
        }
    }
    Ok(groups)
}

/// A weapon at one reload speed (its bits, so it can key a map).
type PriceKey = (i64, Option<u64>);

/// Weapon prices as an assignment would book them now, each looked up once.
pub(super) struct PriceBook<'a> {
    conn: &'a rusqlite::Connection,
    pricing: Pricing<'a>,
    prices: std::collections::HashMap<PriceKey, Option<PricedWeapon>>,
}

impl<'a> PriceBook<'a> {
    pub(super) fn new(conn: &'a rusqlite::Connection, pricing: Pricing<'a>) -> Self {
        Self {
            conn,
            pricing,
            prices: std::collections::HashMap::new(),
        }
    }

    /// The weapon as priced at this reload speed; None when it has left
    /// Equipment.
    pub(super) fn price(
        &mut self,
        equipment_id: i64,
        reload_speed_percent: Option<f64>,
    ) -> Result<Option<PricedWeapon>, WeaponReviewError> {
        let key = (equipment_id, reload_speed_percent.map(f64::to_bits));
        if let Some(known) = self.prices.get(&key) {
            return Ok(known.clone());
        }
        let price = weapon_price(self.conn, equipment_id, self.pricing, reload_speed_percent)?;
        self.prices.insert(key, price.clone());
        Ok(price)
    }
}

/// One carried weapon that explains a group of unpriced shots.
struct Explaining {
    equipment_id: i64,
    weapon: PricedWeapon,
    /// None when its band fitted them.
    margin: Option<HitMargin>,
}

/// The weapons still in Equipment that explain a group's shots: those whose
/// band fitted them, and those holding them in their margin.
fn explaining(
    book: &mut PriceBook<'_>,
    group: &UnpricedGroup,
) -> Result<Vec<Explaining>, WeaponReviewError> {
    let mut found = Vec::new();
    for candidate in &group.candidates {
        let Some(weapon) = book.price(candidate.equipment_id, group.reload_speed_percent)? else {
            continue;
        };
        let margin = stored_margin(candidate, group.amount, group.critical, weapon.band);
        if candidate.fits || margin.is_some() {
            found.push(Explaining {
                equipment_id: candidate.equipment_id,
                weapon,
                margin,
            });
        }
    }
    Ok(found)
}

/// What each requested session's unpriced shots could cost at most, for the
/// sessions holding any, in no particular order.
pub(super) fn unpriced_exposure(
    conn: &rusqlite::Connection,
    session_ids: &[String],
    pricing: Pricing<'_>,
) -> Result<Vec<UnpricedExposure>, WeaponReviewError> {
    let mut book = PriceBook::new(conn, pricing);
    let mut exposures: std::collections::BTreeMap<String, UnpricedExposure> =
        std::collections::BTreeMap::new();
    for group in unpriced_groups(conn, session_ids)? {
        // The dearest weapon that explains it: the most the shot can have
        // cost.
        let dearest = explaining(&mut book, &group)?
            .iter()
            .map(|found| found.weapon.cost)
            .max_by(f64::total_cmp);
        let exposure = exposures
            .entry(group.session_id.clone())
            .or_insert_with(|| UnpricedExposure {
                session_id: group.session_id.clone(),
                shots: 0,
                no_fit_shots: 0,
                cost_bound_ped: 0.0,
            });
        exposure.shots += group.shots;
        match dearest {
            Some(price) => exposure.cost_bound_ped += price * group.shots as f64,
            None => exposure.no_fit_shots += group.shots,
        }
    }
    Ok(exposures.into_values().collect())
}

/// The weapons a session's unpriced shots could all be assigned to at once:
/// each still in Equipment that explains at least one of them, with how many
/// it fitted, how many it would price as boosted or reduced, how many it
/// would leave, and the cost assigning it would add.
pub(super) fn bulk_assign_options(
    conn: &rusqlite::Connection,
    session_id: &str,
    pricing: Pricing<'_>,
) -> Result<Vec<BulkAssignOption>, WeaponReviewError> {
    let mut book = PriceBook::new(conn, pricing);
    let groups = unpriced_groups(conn, &[session_id.to_string()])?;
    let total: i64 = groups.iter().map(|group| group.shots).sum();
    let mut options: std::collections::BTreeMap<i64, BulkAssignOption> =
        std::collections::BTreeMap::new();
    for group in &groups {
        for found in explaining(&mut book, group)? {
            let option = options
                .entry(found.equipment_id)
                .or_insert_with(|| BulkAssignOption {
                    equipment_id: found.equipment_id,
                    name: found.weapon.name.clone(),
                    fitting_shots: 0,
                    boosted_shots: 0,
                    reduced_shots: 0,
                    skipped_shots: 0,
                    added_cost_ped: 0.0,
                });
            match found.margin {
                None => option.fitting_shots += group.shots,
                Some(HitMargin::Boosted) => option.boosted_shots += group.shots,
                Some(HitMargin::Reduced) => option.reduced_shots += group.shots,
            }
            option.added_cost_ped += found.weapon.cost * group.shots as f64;
        }
    }
    let explained = |option: &BulkAssignOption| {
        option.fitting_shots + option.boosted_shots + option.reduced_shots
    };
    let mut options: Vec<BulkAssignOption> = options
        .into_values()
        .map(|option| BulkAssignOption {
            skipped_shots: total - explained(&option),
            ..option
        })
        .collect();
    // The weapon that explains the most shots first, its band before its
    // margin.
    options.sort_by(|a, b| {
        explained(b)
            .cmp(&explained(a))
            .then_with(|| b.fitting_shots.cmp(&a.fitting_shots))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(options)
}

/// The session detail's weapon attribution block: the tallies the session
/// recorded at its stop (null for a session recorded before they were kept),
/// its stored shots by state, the decisions made on a mismatch while it
/// ran, and the damage-over-time effects that ticked in it. A session with
/// nothing to tell reads as all zeros, no decisions, and no effects.
pub fn session_detail_block(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Value, DbError> {
    let (agreed, evidenced, correctable): (Option<i64>, Option<i64>, bool) = conn
        .query_row(
            "SELECT weapon_shots_agreed, weapon_shots_evidenced, COALESCE(is_active, 0) = 0 \
             FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .unwrap_or((None, None, false));
    // A shot's standing: the live correction (if any) decides what it is
    // now, while `attribution` keeps how it was classified as it landed.
    let counts: [i64; 7] = conn.query_row(
        "SELECT COALESCE(SUM(e.attribution = 'evidence'), 0), \
                COALESCE(SUM(e.attribution = 'unresolved'), 0), \
                COALESCE(SUM(e.attribution = 'unresolved' AND e.tool_name IS NULL \
                             AND e.correction_id IS NULL), 0), \
                COALESCE(SUM(e.attribution = 'unresolved' AND c.kind = 'priced'), 0), \
                COALESCE(SUM(e.attribution = 'unresolved' AND c.kind = 'effect_tick'), 0), \
                COALESCE(SUM(e.attribution = 'effect_tick'), 0), \
                COALESCE(SUM(e.attribution = 'effect_tick' AND c.kind = 'priced'), 0) \
         FROM weapon_shot_evidence e \
         LEFT JOIN weapon_attribution_corrections c \
                ON c.id = e.correction_id AND c.undone_at IS NULL \
         WHERE e.session_id = ?1",
        [session_id],
        |row| {
            Ok([
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ])
        },
    )?;
    let [evidence, unresolved, unpriced, assigned, marked_ticks, effect_ticks, priced_ticks] =
        counts;
    // Unresolved shots that landed with no weapon declared: the session ran
    // undeclared once they pass the live warning's threshold. Counted
    // however they were corrected since, so the record outlives the repair.
    let undeclared: i64 = conn.query_row(
        "SELECT COUNT(*) FROM weapon_shot_evidence \
         WHERE session_id = ?1 AND attribution = 'unresolved' AND hotbar_tool IS NULL",
        [session_id],
        |row| row.get(0),
    )?;
    // Hits priced from outside the held weapon's band: every one as it
    // landed, unless a live decision moved it off that weapon.
    let (boosted, reduced): (i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(attribution = 'boosted'), 0), \
                COALESCE(SUM(attribution = 'reduced'), 0) \
         FROM weapon_shot_evidence WHERE session_id = ?1 AND review_id IS NULL",
        [session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    // The ones still standing, per weapon: what the player weighs against
    // what they remember of the session.
    let hit_margins: Vec<Value> = {
        let mut stmt = conn.prepare(
            "SELECT tool_name, SUM(attribution = 'boosted'), SUM(attribution = 'reduced') \
             FROM weapon_shot_evidence \
             WHERE session_id = ?1 AND attribution IN ('boosted', 'reduced') \
               AND review_id IS NULL AND correction_id IS NULL AND tool_name IS NOT NULL \
             GROUP BY tool_name \
             ORDER BY SUM(attribution = 'boosted') DESC, tool_name",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok(json!({
                "toolName": row.get::<_, String>(0)?,
                "boosted": row.get::<_, i64>(1)?,
                "reduced": row.get::<_, i64>(2)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let batches: Vec<Value> = {
        // Live bulk assignments, oldest first.
        let mut stmt = conn.prepare(
            "SELECT batch_id, MAX(tool_name), COUNT(*), COALESCE(SUM(cost_per_shot), 0), \
                    MIN(corrected_at) \
             FROM weapon_attribution_corrections \
             WHERE session_id = ?1 AND batch_id IS NOT NULL AND undone_at IS NULL \
             GROUP BY batch_id ORDER BY MIN(corrected_at), batch_id",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "toolName": row.get::<_, String>(1)?,
                "shots": row.get::<_, i64>(2)?,
                "costPed": row.get::<_, f64>(3)?,
                "correctedAt": row.get::<_, f64>(4)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let reviews: Vec<Value> = {
        let mut stmt = conn.prepare(
            "SELECT id, decision, hotbar_tool, evidence_tool, mismatch_since, decided_at, \
                    repriced_shots, cost_delta_ped \
             FROM weapon_attribution_reviews WHERE session_id = ?1 \
             ORDER BY decided_at, id",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "decision": row.get::<_, String>(1)?,
                "hotbarTool": row.get::<_, String>(2)?,
                "evidenceTool": row.get::<_, String>(3)?,
                "since": row.get::<_, f64>(4)?,
                "decidedAt": row.get::<_, f64>(5)?,
                "repricedShots": row.get::<_, i64>(6)?,
                "costDelta": row.get::<_, f64>(7)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(json!({
        "correctable": correctable,
        "agreed": agreed,
        "evidenced": evidenced,
        "evidenceShots": evidence,
        "unresolved": unresolved,
        "unpriced": unpriced,
        "assigned": assigned,
        "markedTicks": marked_ticks,
        "effectTicks": effect_ticks,
        "pricedTicks": priced_ticks,
        "boostedShots": boosted,
        "reducedShots": reduced,
        "hitMargins": hit_margins,
        "unclaimedTicks": unclaimed_ticks(conn, session_id)?,
        "undeclaredShots": undeclared,
        "ranUndeclared": undeclared >= UNDECLARED_WARNING_SHOTS,
        "batches": batches,
        "effects": session_effects(conn, session_id)?,
        "reviews": reviews,
    }))
}

/// Ticks standing as ticks: effect-tick rows no correction priced, plus
/// unresolved hits a live correction marked as a tick.
/// A tick naming a window that no longer exists (its paying session was
/// deleted while a later one still ran) reads as unclaimed.
const STANDING_TICKS: &str = "\
    SELECT e.amount, \
           (SELECT w.id FROM weapon_effect_windows w \
            WHERE w.id = COALESCE(c.effect_window_id, e.effect_window_id)) AS window_id \
    FROM weapon_shot_evidence e \
    LEFT JOIN weapon_attribution_corrections c \
           ON c.id = e.correction_id AND c.undone_at IS NULL \
    WHERE e.session_id = ?1 \
      AND ((e.attribution = 'effect_tick' AND c.id IS NULL) \
        OR (e.attribution = 'unresolved' AND c.kind = 'effect_tick'))";

/// Standing ticks no one effect claims: several overlapping effects
/// explained them, or the session that paid for their effect was deleted.
fn unclaimed_ticks(conn: &rusqlite::Connection, session_id: &str) -> Result<i64, DbError> {
    Ok(conn.query_row(
        &format!("SELECT COUNT(*) FROM ({STANDING_TICKS}) WHERE window_id IS NULL"),
        [session_id],
        |row| row.get(0),
    )?)
}

/// Every effect the session paid for or saw tick, oldest first: who opened
/// it and when, what the opening hit cost, whether a decision took it back,
/// and the ticks it claims in this session. An effect paid for in an
/// earlier session is listed where its ticks landed, marked as paid
/// elsewhere, so its cost is never counted twice.
fn session_effects(conn: &rusqlite::Connection, session_id: &str) -> Result<Vec<Value>, DbError> {
    let sql = format!(
        "WITH ticks AS ({STANDING_TICKS}) \
         SELECT w.id, w.tool_name, w.started_at, w.expires_at, w.hit_amount, w.cost_per_shot, \
                w.session_id = ?1, w.withdrawn_at IS NOT NULL, \
                (SELECT COUNT(*) FROM ticks t WHERE t.window_id = w.id), \
                (SELECT COALESCE(SUM(t.amount), 0) FROM ticks t WHERE t.window_id = w.id) \
         FROM weapon_effect_windows w \
         WHERE w.session_id = ?1 OR w.id IN (SELECT window_id FROM ticks) \
         ORDER BY w.started_at, w.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([session_id], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "toolName": row.get::<_, String>(1)?,
            "activatedAt": row.get::<_, f64>(2)?,
            "expiresAt": row.get::<_, f64>(3)?,
            "hitAmount": row.get::<_, Option<f64>>(4)?,
            "costPerShot": row.get::<_, f64>(5)?,
            "paidHere": row.get::<_, bool>(6)?,
            "withdrawn": row.get::<_, bool>(7)?,
            "ticks": row.get::<_, i64>(8)?,
            "tickDamage": row.get::<_, f64>(9)?,
        }))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
