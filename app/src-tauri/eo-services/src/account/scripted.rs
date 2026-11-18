//! A scripted stand-in for the account site: replays recorded responses
//! through the same strict parser the real reader uses, accepts one
//! session, and can be told to fail at a given call.
//!
//! A full reading is a balance read and then an item read: each balance
//! read answers the balance of the reading in hand, and each item read
//! answers its items and moves on to the next reading. Past the last
//! reading the last one is served again, as a site at rest would. A
//! balance read on its own (the check made on connecting) answers without
//! moving on.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{json, Value};
use tokio::sync::watch;

use super::amount::Pec;
use super::reading::{balance_from_site, holdings_from_site, Holding, HoldingKey, SiteResponse};
use super::site::{parse_body, AccountSite, SessionSecret, SiteFailure, SiteFuture, SiteOutcome};

/// A failure the stand-in can be told to answer with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptedFault {
    /// The site refuses the session from this call on: it has expired.
    Unauthorised,
    /// The network failed for this one call.
    Network,
    /// This one answer is not the site's shape.
    Malformed,
}

/// Which read a call was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptedRead {
    Balance,
    Items,
}

struct Script {
    readings: Vec<(Value, Value)>,
    next: usize,
    accepted: String,
    expired: bool,
    calls: Vec<ScriptedRead>,
    faults: BTreeMap<usize, ScriptedFault>,
}

/// The stand-in site.
pub struct ScriptedSite {
    script: Mutex<Script>,
    /// While true, every call waits before answering.
    held: watch::Sender<bool>,
}

impl ScriptedSite {
    /// A site that serves `readings` (each the item list and the balance
    /// response, as the site sends them) to the session `accepted`.
    pub fn new(accepted: &str, readings: Vec<(Value, Value)>) -> ScriptedSite {
        ScriptedSite {
            script: Mutex::new(Script {
                readings,
                next: 0,
                accepted: accepted.to_string(),
                expired: false,
                calls: Vec::new(),
                faults: BTreeMap::new(),
            }),
            held: watch::channel(false).0,
        }
    }

    /// Answer the call numbered `call` (counting from zero across both
    /// reads) with `fault`.
    pub fn fail_at(&self, call: usize, fault: ScriptedFault) {
        self.lock().faults.insert(call, fault);
    }

    /// Accept `session` from now on, and no other: the player signed in
    /// again, so an expiry is over.
    pub fn accept(&self, session: &str) {
        let mut script = self.lock();
        script.accepted = session.to_string();
        script.expired = false;
    }

    /// Hold every call until [`ScriptedSite::release`].
    pub fn hold(&self) {
        self.held.send_replace(true);
    }

    pub fn release(&self) {
        self.held.send_replace(false);
    }

    /// Whether calls are being held.
    pub fn is_held(&self) -> bool {
        *self.held.borrow()
    }

    /// The calls made so far, in order.
    pub fn calls(&self) -> Vec<ScriptedRead> {
        self.lock().calls.clone()
    }

    /// How many readings the item reads have moved past.
    pub fn readings_served(&self) -> usize {
        self.lock().next
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Script> {
        self.script
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn wait_released(&self) {
        let mut held = self.held.subscribe();
        let _ = held.wait_for(|held| !*held).await;
    }

    /// Answer one call: the body the site would send, or a refusal.
    fn answer(&self, read: ScriptedRead, session: &SessionSecret) -> SiteOutcome<Value> {
        let mut script = self.lock();
        let call = script.calls.len();
        script.calls.push(read);
        match script.faults.get(&call) {
            Some(ScriptedFault::Unauthorised) => script.expired = true,
            Some(ScriptedFault::Network) => return SiteOutcome::Failed(SiteFailure::Network),
            Some(ScriptedFault::Malformed) => {
                return SiteOutcome::Read(json!({"statusCode": 200}));
            }
            None => {}
        }
        if script.expired || session.expose() != script.accepted {
            return SiteOutcome::Unauthorised;
        }
        let Some(last) = script.readings.len().checked_sub(1) else {
            return SiteOutcome::Failed(SiteFailure::Network);
        };
        let index = script.next.min(last);
        let (items, balance) = &script.readings[index];
        match read {
            ScriptedRead::Balance => SiteOutcome::Read(balance.clone()),
            ScriptedRead::Items => {
                let items = items.clone();
                script.next += 1;
                SiteOutcome::Read(items)
            }
        }
    }
}

/// A body through the strict parser, as the real reader reads one.
fn parsed<T>(
    answer: SiteOutcome<Value>,
    response: SiteResponse,
    parse: impl FnOnce(&Value) -> Result<T, super::AccountError>,
) -> SiteOutcome<T> {
    match answer {
        SiteOutcome::Read(body) => {
            let bytes = serde_json::to_vec(&body).unwrap_or_default();
            parse_body(&bytes, response, parse)
        }
        SiteOutcome::Unauthorised => SiteOutcome::Unauthorised,
        SiteOutcome::Failed(failure) => SiteOutcome::Failed(failure),
    }
}

impl AccountSite for ScriptedSite {
    fn read_balance<'a>(&'a self, session: &'a SessionSecret) -> SiteFuture<'a, Pec> {
        Box::pin(async move {
            self.wait_released().await;
            let answer = self.answer(ScriptedRead::Balance, session);
            parsed(answer, SiteResponse::Balance, balance_from_site)
        })
    }

    fn read_items<'a>(
        &'a self,
        session: &'a SessionSecret,
    ) -> SiteFuture<'a, BTreeMap<HoldingKey, Holding>> {
        Box::pin(async move {
            self.wait_released().await;
            let answer = self.answer(ScriptedRead::Items, session);
            parsed(answer, SiteResponse::Items, holdings_from_site)
        })
    }
}

/// The recorded hunt's responses, in the order they were taken: the
/// stand-in's usual script.
pub fn recorded_hunt_responses() -> Vec<(Value, Value)> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Recorded {
        items: Value,
        balance: Value,
    }
    let recorded: Vec<Recorded> =
        serde_json::from_str(include_str!("../../../fixtures/account/recorded_hunt.json"))
            .expect("the recorded hunt parses");
    recorded
        .into_iter()
        .map(|entry| (entry.items, entry.balance))
        .collect()
}

/// One row of a built reading: name, container text, quantity, and value
/// in PEC.
pub type ScriptedRow<'a> = (&'a str, &'a str, i64, i64);

/// A built reading: the PED Card balance in PEC, and its rows.
pub type ScriptedReading<'a> = (i64, &'a [ScriptedRow<'a>]);

/// Responses for built readings, for tests that need a few small readings
/// rather than the recorded hunt.
pub fn responses(readings: &[ScriptedReading<'_>]) -> Vec<(Value, Value)> {
    let decimal = |pec: i64| {
        serde_json::Number::from_f64(pec as f64 / 100.0).map_or(Value::Null, Value::Number)
    };
    readings
        .iter()
        .map(|(balance, rows)| {
            let items: Vec<Value> = rows
                .iter()
                .enumerate()
                .map(|(index, (name, container, quantity, value))| {
                    json!({
                        "id": index + 1,
                        "value": decimal(*value),
                        "quantity": quantity,
                        "name": name,
                        "container": container,
                        "containerRefId": null,
                    })
                })
                .collect();
            (
                json!({"items": items, "statusCode": 200, "ecoServerMessage": null}),
                json!({"accountBalance": decimal(*balance), "statusCode": 200}),
            )
        })
        .collect()
}

/// A wall clock that moves with tokio's clock, so a test on paused time
/// sees readings timed exactly when the connection's minute falls due.
pub struct PacedClock {
    base: chrono::NaiveDateTime,
    origin: tokio::time::Instant,
}

impl PacedClock {
    /// A clock reading `base` now, and moving on with tokio time.
    pub fn new(base: chrono::NaiveDateTime) -> PacedClock {
        PacedClock {
            base,
            origin: tokio::time::Instant::now(),
        }
    }

    /// The tokio instant `seconds` after this clock started.
    pub fn instant_at(&self, seconds: f64) -> tokio::time::Instant {
        self.origin + std::time::Duration::from_secs_f64(seconds)
    }

    /// The unix time this clock reads `seconds` after it started.
    pub fn epoch_at(&self, seconds: f64) -> f64 {
        crate::time::naive_to_epoch(self.base) + seconds
    }

    /// Seconds since this clock started, of a unix time it read.
    pub fn since_start(&self, epoch: f64) -> f64 {
        ((epoch - crate::time::naive_to_epoch(self.base)) * 1000.0).round() / 1000.0
    }
}

impl crate::clock::Clock for PacedClock {
    fn now(&self) -> chrono::NaiveDateTime {
        let elapsed =
            chrono::TimeDelta::from_std(self.origin.elapsed()).unwrap_or(chrono::TimeDelta::zero());
        self.base + elapsed
    }

    fn monotonic(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }
}
