//! What an enhancer break costs, and how each one is kept and priced.
//!
//! A break consumes the enhancer: its TT, and the markup the player paid
//! over it, which is most of what the break costs. The game returns part of
//! the TT as Shrapnel and states how much on the break's own chat line, so
//! the break costs its TT plus its markup less that return. The fraction
//! returned is read per break, never assumed.
//!
//! The TT is the game data's, found by the enhancer's name (the game's own
//! spelling, or the catalogue's for the same item: [`catalogue_name`]); the
//! markup is the one the player set on the weapon the enhancer was fitted
//! to. Neither is ever guessed: a break whose markup is not set costs its TT
//! less the return and says the markup is absent, and a break whose TT is
//! not known, or whose stated return exceeds the TT known, has no cost and
//! says why.
//!
//! A return is either loot or a deduction, never both. The chat log watcher
//! tags a return on its loot row when it arrives with its break, and every
//! loot reader leaves tagged rows out. The session's kept breaks deduct
//! their stated returns, in the order they broke, only as far as the
//! Shrapnel tagged in the session reaches; a return that stayed untagged is
//! loot and deducts nothing. Tagged Shrapnel no kept break claims (the
//! return of a break the tracker did not apply) is rebated at the session's
//! end. An unpriced break has no cost, yet it still claims its return: that
//! Shrapnel stays out of loot and is not rebated, so the break is absent
//! from every figure rather than turned into a gain.
//!
//! A kept break is priced from its inputs (its enhancer, its weapon, its
//! stated return) whenever they are read again: when its session ends, and
//! whenever the game data's TT or the weapon's markup no longer matches what
//! priced it ([`heal_prices`]). Its cost is carried by the kill its shots
//! went towards (`kills.enhancer_cost`, kept in step with the rows), or,
//! for a break after the session's last kill, by the session itself; the
//! session's tracked enhancer bucket is derived from both in one place
//! (`crate::session_costs`).

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::db::DbError;
use crate::game_data_store::GameDataStore;
use crate::ped::Ped;

/// The game-data collection enhancers are listed in.
const ENHANCERS: &str = "enhancers";

/// The weapon setting that holds the markup of the enhancers fitted to it,
/// in percent of TT, as every other markup the library stores.
pub const MARKUP_KEY: &str = "enhancer_markup";

/// How far a break's cost is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BreakPricing {
    /// The enhancer's TT and the markup the player set: the break costs
    /// both, less the return.
    Priced { tt: Ped, markup: Ped },
    /// The TT is known and the markup is not set: the break costs its TT
    /// less the return, and the markup part is absent.
    MarkupNotSet { tt: Ped },
    /// The enhancer's TT is not known: the break has no cost.
    TtUnknown,
    /// The return the game stated exceeds the TT known for the enhancer:
    /// the two disagree, so the break has no cost.
    ReturnAboveTt { tt: Ped },
}

impl BreakPricing {
    /// Price a break from what is known of it: the enhancer's TT, the
    /// markup the player set (percent of TT), and the Shrapnel the game
    /// stated it returned.
    pub fn price(tt: Option<Ped>, markup_percent: Option<f64>, returned: Ped) -> BreakPricing {
        let Some(tt) = tt else {
            return BreakPricing::TtUnknown;
        };
        if returned.value() > tt.value() {
            return BreakPricing::ReturnAboveTt { tt };
        }
        match markup_percent {
            Some(percent) => BreakPricing::Priced {
                tt,
                markup: tt * ((percent - 100.0) / 100.0),
            },
            None => BreakPricing::MarkupNotSet { tt },
        }
    }

    /// The stored spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            BreakPricing::Priced { .. } => "priced",
            BreakPricing::MarkupNotSet { .. } => "markupNotSet",
            BreakPricing::TtUnknown => "ttUnknown",
            BreakPricing::ReturnAboveTt { .. } => "returnAboveTt",
        }
    }

    /// The enhancer's TT, where it is known.
    pub fn tt(&self) -> Option<Ped> {
        match self {
            BreakPricing::Priced { tt, .. }
            | BreakPricing::MarkupNotSet { tt }
            | BreakPricing::ReturnAboveTt { tt } => Some(*tt),
            BreakPricing::TtUnknown => None,
        }
    }

    /// The markup consumed, where the player set one.
    pub fn markup(&self) -> Option<Ped> {
        match self {
            BreakPricing::Priced { markup, .. } => Some(*markup),
            _ => None,
        }
    }

    /// What a break so priced cost, deducting `deducted` of its return.
    /// None for an unpriced break: it has no cost, never a zero one.
    pub fn cost(&self, deducted: Ped) -> Option<Ped> {
        match *self {
            BreakPricing::Priced { tt, markup } => Some(tt + markup - deducted),
            BreakPricing::MarkupNotSet { tt } => Some(tt - deducted),
            BreakPricing::TtUnknown | BreakPricing::ReturnAboveTt { .. } => None,
        }
    }
}

/// One break the tracker applied to the weapon in use.
#[derive(Debug, Clone, PartialEq)]
pub struct EnhancerBreak {
    pub id: String,
    pub session_id: String,
    /// The activity context in force at the break.
    pub context_id: Option<i64>,
    /// When the chat line says the break happened, epoch seconds.
    pub observed_at: f64,
    pub enhancer_name: String,
    /// The weapon the enhancer was fitted to.
    pub weapon_name: String,
    /// The Shrapnel the break's chat line says came back.
    pub returned: Ped,
    /// The part of the return deducted from its cost. While the session
    /// runs it is the whole stated return; the session's end settles it
    /// against the Shrapnel tagged as returns.
    pub deducted: Ped,
    /// The markup set on the weapon when it was priced, percent of TT.
    pub markup_percent: Option<f64>,
    pub pricing: BreakPricing,
}

impl EnhancerBreak {
    /// What the break cost, where it is known: the TT and any markup set,
    /// less the return deducted.
    pub fn cost(&self) -> Option<Ped> {
        self.pricing.cost(self.deducted)
    }

    /// Keep the break, carried by `kill_id`, or by the session when no kill
    /// followed it. Run in the write that stores what carries its cost.
    pub fn insert(
        &self,
        conn: &rusqlite::Connection,
        kill_id: Option<&str>,
    ) -> rusqlite::Result<()> {
        conn.prepare_cached(
            "INSERT INTO enhancer_breaks \
             (id, session_id, kill_id, context_id, observed_at, enhancer_name, weapon_name, \
              returned_ped, deducted_ped, markup_percent, pricing, tt_ped, markup_ped, cost_ped) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )?
        .execute(rusqlite::params![
            self.id,
            self.session_id,
            kill_id,
            self.context_id,
            self.observed_at,
            self.enhancer_name,
            self.weapon_name,
            self.returned.value(),
            self.deducted.value(),
            self.markup_percent,
            self.pricing.as_str(),
            self.pricing.tt().map(Ped::value),
            self.pricing.markup().map(Ped::value),
            self.cost().map(Ped::value),
        ])?;
        Ok(())
    }
}

// ── The enhancer's TT ───────────────────────────────────────────────

/// An enhancer's TT by its name, from the game data.
pub type EnhancerTtResolver = Arc<dyn Fn(&str) -> Option<Ped> + Send + Sync>;

/// A name with its surrounding space trimmed and every inner run of space
/// read as one: how two spellings of a name are compared, exactly.
fn normalise(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The catalogue's name for an enhancer the game names "<Kind> Enhancer
/// <N>": "T<N> <Kind> Enhancer" (the game's "Weapon Damage Enhancer 1" is
/// the catalogue's "T1 Weapon Damage Enhancer"). None for a name of any
/// other shape.
pub fn catalogue_name(game_name: &str) -> Option<String> {
    let name = normalise(game_name);
    let (kind, tier) = name.rsplit_once(' ')?;
    let tier_is_number = !tier.is_empty() && tier.chars().all(|c| c.is_ascii_digit());
    let kind_is_enhancer = kind.ends_with(" Enhancer") && kind.len() > " Enhancer".len();
    (tier_is_number && kind_is_enhancer).then(|| format!("T{tier} {kind}"))
}

/// The TT the game data lists for the enhancer `name`: the entry of that
/// exact name, else the entry of its catalogue name, each compared after
/// normalising space and nothing else.
pub fn tt_from_game_data(store: &GameDataStore, name: &str) -> Option<Ped> {
    let entities = store.get_entities(ENHANCERS);
    let named = |wanted: &str| {
        entities.iter().find(|entity| {
            entity
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| normalise(name) == wanted)
        })
    };
    let entity = named(&normalise(name)).or_else(|| named(&catalogue_name(name)?))?;
    entity
        .get("economy")?
        .get("max_tt")?
        .as_f64()
        .filter(|tt| tt.is_finite() && *tt >= 0.0)
        .map(Ped)
}

/// [`tt_from_game_data`] over a store the resolver keeps.
pub fn tt_resolver(store: Arc<GameDataStore>) -> EnhancerTtResolver {
    Arc::new(move |name: &str| tt_from_game_data(&store, name))
}

// ── The weapon's enhancer markup ────────────────────────────────────

/// The markup the player set for the enhancers fitted to a weapon, in
/// percent of TT, from the weapon's stored settings; none when it is not
/// set (a figure below TT is no markup the player can have paid).
pub fn markup_percent(props: &Value) -> Option<f64> {
    props
        .get(MARKUP_KEY)
        .and_then(Value::as_f64)
        .filter(|percent| percent.is_finite() && *percent >= 100.0)
}

/// Each library weapon's enhancer markup, by the weapon's name (a break
/// names its weapon as the library does). A name the library holds more
/// than once with different markups has none: which setup broke is not
/// known, so its markup is not set rather than chosen.
pub fn weapon_markups(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<HashMap<String, Option<f64>>> {
    let mut stmt = conn.prepare_cached(
        "SELECT name, properties_json FROM equipment_library WHERE item_type = 'weapon'",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut markups: HashMap<String, Option<f64>> = HashMap::new();
    for row in rows {
        let (name, props) = row?;
        let props = serde_json::from_str::<Value>(&props).unwrap_or(Value::Null);
        let markup = markup_percent(&props);
        markups
            .entry(normalise(&name))
            .and_modify(|known| {
                if *known != markup {
                    *known = None;
                }
            })
            .or_insert(markup);
    }
    Ok(markups)
}

// ── Pricing kept breaks from their inputs ───────────────────────────

/// A PED amount in millionths, for settling returns exactly.
fn micro(ped: f64) -> i64 {
    (ped * 1_000_000.0).round() as i64
}

/// Price a session's kept breaks again from their inputs: the game data's
/// TT through `tt`, the library's markup through `markups`, and each
/// stated return deducted, in the order they broke, only as far as the
/// session's tagged Shrapnel reaches. Every row that moves is written, and
/// every kill carrying one of the session's breaks takes their cost again.
/// Answers whether anything moved.
pub fn reprice_session(
    conn: &rusqlite::Connection,
    session_id: &str,
    tt: &dyn Fn(&str) -> Option<Ped>,
    markups: &HashMap<String, Option<f64>>,
) -> rusqlite::Result<bool> {
    // The Shrapnel tagged as enhancer returns in the session: what its
    // breaks' stated returns may be deducted from.
    let tagged: f64 = conn
        .prepare_cached(
            // every-kill: spend (the session's active loot lines, every row).
            "SELECT COALESCE(SUM(li.value_ped), 0) FROM kill_loot_items li \
             JOIN kills k ON k.id = li.kill_id \
             WHERE k.session_id = ?1 AND li.is_enhancer_shrapnel = 1 \
               AND li.deactivated_at IS NULL",
        )?
        .query_row([session_id], |row| row.get(0))?;
    type Row = (
        String,
        String,
        String,
        f64,
        f64,
        Option<f64>,
        String,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    );
    let rows: Vec<Row> = {
        let mut stmt = conn.prepare_cached(
            "SELECT id, enhancer_name, weapon_name, returned_ped, deducted_ped, markup_percent, \
                    pricing, tt_ped, markup_ped, cost_ped \
             FROM enhancer_breaks WHERE session_id = ?1 ORDER BY observed_at, id",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut pool = micro(tagged).max(0);
    let mut moved = false;
    for (
        id,
        enhancer,
        weapon,
        returned,
        deducted,
        markup_percent,
        pricing,
        tt_ped,
        markup_ped,
        cost,
    ) in rows
    {
        // Every kept break claims its return from the tagged Shrapnel,
        // priced or not: an unpriced break has no cost, and its return is
        // neither loot nor rebated.
        let wanted = micro(returned);
        let new_deducted = if pool >= wanted {
            returned
        } else {
            (pool as f64 / 1_000_000.0).min(returned)
        };
        pool = (pool - wanted).max(0);
        let new_markup_percent = markups.get(&normalise(&weapon)).copied().flatten();
        let new_pricing = BreakPricing::price(tt(&enhancer), new_markup_percent, Ped(returned));
        let new_cost = new_pricing.cost(Ped(new_deducted)).map(Ped::value);
        let same = new_deducted == deducted
            && new_markup_percent == markup_percent
            && new_pricing.as_str() == pricing
            && new_pricing.tt().map(Ped::value) == tt_ped
            && new_pricing.markup().map(Ped::value) == markup_ped
            && new_cost == cost;
        if same {
            continue;
        }
        moved = true;
        conn.prepare_cached(
            "UPDATE enhancer_breaks SET deducted_ped = ?2, markup_percent = ?3, pricing = ?4, \
                    tt_ped = ?5, markup_ped = ?6, cost_ped = ?7 \
             WHERE id = ?1",
        )?
        .execute(rusqlite::params![
            id,
            new_deducted,
            new_markup_percent,
            new_pricing.as_str(),
            new_pricing.tt().map(Ped::value),
            new_pricing.markup().map(Ped::value),
            new_cost,
        ])?;
    }
    if moved {
        conn.prepare_cached(
            "UPDATE kills SET enhancer_cost = \
                 (SELECT COALESCE(SUM(b.cost_ped), 0) FROM enhancer_breaks b \
                  WHERE b.kill_id = kills.id) \
             WHERE id IN (SELECT kill_id FROM enhancer_breaks \
                          WHERE session_id = ?1 AND kill_id IS NOT NULL)",
        )?
        .execute([session_id])?;
    }
    Ok(moved)
}

/// Price again every ended session's kept breaks whose game-data TT or
/// weapon markup no longer matches what priced them, and bring every figure
/// derived from those sessions' costs up to date. A running session is
/// left to its end, which prices its breaks anyway. Idempotent, and cheap
/// once nothing has moved; run as the tracker starts and when a weapon's
/// settings are saved.
pub fn heal_prices(
    conn: &rusqlite::Connection,
    tt: &dyn Fn(&str) -> Option<Ped>,
) -> Result<(), DbError> {
    let markups = weapon_markups(conn)?;
    type Inputs = (String, String, String, Option<f64>, Option<f64>);
    let kept: Vec<Inputs> = {
        let mut stmt = conn.prepare_cached(
            "SELECT b.session_id, b.enhancer_name, b.weapon_name, b.tt_ped, b.markup_percent \
             FROM enhancer_breaks b JOIN tracking_sessions s ON s.id = b.session_id \
             WHERE s.is_active = 0",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut stale: Vec<String> = Vec::new();
    for (session_id, enhancer, weapon, tt_ped, markup_percent) in kept {
        let now_tt = tt(&enhancer).map(Ped::value);
        let now_markup = markups.get(&normalise(&weapon)).copied().flatten();
        if (now_tt != tt_ped || now_markup != markup_percent) && !stale.contains(&session_id) {
            stale.push(session_id);
        }
    }
    for session_id in stale {
        // A session's breaks, its kills and its derived figures move
        // together or not at all.
        conn.execute_batch("SAVEPOINT break_prices")?;
        let healed = reprice_session(conn, &session_id, tt, &markups)
            .map_err(DbError::from)
            .and_then(|moved| {
                if moved {
                    crate::session_costs::refresh_derived(conn, &session_id)?;
                }
                Ok(())
            });
        match healed {
            Ok(()) => conn.execute_batch("RELEASE break_prices")?,
            Err(error) => {
                conn.execute_batch("ROLLBACK TO break_prices; RELEASE break_prices")?;
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn approx(actual: Option<Ped>, expected: f64) {
        let actual = actual.expect("a cost").value();
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn a_break_costs_its_tt_and_markup_less_the_return() {
        let pricing = BreakPricing::price(Some(Ped(1.0)), Some(150.0), Ped(0.8));
        assert_eq!(pricing.as_str(), "priced");
        approx(pricing.markup(), 0.5);
        approx(pricing.cost(Ped(0.8)), 0.7);
        // A return that stayed loot is not deducted.
        approx(pricing.cost(Ped::ZERO), 1.5);
    }

    #[test]
    fn a_break_with_no_markup_set_costs_its_tt_less_the_return() {
        let pricing = BreakPricing::price(Some(Ped(1.0)), None, Ped(0.8));
        assert_eq!(pricing, BreakPricing::MarkupNotSet { tt: Ped(1.0) });
        assert_eq!(pricing.markup(), None);
        approx(pricing.cost(Ped(0.8)), 0.2);
    }

    #[test]
    fn a_break_of_unknown_tt_or_a_return_above_it_has_no_cost() {
        let unknown = BreakPricing::price(None, Some(150.0), Ped(0.8));
        assert_eq!(unknown, BreakPricing::TtUnknown);
        assert_eq!(unknown.cost(Ped(0.8)), None);

        let above = BreakPricing::price(Some(Ped(0.5)), Some(150.0), Ped(0.8));
        assert_eq!(above, BreakPricing::ReturnAboveTt { tt: Ped(0.5) });
        assert_eq!(above.cost(Ped(0.8)), None);

        // A return equal to the TT is a break that cost its markup alone.
        let whole = BreakPricing::price(Some(Ped(0.8)), Some(200.0), Ped(0.8));
        approx(whole.cost(Ped(0.8)), 0.8);
    }

    #[test]
    fn the_game_s_enhancer_names_map_to_the_catalogue_s() {
        assert_eq!(
            catalogue_name("Weapon Damage Enhancer 1").as_deref(),
            Some("T1 Weapon Damage Enhancer")
        );
        assert_eq!(
            catalogue_name("  Armor  Defense Enhancer 10 ").as_deref(),
            Some("T10 Armor Defense Enhancer")
        );
        // Only the one shape maps; nothing is matched loosely.
        assert_eq!(catalogue_name("T1 Weapon Damage Enhancer"), None);
        assert_eq!(catalogue_name("Weapon Damage Enhancer"), None);
        assert_eq!(catalogue_name("Weapon Damage Enhancer I"), None);
        assert_eq!(catalogue_name("Weapon Damage Amplifier 1"), None);
        assert_eq!(catalogue_name("Enhancer 1"), None);
    }

    fn store() -> (tempfile::TempDir, GameDataStore) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("enhancers.json"),
            json!([
                {"name": "T1 Weapon Damage Enhancer", "economy": {"max_tt": 1.0}},
                {"name": "Odd Enhancer", "economy": {}},
            ])
            .to_string(),
        )
        .unwrap();
        let store = GameDataStore::new(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn the_tt_is_found_by_the_exact_name_or_its_catalogue_name() {
        let (_dir, store) = store();
        approx(tt_from_game_data(&store, "T1 Weapon Damage Enhancer"), 1.0);
        approx(tt_from_game_data(&store, "Weapon Damage Enhancer 1"), 1.0);
        approx(tt_from_game_data(&store, " Weapon  Damage Enhancer 1"), 1.0);
        assert_eq!(tt_from_game_data(&store, "t1 weapon damage enhancer"), None);
        assert_eq!(tt_from_game_data(&store, "weapon damage enhancer 1"), None);
        assert_eq!(tt_from_game_data(&store, "Weapon Damage Enhancer 2"), None);
        assert_eq!(tt_from_game_data(&store, "Odd Enhancer"), None);
    }

    #[test]
    fn a_weapon_s_enhancer_markup_is_read_only_when_it_is_a_markup() {
        assert_eq!(
            markup_percent(&json!({"enhancer_markup": 150})),
            Some(150.0)
        );
        assert_eq!(
            markup_percent(&json!({"enhancer_markup": 100})),
            Some(100.0)
        );
        assert_eq!(markup_percent(&json!({"enhancer_markup": 90})), None);
        assert_eq!(markup_percent(&json!({"enhancer_markup": null})), None);
        assert_eq!(markup_percent(&json!({"weapon_markup": 150})), None);
    }

    // ── Pricing kept breaks again ───────────────────────────────────

    /// An ended session with one kill carrying `tagged` PED of Shrapnel
    /// tagged as returns, and a library weapon "Rifle" whose enhancer
    /// markup is `markup`.
    fn hunt(conn: &rusqlite::Connection, tagged: f64, markup: Option<i64>) {
        conn.execute_batch(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, session_name) \
             VALUES ('s1', 1000.0, 4600.0, 0, 'Run');
             INSERT INTO kills (id, session_id, mob_name, timestamp, cost_ped, enhancer_cost, \
                                loot_total_ped) \
             VALUES ('k1', 's1', 'Atrox', 1500.0, 1.0, 0.0, 2.0);
             INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
                                          is_enhancer_shrapnel) \
             VALUES ('k1', 'Animal Hide', 1, 2.0, 0);",
        )
        .unwrap();
        if tagged > 0.0 {
            conn.execute(
                "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
                                              is_enhancer_shrapnel) \
                 VALUES ('k1', 'Shrapnel', 8000, ?1, 1)",
                [tagged],
            )
            .unwrap();
        }
        set_markup(conn, markup);
    }

    fn set_markup(conn: &rusqlite::Connection, markup: Option<i64>) {
        conn.execute("DELETE FROM equipment_library", []).unwrap();
        let mut props = json!({"weapon_entity": {"name": "Rifle"}, "damage_enhancers": 2});
        if let Some(markup) = markup {
            props[MARKUP_KEY] = json!(markup);
        }
        conn.execute(
            "INSERT INTO equipment_library (name, item_type, properties_json) \
             VALUES ('Rifle', 'weapon', ?1)",
            [props.to_string()],
        )
        .unwrap();
    }

    /// A break kept on k1 as the tracker keeps it while the session runs,
    /// priced with no TT known.
    fn kept(conn: &rusqlite::Connection, id: &str, at: f64) {
        EnhancerBreak {
            id: id.into(),
            session_id: "s1".into(),
            context_id: None,
            observed_at: at,
            enhancer_name: "Weapon Damage Enhancer 1".into(),
            weapon_name: "Rifle".into(),
            returned: Ped(0.8),
            deducted: Ped(0.8),
            markup_percent: None,
            pricing: BreakPricing::TtUnknown,
        }
        .insert(conn, Some("k1"))
        .unwrap();
    }

    fn scalar(conn: &rusqlite::Connection, sql: &str) -> f64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn a_kept_break_is_priced_again_when_its_tt_or_its_weapon_s_markup_arrives() {
        let conn = crate::db::migrated_in_memory().unwrap();
        hunt(&conn, 0.8, None);
        kept(&conn, "b1", 1400.0);
        let (_dir, store) = store();
        let tt = |name: &str| tt_from_game_data(&store, name);

        // The game's name now maps to the catalogue's: 1.00 TT less 0.80.
        heal_prices(&conn, &tt).unwrap();
        let pricing: String = conn
            .query_row("SELECT pricing FROM enhancer_breaks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pricing, "markupNotSet");
        approx(
            Some(Ped(scalar(&conn, "SELECT enhancer_cost FROM kills"))),
            0.2,
        );
        approx(
            Some(Ped(scalar(
                &conn,
                "SELECT enhancer_cost FROM session_summaries WHERE session_id = 's1'",
            ))),
            0.2,
        );

        // The weapon's markup is set later: 1.00 + 0.50 - 0.80.
        set_markup(&conn, Some(150));
        heal_prices(&conn, &tt).unwrap();
        approx(
            Some(Ped(scalar(&conn, "SELECT cost_ped FROM enhancer_breaks"))),
            0.7,
        );
        approx(
            Some(Ped(scalar(&conn, "SELECT enhancer_cost FROM kills"))),
            0.7,
        );
        approx(
            Some(Ped(scalar(
                &conn,
                "SELECT enhancer_cost FROM session_summaries WHERE session_id = 's1'",
            ))),
            0.7,
        );
        // Nothing moved since: priced again, nothing is written.
        assert!(!reprice_session(&conn, "s1", &tt, &weapon_markups(&conn).unwrap()).unwrap());
    }

    #[test]
    fn a_return_is_deducted_only_as_far_as_the_tagged_shrapnel_reaches() {
        let conn = crate::db::migrated_in_memory().unwrap();
        // Two breaks of 0.80 each; only one return arrived tagged, the
        // other stayed loot.
        hunt(&conn, 0.8, Some(150));
        kept(&conn, "b1", 1400.0);
        kept(&conn, "b2", 1450.0);
        let (_dir, store) = store();
        let tt = |name: &str| tt_from_game_data(&store, name);
        reprice_session(&conn, "s1", &tt, &weapon_markups(&conn).unwrap()).unwrap();
        // The first claims the tagged 0.80; the second deducts nothing.
        approx(
            Some(Ped(scalar(
                &conn,
                "SELECT deducted_ped FROM enhancer_breaks WHERE id = 'b1'",
            ))),
            0.8,
        );
        approx(
            Some(Ped(scalar(
                &conn,
                "SELECT deducted_ped FROM enhancer_breaks WHERE id = 'b2'",
            ))),
            0.0,
        );
        approx(
            Some(Ped(scalar(&conn, "SELECT enhancer_cost FROM kills"))),
            0.7 + 1.5,
        );
    }

    #[test]
    fn a_name_held_twice_with_different_markups_has_none() {
        let conn = crate::db::migrated_in_memory().unwrap();
        for markup in [150, 160] {
            conn.execute(
                "INSERT INTO equipment_library (name, item_type, properties_json) \
                 VALUES ('Rifle', 'weapon', ?1)",
                [json!({"enhancer_markup": markup}).to_string()],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO equipment_library (name, item_type, properties_json) \
             VALUES ('Pistol', 'weapon', ?1), ('Pistol', 'weapon', ?1)",
            [json!({"enhancer_markup": 120}).to_string()],
        )
        .unwrap();
        let markups = weapon_markups(&conn).unwrap();
        assert_eq!(markups["Rifle"], None);
        assert_eq!(markups["Pistol"], Some(120.0));
    }
}
