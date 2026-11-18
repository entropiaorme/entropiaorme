//! Read the account site's Inventory Changes export, and what a strongbox
//! opening looks like in it.
//!
//! The site's Inventory Changes page keeps a starting point and shows how
//! the inventory moved since, item by item; its "Copy CSV" button emits a
//! header row (`Name`, `Quantity`, `Value(PED)`) and then one row per item
//! name with the signed change in count and in TT. Opening strongboxes
//! writes nothing to the chat log, so this export is the one record the game
//! gives of what a run of openings produced.
//!
//! Parsing follows the market paste's discipline: pure functions over lines,
//! no panics, and a malformed line is reported with its number and a reason
//! rather than failing the paste. Each row is read from the right (the last
//! two fields are numbers, everything before them is the name), so a paste
//! whose tabs were turned into spaces on the way still reads, and a comma is
//! accepted as the decimal separator.

use std::sync::OnceLock;

use regex::Regex;

/// The key every strongbox takes, as the game names it.
pub const STRONGBOX_KEY: &str = "Strongbox Key";

const UNIVERSAL_AMMO: &str = "Universal Ammo";

/// One item's change since the page's starting point.
#[derive(Debug, Clone, PartialEq)]
pub struct InventoryChange {
    /// 1-based line number in the pasted text.
    pub line: usize,
    pub item_name: String,
    /// Signed: negative when units left the inventory.
    pub quantity: f64,
    /// Signed TT change in PED.
    pub value_ped: f64,
}

/// A line that carried text but not a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedLine {
    pub line: usize,
    pub text: String,
    pub reason: String,
}

/// Everything a paste held.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InventoryChanges {
    pub rows: Vec<InventoryChange>,
    pub skipped: Vec<SkippedLine>,
}

fn row_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(.+?)\s+([+-]?\d+(?:[.,]\d+)?)\s+([+-]?\d+(?:[.,]\d+)?)$")
            .expect("inventory change row pattern compiles")
    })
}

fn number(text: &str) -> Option<f64> {
    text.replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn is_header(line: &str) -> bool {
    let first = line.split(['\t', ' ']).next().unwrap_or_default();
    first.eq_ignore_ascii_case("name") && line.to_ascii_lowercase().contains("quantity")
}

/// Parse a pasted export. Blank lines and the header are passed over
/// silently; anything else that is not a row is reported.
pub fn parse_inventory_changes(text: &str) -> InventoryChanges {
    let mut parsed = InventoryChanges::default();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || is_header(trimmed) {
            continue;
        }
        let skip = |reason: &str| SkippedLine {
            line,
            text: trimmed.to_string(),
            reason: reason.to_string(),
        };
        let Some(captures) = row_re().captures(trimmed) else {
            parsed
                .skipped
                .push(skip("not an item name followed by a quantity and a value"));
            continue;
        };
        let name = captures[1].trim().to_string();
        let (Some(quantity), Some(value_ped)) = (number(&captures[2]), number(&captures[3])) else {
            parsed
                .skipped
                .push(skip("the quantity or value is not a number"));
            continue;
        };
        if quantity == 0.0 && value_ped == 0.0 {
            parsed.skipped.push(skip("nothing changed on this row"));
            continue;
        }
        parsed.rows.push(InventoryChange {
            line,
            item_name: name,
            quantity,
            value_ped,
        });
    }
    parsed
}

/// The keys an opening used up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeysUsed {
    pub count: f64,
    /// TT of the keys used, as a positive amount.
    pub tt_value: f64,
}

/// Why a row of the export does not fit an opening of the chosen box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnexpectedReason {
    /// Another kind of strongbox moved: one box type is opened per run.
    OtherBox,
    /// Something left the inventory that opening a box does not consume.
    Removed,
    /// Boxes or keys arrived rather than left.
    Gained,
    /// The row changed in TT but not in count, as a worn item does.
    Worn,
}

/// One row the opening does not account for.
#[derive(Debug, Clone, PartialEq)]
pub struct UnexpectedChange {
    pub change: InventoryChange,
    pub reason: UnexpectedReason,
}

/// An export read as an opening of one box type.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpeningRead {
    /// How many of the chosen box left the inventory.
    pub boxes_opened: f64,
    pub keys: Option<KeysUsed>,
    /// The Universal Ammo that came back, in PED.
    pub ammo_ped: f64,
    /// What the boxes held besides ammo, in the export's order.
    pub contents: Vec<InventoryChange>,
    pub unexpected: Vec<UnexpectedChange>,
    pub skipped: Vec<SkippedLine>,
}

impl OpeningRead {
    /// TT the run produced minus the TT of the keys it used: zero for a
    /// clean opening, up to the PEC the export truncates per row.
    pub fn balance_gap(&self) -> f64 {
        let produced: f64 =
            self.contents.iter().map(|row| row.value_ped).sum::<f64>() + self.ammo_ped;
        produced - self.keys.map_or(0.0, |keys| keys.tt_value)
    }
}

fn is_strongbox(name: &str) -> bool {
    name.trim().to_ascii_lowercase().ends_with("strongbox")
}

/// Read a parsed export as an opening of `box_item`.
pub fn read_opening(changes: InventoryChanges, box_item: &str) -> OpeningRead {
    let mut read = OpeningRead {
        skipped: changes.skipped,
        ..OpeningRead::default()
    };
    let unexpected = |change: InventoryChange, reason| UnexpectedChange { change, reason };
    for change in changes.rows {
        let name = change.item_name.as_str();
        if name.eq_ignore_ascii_case(box_item.trim()) {
            if change.quantity < 0.0 {
                read.boxes_opened += -change.quantity;
            } else {
                read.unexpected
                    .push(unexpected(change, UnexpectedReason::Gained));
            }
        } else if name.eq_ignore_ascii_case(STRONGBOX_KEY) {
            if change.quantity < 0.0 {
                let keys = read.keys.get_or_insert(KeysUsed {
                    count: 0.0,
                    tt_value: 0.0,
                });
                keys.count += -change.quantity;
                keys.tt_value += -change.value_ped;
            } else {
                read.unexpected
                    .push(unexpected(change, UnexpectedReason::Gained));
            }
        } else if is_strongbox(name) {
            read.unexpected
                .push(unexpected(change, UnexpectedReason::OtherBox));
        } else if name.eq_ignore_ascii_case(UNIVERSAL_AMMO) {
            if change.value_ped >= 0.0 {
                read.ammo_ped += change.value_ped;
            } else {
                read.unexpected
                    .push(unexpected(change, UnexpectedReason::Removed));
            }
        } else if change.quantity < 0.0 {
            read.unexpected
                .push(unexpected(change, UnexpectedReason::Removed));
        } else if change.quantity == 0.0 {
            read.unexpected
                .push(unexpected(change, UnexpectedReason::Worn));
        } else {
            read.contents.push(change);
        }
    }
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The export of eight Halloween Strongboxes opened in game, as the
    /// site's Copy CSV emits it.
    const HALLOWEEN: &str = "Name\tQuantity\tValue(PED)\n\
        Red Heart Fireworks\t1\t2.0000\n\
        Skeleton Shirt White (F)\t1\t10.0000\n\
        Skeleton Shirt White (M)\t1\t10.0000\n\
        Halloween Strongbox\t-8\t0.0000\n\
        Strongbox Key\t-8\t-80.0000\n\
        Universal Ammo\t580000\t58.0000\n";

    #[test]
    fn reads_every_row_of_the_export_and_passes_over_its_header() {
        let parsed = parse_inventory_changes(HALLOWEEN);
        assert!(parsed.skipped.is_empty());
        assert_eq!(parsed.rows.len(), 6);
        assert_eq!(
            parsed.rows[1],
            InventoryChange {
                line: 3,
                item_name: "Skeleton Shirt White (F)".to_string(),
                quantity: 1.0,
                value_ped: 10.0,
            }
        );
        assert_eq!(parsed.rows[4].quantity, -8.0);
        assert_eq!(parsed.rows[4].value_ped, -80.0);
    }

    #[test]
    fn a_paste_whose_tabs_became_spaces_reads_the_same() {
        let mangled = HALLOWEEN.replace('\t', "    ");
        assert_eq!(
            parse_inventory_changes(&mangled),
            parse_inventory_changes(HALLOWEEN)
        );
        let single = HALLOWEEN.replace('\t', " ");
        assert_eq!(
            parse_inventory_changes(&single).rows,
            parse_inventory_changes(HALLOWEEN).rows
        );
    }

    #[test]
    fn a_comma_decimal_and_windows_line_endings_are_accepted() {
        let parsed =
            parse_inventory_changes("Nanocube\t250\t2,5000\r\nStrongbox Key\t-1\t-10,0000\r\n");
        assert!(parsed.skipped.is_empty());
        assert_eq!(parsed.rows[0].value_ped, 2.5);
        assert_eq!(parsed.rows[1].value_ped, -10.0);
    }

    #[test]
    fn lines_that_are_not_rows_are_reported_with_their_number() {
        let parsed = parse_inventory_changes(
            "Name\tQuantity\tValue(PED)\nhello there\nShirt\tx\t1\n\nCube\t0\t0\n",
        );
        assert!(parsed.rows.is_empty());
        let lines: Vec<usize> = parsed.skipped.iter().map(|skip| skip.line).collect();
        assert_eq!(lines, vec![2, 3, 5]);
        assert_eq!(parsed.skipped[2].reason, "nothing changed on this row");
    }

    #[test]
    fn the_halloween_export_reads_as_a_clean_opening_of_eight_boxes() {
        let read = read_opening(parse_inventory_changes(HALLOWEEN), "Halloween Strongbox");
        assert_eq!(read.boxes_opened, 8.0);
        assert_eq!(
            read.keys,
            Some(KeysUsed {
                count: 8.0,
                tt_value: 80.0
            })
        );
        assert_eq!(read.ammo_ped, 58.0);
        let names: Vec<&str> = read
            .contents
            .iter()
            .map(|row| row.item_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "Red Heart Fireworks",
                "Skeleton Shirt White (F)",
                "Skeleton Shirt White (M)"
            ]
        );
        assert!(read.unexpected.is_empty());
        assert!(read.balance_gap().abs() < 1e-9);
    }

    #[test]
    fn the_box_name_matches_without_regard_to_case_or_padding() {
        let read = read_opening(parse_inventory_changes(HALLOWEEN), " halloween strongbox ");
        assert_eq!(read.boxes_opened, 8.0);
    }

    #[test]
    fn rows_an_opening_does_not_explain_are_set_apart_with_a_reason() {
        let text = "Halloween Strongbox\t-2\t0\n\
            Strongbox Key\t-2\t-20\n\
            Easter Strongbox\t-1\t0\n\
            Armatrix LR-35 (L)\t0\t-0.5000\n\
            Animal Oil Residue\t-40\t-0.4000\n\
            Universal Ammo\t200000\t20.0000\n";
        let read = read_opening(parse_inventory_changes(text), "Halloween Strongbox");
        let reasons: Vec<UnexpectedReason> = read.unexpected.iter().map(|row| row.reason).collect();
        assert_eq!(
            reasons,
            vec![
                UnexpectedReason::OtherBox,
                UnexpectedReason::Worn,
                UnexpectedReason::Removed
            ]
        );
        assert!(read.contents.is_empty());
        assert!(read.balance_gap().abs() < 1e-9);
    }

    #[test]
    fn boxes_or_keys_that_arrived_are_not_counted_as_opened() {
        let text = "Halloween Strongbox\t3\t0\nStrongbox Key\t5\t50\n";
        let read = read_opening(parse_inventory_changes(text), "Halloween Strongbox");
        assert_eq!(read.boxes_opened, 0.0);
        assert_eq!(read.keys, None);
        assert_eq!(read.unexpected.len(), 2);
        assert!(read
            .unexpected
            .iter()
            .all(|row| row.reason == UnexpectedReason::Gained));
    }

    #[test]
    fn the_balance_gap_shows_what_the_keys_did_not_pay_for() {
        let text = "Halloween Strongbox\t-1\t0\nStrongbox Key\t-1\t-10\nNanocube\t500\t5\n";
        let read = read_opening(parse_inventory_changes(text), "Halloween Strongbox");
        assert!((read.balance_gap() + 5.0).abs() < 1e-9);
    }
}
