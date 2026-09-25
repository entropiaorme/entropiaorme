//! Startup readiness: the one record of whether the native backend has come
//! up, and the command every window awaits it through.
//!
//! The shell composes the native services off the setup path, so the webview
//! can be running (and issuing typed commands) before the facade exists. This
//! module gives that interval an explicit, level-triggered boundary. The
//! record starts unsettled and is settled exactly once, after composition
//! either installs the complete facade or declines. [`substrate_ready`]
//! answers the settled outcome, waiting for it if necessary, so a caller that
//! asks after the transition sees it just as surely as one that asked before:
//! there is no one-shot signal to miss.
//!
//! The frontend's typed transport holds every facade command behind this
//! answer, so nothing reaches the facade while it is still composing. The
//! facade's own `unavailable` error stays in place for any caller that
//! bypasses the gate.

use serde::Serialize;
use tokio::sync::watch;

/// Why composition declined, as a closed set the frontend turns into one
/// plain sentence and a recovery hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclineReason {
    /// The data directory could not be created.
    DataDirUnavailable,
    /// The database predates the oldest schema this version can upgrade.
    DatabaseBelowBaseline,
    /// The database exists but could not be opened or adopted.
    DatabaseUnreadable,
    /// The bundled game data is missing or empty.
    GameDataUnavailable,
    /// The live-tracking services failed to start.
    TrackingUnavailable,
    /// Composition stopped without reporting an outcome (a panic).
    Unexpected,
}

/// A terminal decline: the closed reason plus the logged detail, carried so
/// the failure surface can offer it for a bug report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decline {
    pub reason: DeclineReason,
    pub detail: String,
}

impl Decline {
    pub fn new(reason: DeclineReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

/// The settled startup outcome, as the webview receives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SubstrateOutcome {
    /// The facade and every lifecycle-owned service are installed.
    Ready,
    /// Composition declined; the backend is unavailable for this session.
    Failed {
        reason: DeclineReason,
        detail: String,
    },
}

impl SubstrateOutcome {
    /// The outcome as a given window may see it. The logged detail can carry
    /// local paths (and with them the account name), and only the main
    /// window, which hosts the failure surface and its "Copy details", needs
    /// it; every other window receives the state and the closed reason alone.
    pub fn for_window(self, label: &str) -> Self {
        match self {
            Self::Failed { reason, .. } if label != MAIN_WINDOW => Self::Failed {
                reason,
                detail: String::new(),
            },
            outcome => outcome,
        }
    }
}

/// The label of the window that hosts the startup failure surface.
const MAIN_WINDOW: &str = "main";

impl From<Decline> for SubstrateOutcome {
    fn from(decline: Decline) -> Self {
        Self::Failed {
            reason: decline.reason,
            detail: decline.detail,
        }
    }
}

/// The managed readiness record. Registered on the builder, so it exists
/// before any window can issue a command.
pub struct SubstrateReadiness(watch::Sender<Option<SubstrateOutcome>>);

impl Default for SubstrateReadiness {
    fn default() -> Self {
        Self(watch::Sender::new(None))
    }
}

impl SubstrateReadiness {
    /// Record the outcome. Called once, after the facade is published (so a
    /// `Ready` answer guarantees every typed command can dispatch) or after a
    /// decline. `send_replace` stores the value whether or not anyone is
    /// waiting yet, which is what makes a late caller see it.
    pub fn settle(&self, outcome: SubstrateOutcome) {
        self.0.send_replace(Some(outcome));
    }

    /// The settled outcome, waiting for it if composition is still running.
    pub async fn settled(&self) -> SubstrateOutcome {
        let mut receiver = self.0.subscribe();
        // `wait_for` errors only once the sender is dropped, and `self` owns
        // the sender for as long as this borrow lives, so the error arm is
        // unreachable; it maps to a decline rather than a panic regardless.
        let settled = match receiver.wait_for(Option::is_some).await {
            Ok(settled) => settled.clone(),
            Err(_) => None,
        };
        settled.unwrap_or_else(interrupted)
    }
}

fn interrupted() -> SubstrateOutcome {
    Decline::new(
        DeclineReason::Unexpected,
        "startup readiness closed before it settled",
    )
    .into()
}

/// Answer the settled startup outcome, waiting while the backend composes.
/// Every window's typed transport awaits this once before its first facade
/// command; see [`SubstrateOutcome::for_window`] for what each window sees.
#[tauri::command]
pub async fn substrate_ready(app: tauri::AppHandle, window: tauri::Window) -> SubstrateOutcome {
    use tauri::Manager as _;
    app.state::<SubstrateReadiness>()
        .settled()
        .await
        .for_window(window.label())
}

/// Relaunch the app: the recovery action the startup failure surface offers.
/// Goes through the event loop, so the normal exit teardown runs first.
#[tauri::command]
pub fn restart_app(app: tauri::AppHandle) {
    app.request_restart();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn a_waiter_that_arrives_before_the_outcome_receives_it() {
        let readiness = Arc::new(SubstrateReadiness::default());
        let waiter = {
            let readiness = readiness.clone();
            tokio::spawn(async move { readiness.settled().await })
        };
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "nothing settles while composing");
        readiness.settle(SubstrateOutcome::Ready);
        let outcome = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("the waiter wakes on settle")
            .expect("the waiter task completes");
        assert_eq!(outcome, SubstrateOutcome::Ready);
    }

    #[tokio::test]
    async fn a_waiter_that_arrives_after_the_outcome_still_receives_it() {
        let readiness = SubstrateReadiness::default();
        readiness.settle(SubstrateOutcome::Ready);
        let outcome = tokio::time::timeout(Duration::from_secs(1), readiness.settled())
            .await
            .expect("an already settled record answers at once");
        assert_eq!(outcome, SubstrateOutcome::Ready);
    }

    #[tokio::test]
    async fn a_decline_reaches_every_waiter_with_its_reason() {
        let readiness = SubstrateReadiness::default();
        readiness.settle(Decline::new(DeclineReason::DatabaseBelowBaseline, "schema v28").into());
        for _ in 0..2 {
            assert_eq!(
                readiness.settled().await,
                SubstrateOutcome::Failed {
                    reason: DeclineReason::DatabaseBelowBaseline,
                    detail: "schema v28".into(),
                }
            );
        }
    }

    #[test]
    fn only_the_main_window_receives_the_logged_detail() {
        let failed: SubstrateOutcome =
            Decline::new(DeclineReason::DatabaseUnreadable, "/home/someone/data").into();
        assert_eq!(failed.clone().for_window("main"), failed);
        for overlay in ["overlay", "cartography-overlay", "overlay-sale-capture"] {
            assert_eq!(
                failed.clone().for_window(overlay),
                SubstrateOutcome::Failed {
                    reason: DeclineReason::DatabaseUnreadable,
                    detail: String::new(),
                }
            );
        }
        assert_eq!(
            SubstrateOutcome::Ready.for_window("overlay"),
            SubstrateOutcome::Ready
        );
    }

    #[test]
    fn the_outcome_serialises_to_the_shape_the_webview_reads() {
        assert_eq!(
            serde_json::to_value(SubstrateOutcome::Ready).unwrap(),
            serde_json::json!({ "state": "ready" })
        );
        assert_eq!(
            serde_json::to_value(SubstrateOutcome::from(Decline::new(
                DeclineReason::GameDataUnavailable,
                "snapshot empty"
            )))
            .unwrap(),
            serde_json::json!({
                "state": "failed",
                "reason": "game_data_unavailable",
                "detail": "snapshot empty"
            })
        );
    }
}
