//! A stock reconciliation over the typed facade, from an inventory pasted
//! from the account site with no connection: planned with its defaults,
//! revised and validated, applied once (and a second apply changing
//! nothing), undone as a whole; refused while a session is tracked; and a
//! paste with a row that does not read refused whole.

use eo_api::account_reconciliation::{
    StockReconciliationApplied, StockReconciliationApplyInput, StockReconciliationChoice,
    StockReconciliationDisposition, StockReconciliationExclusion, StockReconciliationPasteInput,
    StockReconciliationReviseInput, StockReconciliationRevision,
};
use eo_api::analytics::{ActivityUndoInput, Profession};
use eo_api::ApiError;

use crate::account_facade::{harness_with, Harness, Seed};

/// Twelve Iron Stone and 200 PED of Shrapnel held from one hunt.
fn held_stock() -> Seed {
    Box::new(|conn, _| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES ('stocked', 1000.0, 2000.0, 0)",
            [],
        )
        .expect("a session");
        conn.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
             shots_fired, damage_dealt, cost_ped, loot_total_ped, is_global, is_hof) \
             VALUES ('k1', 'stocked', 'Atrox', 'Atrox', '', 1500.0, 1, 1.0, 0.1, 201.56, 0, 0)",
            [],
        )
        .expect("a kill");
        conn.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
             VALUES ('k1', 'Iron Stone', 12, 1.56), ('k1', 'Shrapnel', 2000000, 200.0)",
            [],
        )
        .expect("its loot");
    })
}

const PASTE: &str = "Name\tQuantity\tValue(PED)\tContainer\tContainerRefId\n\
    Iron Stone\t5\t0.6500\tCARRIED\t\n\
    Shrapnel\t1500000\t150.0000\tSTORAGE (The Hub\n)\t\n\
    Nanocube\t500\t5.0000\tCARRIED\t\n\
    Universal Ammo\t300000\t30.0000\tCARRIED\t\n";

async fn held(harness: &Harness, item: &str) -> f64 {
    harness
        .api
        .activity_stock(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.item_name == item)
        .map_or(0.0, |row| row.quantity)
}

async fn paste(
    harness: &Harness,
    text: &str,
) -> Result<eo_api::account_reconciliation::StockReconciliationDraft, ApiError> {
    harness
        .api
        .account_reconciliation_paste(StockReconciliationPasteInput {
            text: text.to_string(),
        })
        .await
}

#[tokio::test(start_paused = true)]
async fn a_pasted_inventory_is_planned_revised_applied_once_and_undone_whole() {
    let harness = harness_with(Vec::new(), &[], held_stock()).await;
    let draft = paste(&harness, PASTE).await.unwrap();
    let disposition = |name: &str| {
        draft
            .lines
            .iter()
            .find(|line| line.item_name == name)
            .map(|line| line.disposition.clone())
    };
    assert_eq!(
        disposition("Shrapnel"),
        Some(StockReconciliationDisposition::ShrapnelConverted {
            sold_at_tt: None.into()
        })
    );
    assert_eq!(
        disposition("Iron Stone"),
        Some(StockReconciliationDisposition::Removed)
    );
    assert_eq!(
        disposition("Nanocube"),
        Some(StockReconciliationDisposition::UnknownOrigin)
    );
    assert!(draft
        .excluded
        .iter()
        .any(|item| item.reason == StockReconciliationExclusion::UniversalAmmo));
    // The read gives the same draft back.
    let stored = harness.api.account_reconciliation_draft().await.unwrap();
    assert_eq!(stored.0.as_ref(), Some(&draft));

    // A disposition that does not fit is refused, and the draft kept.
    let refused = harness
        .api
        .account_reconciliation_revise(StockReconciliationReviseInput {
            draft_id: draft.id.clone(),
            revision: StockReconciliationRevision::Set {
                items: vec!["Nanocube".to_string()],
                choice: StockReconciliationChoice::Removed,
            },
        })
        .await;
    assert!(
        matches!(refused, Err(ApiError::BadRequest { .. })),
        "{refused:?}"
    );
    let draft = harness
        .api
        .account_reconciliation_revise(StockReconciliationReviseInput {
            draft_id: draft.id.clone(),
            revision: StockReconciliationRevision::ShrapnelSoldAtTt {
                quantity: Some(100_000.0),
            },
        })
        .await
        .unwrap()
        .draft;

    let applied = harness
        .api
        .account_reconciliation_apply(StockReconciliationApplyInput {
            draft_id: draft.id.clone(),
        })
        .await
        .unwrap();
    let StockReconciliationApplied::Applied {
        reconciliation_id,
        refused,
        ..
    } = applied
    else {
        panic!("applied: {applied:?}");
    };
    assert!(refused.is_empty());
    let id = reconciliation_id.0.expect("recorded");
    assert!((held(&harness, "Iron Stone").await - 5.0).abs() < 1e-9);
    assert!((held(&harness, "Shrapnel").await - 1_500_000.0).abs() < 1e-6);
    assert!((held(&harness, "Nanocube").await - 500.0).abs() < 1e-9);

    // History names the reconciliation on the removal it made, which is
    // undone only with it.
    let removal = harness
        .api
        .activity_history(Profession::Inventory)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == "removal")
        .expect("the removal");
    assert_eq!(removal.reconciliation_id.0.as_deref(), Some(id.as_str()));
    assert!(!removal.can_delete);
    assert!(matches!(
        harness
            .api
            .stock_removal_undo(ActivityUndoInput {
                id: removal.id.clone()
            })
            .await,
        Err(ApiError::BadRequest { .. })
    ));

    assert_eq!(
        harness
            .api
            .account_reconciliation_apply(StockReconciliationApplyInput {
                draft_id: draft.id.clone(),
            })
            .await
            .unwrap(),
        StockReconciliationApplied::AlreadyApplied {
            reconciliation_id: id.clone()
        }
    );
    assert!((held(&harness, "Iron Stone").await - 5.0).abs() < 1e-9);

    harness
        .api
        .account_reconciliation_undo(ActivityUndoInput { id: id.clone() })
        .await
        .unwrap();
    assert!((held(&harness, "Iron Stone").await - 12.0).abs() < 1e-9);
    assert!((held(&harness, "Shrapnel").await - 2_000_000.0).abs() < 1e-6);
    assert!(held(&harness, "Nanocube").await.abs() < 1e-9);
    assert!(matches!(
        harness
            .api
            .account_reconciliation_undo(ActivityUndoInput { id })
            .await,
        Err(ApiError::NotFound { .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn nothing_is_applied_while_a_session_is_tracked() {
    let harness = harness_with(Vec::new(), &[], held_stock()).await;
    let draft = paste(&harness, PASTE).await.unwrap();
    harness
        .db
        .with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions (id, started_at, is_active) \
                 VALUES ('running', 3000.0, 1)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        harness
            .api
            .account_reconciliation_apply(StockReconciliationApplyInput {
                draft_id: draft.id.clone(),
            })
            .await
            .unwrap(),
        StockReconciliationApplied::SessionTracked
    );
    assert!((held(&harness, "Iron Stone").await - 12.0).abs() < 1e-9);
    assert!(matches!(
        paste(&harness, PASTE).await,
        Err(ApiError::BadRequest { .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn a_paste_with_a_row_that_does_not_read_is_refused_whole() {
    let harness = harness_with(Vec::new(), &[], held_stock()).await;
    let refused = paste(
        &harness,
        "Name\tQuantity\tValue(PED)\tContainer\tContainerRefId\n\
         Iron Stone\t5\t0.6500\tCARRIED\t\n\
         Shrapnel\tlots\t150.0000\tCARRIED\t\n",
    )
    .await;
    assert!(
        matches!(&refused, Err(ApiError::BadRequest { message }) if message.contains("line 3")),
        "{refused:?}"
    );
    assert_eq!(
        harness.api.account_reconciliation_draft().await.unwrap().0,
        None
    );
}
