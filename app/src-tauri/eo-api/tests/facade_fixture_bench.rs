//! Fixture-backed typed-facade read bench: the read-path latency of the
//! analytical commands over a copy of a real-scale database, the
//! successor to the HTTP-era perf fixture bench that retired with the
//! transport. Same discipline as its predecessor (temp-dir fixture
//! copy through the real migrate path, warm-ups then samples, median /
//! p95 / min / max in milliseconds, aggregate timings only), so its
//! figures compare directly with the earlier read-path measurements.
//!
//! The first Overview call is timed separately: on a fixture predating
//! the daily-rollup read model it performs the full backfill, and on a
//! current fixture it is simply the cold first read.
//!
//! `#[ignore]`d on purpose: a measurement harness, not a correctness
//! gate. Run it with:
//!
//! ```text
//! EO_PERF_FIXTURE=<path to a database copy> \
//!   cargo test -p eo-api --release --test facade_fixture_bench -- --ignored --nocapture
//! ```

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use eo_api::analytics::Profession;
use eo_api::market::MarketHorizon;
use eo_api::Api;
use eo_services::db::Db;
use eo_services::game_data_store::GameDataStore;
use serde::Serialize;

mod common;

const WARMUPS: usize = 3;
const SAMPLES: usize = 30;

fn wire_bytes(value: &impl Serialize) -> usize {
    serde_json::to_vec(value)
        .expect("serialisable facade result")
        .len()
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n.is_multiple_of(2) {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    } else {
        sorted[n / 2]
    }
}

fn p95(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    let index = (0.95 * (n - 1) as f64).round() as usize;
    sorted[index.min(n - 1)]
}

#[test]
#[ignore = "measurement harness, not a correctness gate; run with --release --ignored --nocapture and EO_PERF_FIXTURE"]
fn facade_fixture_bench() {
    let Ok(fixture) = std::env::var("EO_PERF_FIXTURE") else {
        eprintln!("EO_PERF_FIXTURE not set; skipping (point it at a database copy)");
        return;
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let dir = tempfile::tempdir().expect("temp dir");
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("data dir");
    let db_path = data_dir.join("entropia_orme.db");
    std::fs::copy(&fixture, &db_path).expect("fixture copy");
    let wal = format!("{fixture}-wal");
    if Path::new(&wal).exists()
        && std::fs::metadata(&wal)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    {
        std::fs::copy(&wal, data_dir.join("entropia_orme.db-wal")).expect("fixture wal copy");
    }

    let db = runtime
        .block_on(Db::open(&db_path))
        .expect("migrated fixture database");
    let bench_db = db.clone();
    let game_data =
        Arc::new(GameDataStore::new(&dir.path().join("empty")).expect("empty game-data store"));
    let clock = Arc::new(eo_services::clock::RealClock::new());
    let handles = runtime.block_on(common::producer_handles(
        &db,
        &data_dir,
        runtime.handle().clone(),
    ));
    let api = Api::new(
        db,
        game_data,
        clock,
        data_dir,
        handles.config_service,
        handles.tracker,
        handles.hotbar,
        handles.watcher,
        handles.skill_tracker,
        handles.skill_scan,
        handles.spacebar,
        handles.repair_ocr,
        handles.sale_window_ocr,
        handles.quests.clone(),
        None,
        None,
        None,
    );

    let mut rows: Vec<(String, f64, f64, f64, f64)> = Vec::new();
    let mut payloads: Vec<(String, usize)> = Vec::new();
    let mut backfill_ms = 0.0;
    let mut first_hunting_ms = 0.0;
    runtime.block_on(async {
        // The cold first Overview read (the rollup backfill, when the
        // fixture predates the read model), a one-off outside the
        // warm-up/sample discipline.
        let started = Instant::now();
        api.analytics_overview("all").await.expect("first overview");
        backfill_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        api.analytics_hunting_activity("all")
            .await
            .expect("first hunting activity");
        first_hunting_ms = started.elapsed().as_secs_f64() * 1000.0;

        macro_rules! bench {
            ($label:expr, $call:expr) => {{
                for _ in 0..WARMUPS {
                    $call.await.expect($label);
                }
                let mut timings = Vec::with_capacity(SAMPLES);
                for _ in 0..SAMPLES {
                    let started = Instant::now();
                    $call.await.expect($label);
                    timings.push(started.elapsed().as_secs_f64() * 1000.0);
                }
                timings.sort_by(|a, b| a.partial_cmp(b).expect("finite timings"));
                rows.push((
                    $label.to_string(),
                    median(&timings),
                    p95(&timings),
                    timings[0],
                    timings[timings.len() - 1],
                ));
            }};
        }

        bench!("overview_all", api.analytics_overview("all"));
        bench!("overview_30d", api.analytics_overview("30d"));
        bench!("overview_90d", api.analytics_overview("90d"));
        bench!("overview_1y", api.analytics_overview("1y"));
        bench!("hunting", api.analytics_hunting());
        bench!(
            "session_rollup_heal",
            bench_db.with_writer(eo_services::session_rollup::heal)
        );
        bench!(
            "session_summary_heal",
            bench_db.with_writer(|connection| {
                eo_services::session_summary::heal_summaries(connection)
            })
        );
        bench!(
            "hunting_activity_all",
            api.analytics_hunting_activity("all")
        );
        bench!("hunting_markups", api.market_hunt_markups());
        bench!("hunting_stock", api.activity_stock(Profession::Hunting));
        bench!(
            "hunting_auction_listings",
            api.auction_listings(Profession::Hunting)
        );
        bench!("hunting_realised_markup", api.hunting_realised_markup());
        bench!("hunting_tab_bundle", async {
            tokio::try_join!(
                api.analytics_hunting_activity("all"),
                api.market_hunt_markups(),
                api.activity_stock(Profession::Hunting),
                api.auction_listings(Profession::Hunting),
                api.hunting_realised_markup(),
            )
        });
        bench!(
            "market_mobs_week",
            api.market_mob_ranking(MarketHorizon::Week)
        );
        bench!("harvest", api.analytics_harvest("all"));
        bench!("session_list", api.tracking_sessions(None, None, None));
        bench!("ledger_page", api.ledger_list(None, None));

        // The newest session's full detail, the O(session-size) read.
        let page = api
            .tracking_sessions(None, None, None)
            .await
            .expect("session list");
        if let Some(newest) = page.sessions.first() {
            let id = newest.id.clone();
            bench!("session_detail", api.tracking_session_detail(id.clone()));
        } else {
            eprintln!("fixture has no sessions; session_detail skipped");
        }

        let (activity, markups, stock, listings, realised) = tokio::try_join!(
            api.analytics_hunting_activity("all"),
            api.market_hunt_markups(),
            api.activity_stock(Profession::Hunting),
            api.auction_listings(Profession::Hunting),
            api.hunting_realised_markup(),
        )
        .expect("hunting tab payloads");
        payloads.extend([
            ("hunting_activity_all".to_string(), wire_bytes(&activity)),
            ("hunting_markups".to_string(), wire_bytes(&markups)),
            ("hunting_stock".to_string(), wire_bytes(&stock)),
            (
                "hunting_auction_listings".to_string(),
                wire_bytes(&listings),
            ),
            ("hunting_realised_markup".to_string(), wire_bytes(&realised)),
        ]);
        payloads.push((
            "hunting_tab_bundle".to_string(),
            payloads.iter().map(|(_, bytes)| bytes).sum(),
        ));
        let mobs = api
            .market_mob_ranking(MarketHorizon::Week)
            .await
            .expect("market mobs payload");
        payloads.push(("market_mobs_week".to_string(), wire_bytes(&mobs)));
    });

    println!("\ntyped-facade fixture bench (read path over a real-scale database copy)");
    println!(
        "{SAMPLES} samples per operation after {WARMUPS} warm-ups; fixture: {fixture}\n\
         first-overview (cold, incl. any rollup backfill): {backfill_ms:.1} ms\n\
         first-hunting (cold process, incl. any projection heal): {first_hunting_ms:.1} ms\n"
    );
    println!("| Operation | p50 ms | p95 ms | min ms | max ms |");
    println!("| --- | --- | --- | --- | --- |");
    for (id, p50, p95v, min, max) in &rows {
        println!("| `{id}` | {p50:.4} | {p95v:.4} | {min:.4} | {max:.4} |");
    }
    println!("\n| Payload | JSON bytes |");
    println!("| --- | ---: |");
    for (id, bytes) in &payloads {
        println!("| `{id}` | {bytes} |");
    }
    println!();
}
