//! One account reading: the site's full item list and the PED Card
//! balance at one instant, read strictly into typed holdings.
//!
//! The item list arrives as rows of `{id, name, quantity, value,
//! container, containerRefId}`. A row's `id` is its position in that
//! response, not an identity: it changes between reads. A holding is
//! therefore keyed by what the row is and where it sits (its name and its
//! [`Container`]), and keeps the multiset of its rows, because the rows
//! themselves are evidence (a strongbox's ammo arrives as a row of its own,
//! and the row being fired is the one that shrinks).
//!
//! A reading is exact or refused: a response of the wrong shape, an
//! unknown status, a missing field, a negative or out-of-range quantity or
//! value, a value finer than the whole PEC, an attached row whose parent is
//! not in the list, or totals too large to add up refuses the whole
//! reading. Fields the site adds are tolerated, and a place the app does
//! not know is kept under its own name rather than refused.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::amount::{checked_sum, pec_from_decimal, Pec, ValueRefusal, MAX_QUANTITY};
use super::AccountError;

/// The status a successful site response carries.
const SITE_OK: i64 = 200;

/// Which of the site's responses a refusal is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteResponse {
    Items,
    Balance,
}

impl SiteResponse {
    pub fn describe(self) -> &'static str {
        match self {
            SiteResponse::Items => "item list",
            SiteResponse::Balance => "PED Card balance",
        }
    }
}

/// Why one row of the item list refuses the reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRefusal {
    NegativeQuantity,
    /// Above the largest quantity a row may carry.
    QuantityOutOfRange,
    Value(ValueRefusal),
    BlankName,
    BlankContainer,
    BlankPlanet,
    /// Two rows share one position in the response.
    DuplicatePosition,
    /// A row whose parent reference names no row of the response, or a
    /// row of another name than its container.
    DanglingAttachment,
}

impl RowRefusal {
    pub fn describe(self) -> &'static str {
        match self {
            RowRefusal::NegativeQuantity => "has a negative quantity",
            RowRefusal::QuantityOutOfRange => "has a quantity beyond the largest the app reads",
            RowRefusal::Value(refusal) => refusal.describe(),
            RowRefusal::BlankName => "has no name",
            RowRefusal::BlankContainer => "has no container",
            RowRefusal::BlankPlanet => "is in storage on no named planet",
            RowRefusal::DuplicatePosition => "shares its position with another row",
            RowRefusal::DanglingAttachment => "is attached to a row the list does not hold",
        }
    }
}

/// Where a holding sits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Container {
    /// The avatar's carried inventory.
    Carried,
    /// Listed on the auction.
    Auction,
    /// Storage on one planet, the planet's name trimmed.
    Storage { planet: String },
    /// Attached to (or held inside) another item, named by that item.
    Attached { parent: String },
    /// A place the app does not know, under the site's name for it
    /// (trimmed), so a new kind of place never blinds the reader.
    Other { name: String },
}

/// The closed set of container kinds, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Carried,
    Auction,
    Storage,
    Attached,
    Other,
}

impl ContainerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ContainerKind::Carried => "carried",
            ContainerKind::Auction => "auction",
            ContainerKind::Storage => "storage",
            ContainerKind::Attached => "attached",
            ContainerKind::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<ContainerKind> {
        match value {
            "carried" => Some(ContainerKind::Carried),
            "auction" => Some(ContainerKind::Auction),
            "storage" => Some(ContainerKind::Storage),
            "attached" => Some(ContainerKind::Attached),
            "other" => Some(ContainerKind::Other),
            _ => None,
        }
    }
}

impl Container {
    pub fn kind(&self) -> ContainerKind {
        match self {
            Container::Carried => ContainerKind::Carried,
            Container::Auction => ContainerKind::Auction,
            Container::Storage { .. } => ContainerKind::Storage,
            Container::Attached { .. } => ContainerKind::Attached,
            Container::Other { .. } => ContainerKind::Other,
        }
    }

    /// The planet, parent item, or unknown place the container names;
    /// empty for a place that needs no name.
    pub fn name(&self) -> &str {
        match self {
            Container::Carried | Container::Auction => "",
            Container::Storage { planet } => planet,
            Container::Attached { parent } => parent,
            Container::Other { name } => name,
        }
    }

    /// The container a stored kind and name describe.
    pub fn from_parts(kind: ContainerKind, name: &str) -> Option<Container> {
        match (kind, name) {
            (ContainerKind::Carried, "") => Some(Container::Carried),
            (ContainerKind::Auction, "") => Some(Container::Auction),
            (ContainerKind::Storage, planet) if !planet.trim().is_empty() => {
                Some(Container::Storage {
                    planet: planet.to_string(),
                })
            }
            (ContainerKind::Attached, parent) if !parent.trim().is_empty() => {
                Some(Container::Attached {
                    parent: parent.to_string(),
                })
            }
            (ContainerKind::Other, name) if !name.trim().is_empty() => Some(Container::Other {
                name: name.to_string(),
            }),
            _ => None,
        }
    }

    /// The site's container text for a row with no parent: `CARRIED`,
    /// `AUCTION`, `STORAGE (<planet>)`, or a place the app does not know.
    /// A row with a parent is attached (see `holdings_of`).
    fn from_site(raw: &str) -> Result<Container, RowRefusal> {
        match raw {
            "CARRIED" => Ok(Container::Carried),
            "AUCTION" => Ok(Container::Auction),
            _ if raw.trim().is_empty() => Err(RowRefusal::BlankContainer),
            _ => match raw
                .strip_prefix("STORAGE (")
                .and_then(|rest| rest.strip_suffix(')'))
            {
                Some(planet) if planet.trim().is_empty() => Err(RowRefusal::BlankPlanet),
                Some(planet) => Ok(Container::Storage {
                    planet: planet.trim().to_string(),
                }),
                None => Ok(Container::Other {
                    name: raw.trim().to_string(),
                }),
            },
        }
    }
}

/// What a holding is and where it sits: the identity rows are grouped by.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HoldingKey {
    pub name: String,
    pub container: Container,
}

/// One row of the site's list, as a holding keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Row {
    pub quantity: i64,
    /// TT, truncated by the site to the whole PEC.
    pub value: Pec,
}

/// The rows of one key in one reading, in canonical (ascending) order,
/// with their totals.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Holding {
    rows: Vec<Row>,
    quantity: i64,
    value: Pec,
}

impl Holding {
    /// A holding of these rows; the order they arrive in does not matter.
    /// Rows whose totals cannot be added up refuse.
    pub fn new(mut rows: Vec<Row>) -> Result<Holding, AccountError> {
        rows.sort_unstable();
        let quantity = checked_sum(rows.iter().map(|row| row.quantity))?;
        let value = Pec::total(rows.iter().map(|row| row.value))?;
        Ok(Holding {
            rows,
            quantity,
            value,
        })
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn quantity(&self) -> i64 {
        self.quantity
    }

    /// The sum of the rows' truncated values.
    pub fn value(&self) -> Pec {
        self.value
    }
}

/// One reading of the account, keyed by holding.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    taken_at: f64,
    balance: Pec,
    holdings: BTreeMap<HoldingKey, Holding>,
}

impl Reading {
    /// A reading taken at `taken_at` (unix seconds). Holdings without rows
    /// are dropped: a key is present only while it holds something.
    pub fn new(
        taken_at: f64,
        balance: Pec,
        holdings: BTreeMap<HoldingKey, Holding>,
    ) -> Result<Reading, AccountError> {
        if !taken_at.is_finite() {
            return Err(AccountError::TakenAtNotFinite);
        }
        let holdings = holdings
            .into_iter()
            .filter(|(_, holding)| holding.row_count() > 0)
            .collect();
        Ok(Reading {
            taken_at,
            balance,
            holdings,
        })
    }

    /// When the reading was taken, unix seconds.
    pub fn taken_at(&self) -> f64 {
        self.taken_at
    }

    /// The PED Card balance.
    pub fn balance(&self) -> Pec {
        self.balance
    }

    pub fn holdings(&self) -> &BTreeMap<HoldingKey, Holding> {
        &self.holdings
    }

    pub fn holding(&self, key: &HoldingKey) -> Option<&Holding> {
        self.holdings.get(key)
    }

    /// How many rows the site listed.
    pub fn row_count(&self) -> usize {
        self.holdings.values().map(Holding::row_count).sum()
    }

    /// Read the site's two response bodies, as text.
    pub fn parse(
        taken_at: f64,
        items_body: &str,
        balance_body: &str,
    ) -> Result<Reading, AccountError> {
        let not_json = |response| AccountError::shape(response, "the body (not JSON)".to_string());
        let items =
            serde_json::from_str::<Value>(items_body).map_err(|_| not_json(SiteResponse::Items))?;
        let balance = serde_json::from_str::<Value>(balance_body)
            .map_err(|_| not_json(SiteResponse::Balance))?;
        Reading::from_site(taken_at, &items, &balance)
    }

    /// Read the site's two responses, already parsed as JSON.
    pub fn from_site(
        taken_at: f64,
        items: &Value,
        balance: &Value,
    ) -> Result<Reading, AccountError> {
        if !taken_at.is_finite() {
            return Err(AccountError::TakenAtNotFinite);
        }
        let holdings = holdings_from_site(items)?;
        Reading::new(taken_at, balance_from_site(balance)?, holdings)
    }
}

/// Read the site's item list response, already parsed as JSON, into
/// holdings: one half of a reading, read as strictly as a whole one.
pub fn holdings_from_site(items: &Value) -> Result<BTreeMap<HoldingKey, Holding>, AccountError> {
    let items = typed::<SiteItems>(items, SiteResponse::Items)?;
    holdings_of(items.items)
}

/// Read the site's PED Card balance response, already parsed as JSON: the
/// other half of a reading.
pub fn balance_from_site(balance: &Value) -> Result<Pec, AccountError> {
    let balance = typed::<SiteBalance>(balance, SiteResponse::Balance)?;
    pec_from_decimal(&balance.account_balance).map_err(AccountError::BalanceRefused)
}

/// The response's status, read before its shape, so a refusal from the
/// site reads as the status it carried.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SiteStatus {
    status_code: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SiteItems {
    items: Vec<SiteItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SiteItem {
    id: i64,
    value: serde_json::Number,
    quantity: i64,
    name: String,
    container: String,
    /// Present on every row, null for a row in a place.
    #[serde(deserialize_with = "present")]
    container_ref_id: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SiteBalance {
    account_balance: serde_json::Number,
}

/// A nullable field that must still be present: a custom deserialiser
/// turns off serde's default of reading a missing `Option` as `None`.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    Option::<i64>::deserialize(deserializer)
}

fn typed<T: serde::de::DeserializeOwned>(
    value: &Value,
    response: SiteResponse,
) -> Result<T, AccountError> {
    let refused = || AccountError::shape(response, shape_path(value, response));
    let status = SiteStatus::deserialize(value).map_err(|_| refused())?;
    if status.status_code != SITE_OK {
        return Err(AccountError::SiteStatus {
            response,
            code: status.status_code,
        });
    }
    T::deserialize(value).map_err(|_| refused())
}

/// One expected field of a response: its name, and whether a value fits.
type Field = (&'static str, fn(&Value) -> bool);

fn whole(value: &Value) -> bool {
    value.as_i64().is_some()
}

fn number(value: &Value) -> bool {
    value.is_number()
}

fn text(value: &Value) -> bool {
    value.is_string()
}

fn parent(value: &Value) -> bool {
    value.is_null() || value.as_i64().is_some()
}

/// The first of `fields` that `object` lacks or holds a value of the
/// wrong kind in, as a path.
fn first_misfit(
    object: &serde_json::Map<String, Value>,
    fields: &[Field],
    prefix: &str,
) -> Option<String> {
    fields.iter().find_map(|(name, fits)| {
        (!object.get(*name).is_some_and(fits)).then(|| format!("{prefix}{name}"))
    })
}

/// Where a response first departs from the site's shape, as a field path
/// (`items[3].quantity`). Names fields only: nothing of the response's
/// values.
fn shape_path(value: &Value, response: SiteResponse) -> String {
    let Some(object) = value.as_object() else {
        return "the body (not an object)".to_string();
    };
    if let Some(path) = first_misfit(object, &[("statusCode", whole)], "") {
        return path;
    }
    match response {
        SiteResponse::Balance => first_misfit(object, &[("accountBalance", number)], "")
            .unwrap_or_else(|| "the body".to_string()),
        SiteResponse::Items => {
            let Some(items) = object.get("items").and_then(Value::as_array) else {
                return "items".to_string();
            };
            const ROW: [Field; 6] = [
                ("id", whole),
                ("value", number),
                ("quantity", whole),
                ("name", text),
                ("container", text),
                ("containerRefId", parent),
            ];
            for (index, item) in items.iter().enumerate() {
                let Some(row) = item.as_object() else {
                    return format!("items[{index}]");
                };
                if let Some(path) = first_misfit(row, &ROW, &format!("items[{index}].")) {
                    return path;
                }
            }
            "the body".to_string()
        }
    }
}

fn holdings_of(items: Vec<SiteItem>) -> Result<BTreeMap<HoldingKey, Holding>, AccountError> {
    let refuse = |row: i64, reason: RowRefusal| AccountError::RowRefused { row, reason };
    let mut names_by_position: HashMap<i64, &str> = HashMap::with_capacity(items.len());
    for item in &items {
        if names_by_position.insert(item.id, &item.name).is_some() {
            return Err(refuse(item.id, RowRefusal::DuplicatePosition));
        }
    }
    let mut rows: BTreeMap<HoldingKey, Vec<Row>> = BTreeMap::new();
    for item in &items {
        if item.name.trim().is_empty() {
            return Err(refuse(item.id, RowRefusal::BlankName));
        }
        if item.quantity < 0 {
            return Err(refuse(item.id, RowRefusal::NegativeQuantity));
        }
        if item.quantity > MAX_QUANTITY {
            return Err(refuse(item.id, RowRefusal::QuantityOutOfRange));
        }
        let value = pec_from_decimal(&item.value)
            .map_err(|reason| refuse(item.id, RowRefusal::Value(reason)))?;
        // A row that names a parent row is attached to it, and its
        // container is that parent's name; any other row sits in a place.
        let container = match item.container_ref_id {
            Some(parent_position) => {
                if item.container.trim().is_empty() {
                    return Err(refuse(item.id, RowRefusal::BlankContainer));
                }
                if names_by_position.get(&parent_position) != Some(&item.container.as_str()) {
                    return Err(refuse(item.id, RowRefusal::DanglingAttachment));
                }
                Container::Attached {
                    parent: item.container.clone(),
                }
            }
            None => {
                Container::from_site(&item.container).map_err(|reason| refuse(item.id, reason))?
            }
        };
        rows.entry(HoldingKey {
            name: item.name.clone(),
            container,
        })
        .or_default()
        .push(Row {
            quantity: item.quantity,
            value,
        });
    }
    rows.into_iter()
        .map(|(key, rows)| Ok((key, Holding::new(rows)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn items(rows: Value) -> Value {
        json!({"items": rows, "statusCode": 200, "ecoServerMessage": null})
    }

    fn balance(figure: Value) -> Value {
        json!({"accountBalance": figure, "statusCode": 200, "elevationResult": null})
    }

    fn row(id: i64, name: &str, quantity: Value, value: Value, container: &str) -> Value {
        json!({"id": id, "value": value, "quantity": quantity, "name": name,
               "container": container, "containerRefId": null})
    }

    fn sample() -> Value {
        items(json!([
            {"id": 1, "value": 25.0, "quantity": 1, "name": "Armor Plating Mk. 6A",
             "container": "Paneleon Spec Imperium Arm Guards (M)", "containerRefId": 3},
            row(2, "Universal Ammo", json!(9965587), json!(996.55), "CARRIED"),
            row(3, "Paneleon Spec Imperium Arm Guards (M)", json!(1), json!(21.5), "CARRIED"),
            row(4, "Common Dung", json!(29), json!(0), "STORAGE (The Hub\n)"),
            row(5, "Universal Ammo", json!(100000), json!(10.0), "CARRIED"),
            row(6, "Gorgon Gloves (M)", json!(1), json!(1.4), "AUCTION"),
        ]))
    }

    fn read(items: &Value) -> Result<Reading, AccountError> {
        Reading::from_site(1_794_754_516.0, items, &balance(json!(250.0)))
    }

    fn key(name: &str, container: Container) -> HoldingKey {
        HoldingKey {
            name: name.to_string(),
            container,
        }
    }

    #[test]
    fn rows_group_by_name_and_container_with_their_rows_kept() {
        let reading = read(&sample()).expect("a whole reading");
        assert_eq!(reading.balance(), Pec(25_000));
        assert_eq!(reading.row_count(), 6);
        assert_eq!(reading.holdings().len(), 5);
        let ammo = reading
            .holding(&key("Universal Ammo", Container::Carried))
            .expect("ammo");
        assert_eq!(
            ammo.rows(),
            &[
                Row {
                    quantity: 100_000,
                    value: Pec(1_000)
                },
                Row {
                    quantity: 9_965_587,
                    value: Pec(99_655)
                },
            ]
        );
        assert_eq!(ammo.quantity(), 10_065_587);
        assert_eq!(ammo.value(), Pec(100_655));
        let plate = key(
            "Armor Plating Mk. 6A",
            Container::Attached {
                parent: "Paneleon Spec Imperium Arm Guards (M)".to_string(),
            },
        );
        assert_eq!(
            reading.holding(&plate).map(Holding::value),
            Some(Pec(2_500))
        );
        assert!(reading
            .holding(&key("Gorgon Gloves (M)", Container::Auction))
            .is_some());
    }

    #[test]
    fn a_storage_planet_is_trimmed_of_the_site_s_stray_newline() {
        let reading = read(&sample()).expect("a whole reading");
        let dung = key(
            "Common Dung",
            Container::Storage {
                planet: "The Hub".to_string(),
            },
        );
        assert_eq!(reading.holding(&dung).map(Holding::quantity), Some(29));
    }

    #[test]
    fn the_order_rows_arrive_in_does_not_change_the_reading() {
        let mut reversed = sample();
        reversed["items"].as_array_mut().expect("rows").reverse();
        assert_eq!(read(&reversed).unwrap(), read(&sample()).unwrap());
    }

    #[test]
    fn fields_the_site_adds_are_tolerated() {
        let mut extra = sample();
        extra["pageSize"] = json!(50);
        extra["items"][1]["colour"] = json!("blue");
        assert!(read(&extra).is_ok());
    }

    #[test]
    fn an_unknown_status_refuses_with_that_status() {
        let mut refused = sample();
        refused["statusCode"] = json!(401);
        assert!(matches!(
            read(&refused),
            Err(AccountError::SiteStatus {
                response: SiteResponse::Items,
                code: 401
            })
        ));
        let balance = json!({"accountBalance": null, "statusCode": 500});
        assert!(matches!(
            Reading::from_site(1.0, &sample(), &balance),
            Err(AccountError::SiteStatus {
                response: SiteResponse::Balance,
                code: 500
            })
        ));
    }

    #[test]
    fn a_missing_field_refuses_the_reading() {
        for field in [
            "id",
            "value",
            "quantity",
            "name",
            "container",
            "containerRefId",
        ] {
            let mut missing = sample();
            missing["items"][1]
                .as_object_mut()
                .expect("row")
                .remove(field);
            assert!(
                matches!(read(&missing), Err(AccountError::SiteShape { .. })),
                "a row without {field} refuses"
            );
        }
        let mut no_items = sample();
        no_items.as_object_mut().unwrap().remove("items");
        assert!(matches!(
            read(&no_items),
            Err(AccountError::SiteShape { .. })
        ));
        let mut no_status = sample();
        no_status.as_object_mut().unwrap().remove("statusCode");
        assert!(matches!(
            read(&no_status),
            Err(AccountError::SiteShape { .. })
        ));
        assert!(matches!(
            Reading::from_site(1.0, &sample(), &json!({"statusCode": 200})),
            Err(AccountError::SiteShape {
                response: SiteResponse::Balance,
                ..
            })
        ));
    }

    #[test]
    fn a_negative_or_fractional_quantity_refuses_the_reading() {
        let mut negative = sample();
        negative["items"][1]["quantity"] = json!(-1);
        assert!(matches!(
            read(&negative),
            Err(AccountError::RowRefused {
                row: 2,
                reason: RowRefusal::NegativeQuantity
            })
        ));
        let mut fractional = sample();
        fractional["items"][1]["quantity"] = json!(1.5);
        assert!(matches!(
            read(&fractional),
            Err(AccountError::SiteShape { .. })
        ));
    }

    #[test]
    fn a_negative_or_sub_pec_value_refuses_the_reading() {
        let mut negative = sample();
        negative["items"][2]["value"] = json!(-21.5);
        assert!(matches!(
            read(&negative),
            Err(AccountError::RowRefused {
                row: 3,
                reason: RowRefusal::Value(ValueRefusal::Negative)
            })
        ));
        let mut fine = sample();
        fine["items"][2]["value"] = json!(21.505);
        assert!(matches!(
            read(&fine),
            Err(AccountError::RowRefused {
                row: 3,
                reason: RowRefusal::Value(ValueRefusal::NotWholePec)
            })
        ));
        assert!(matches!(
            Reading::from_site(1.0, &sample(), &balance(json!(-1.0))),
            Err(AccountError::BalanceRefused(ValueRefusal::Negative))
        ));
        assert!(matches!(
            Reading::from_site(1.0, &sample(), &balance(json!("250.00"))),
            Err(AccountError::SiteShape { .. })
        ));
    }

    #[test]
    fn a_non_finite_reading_time_refuses() {
        assert!(matches!(
            Reading::from_site(f64::NAN, &sample(), &balance(json!(1.0))),
            Err(AccountError::TakenAtNotFinite)
        ));
        assert!(matches!(
            Reading::new(f64::INFINITY, Pec(0), BTreeMap::new()),
            Err(AccountError::TakenAtNotFinite)
        ));
    }

    #[test]
    fn an_attachment_must_name_a_row_of_its_parent_s_name() {
        let mut dangling = sample();
        dangling["items"][0]["containerRefId"] = json!(99);
        assert!(matches!(
            read(&dangling),
            Err(AccountError::RowRefused {
                row: 1,
                reason: RowRefusal::DanglingAttachment
            })
        ));
        let mut wrong_parent = sample();
        wrong_parent["items"][0]["containerRefId"] = json!(2);
        assert!(matches!(
            read(&wrong_parent),
            Err(AccountError::RowRefused {
                row: 1,
                reason: RowRefusal::DanglingAttachment
            })
        ));
        let mut placed_with_parent = sample();
        placed_with_parent["items"][1]["containerRefId"] = json!(3);
        assert!(matches!(
            read(&placed_with_parent),
            Err(AccountError::RowRefused {
                row: 2,
                reason: RowRefusal::DanglingAttachment
            })
        ));
    }

    #[test]
    fn blank_names_containers_and_planets_and_shared_positions_refuse() {
        let cases: [(usize, &str, Value, RowRefusal); 4] = [
            (1, "name", json!("  "), RowRefusal::BlankName),
            (1, "container", json!(""), RowRefusal::BlankContainer),
            (
                1,
                "container",
                json!("STORAGE ( \n)"),
                RowRefusal::BlankPlanet,
            ),
            (1, "id", json!(1), RowRefusal::DuplicatePosition),
        ];
        for (index, field, replacement, expected) in cases {
            let mut refused = sample();
            refused["items"][index][field] = replacement;
            match read(&refused) {
                Err(AccountError::RowRefused { reason, .. }) => assert_eq!(reason, expected),
                other => panic!("{field}: expected {expected:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_refusal_names_where_and_quotes_nothing_the_site_sent() {
        let mut hostile = sample();
        hostile["items"][3]["quantity"] = json!("SECRET-VALUE-quantity");
        let error = read(&hostile).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("items[3].quantity"), "{text}");
        assert!(!text.contains("SECRET"), "{text}");
        let mut hostile = sample();
        hostile["items"][1]
            .as_object_mut()
            .unwrap()
            .remove("containerRefId");
        hostile["items"][1]["name"] = json!("SECRET name");
        let text = read(&hostile).unwrap_err().to_string();
        assert!(text.contains("items[1].containerRefId"), "{text}");
        assert!(!text.contains("SECRET"), "{text}");
        let text = Reading::from_site(
            1.0,
            &sample(),
            &json!({"statusCode": 200, "accountBalance": "SECRET"}),
        )
        .unwrap_err()
        .to_string();
        assert!(
            text.contains("accountBalance") && !text.contains("SECRET"),
            "{text}"
        );
        let text = Reading::parse(1.0, "<html>SECRET</html>", "{}")
            .unwrap_err()
            .to_string();
        assert!(!text.contains("SECRET"), "{text}");
    }

    #[test]
    fn text_bodies_parse_like_json_values() {
        let reading = Reading::parse(
            5.0,
            &sample().to_string(),
            r#"{"accountBalance":249.52,"statusCode":200}"#,
        )
        .expect("a whole reading");
        assert_eq!(reading.balance(), Pec(24_952));
        assert!(matches!(
            Reading::parse(5.0, "<html>", "{}"),
            Err(AccountError::SiteShape {
                response: SiteResponse::Items,
                ..
            })
        ));
    }

    #[test]
    fn a_place_the_app_does_not_know_is_kept_under_its_own_name() {
        let mut unknown = sample();
        // The plate loses its parent reference: the text alone no longer
        // makes it attached, so it sits in an unknown place.
        unknown["items"][0]["containerRefId"] = json!(null);
        unknown["items"][1]["container"] = json!(" ESTATE (Calypso) ");
        let reading = read(&unknown).expect("an unknown place does not refuse");
        let estate = key(
            "Universal Ammo",
            Container::Other {
                name: "ESTATE (Calypso)".to_string(),
            },
        );
        assert_eq!(
            reading.holding(&estate).map(Holding::quantity),
            Some(9_965_587)
        );
        let plate = key(
            "Armor Plating Mk. 6A",
            Container::Other {
                name: "Paneleon Spec Imperium Arm Guards (M)".to_string(),
            },
        );
        assert!(reading.holding(&plate).is_some());
    }

    #[test]
    fn quantities_and_values_beyond_the_range_refuse_and_totals_that_overflow_refuse() {
        let mut huge = sample();
        huge["items"][1]["quantity"] = json!(1_000_000_000_001i64);
        assert!(matches!(
            read(&huge),
            Err(AccountError::RowRefused {
                row: 2,
                reason: RowRefusal::QuantityOutOfRange
            })
        ));
        let mut largest = sample();
        largest["items"][1]["quantity"] = json!(1_000_000_000_000i64);
        assert!(read(&largest).is_ok());
        let mut costly = sample();
        costly["items"][2]["value"] = json!(10_000_000_000.01);
        assert!(matches!(
            read(&costly),
            Err(AccountError::RowRefused {
                row: 3,
                reason: RowRefusal::Value(ValueRefusal::OutOfRange)
            })
        ));
        assert!(matches!(
            Holding::new(vec![
                Row {
                    quantity: i64::MAX,
                    value: Pec(0)
                },
                Row {
                    quantity: 1,
                    value: Pec(0)
                },
            ]),
            Err(AccountError::Overflow)
        ));
    }

    #[test]
    fn stored_container_parts_round_trip_and_blank_names_are_refused() {
        for container in [
            Container::Carried,
            Container::Auction,
            Container::Storage {
                planet: "Calypso".into(),
            },
            Container::Attached {
                parent: "Pegasus Helmet (M,L)".into(),
            },
        ] {
            let kind = ContainerKind::parse(container.kind().as_str()).unwrap();
            assert_eq!(
                Container::from_parts(kind, container.name()),
                Some(container)
            );
        }
        assert_eq!(Container::from_parts(ContainerKind::Storage, " "), None);
        assert_eq!(Container::from_parts(ContainerKind::Carried, "x"), None);
        assert_eq!(ContainerKind::parse("vault"), None);
    }
}
