//! The limited armour and plates the player holds, each with the markup it
//! is billed at: what the Armour tab shows and edits.
//!
//! A piece is an item the readings bill by the TT it loses, and a limited
//! one carries the markup the player paid for it. The pieces listed are
//! the limited armour and plates the latest reading holds (by their own
//! names, or as the player placed them), and any piece no longer held that
//! has a markup or a placement of its own. Each is listed under Armour or
//! Plates for the player's own order only: both bill alike.
//!
//! A markup is stamped when it is set (see [`super::roles::PieceMarkup`]):
//! the first value set for a piece covers its wear from the start, and
//! every later one applies from when it was set, so a session already
//! billed keeps the markup it was billed at. A piece with none is billed at
//! the TT it lost.
//!
//! A piece whose TT rose in a later reading, after the markup in force was
//! set, has been replaced by another of the same name: its row asks whether
//! the new one cost the same.

use std::collections::{BTreeMap, BTreeSet};

use super::amount::Pec;
use super::placement;
use super::reading::Container;
use super::roles::{
    has_gender_tag, is_blueprint, known_category, CostCategory, KnownRoles, RoleSource,
    UNIVERSAL_AMMO,
};
use super::store;
use super::AccountError;

/// Which list a piece is shown in: for the player's order only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PieceList {
    Armour,
    Plates,
}

impl PieceList {
    fn of(category: CostCategory) -> Option<PieceList> {
        match category {
            CostCategory::LimitedArmour => Some(PieceList::Armour),
            CostCategory::LimitedPlate => Some(PieceList::Plates),
            _ => None,
        }
    }
}

/// One limited piece.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmourPiece {
    /// Its exact name, as the account names it.
    pub item: String,
    pub list: PieceList,
    /// Whether the latest reading holds it.
    pub held: bool,
    /// Its TT in the latest reading, when held.
    pub tt: Option<Pec>,
    /// The markup in force now, in percent; `None` bills it at TT.
    pub markup_percent: Option<f64>,
    /// When the markup in force was set.
    pub markup_set_at: Option<f64>,
    /// Whether the player placed it here themselves (it can be taken back
    /// off the list); otherwise its own name makes it a limited piece.
    pub declared: bool,
    /// When a reading showed its TT rise after the markup in force was set:
    /// a new piece of the same name, whose markup may differ.
    pub replaced_at: Option<f64>,
}

/// Why an item is offered first when the player declares a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateHint {
    /// It wore in a measured session and nothing places it yet.
    AwaitingPlacement,
    /// It sits attached to another item, as a plate does.
    Attached,
    /// Its name carries the gender tag armour carries.
    Tagged,
    /// Nothing about it suggests a piece.
    Other,
}

/// An item of the latest reading the player may declare a piece.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmourCandidate {
    pub item: String,
    pub hint: CandidateHint,
}

/// The pieces and the items that may be declared one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ArmourPieces {
    /// Held first, then by list and name.
    pub pieces: Vec<ArmourPiece>,
    /// The likeliest first, then by name.
    pub candidates: Vec<ArmourCandidate>,
    /// When the latest reading was taken; `None` before any.
    pub read_at: Option<f64>,
}

/// The markups set for one piece, oldest first: `(markup, effective_from,
/// set_at)`.
type MarkupRows = Vec<(Option<f64>, Option<f64>, f64)>;

fn markups(conn: &rusqlite::Connection) -> Result<BTreeMap<String, MarkupRows>, AccountError> {
    let mut stmt = conn.prepare(
        // id-order: insertion (values set at one instant keep their order)
        "SELECT item_name, markup_percent, effective_from, set_at \
         FROM limited_piece_markups ORDER BY set_at, id",
    )?;
    let mut rows = stmt.query([])?;
    let mut out: BTreeMap<String, MarkupRows> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        out.entry(row.get::<_, String>(0)?.trim().to_string())
            .or_default()
            .push((row.get(1)?, row.get(2)?, row.get(3)?));
    }
    Ok(out)
}

/// When a reading after `since` showed the item's TT rise in one place
/// above what it last held there: a new piece of the same name.
fn replaced_since(
    conn: &rusqlite::Connection,
    item: &str,
    since: f64,
) -> Result<Option<f64>, AccountError> {
    let mut stmt = conn.prepare(
        // id-order: insertion (the chain follows the order readings were stored)
        "SELECT r.taken_at, h.container_kind, h.container_name, h.quantity, h.value_pec \
         FROM account_reading_holdings h JOIN account_readings r ON r.id = h.reading_id \
         WHERE h.item_name = ?1 AND h.container_kind != 'auction' ORDER BY h.reading_id",
    )?;
    let mut rows = stmt.query([item])?;
    let mut last: BTreeMap<(String, Option<String>), i64> = BTreeMap::new();
    let mut replaced = None;
    while let Some(row) = rows.next()? {
        let taken_at: f64 = row.get(0)?;
        let place = (row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?);
        let quantity: i64 = row.get(3)?;
        let value: i64 = row.get(4)?;
        if quantity <= 0 {
            continue;
        }
        if let Some(before) = last.get(&place) {
            if value > before + 1 && taken_at > since {
                replaced = Some(taken_at);
            }
        }
        last.insert(place, value);
    }
    Ok(replaced)
}

/// The category an item takes for the Armour tab: the player's placement,
/// else what its name or the game data say. A blueprint is never a piece.
fn piece_category(roles: &KnownRoles, name: &str) -> Option<CostCategory> {
    if is_blueprint(name) {
        return None;
    }
    roles
        .placed_category(name)
        .or_else(|| known_category(roles, name))
}

/// The limited pieces and the declarable items, from the latest reading,
/// the player's placements and the markups set. A read.
pub fn pieces(
    conn: &rusqlite::Connection,
    roles: &KnownRoles,
) -> Result<ArmourPieces, AccountError> {
    let latest = store::latest_reading(conn)?;
    let markups = markups(conn)?;
    let placed: BTreeMap<String, CostCategory> = placement::read_all(conn)?
        .into_iter()
        .map(|placed| (placed.item.trim().to_string(), placed.category))
        .collect();
    let awaiting: BTreeSet<String> = placement::placements(conn)?
        .awaiting
        .into_iter()
        .filter(|awaiting| awaiting.lost.is_some())
        .map(|awaiting| awaiting.item.trim().to_string())
        .collect();

    // What the latest reading holds, by name: TT, and whether any of it
    // sits attached to another item.
    let mut held: BTreeMap<String, (Pec, bool)> = BTreeMap::new();
    if let Some(latest) = &latest {
        for (key, holding) in latest.reading.holdings() {
            if key.container == Container::Auction {
                continue;
            }
            let name = key.name.trim().to_string();
            let entry = held.entry(name).or_insert((Pec::ZERO, false));
            entry.0 = Pec(entry.0 .0.saturating_add(holding.value().0));
            entry.1 |= matches!(key.container, Container::Attached { .. });
        }
    }

    let mut names: BTreeSet<String> = held.keys().cloned().collect();
    names.extend(markups.keys().cloned());
    names.extend(placed.keys().cloned());
    let mut pieces = Vec::new();
    let mut candidates = Vec::new();
    for name in names {
        let list = piece_category(roles, &name).and_then(PieceList::of);
        let holding = held.get(&name);
        let Some(list) = list else {
            // Only what the latest reading holds may be declared.
            if holding.is_some() && name != UNIVERSAL_AMMO && !is_blueprint(&name) {
                let hint = if awaiting.contains(&name) {
                    CandidateHint::AwaitingPlacement
                } else if holding.is_some_and(|(_, attached)| *attached) {
                    CandidateHint::Attached
                } else if has_gender_tag(&name) {
                    CandidateHint::Tagged
                } else {
                    CandidateHint::Other
                };
                candidates.push(ArmourCandidate { item: name, hint });
            }
            continue;
        };
        let in_force = markups.get(&name).and_then(|rows| rows.last()).copied();
        let markup_set_at = in_force.map(|(_, _, set_at)| set_at);
        let markup_percent = in_force.and_then(|(markup, _, _)| markup);
        let replaced_at = match (markup_percent, markup_set_at) {
            (Some(_), Some(set_at)) if holding.is_some() => replaced_since(conn, &name, set_at)?,
            _ => None,
        };
        pieces.push(ArmourPiece {
            declared: placed.contains_key(&name),
            held: holding.is_some(),
            tt: holding.map(|(tt, _)| *tt),
            item: name,
            list,
            markup_percent,
            markup_set_at,
            replaced_at,
        });
    }
    pieces.sort_by(|a, b| {
        b.held
            .cmp(&a.held)
            .then(a.list.cmp(&b.list))
            .then_with(|| a.item.cmp(&b.item))
    });
    candidates.sort_by(|a, b| a.hint.cmp(&b.hint).then_with(|| a.item.cmp(&b.item)));
    Ok(ArmourPieces {
        pieces,
        candidates,
        read_at: latest.map(|latest| latest.reading.taken_at()),
    })
}

/// Set the markup a limited piece is billed at, in percent (`None` returns
/// it to TT), from `now` on; the first value set for a piece covers its
/// wear from the start. The caller has checked the markup.
pub fn set_markup(
    conn: &rusqlite::Connection,
    item: &str,
    markup_percent: Option<f64>,
    now: f64,
) -> Result<(), AccountError> {
    let item = item.trim();
    let first: bool = conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM limited_piece_markups WHERE item_name = ?1)",
        [item],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO limited_piece_markups (item_name, markup_percent, effective_from, set_at) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![item, markup_percent, (!first).then_some(now), now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::store::{record_reading, Purpose};
    use crate::account::test_support::reading;
    use crate::db::migrated_in_memory;

    const C: Container = Container::Carried;

    fn read(conn: &mut rusqlite::Connection, at: f64, rows: &[(&str, Container, i64, i64)]) {
        record_reading(conn, &reading(at, 1_000, rows), Purpose::Ambient, None).unwrap();
    }

    fn on(parent: &str) -> Container {
        Container::Attached {
            parent: parent.to_string(),
        }
    }

    fn pieces_of(conn: &rusqlite::Connection) -> ArmourPieces {
        let roles = KnownRoles::read(conn, None).unwrap();
        pieces(conn, &roles).unwrap()
    }

    fn piece<'a>(listed: &'a ArmourPieces, item: &str) -> &'a ArmourPiece {
        listed
            .pieces
            .iter()
            .find(|piece| piece.item == item)
            .unwrap_or_else(|| panic!("{item} is listed: {listed:?}"))
    }

    #[test]
    fn the_limited_armour_and_plates_held_are_listed_and_the_rest_offered() {
        let mut conn = migrated_in_memory().unwrap();
        read(
            &mut conn,
            100.0,
            &[
                ("Pegasus Helmet (M,L)", C, 1, 1_000),
                ("Frontier Harness (M)", C, 1, 4_000),
                (
                    "Armor Plating Mk. 5B (L)",
                    on("Frontier Harness (M)"),
                    1,
                    2_500,
                ),
                ("Odd Plate", on("Frontier Harness (M)"), 1, 300),
                ("Pegasus Helmet (M,L) Blueprint (L)", C, 1, 1),
                ("Animal Hide", C, 10, 50),
            ],
        );
        let listed = pieces_of(&conn);
        let names: Vec<(&str, PieceList, bool)> = listed
            .pieces
            .iter()
            .map(|piece| (piece.item.as_str(), piece.list, piece.held))
            .collect();
        assert_eq!(
            names,
            [
                ("Pegasus Helmet (M,L)", PieceList::Armour, true),
                ("Armor Plating Mk. 5B (L)", PieceList::Plates, true),
            ]
        );
        let helmet = piece(&listed, "Pegasus Helmet (M,L)");
        assert_eq!(helmet.tt, Some(Pec(1_000)));
        assert_eq!(helmet.markup_percent, None, "billed at TT until set");
        assert!(!helmet.declared);
        assert_eq!(listed.read_at, Some(100.0));

        // What may be declared a piece, the likeliest first; a blueprint
        // never.
        let offered: Vec<(&str, CandidateHint)> = listed
            .candidates
            .iter()
            .map(|candidate| (candidate.item.as_str(), candidate.hint))
            .collect();
        assert_eq!(
            offered,
            [
                ("Odd Plate", CandidateHint::Attached),
                ("Frontier Harness (M)", CandidateHint::Tagged),
                ("Animal Hide", CandidateHint::Other),
            ]
        );
    }

    #[test]
    fn a_markup_set_is_shown_with_when_and_a_declared_piece_is_listed() {
        let mut conn = migrated_in_memory().unwrap();
        read(
            &mut conn,
            100.0,
            &[
                ("Pegasus Helmet (M,L)", C, 1, 1_000),
                ("Odd Plate", on("Pegasus Helmet (M,L)"), 1, 300),
            ],
        );
        set_markup(&conn, "Pegasus Helmet (M,L)", Some(140.0), 200.0).unwrap();
        set_markup(&conn, "Pegasus Helmet (M,L)", Some(150.0), 300.0).unwrap();
        conn.execute(
            "INSERT INTO cost_item_placements (item_name, category, placed_at) \
             VALUES ('Odd Plate', 'limitedPlate', 1.0)",
            [],
        )
        .unwrap();
        let listed = pieces_of(&conn);
        let helmet = piece(&listed, "Pegasus Helmet (M,L)");
        assert_eq!(helmet.markup_percent, Some(150.0));
        assert_eq!(helmet.markup_set_at, Some(300.0));
        let plate = piece(&listed, "Odd Plate");
        assert_eq!(plate.list, PieceList::Plates);
        assert!(plate.declared);
        assert!(listed.candidates.is_empty());

        // The first value covers the piece from the start; later ones from
        // when they were set.
        let stamped: Vec<(Option<f64>, Option<f64>)> = conn
            .prepare(
                // id-order: insertion (a test reading what it just wrote)
                "SELECT markup_percent, effective_from FROM limited_piece_markups ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(stamped, [(Some(140.0), None), (Some(150.0), Some(300.0))]);
    }

    #[test]
    fn a_piece_whose_tt_rose_after_its_markup_was_set_was_replaced() {
        let mut conn = migrated_in_memory().unwrap();
        read(&mut conn, 100.0, &[("Pegasus Helmet (M,L)", C, 1, 1_000)]);
        set_markup(&conn, "Pegasus Helmet (M,L)", Some(140.0), 150.0).unwrap();
        read(&mut conn, 200.0, &[("Pegasus Helmet (M,L)", C, 1, 900)]);
        assert_eq!(
            piece(&pieces_of(&conn), "Pegasus Helmet (M,L)").replaced_at,
            None,
            "wear is no replacement"
        );
        read(&mut conn, 300.0, &[("Pegasus Helmet (M,L)", C, 1, 1_000)]);
        assert_eq!(
            piece(&pieces_of(&conn), "Pegasus Helmet (M,L)").replaced_at,
            Some(300.0)
        );
    }

    #[test]
    fn a_piece_no_longer_held_stays_listed_while_it_has_a_markup() {
        let mut conn = migrated_in_memory().unwrap();
        read(&mut conn, 100.0, &[("Pegasus Helmet (M,L)", C, 1, 1_000)]);
        set_markup(&conn, "Pegasus Helmet (M,L)", Some(140.0), 150.0).unwrap();
        read(&mut conn, 200.0, &[("Animal Hide", C, 10, 50)]);
        let helmet = piece(&pieces_of(&conn), "Pegasus Helmet (M,L)").clone();
        assert!(!helmet.held);
        assert_eq!(helmet.tt, None);
        assert_eq!(helmet.markup_percent, Some(140.0));
    }
}
