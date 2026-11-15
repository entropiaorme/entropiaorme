//! Readings built in tests, and the recorded hunt.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use super::amount::Pec;
use super::reading::{Container, Holding, HoldingKey, Reading, Row};

/// Fifteen readings a minute apart across one hunt, the site's responses
/// verbatim in shape.
const RECORDED_HUNT: &str = include_str!("../../../fixtures/account/recorded_hunt.json");

/// A reading at `at` with `balance` PEC on the Card, from rows of
/// `(name, container, quantity, value in PEC)`.
pub(crate) fn reading(at: f64, balance: i64, rows: &[(&str, Container, i64, i64)]) -> Reading {
    let mut holdings: BTreeMap<HoldingKey, Vec<Row>> = BTreeMap::new();
    for (name, container, quantity, value) in rows {
        holdings
            .entry(HoldingKey {
                name: (*name).to_string(),
                container: container.clone(),
            })
            .or_default()
            .push(Row {
                quantity: *quantity,
                value: Pec(*value),
            });
    }
    Reading::new(
        at,
        Pec(balance),
        holdings
            .into_iter()
            .map(|(key, rows)| (key, Holding::new(rows).expect("totals that add up")))
            .collect(),
    )
    .expect("a finite reading time")
}

/// A reading's contents, for property tests to generate.
#[derive(Debug, Clone)]
pub(crate) struct ReadingSpec {
    pub balance: i64,
    pub rows: Vec<(&'static str, Container, i64, i64)>,
}

impl ReadingSpec {
    pub fn reading(&self, at: f64) -> Reading {
        reading(at, self.balance, &self.rows)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordedReading {
    taken_at: f64,
    items: Value,
    balance: Value,
}

/// The recorded hunt's readings, in the order they were taken.
pub(crate) fn recorded_hunt() -> Vec<Reading> {
    let recorded: Vec<RecordedReading> =
        serde_json::from_str(RECORDED_HUNT).expect("the recorded hunt parses");
    recorded
        .iter()
        .map(|entry| {
            Reading::from_site(entry.taken_at, &entry.items, &entry.balance)
                .expect("every recorded reading is whole")
        })
        .collect()
}
