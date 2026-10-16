//! Per-weapon cells of a session: what each weapon did alone, the skill it
//! earned, and its pace on each mob. The skilling reads that narrow recorded
//! play to one weapon (a named session's part played with it, a player's
//! pace on a mob) fold these cells instead of every raw kill.
//!
//! The cells are part of the session rollups ([`crate::session_rollup`]):
//! they settle with the session's other cells, under the same marker and
//! version, and heal with them. A session the marker does not cover (the
//! live one, one just edited) is computed raw by the same [`compute`], so a
//! settled cell and a raw read can never disagree.
//!
//! ## Attribution
//!
//! A weapon's part of a session is the kills it made alone; a kill shared
//! with another tool, or made by an unrecognised one, is no one's. A skill
//! gain belongs to the next recorded event in its session (a kill or a
//! harvesting swing) and counts for a weapon only when that event is one
//! of its own kills; a gain with no event within [`MAX_EVENT_GAP_SECONDS`]
//! either side belongs to none. A weapon's play time is the gaps leading
//! up to its kills, against the session's active time (every gap between
//! events short of a break). Attributes are left out: they have no TT value
//! curve for a gain to be read from.

use crate::character_calc::is_attribute;

/// The longest gap between two recorded events still counted as play; a
/// longer one is a break, and a skill gain that far from any event is not
/// a kill's.
pub const MAX_EVENT_GAP_SECONDS: f64 = 60.0;

/// The tool name recorded when the firing weapon could not be read.
const UNKNOWN_TOOL: &str = "Unknown";

/// The rollup tables these cells settle into.
pub const TABLES: [&str; 3] = [
    "session_weapon_rollups",
    "session_weapon_pes_rollups",
    "session_weapon_pace_rollups",
];

/// What one weapon did alone in a session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeaponCell {
    pub tool: String,
    pub kills: u64,
    /// Shots and enhancers spent on its own kills.
    pub cycled_ped: f64,
    pub loot_tt: f64,
    /// The gaps leading up to its own kills.
    pub play_seconds: f64,
    /// The session's whole active time.
    pub active_seconds: f64,
    /// PES earned per regular skill, in first-seen order.
    pub pes: Vec<(String, f64)>,
}

/// A weapon's pace on one mob in a session: its own kills made within a
/// break of the kill before, the gaps that led up to them, and their shots.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PaceCell {
    pub tool: String,
    pub species: String,
    pub maturity: String,
    pub kills: u64,
    pub gap_seconds: f64,
    pub shots: f64,
}

/// One session's cells.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionWeaponCells {
    pub weapons: Vec<WeaponCell>,
    pub paces: Vec<PaceCell>,
}

struct Event {
    timestamp: f64,
    /// The weapon that made this kill alone; None for a swing or a kill
    /// that is no one's.
    solo: Option<usize>,
}

fn weapon_index(weapons: &mut Vec<WeaponCell>, tool: &str) -> usize {
    match weapons.iter().position(|cell| cell.tool == tool) {
        Some(index) => index,
        None => {
            weapons.push(WeaponCell {
                tool: tool.to_string(),
                ..WeaponCell::default()
            });
            weapons.len() - 1
        }
    }
}

/// A session's cells, from its raw kills, swings, and skill gains.
pub fn compute(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> rusqlite::Result<SessionWeaponCells> {
    let mut weapons: Vec<WeaponCell> = Vec::new();
    let mut paces: Vec<PaceCell> = Vec::new();
    let mut events: Vec<Event> = Vec::new();

    let mut kills = conn.prepare(
        "SELECT k.timestamp, COALESCE(k.enhancer_cost, 0), COALESCE(k.loot_total_ped, 0), \
                COALESCE(k.mob_species, ''), COALESCE(k.mob_maturity, ''), \
                COUNT(t.kill_id), MIN(t.tool_name), \
                COALESCE(SUM(COALESCE(t.cost_per_shot, 0) * COALESCE(t.shots_fired, 0)), 0), \
                COALESCE(SUM(COALESCE(t.shots_fired, 0)), 0) \
         FROM kills k LEFT JOIN kill_tool_stats t ON t.kill_id = k.id \
         WHERE k.session_id = ?1 \
         GROUP BY k.id ORDER BY k.timestamp, k.id",
    )?;
    let mut rows = kills.query(rusqlite::params![session_id])?;
    let mut previous_kill: Option<f64> = None;
    while let Some(row) = rows.next()? {
        let timestamp: f64 = row.get(0)?;
        let tools: i64 = row.get(5)?;
        let tool: Option<String> = row.get(6)?;
        let solo = match tool {
            Some(tool) if tools == 1 && tool != UNKNOWN_TOOL => {
                let index = weapon_index(&mut weapons, &tool);
                let cell = &mut weapons[index];
                cell.kills += 1;
                cell.cycled_ped += row.get::<_, f64>(7)? + row.get::<_, f64>(1)?;
                cell.loot_tt += row.get::<_, f64>(2)?;
                let gap = previous_kill.map(|previous| timestamp - previous);
                if let Some(gap) = gap.filter(|gap| *gap > 0.0 && *gap <= MAX_EVENT_GAP_SECONDS) {
                    let (species, maturity): (String, String) = (row.get(3)?, row.get(4)?);
                    let pace = match paces.iter().position(|pace| {
                        pace.tool == tool && pace.species == species && pace.maturity == maturity
                    }) {
                        Some(index) => &mut paces[index],
                        None => {
                            paces.push(PaceCell {
                                tool: tool.clone(),
                                species,
                                maturity,
                                ..PaceCell::default()
                            });
                            paces.last_mut().expect("just pushed")
                        }
                    };
                    pace.kills += 1;
                    pace.gap_seconds += gap;
                    pace.shots += row.get::<_, f64>(8)?;
                }
                Some(index)
            }
            _ => None,
        };
        previous_kill = Some(timestamp);
        events.push(Event { timestamp, solo });
    }
    drop(rows);

    let mut swings = conn.prepare("SELECT timestamp FROM harvest_events WHERE session_id = ?1")?;
    for timestamp in swings.query_map(rusqlite::params![session_id], |row| row.get::<_, f64>(0))? {
        events.push(Event {
            timestamp: timestamp?,
            solo: None,
        });
    }
    events.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));

    let mut active = 0.0;
    for pair in events.windows(2) {
        let gap = pair[1].timestamp - pair[0].timestamp;
        if gap > 0.0 && gap <= MAX_EVENT_GAP_SECONDS {
            active += gap;
            if let Some(index) = pair[1].solo {
                weapons[index].play_seconds += gap;
            }
        }
    }
    for cell in &mut weapons {
        cell.active_seconds = active;
    }

    let mut gains = conn.prepare(
        "SELECT timestamp, skill_name, ped_value FROM skill_gains \
         WHERE session_id = ?1 AND ped_value > 0 ORDER BY timestamp",
    )?;
    let mut rows = gains.query(rusqlite::params![session_id])?;
    while let Some(row) = rows.next()? {
        let timestamp: f64 = row.get(0)?;
        let skill: String = row.get(1)?;
        if is_attribute(&skill) {
            continue;
        }
        let next = events.partition_point(|event| event.timestamp < timestamp);
        let Some(Event {
            timestamp: at,
            solo: Some(index),
        }) = events.get(next)
        else {
            continue;
        };
        let after = at - timestamp;
        let before = next.checked_sub(1).map_or(f64::INFINITY, |previous| {
            timestamp - events[previous].timestamp
        });
        if after > MAX_EVENT_GAP_SECONDS && before > MAX_EVENT_GAP_SECONDS {
            continue;
        }
        let pes = &mut weapons[*index].pes;
        let amount: f64 = row.get(2)?;
        match pes.iter_mut().find(|(name, _)| *name == skill) {
            Some((_, total)) => *total += amount,
            None => pes.push((skill, amount)),
        }
    }

    Ok(SessionWeaponCells { weapons, paces })
}

/// Settle one session's cells (computed raw) into the rollup tables. The
/// caller has dropped any earlier cells and owns the commit.
pub fn settle(conn: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<()> {
    let cells = compute(conn, session_id)?;
    let mut weapon = conn.prepare(
        "INSERT INTO session_weapon_rollups \
             (session_id, tool_name, kills, cycled_ped, loot_tt, play_seconds, active_seconds) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let mut pes = conn.prepare(
        "INSERT INTO session_weapon_pes_rollups (session_id, tool_name, skill_name, pes) \
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for cell in &cells.weapons {
        weapon.execute(rusqlite::params![
            session_id,
            cell.tool,
            cell.kills as i64,
            cell.cycled_ped,
            cell.loot_tt,
            cell.play_seconds,
            cell.active_seconds
        ])?;
        for (skill, amount) in &cell.pes {
            pes.execute(rusqlite::params![session_id, cell.tool, skill, amount])?;
        }
    }
    let mut pace = conn.prepare(
        "INSERT INTO session_weapon_pace_rollups \
             (session_id, tool_name, mob_species, mob_maturity, kills, gap_seconds, shots) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for cell in &cells.paces {
        pace.execute(rusqlite::params![
            session_id,
            cell.tool,
            cell.species,
            cell.maturity,
            cell.kills as i64,
            cell.gap_seconds,
            cell.shots
        ])?;
    }
    Ok(())
}

/// Every session's weapon cells (with their skill gains), as `(session id,
/// cell)`: settled sessions from the rollups, the rest computed raw. `tool`
/// narrows to one weapon. The caller heals the
/// rollups first so the raw leg is only the live or just-edited edge.
pub fn effective_weapon_cells(
    conn: &rusqlite::Connection,
    tool: Option<&str>,
) -> rusqlite::Result<Vec<(String, WeaponCell)>> {
    let version = crate::session_rollup::ROLLUP_VERSION;
    let mut cells: Vec<(String, WeaponCell)> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT r.session_id, r.tool_name, r.kills, r.cycled_ped, r.loot_tt, \
                    r.play_seconds, r.active_seconds \
             FROM session_weapon_rollups r \
             JOIN session_rollup_meta m ON m.session_id = r.session_id \
                  AND m.rollup_version >= ?1 \
             WHERE ?2 IS NULL OR r.tool_name = ?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![version, tool])?;
        while let Some(row) = rows.next()? {
            cells.push((
                row.get(0)?,
                WeaponCell {
                    tool: row.get(1)?,
                    kills: row.get::<_, i64>(2)?.max(0) as u64,
                    cycled_ped: row.get(3)?,
                    loot_tt: row.get(4)?,
                    play_seconds: row.get(5)?,
                    active_seconds: row.get(6)?,
                    pes: Vec::new(),
                },
            ));
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT r.session_id, r.tool_name, r.skill_name, r.pes \
             FROM session_weapon_pes_rollups r \
             JOIN session_rollup_meta m ON m.session_id = r.session_id \
                  AND m.rollup_version >= ?1 \
             WHERE ?2 IS NULL OR r.tool_name = ?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![version, tool])?;
        while let Some(row) = rows.next()? {
            let (session, weapon): (String, String) = (row.get(0)?, row.get(1)?);
            if let Some((_, cell)) = cells
                .iter_mut()
                .find(|(id, cell)| *id == session && cell.tool == weapon)
            {
                cell.pes.push((row.get(2)?, row.get(3)?));
            }
        }
    }
    for session_id in crate::session_rollup::unsettled_sessions(conn)? {
        for cell in compute(conn, &session_id)?.weapons {
            if tool.is_none_or(|tool| tool == cell.tool) {
                cells.push((session_id.clone(), cell));
            }
        }
    }
    Ok(cells)
}

/// Every pace cell on one mob maturity across all sessions, settled from
/// the rollups and the rest computed raw. The caller heals first.
pub fn effective_pace_cells(
    conn: &rusqlite::Connection,
    species: &str,
    maturity: &str,
) -> rusqlite::Result<Vec<PaceCell>> {
    let mut cells: Vec<PaceCell> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT r.tool_name, r.kills, r.gap_seconds, r.shots \
             FROM session_weapon_pace_rollups r \
             JOIN session_rollup_meta m ON m.session_id = r.session_id \
                  AND m.rollup_version >= ?1 \
             WHERE r.mob_species = ?2 AND r.mob_maturity = ?3",
        )?;
        let mut rows = stmt.query(rusqlite::params![
            crate::session_rollup::ROLLUP_VERSION,
            species,
            maturity
        ])?;
        while let Some(row) = rows.next()? {
            cells.push(PaceCell {
                tool: row.get(0)?,
                species: species.to_string(),
                maturity: maturity.to_string(),
                kills: row.get::<_, i64>(1)?.max(0) as u64,
                gap_seconds: row.get(2)?,
                shots: row.get(3)?,
            });
        }
    }
    for session_id in crate::session_rollup::unsettled_sessions(conn)? {
        cells.extend(
            compute(conn, &session_id)?
                .paces
                .into_iter()
                .filter(|pace| pace.species == species && pace.maturity == maturity),
        );
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn open_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("test.db")).await.expect("open");
        (dir, db)
    }

    fn session(conn: &rusqlite::Connection, id: &str, ended: bool) {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES (?1, 1000.0, ?2, ?3)",
            rusqlite::params![id, ended.then_some(9000.0), i64::from(!ended)],
        )
        .expect("session");
    }

    /// A kill at `at` by each of `tools` (one shot at 0.1 PED each), with
    /// 0.5 PED of enhancer cost and 2 PED of loot.
    fn kill(conn: &rusqlite::Connection, session: &str, id: &str, at: f64, tools: &[&str]) {
        conn.execute(
            "INSERT INTO kills (id, session_id, timestamp, mob_name, mob_species, mob_maturity, \
                                cost_ped, enhancer_cost, loot_total_ped) \
             VALUES (?1, ?2, ?3, 'Puny Carabok', 'Carabok', 'Puny', 0.1, 0.5, 2.0)",
            rusqlite::params![id, session, at],
        )
        .expect("kill");
        for tool in tools {
            conn.execute(
                "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
                                              cost_per_shot) VALUES (?1, ?2, 1, 10.0, 0.1)",
                rusqlite::params![id, tool],
            )
            .expect("tool");
        }
    }

    fn gain(conn: &rusqlite::Connection, session: &str, at: f64, skill: &str, pes: f64) {
        conn.execute(
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES (?1, ?2, ?3, 0.1, ?4)",
            rusqlite::params![session, at, skill, pes],
        )
        .expect("gain");
    }

    fn swing(conn: &rusqlite::Connection, session: &str, at: f64) {
        conn.execute(
            "INSERT INTO harvest_events (session_id, timestamp, success, tool_name, cost_ped, \
                                         loot_total_ped) VALUES (?1, ?2, 1, 'Axe', 0.1, 0.1)",
            rusqlite::params![session, at],
        )
        .expect("swing");
    }

    /// Kills by a sword alone, a chip alone, one shared, and one by an
    /// unread tool; a swing; gains before each, one long before anything,
    /// and an attribute.
    fn seed(conn: &rusqlite::Connection, id: &str, ended: bool) {
        session(conn, id, ended);
        kill(conn, id, &format!("{id}-1"), 1000.0, &["Sword"]);
        kill(conn, id, &format!("{id}-2"), 1002.0, &["Sword"]);
        kill(conn, id, &format!("{id}-3"), 1005.0, &["Chip"]);
        kill(conn, id, &format!("{id}-4"), 1007.0, &["Sword", "Chip"]);
        kill(conn, id, &format!("{id}-5"), 1009.0, &["Unknown"]);
        swing(conn, id, 1010.0);
        kill(conn, id, &format!("{id}-6"), 1200.0, &["Sword"]);
        gain(conn, id, 1001.5, "Longblades", 0.2);
        gain(conn, id, 1002.0, "Longblades", 0.1);
        gain(conn, id, 1004.0, "Electrokinesis", 0.3);
        gain(conn, id, 1006.5, "Anatomy", 0.4);
        gain(conn, id, 1009.5, "Axe Skill", 0.5);
        gain(conn, id, 1100.0, "Longblades", 9.0);
        gain(conn, id, 1001.0, "Agility", 0.01);
    }

    #[tokio::test]
    async fn a_weapon_owns_only_the_kills_it_made_alone_and_the_gains_before_them() {
        let (_dir, db) = open_db().await;
        let cells = db
            .with_writer(|conn| {
                seed(conn, "s", true);
                Ok(compute(conn, "s")?)
            })
            .await
            .unwrap();
        let sword = cells
            .weapons
            .iter()
            .find(|cell| cell.tool == "Sword")
            .unwrap();
        let chip = cells
            .weapons
            .iter()
            .find(|cell| cell.tool == "Chip")
            .unwrap();
        // The shared kill and the unread tool's are no one's.
        assert_eq!(cells.weapons.len(), 2);
        assert_eq!((sword.kills, chip.kills), (3, 1));
        assert!((sword.cycled_ped - 3.0 * 0.6).abs() < 1e-9);
        assert!((sword.loot_tt - 6.0).abs() < 1e-9);
        // Gains go to the next event: the sword's two kills, the chip's
        // kill; the gain before the shared kill and the one before the
        // swing are no weapon's, nor is one far from any event, nor an
        // attribute.
        assert_eq!(sword.pes.len(), 1);
        assert_eq!(sword.pes[0].0, "Longblades");
        assert!((sword.pes[0].1 - 0.3).abs() < 1e-9);
        assert_eq!(chip.pes, vec![("Electrokinesis".to_string(), 0.3)]);
        // Active time is every gap of a minute or less: 2, 3, 2, 2, 1; the
        // 190 s before the last kill is a break.
        assert!((sword.active_seconds - 10.0).abs() < 1e-9);
        assert!((sword.play_seconds - 2.0).abs() < 1e-9);
        assert!((chip.play_seconds - 3.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn pace_counts_a_weapons_own_kills_within_a_break_of_the_last() {
        let (_dir, db) = open_db().await;
        let cells = db
            .with_writer(|conn| {
                seed(conn, "s", true);
                Ok(compute(conn, "s")?)
            })
            .await
            .unwrap();
        let sword = cells
            .paces
            .iter()
            .find(|pace| pace.tool == "Sword")
            .unwrap();
        // Only the second sword kill: the first has no kill before it and
        // the last follows a break.
        assert_eq!((sword.kills, sword.gap_seconds, sword.shots), (1, 2.0, 1.0));
        assert_eq!(
            (sword.species.as_str(), sword.maturity.as_str()),
            ("Carabok", "Puny")
        );
        let chip = cells.paces.iter().find(|pace| pace.tool == "Chip").unwrap();
        assert_eq!((chip.kills, chip.gap_seconds), (1, 3.0));
    }

    #[tokio::test]
    async fn settled_cells_read_back_as_the_raw_computation() {
        let (_dir, db) = open_db().await;
        db.with_writer(|conn| {
            seed(conn, "ended", true);
            seed(conn, "live", false);
            Ok(())
        })
        .await
        .unwrap();
        db.with_writer(crate::session_rollup::heal).await.unwrap();
        let (cells, paces, raw, settled) = db
            .with_reader(|conn| {
                let settled: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM session_weapon_rollups WHERE session_id = 'ended'",
                    [],
                    |row| row.get(0),
                )?;
                Ok((
                    effective_weapon_cells(conn, Some("Sword"))?,
                    effective_pace_cells(conn, "Carabok", "Puny")?,
                    compute(conn, "ended")?,
                    settled,
                ))
            })
            .await
            .unwrap();
        // The ended session settled; the live one is read raw.
        assert_eq!(settled, 2);
        assert_eq!(cells.len(), 2);
        let raw_sword = raw
            .weapons
            .iter()
            .find(|cell| cell.tool == "Sword")
            .unwrap();
        for (_, cell) in &cells {
            assert_eq!(cell, raw_sword);
        }
        assert_eq!(paces.len(), 4);

        // Dropping the session's rollups drops its cells with them.
        db.with_writer(|conn| {
            crate::session_rollup::drop_session(conn, "ended")?;
            let left: i64 = conn.query_row(
                "SELECT COUNT(*) FROM session_weapon_pace_rollups WHERE session_id = 'ended'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(left, 0);
            Ok(())
        })
        .await
        .unwrap();
    }
}
