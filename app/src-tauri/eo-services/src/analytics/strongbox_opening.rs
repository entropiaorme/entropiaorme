//! Strongbox openings: a run of boxes of one type, opened with their keys,
//! recorded as the conversion it is.
//!
//! A strongbox has no TT of its own and opens only with a key, which is
//! bought from the webshop for 10 PED of TT and pays back exactly that TT in
//! whatever the box held: mostly Universal Ammo, sometimes items. So an
//! opening never gains or loses TT, and the key is money the player put in,
//! not money the game took. What a box is worth is the markup on what it
//! held, realised only when that sells.
//!
//! The run is written as one itemised conversion: the boxes (looted, held as
//! an asset, or bought for the run at a markup) and their keys go in; the
//! items come out carrying the boxes' origin, and the ammo comes out as PED.
//! Items the player turns into Nanocubes are a second conversion each, in
//! the same transaction, so the cubes keep the origin too. Selling any of it
//! later credits the gameplay that looted the boxes through the one realised
//! reader, with TT never counted as gain.

use uuid::Uuid;

use super::conversion::{self, ConversionTerms};
use super::{
    is_universal_ammo, naive_to_epoch, undo_conversion_within, AnalyticsError, AnalyticsService,
    ConversionHolding, ConversionInput, ConversionOutput, ItemConversion, Profession,
};
use crate::db::DbError;

/// What a Nanocube is worth in TT: the unit a cubed item is counted in.
const NANOCUBE: &str = "Nanocube";
const NANOCUBE_UNIT_TT: f64 = 0.01;

/// How far the contents may fall short of or exceed the keys' TT, per row:
/// the site truncates each value to the whole PEC.
const PEC_PER_ROW: f64 = 0.01;

/// Where the boxes of a run came from.
#[derive(Debug, Clone, PartialEq)]
pub enum BoxSource {
    /// Looted, or otherwise held as stock: drawn by the usual weighted split,
    /// so their origin rides forward.
    Held { quantity: f64 },
    /// Held as an asset, consumed at its average cost.
    Asset { item_id: String, quantity: f64 },
    /// Bought for this run: no origin, and `markup_paid` is what they cost.
    Bought { quantity: f64, markup_paid: f64 },
}

impl BoxSource {
    fn quantity(&self) -> f64 {
        match self {
            Self::Held { quantity }
            | Self::Asset { quantity, .. }
            | Self::Bought { quantity, .. } => *quantity,
        }
    }
}

/// How the result of a run reached the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpeningIntake {
    /// Read from the account site's Inventory Changes export.
    Export,
    /// Entered by the player.
    Manual,
}

impl OpeningIntake {
    fn as_str(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Manual => "manual",
        }
    }

    fn from_db(value: &str) -> Self {
        if value == "manual" {
            Self::Manual
        } else {
            Self::Export
        }
    }
}

/// One thing the boxes held besides ammo.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedItem {
    pub item_name: String,
    pub quantity: f64,
    pub tt_value: f64,
    /// Turned into Nanocubes rather than kept as it is.
    pub recycle: bool,
}

/// A run of openings, as the player records it.
#[derive(Debug, Clone, PartialEq)]
pub struct StrongboxOpening {
    pub box_item: String,
    pub sources: Vec<BoxSource>,
    /// TT of one key: 10 PED for every box seen so far.
    pub key_unit_tt: f64,
    /// Markup paid on the keys, for keys bought from other players. Webshop
    /// keys cost nothing above their TT.
    pub key_markup_paid: f64,
    pub contents: Vec<OpenedItem>,
    /// The Universal Ammo that came back, in PED.
    pub ammo_ped: f64,
    pub intake: OpeningIntake,
    pub opened_at: Option<String>,
}

/// One item of a recorded run, as it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedItemRow {
    pub item_name: String,
    pub quantity: f64,
    pub tt_value: f64,
    /// The Nanocubes it became, while that conversion stands.
    pub nanocubes: Option<f64>,
}

/// A recorded run, for the openings list.
#[derive(Debug, Clone, PartialEq)]
pub struct StrongboxOpeningRow {
    pub id: String,
    pub box_item: String,
    pub box_count: f64,
    pub box_markup_paid: f64,
    pub key_tt: f64,
    pub key_markup_paid: f64,
    pub ammo_ped: f64,
    pub contents: Vec<OpenedItemRow>,
    pub intake: OpeningIntake,
    pub opened_at: String,
    /// Taken back, here or by undoing its conversion from History.
    pub undone: bool,
}

fn whole(value: f64) -> bool {
    value.is_finite() && value > 0.0 && (value - value.round()).abs() < 1e-9
}

fn validate(opening: &StrongboxOpening) -> Result<(), AnalyticsError> {
    let invalid = AnalyticsError::InvalidInput;
    let name = opening.box_item.trim();
    if name.is_empty() {
        return Err(invalid("an opening needs the box it opened"));
    }
    if is_universal_ammo(name) {
        return Err(invalid("Universal Ammo is not a box"));
    }
    if opening.sources.is_empty() {
        return Err(invalid("an opening needs the boxes it opened"));
    }
    if !opening
        .sources
        .iter()
        .all(|source| whole(source.quantity()))
    {
        return Err(invalid(
            "boxes are opened whole: every count must be a whole number",
        ));
    }
    if opening.sources.iter().any(|source| {
        matches!(source, BoxSource::Bought { markup_paid, .. }
            if !markup_paid.is_finite() || *markup_paid < 0.0)
    }) {
        return Err(invalid("a markup paid cannot be negative"));
    }
    if !(opening.key_unit_tt.is_finite() && opening.key_unit_tt > 0.0) {
        return Err(invalid("a key needs its TT value"));
    }
    if !opening.key_markup_paid.is_finite() || opening.key_markup_paid < 0.0 {
        return Err(invalid("a markup paid cannot be negative"));
    }
    if !opening.ammo_ped.is_finite() || opening.ammo_ped < 0.0 {
        return Err(invalid("the ammo that came back cannot be negative"));
    }
    if opening.contents.is_empty() && opening.ammo_ped <= 0.0 {
        return Err(invalid("an opening needs what the boxes held"));
    }
    for item in &opening.contents {
        let item_name = item.item_name.trim();
        if item_name.is_empty() {
            return Err(invalid("every item needs a name"));
        }
        if is_universal_ammo(item_name) {
            return Err(invalid(
                "Universal Ammo is counted as the ammo that came back",
            ));
        }
        if !(item.quantity.is_finite() && item.quantity > 0.0) {
            return Err(invalid("every item needs a positive quantity"));
        }
        if !item.tt_value.is_finite() || item.tt_value < 0.0 {
            return Err(invalid("a TT value cannot be negative"));
        }
        if item.recycle && item_name.eq_ignore_ascii_case(NANOCUBE) {
            return Err(invalid("Nanocubes are already Nanocubes"));
        }
        if item.recycle && item.tt_value < NANOCUBE_UNIT_TT {
            return Err(invalid(
                "an item worth less than a Nanocube cannot be cubed",
            ));
        }
    }
    Ok(())
}

fn box_count(opening: &StrongboxOpening) -> f64 {
    opening.sources.iter().map(BoxSource::quantity).sum()
}

/// TT the contents and ammo come to against the keys' TT, refused when the
/// gap is more than the export's per-row truncation explains. A key always
/// pays back its TT, so a larger gap means the result was read or entered
/// wrong, and recording it would book the difference as a gain or a loss.
fn check_balance(opening: &StrongboxOpening) -> Result<(), AnalyticsError> {
    let keys = box_count(opening) * opening.key_unit_tt;
    let produced = opening
        .contents
        .iter()
        .map(|item| item.tt_value)
        .sum::<f64>()
        + opening.ammo_ped;
    let tolerance = PEC_PER_ROW * (opening.contents.len() as f64 + 1.0) + 1e-9;
    if (produced - keys).abs() > tolerance {
        return Err(AnalyticsError::Rejected(format!(
            "What came out comes to {produced:.2} PED TT, but {count} keys are \
             {keys:.2} PED. A key always pays back its TT, so check the result.",
            count = box_count(opening),
        )));
    }
    Ok(())
}

/// Take the ammo as what the keys did not pay back in items. A key always
/// pays back its TT, and the export truncates every value to the whole PEC,
/// so the ammo row is the one figure that can absorb the truncation; reading
/// it as given would book the lost PEC as a loss nobody made.
fn settle_ammo(mut opening: StrongboxOpening) -> StrongboxOpening {
    let keys = box_count(&opening) * opening.key_unit_tt;
    let items: f64 = opening.contents.iter().map(|item| item.tt_value).sum();
    opening.ammo_ped = eo_wire::normalizer::round_half_even((keys - items).max(0.0), 4);
    opening
}

fn opening_conversion(opening: &StrongboxOpening, converted_at: &str) -> ItemConversion {
    let box_item = opening.box_item.trim().to_string();
    let mut inputs: Vec<ConversionInput> = opening
        .sources
        .iter()
        .map(|source| match source {
            BoxSource::Held { quantity } => ConversionInput::Held {
                item_name: box_item.clone(),
                quantity: *quantity,
                worth: None,
            },
            BoxSource::Asset { item_id, quantity } => ConversionInput::Asset {
                item_id: item_id.clone(),
                quantity: Some(*quantity),
                worth: None,
            },
            BoxSource::Bought {
                quantity,
                markup_paid,
            } => ConversionInput::Bought {
                item_name: box_item.clone(),
                quantity: *quantity,
                tt_value: 0.0,
                markup_paid: *markup_paid,
            },
        })
        .collect();
    let count = box_count(opening);
    inputs.push(ConversionInput::Bought {
        item_name: crate::inventory_changes::STRONGBOX_KEY.to_string(),
        quantity: count,
        tt_value: count * opening.key_unit_tt,
        markup_paid: opening.key_markup_paid,
    });
    ItemConversion {
        inputs,
        outputs: opening
            .contents
            .iter()
            .map(|item| ConversionOutput {
                item_name: item.item_name.trim().to_string(),
                quantity: item.quantity,
                tt_value: item.tt_value,
                holding: ConversionHolding::Loot,
            })
            .collect(),
        ped_amount: (opening.ammo_ped > 0.0).then_some(opening.ammo_ped),
        converted_at: Some(converted_at.to_string()),
    }
}

fn recycle_conversion(item: &OpenedItem, converted_at: &str) -> ItemConversion {
    ItemConversion {
        inputs: vec![ConversionInput::Held {
            item_name: item.item_name.trim().to_string(),
            quantity: item.quantity,
            worth: None,
        }],
        outputs: vec![ConversionOutput {
            item_name: NANOCUBE.to_string(),
            quantity: item.tt_value / NANOCUBE_UNIT_TT,
            tt_value: item.tt_value,
            holding: ConversionHolding::Loot,
        }],
        ped_amount: None,
        converted_at: Some(converted_at.to_string()),
    }
}

/// Every asset source must be a held position of the box being opened.
fn check_assets(
    tx: &rusqlite::Transaction<'_>,
    opening: &StrongboxOpening,
) -> Result<Result<(), String>, DbError> {
    use rusqlite::OptionalExtension as _;
    for source in &opening.sources {
        let BoxSource::Asset { item_id, .. } = source else {
            continue;
        };
        let name: Option<String> = tx
            .query_row(
                "SELECT name FROM inventory_items WHERE id = ? AND state = 'held'",
                rusqlite::params![item_id],
                |row| row.get(0),
            )
            .optional()?;
        match name {
            Some(name) if name.trim().eq_ignore_ascii_case(opening.box_item.trim()) => {}
            Some(name) => {
                return Ok(Err(format!(
                    "That asset is {name}, not the box being opened"
                )))
            }
            None => return Ok(Err("That asset is no longer held".to_string())),
        }
    }
    Ok(Ok(()))
}

/// What the run's asset boxes cost above TT, read from the units the opening
/// conversion carved out of their positions.
fn asset_markup_paid(tx: &rusqlite::Transaction<'_>, conversion_id: &str) -> rusqlite::Result<f64> {
    tx.query_row(
        "SELECT COALESCE(SUM(i.markup_paid), 0) FROM stock_conversion_lines l \
         JOIN inventory_items i ON i.id = l.inventory_item_id \
         WHERE l.conversion_id = ? AND l.side = 'input' AND l.acquisition = 'asset'",
        rusqlite::params![conversion_id],
        |row| row.get(0),
    )
}

impl AnalyticsService {
    /// Record a run of strongbox openings: the boxes and keys in, what they
    /// held out, and any of it turned into Nanocubes, all or nothing.
    pub async fn open_strongboxes(
        &self,
        opening: StrongboxOpening,
    ) -> Result<String, AnalyticsError> {
        validate(&opening)?;
        check_balance(&opening)?;
        let opening = settle_ammo(opening);
        let converted_at = opening
            .opened_at
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| self.default_date());
        let opening_id = Uuid::new_v4().to_string();
        let now = naive_to_epoch(self.clock.now());
        let mut request = opening_conversion(&opening, &converted_at);
        conversion::validate(&mut request)?;
        let mut recycles: Vec<ItemConversion> = Vec::new();
        for item in opening.contents.iter().filter(|item| item.recycle) {
            let mut recycle = recycle_conversion(item, &converted_at);
            conversion::validate(&mut recycle)?;
            recycles.push(recycle);
        }
        let box_item = opening.box_item.trim().to_string();
        let bought_markup: f64 = opening
            .sources
            .iter()
            .map(|source| match source {
                BoxSource::Bought { markup_paid, .. } => *markup_paid,
                _ => 0.0,
            })
            .sum();
        let count = box_count(&opening);
        let id = opening_id.clone();
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                if let Err(reason) = check_assets(&tx, &opening)? {
                    return Ok(Err(reason));
                }
                let conversion_id = Uuid::new_v4().to_string();
                let terms = ConversionTerms {
                    itemised: true,
                    target_label: None,
                    gain_description: format!("Conversion: {box_item}"),
                };
                if let Err(reason) = conversion::write_conversion(
                    &tx,
                    &conversion_id,
                    Profession::Inventory,
                    &request,
                    &converted_at,
                    now,
                    &terms,
                )? {
                    return Ok(Err(reason));
                }
                let box_markup_paid = bought_markup + asset_markup_paid(&tx, &conversion_id)?;
                tx.execute(
                    "INSERT INTO strongbox_openings \
                         (id, box_item, box_count, box_markup_paid, intake, \
                          opening_conversion_id, opened_at, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        id,
                        box_item,
                        count,
                        box_markup_paid,
                        opening.intake.as_str(),
                        conversion_id,
                        converted_at,
                        now,
                    ],
                )?;
                for recycle in &recycles {
                    let recycle_id = Uuid::new_v4().to_string();
                    let terms = ConversionTerms {
                        itemised: false,
                        target_label: Some(NANOCUBE.to_string()),
                        gain_description: format!("Conversion: {box_item}"),
                    };
                    if let Err(reason) = conversion::write_conversion(
                        &tx,
                        &recycle_id,
                        Profession::Inventory,
                        recycle,
                        &converted_at,
                        now,
                        &terms,
                    )? {
                        return Ok(Err(reason));
                    }
                    tx.execute(
                        "INSERT INTO strongbox_opening_recycles (opening_id, conversion_id) \
                         VALUES (?, ?)",
                        rusqlite::params![id, recycle_id],
                    )?;
                }
                tx.commit()?;
                Ok(Ok(()))
            })
            .await?
            .map_err(AnalyticsError::Rejected)?;
        Ok(opening_id)
    }

    /// Take a run back whole: its Nanocubes become the items again, and the
    /// items, ammo, and keys become the boxes. Refused, with nothing changed,
    /// when anything it produced has since been sold or converted.
    pub async fn undo_strongbox_opening(&self, opening_id: &str) -> Result<bool, AnalyticsError> {
        let opening_id = opening_id.to_string();
        let undone_at = self.default_date();
        let now = naive_to_epoch(self.clock.now());
        self.db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let conversion_id: Option<String> = tx
                    .query_row(
                        "SELECT opening_conversion_id FROM strongbox_openings \
                         WHERE id = ? AND undone_at IS NULL",
                        rusqlite::params![opening_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let Some(conversion_id) = conversion_id else {
                    return Ok(Ok(false));
                };
                let recycles: Vec<String> = {
                    let mut stmt = tx.prepare(
                        "SELECT conversion_id FROM strongbox_opening_recycles \
                         WHERE opening_id = ? ORDER BY rowid",
                    )?;
                    let ids = stmt
                        .query_map(rusqlite::params![opening_id], |row| row.get(0))?
                        .collect::<rusqlite::Result<Vec<String>>>()?;
                    ids
                };
                // The cubes first, so the items they took are back in stock
                // when the opening itself is unmade. A conversion already
                // undone from History is simply passed over.
                for id in recycles.iter().chain(std::iter::once(&conversion_id)) {
                    if let Err(reason) = undo_conversion_within(&tx, id, &undone_at, now)? {
                        return Ok(Err(reason));
                    }
                }
                tx.execute(
                    "UPDATE strongbox_openings SET undone_at = ? WHERE id = ?",
                    rusqlite::params![undone_at, opening_id],
                )?;
                tx.commit()?;
                Ok(Ok(true))
            })
            .await?
            .map_err(AnalyticsError::Rejected)
    }

    /// Every recorded run, newest first, with what each opened as it stands.
    pub async fn strongbox_openings(&self) -> Result<Vec<StrongboxOpeningRow>, AnalyticsError> {
        Ok(self
            .db
            .with_reader(|conn| {
                struct Header {
                    id: String,
                    box_item: String,
                    box_count: f64,
                    box_markup_paid: f64,
                    intake: String,
                    conversion_id: String,
                    opened_at: String,
                    undone: bool,
                    ammo_ped: Option<f64>,
                }
                let headers: Vec<Header> = {
                    let mut stmt = conn.prepare(
                        "SELECT o.id, o.box_item, o.box_count, o.box_markup_paid, o.intake, \
                                o.opening_conversion_id, o.opened_at, \
                                o.undone_at IS NOT NULL OR c.undone_at IS NOT NULL, c.ped_amount \
                         FROM strongbox_openings o \
                         JOIN stock_conversions c ON c.id = o.opening_conversion_id \
                         ORDER BY o.opened_at DESC, o.created_at DESC",
                    )?;
                    let rows = stmt
                        .query_map([], |row| {
                            Ok(Header {
                                id: row.get(0)?,
                                box_item: row.get(1)?,
                                box_count: row.get(2)?,
                                box_markup_paid: row.get(3)?,
                                intake: row.get(4)?,
                                conversion_id: row.get(5)?,
                                opened_at: row.get(6)?,
                                undone: row.get(7)?,
                                ammo_ped: row.get(8)?,
                            })
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                let mut lines = conn.prepare(
                    "SELECT side, item_name, quantity, tt_value, acquisition, markup \
                     FROM stock_conversion_lines WHERE conversion_id = ? ORDER BY position",
                )?;
                let mut cubes = conn.prepare(
                    "SELECT input.item_name, output.quantity \
                     FROM strongbox_opening_recycles r \
                     JOIN stock_conversions c ON c.id = r.conversion_id AND c.undone_at IS NULL \
                     JOIN stock_conversion_lines input \
                       ON input.conversion_id = r.conversion_id AND input.side = 'input' \
                     JOIN stock_conversion_lines output \
                       ON output.conversion_id = r.conversion_id AND output.side = 'output' \
                     WHERE r.opening_id = ?",
                )?;
                let mut rows = Vec::with_capacity(headers.len());
                for header in headers {
                    let cubed: Vec<(String, f64)> = cubes
                        .query_map(rusqlite::params![header.id], |row| {
                            Ok((row.get(0)?, row.get(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    let mut key_tt = 0.0;
                    let mut key_markup_paid = 0.0;
                    let mut contents = Vec::new();
                    let read = lines.query_map(rusqlite::params![header.conversion_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<f64>>(5)?,
                        ))
                    })?;
                    for line in read {
                        let (side, item_name, quantity, tt_value, acquisition, markup) = line?;
                        if side == "output" {
                            let nanocubes = (!header.undone)
                                .then(|| {
                                    cubed
                                        .iter()
                                        .find(|(name, _)| *name == item_name)
                                        .map(|(_, cubes)| *cubes)
                                })
                                .flatten();
                            contents.push(OpenedItemRow {
                                item_name,
                                quantity,
                                tt_value,
                                nanocubes,
                            });
                        } else if acquisition.as_deref() == Some("bought")
                            && item_name == crate::inventory_changes::STRONGBOX_KEY
                        {
                            key_tt += tt_value;
                            key_markup_paid += markup.unwrap_or(0.0);
                        }
                    }
                    rows.push(StrongboxOpeningRow {
                        id: header.id,
                        box_item: header.box_item,
                        box_count: header.box_count,
                        box_markup_paid: header.box_markup_paid,
                        key_tt,
                        key_markup_paid,
                        ammo_ped: header.ammo_ped.unwrap_or(0.0),
                        contents,
                        intake: OpeningIntake::from_db(&header.intake),
                        opened_at: header.opened_at,
                        undone: header.undone,
                    });
                }
                Ok(rows)
            })
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::super::conversion::tests::{credited, ledger, list_and_sell, seed_hunted, stock};
    use super::super::tests::write_service;
    use super::*;

    const MAYHEM: i64 = 17;

    fn item(name: &str, quantity: f64, tt_value: f64, recycle: bool) -> OpenedItem {
        OpenedItem {
            item_name: name.to_string(),
            quantity,
            tt_value,
            recycle,
        }
    }

    /// The eight Halloween Strongboxes opened in game: two shirts, a
    /// firework, and the rest back as ammo.
    fn halloween(sources: Vec<BoxSource>, recycle_firework: bool) -> StrongboxOpening {
        StrongboxOpening {
            box_item: "Halloween Strongbox".to_string(),
            sources,
            key_unit_tt: 10.0,
            key_markup_paid: 0.0,
            contents: vec![
                item("Red Heart Fireworks", 1.0, 2.0, recycle_firework),
                item("Skeleton Shirt White (F)", 1.0, 10.0, false),
                item("Skeleton Shirt White (M)", 1.0, 10.0, false),
            ],
            ammo_ped: 58.0,
            intake: OpeningIntake::Export,
            opened_at: Some("2026-11-15".to_string()),
        }
    }

    #[tokio::test]
    async fn an_opening_of_own_boxes_books_nothing_and_carries_their_origin() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 20, 0.0).await;

        service
            .open_strongboxes(halloween(vec![BoxSource::Held { quantity: 8.0 }], false))
            .await
            .unwrap();

        // The keys paid back their TT exactly: no gain, no loss.
        assert!(ledger(&service).await.is_empty());
        let boxes = stock(&service, "Halloween Strongbox").await.unwrap();
        assert!((boxes.quantity - 12.0).abs() < 1e-9);
        let shirt = stock(&service, "Skeleton Shirt White (F)").await.unwrap();
        assert!((shirt.quantity - 1.0).abs() < 1e-9);
        assert!((shirt.tt_value - 10.0).abs() < 1e-9);

        // Selling a shirt realises only its markup, for the hunt that
        // looted the boxes.
        list_and_sell(&service, "Skeleton Shirt White (F)", 25.0, 1.0).await;
        assert!((credited(&service, MAYHEM).await - 14.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn cubed_items_become_nanocubes_that_keep_the_boxes_origin() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 8, 0.0).await;

        service
            .open_strongboxes(halloween(vec![BoxSource::Held { quantity: 8.0 }], true))
            .await
            .unwrap();

        let firework = stock(&service, "Red Heart Fireworks").await;
        assert!(
            firework
                .as_ref()
                .is_none_or(|row| row.quantity.abs() < 1e-9),
            "{firework:?}"
        );
        let cubes = stock(&service, "Nanocube").await.unwrap();
        assert!((cubes.quantity - 200.0).abs() < 1e-6);
        assert!(ledger(&service).await.is_empty());

        let openings = service.strongbox_openings().await.unwrap();
        assert_eq!(openings.len(), 1);
        let firework = &openings[0].contents[0];
        assert_eq!(firework.item_name, "Red Heart Fireworks");
        assert_eq!(firework.nanocubes, Some(200.0));
        assert_eq!(openings[0].contents[1].nanocubes, None);
        assert!((openings[0].key_tt - 80.0).abs() < 1e-9);
        assert!((openings[0].ammo_ped - 58.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn bought_boxes_carry_their_markup_as_cost_recovered_at_sale() {
        let (_dir, service) = write_service().await;

        service
            .open_strongboxes(halloween(
                vec![BoxSource::Bought {
                    quantity: 8.0,
                    markup_paid: 12.0,
                }],
                false,
            ))
            .await
            .unwrap();

        let openings = service.strongbox_openings().await.unwrap();
        assert!((openings[0].box_markup_paid - 12.0).abs() < 1e-9);
        // The markup paid waits as basis on what came out; nothing is
        // booked until a sale.
        assert!(ledger(&service).await.is_empty());
        // The ammo carries none of it: the whole markup rides on the items,
        // which are what the run has to beat it with. By TT share a shirt
        // carries 12 x 10/22 of it, recovered before any gain, and bought
        // boxes have no origin, so the sale credits no activity.
        list_and_sell(&service, "Skeleton Shirt White (F)", 25.0, 1.0).await;
        let entries = ledger(&service).await;
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert_eq!(entries[0].0, "markup");
        assert!(
            (entries[0].2 - (15.0 - 12.0 * 10.0 / 22.0)).abs() < 1e-9,
            "{entries:?}"
        );
        assert_eq!((entries[1].0.as_str(), entries[1].2), ("expense", 1.0));
        assert!((credited(&service, MAYHEM).await).abs() < 1e-9);
    }

    #[tokio::test]
    async fn contents_that_do_not_balance_the_keys_are_refused() {
        let (_dir, service) = write_service().await;
        let mut opening = halloween(
            vec![BoxSource::Bought {
                quantity: 8.0,
                markup_paid: 0.0,
            }],
            false,
        );
        opening.ammo_ped = 50.0;

        let refused = service.open_strongboxes(opening).await.unwrap_err();
        assert!(
            matches!(&refused, AnalyticsError::Rejected(reason) if reason.contains("72.00") && reason.contains("80.00")),
            "{refused:?}"
        );
        assert!(service.strongbox_openings().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_export_truncation_to_whole_pec_is_tolerated() {
        let (_dir, service) = write_service().await;
        let mut opening = halloween(
            vec![BoxSource::Bought {
                quantity: 8.0,
                markup_paid: 0.0,
            }],
            false,
        );
        opening.ammo_ped = 57.98;

        service.open_strongboxes(opening).await.unwrap();
        // The keys paid back their TT whole; the PEC the export dropped is
        // not booked as a loss.
        assert!(ledger(&service).await.is_empty());
        let openings = service.strongbox_openings().await.unwrap();
        assert!((openings[0].ammo_ped - 58.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn malformed_openings_are_refused_before_anything_is_written() {
        let (_dir, service) = write_service().await;
        let half = halloween(vec![BoxSource::Held { quantity: 7.5 }], false);
        assert!(matches!(
            service.open_strongboxes(half).await,
            Err(AnalyticsError::InvalidInput(_))
        ));
        let mut ammo = halloween(vec![BoxSource::Held { quantity: 8.0 }], false);
        ammo.contents
            .push(item("Universal Ammo", 10.0, 0.001, false));
        assert!(matches!(
            service.open_strongboxes(ammo).await,
            Err(AnalyticsError::InvalidInput(_))
        ));
        let mut cube = halloween(vec![BoxSource::Held { quantity: 8.0 }], false);
        cube.contents[0] = item("Nanocube", 200.0, 2.0, true);
        assert!(matches!(
            service.open_strongboxes(cube).await,
            Err(AnalyticsError::InvalidInput(_))
        ));
        assert!(service.strongbox_openings().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn undoing_a_run_restores_the_boxes_and_unmakes_everything_it_made() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 8, 0.0).await;
        let id = service
            .open_strongboxes(halloween(vec![BoxSource::Held { quantity: 8.0 }], true))
            .await
            .unwrap();

        assert!(service.undo_strongbox_opening(&id).await.unwrap());

        let boxes = stock(&service, "Halloween Strongbox").await.unwrap();
        assert!((boxes.quantity - 8.0).abs() < 1e-9);
        assert!(stock(&service, "Nanocube").await.is_none());
        assert!(stock(&service, "Skeleton Shirt White (M)").await.is_none());
        let openings = service.strongbox_openings().await.unwrap();
        assert!(openings[0].undone);
        assert_eq!(openings[0].contents[0].nanocubes, None);
        // A second undo finds nothing standing.
        assert!(!service.undo_strongbox_opening(&id).await.unwrap());
    }

    #[tokio::test]
    async fn a_run_whose_output_has_sold_is_not_undone_and_nothing_moves() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 8, 0.0).await;
        let id = service
            .open_strongboxes(halloween(vec![BoxSource::Held { quantity: 8.0 }], true))
            .await
            .unwrap();
        list_and_sell(&service, "Skeleton Shirt White (M)", 30.0, 1.0).await;

        let refused = service.undo_strongbox_opening(&id).await.unwrap_err();
        assert!(matches!(refused, AnalyticsError::Rejected(_)));
        // The cubes were not quietly unmade on the way to the refusal.
        assert!(stock(&service, "Nanocube").await.is_some());
        assert!(!service.strongbox_openings().await.unwrap()[0].undone);
    }

    #[tokio::test]
    async fn undoing_the_cubes_from_history_leaves_the_item_kept() {
        let (_dir, service) = write_service().await;
        seed_hunted(&service, MAYHEM, "Halloween Strongbox", 8, 0.0).await;
        service
            .open_strongboxes(halloween(vec![BoxSource::Held { quantity: 8.0 }], true))
            .await
            .unwrap();
        let recycle: String = service
            .db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT conversion_id FROM strongbox_opening_recycles",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();

        assert!(service.undo_stock_conversion(&recycle).await.unwrap());

        let openings = service.strongbox_openings().await.unwrap();
        assert!(!openings[0].undone);
        assert_eq!(openings[0].contents[0].nanocubes, None);
        assert!(stock(&service, "Red Heart Fireworks").await.is_some());
    }
}
