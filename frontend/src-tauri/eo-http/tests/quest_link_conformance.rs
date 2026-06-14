//! Quest-link conformance through the PUBLIC PORT: the two routes ported
//! to the native substrate, proven byte-identical against the running
//! Python sidecar over identically-seeded state.
//!
//!   GET  /api/tracking/session/{id}/quest-link-suggestion -> ETag-scoped
//!        (the `/api/tracking` conditional-GET contract: 200 + strong ETag
//!        + `Cache-Control: no-cache`, with a 304 leg).
//!   POST /api/tracking/session/{id}/quest-link            -> plain 200
//!        (no ETag); accept/decline/bad-action/404/422.
//!
//! TOPOLOGY (tracking_edits_conformance's two-arm form): two sidecars,
//! both clocks frozen at `CLOCK`; the native substrate stands over the
//! upstream sidecar's database with a `MockClock` at the same instant.
//! The suggestion responses carry no random ids and no clock-derived
//! fields, so the comparison is a direct byte-for-byte (status, contract
//! headers, body). The accept/decline WRITES stamp `linked_at` from the
//! clock, but that value is in neither the response nor the read-back
//! catalogue below; the read-back compares the link STATE (link_type +
//! the nullable quest/playlist ids), which is what the affordance and the
//! follow-up suggestion observe.
//!
//! THE MAKE-OR-BREAK ASSERTION: the reference serialises the decision with
//! `response_model_exclude_unset=True`, so an ACCEPT reply carries all
//! seven link fields while a DECLINE reply carries EXACTLY two
//! (`sessionId`, `status`) and OMITS the link fields entirely. The native
//! arm reproduces that field-set difference; the byte-for-byte body
//! comparison here is what proves it.
//!
//! Gated behind the `cross-language` feature because it needs the Python
//! interpreter and the backend package at runtime. Run it with:
//!   cargo test -p eo-http --features cross-language --test quest_link_conformance
#![cfg(feature = "cross-language")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use chrono::NaiveDateTime;
use eo_http::arms::ArmOverrides;
use eo_http::cors::CorsConfig;
use eo_http::hydration::HydrationState;
use eo_http::AppState;
use eo_services::clock::MockClock;
use eo_services::game_data_store::GameDataStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

const CLOCK: &str = "2026-06-01T12:00:00";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn oracle_python() -> PathBuf {
    if let Ok(explicit) = std::env::var("EO_ORACLE_PYTHON") {
        return PathBuf::from(explicit);
    }
    let root = repo_root();
    let windows = root.join(".venv/Scripts/python.exe");
    if windows.exists() {
        windows
    } else {
        root.join(".venv/bin/python")
    }
}

struct Sidecar {
    child: Child,
    port: u16,
    data_dir: tempfile::TempDir,
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .expect("local addr")
        .port()
}

fn spawn_sidecar() -> Sidecar {
    let data_dir = tempfile::TempDir::new().expect("temp data dir");
    let port = free_port();
    let mut command = Command::new(oracle_python());
    command
        .args(["-m", "backend.main"])
        .current_dir(repo_root())
        .env("ENTROPIAORME_BACKEND_PORT", port.to_string())
        .env("ENTROPIAORME_DATA_DIR", data_dir.path())
        .env("ENTROPIA_TEST_CLOCK_START", CLOCK)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let child = command.spawn().expect("spawn backend sidecar");
    Sidecar {
        child,
        port,
        data_dir,
    }
}

fn client() -> eo_http::proxy::ProxyClient {
    eo_http::proxy::build_client()
}

async fn request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    if_none_match: Option<&str>,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    let authority = format!("127.0.0.1:{port}");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{authority}{path}"))
        .header("host", &authority)
        .header("origin", "tauri://localhost");
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(value) = if_none_match {
        builder = builder.header("if-none-match", value);
    }
    let request = match body {
        Some(payload) => builder
            .body(Body::from(payload.as_bytes().to_vec()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = client().request(request).await.expect("request succeeds");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

async fn wait_healthy(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if Instant::now() > deadline {
            panic!("backend never became healthy on port {port}");
        }
        let authority = format!("127.0.0.1:{port}");
        let probe = http::Request::builder()
            .uri(format!("http://{authority}/api/health"))
            .header("host", &authority)
            .body(Body::empty())
            .unwrap();
        if let Ok(response) = client().request(probe).await {
            if response.status() == http::StatusCode::OK {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn contract_axes(headers: &http::HeaderMap) -> Vec<Option<String>> {
    let value = |name: http::header::HeaderName| {
        headers
            .get(name)
            .map(|v| v.to_str().unwrap_or("<non-utf8>").to_string())
    };
    vec![
        value(http::header::CONTENT_TYPE),
        value(http::header::CACHE_CONTROL),
        value(http::header::ETAG),
    ]
}

async fn open_pool(path: &Path) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .foreign_keys(false)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await
        .expect("open shared database")
}

struct Arms {
    substrate_port: u16,
    comparison_port: u16,
    native_db: PathBuf,
    comparison_db: PathBuf,
}

impl Arms {
    /// Drive a request through both arms; assert status + contract headers
    /// match and the bodies are byte-identical (no per-arm random ids or
    /// clock fields in these responses). Returns the native body as JSON
    /// for follow-up assertions.
    async fn compare(&self, method: &str, path: &str, body: Option<&str>) -> Value {
        let (native_status, native_headers, native_body) =
            request(self.substrate_port, method, path, body, None).await;
        let (cmp_status, cmp_headers, cmp_body) =
            request(self.comparison_port, method, path, body, None).await;
        assert_eq!(
            native_status,
            cmp_status,
            "status diverged on {method} {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&cmp_body),
        );
        assert_eq!(
            contract_axes(&native_headers),
            contract_axes(&cmp_headers),
            "contract headers diverged on {method} {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&cmp_body),
        );
        assert_eq!(
            native_body,
            cmp_body,
            "response body diverged on {method} {path}\n  native: {}\n  python: {}",
            String::from_utf8_lossy(&native_body),
            String::from_utf8_lossy(&cmp_body),
        );
        serde_json::from_slice(&native_body).unwrap_or(Value::Null)
    }

    /// The conditional-GET 304 leg on the ETag-scoped suggestion: fetch to
    /// learn the ETag (which must already be equal across arms because the
    /// 200 bodies are byte-identical), then re-fetch with `If-None-Match`.
    /// Both arms answer 304 with no body and matching contract headers.
    async fn compare_conditional(&self, path: &str) {
        let (_, native_headers, _) = request(self.substrate_port, "GET", path, None, None).await;
        let (_, cmp_headers, _) = request(self.comparison_port, "GET", path, None, None).await;
        let native_etag = native_headers
            .get(http::header::ETAG)
            .expect("native etag")
            .to_str()
            .unwrap()
            .to_string();
        let cmp_etag = cmp_headers
            .get(http::header::ETAG)
            .expect("python etag")
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(native_etag, cmp_etag, "etag diverged on {path}");

        let (native_status, native_headers, native_body) =
            request(self.substrate_port, "GET", path, None, Some(&native_etag)).await;
        let (cmp_status, cmp_headers, cmp_body) =
            request(self.comparison_port, "GET", path, None, Some(&cmp_etag)).await;
        assert_eq!(native_status, http::StatusCode::NOT_MODIFIED, "native 304");
        assert_eq!(cmp_status, http::StatusCode::NOT_MODIFIED, "python 304");
        assert!(native_body.is_empty(), "native 304 has no body");
        assert!(cmp_body.is_empty(), "python 304 has no body");
        assert_eq!(
            contract_axes(&native_headers),
            contract_axes(&cmp_headers),
            "304 contract headers diverged on {path}"
        );
    }

    /// Read back the persisted link state (the columns the accept/decline
    /// writes mutate, minus the clock-stamped `linked_at`), one row per
    /// session, ordered by session id. The literal `linked_at` differs per
    /// arm/run (the writes stamp it from each arm's clock), so it is
    /// excluded; the link_type + nullable ids are the observable state.
    async fn compare_links(&self, step: &str) {
        let native = links_readback(&self.native_db).await;
        let comparison = links_readback(&self.comparison_db).await;
        assert_eq!(
            native, comparison,
            "analytics-link state diverged after {step}"
        );
    }
}

async fn links_readback(db_path: &Path) -> Value {
    let pool = open_pool(db_path).await;
    let rows = sqlx::query(
        "SELECT session_id, link_type, quest_id, playlist_id \
         FROM session_quest_analytics_links ORDER BY session_id",
    )
    .fetch_all(&pool)
    .await
    .expect("read analytics links")
    .into_iter()
    .map(|row| {
        json!({
            "sessionId": row.get::<String, _>(0),
            "linkType": row.get::<String, _>(1),
            "questId": row.get::<Option<i64>, _>(2),
            "playlistId": row.get::<Option<i64>, _>(3),
        })
    })
    .collect::<Vec<_>>();
    json!(rows)
}

// ── Identical state seeding into BOTH arms' databases ──

// Session ids, one per GET-suggestion reason plus the POST cases. Real
// UUID-shaped strings so they survive the single-segment id de-match.
const S_NONE: &str = "00000000-0000-4000-8000-000000000001"; // no_completions
const S_QUEST: &str = "00000000-0000-4000-8000-000000000002"; // single_quest -> accept-quest
const S_EXACT: &str = "00000000-0000-4000-8000-000000000003"; // exact_playlist -> accept-playlist
const S_AMBIG: &str = "00000000-0000-4000-8000-000000000004"; // ambiguous_playlist
const S_UNCLEAN: &str = "00000000-0000-4000-8000-000000000005"; // unclean
const S_DECLINE: &str = "00000000-0000-4000-8000-000000000006"; // decline -> declined
const S_MISSING: &str = "00000000-0000-4000-8000-0000000000ff"; // never seeded (404)

/// Seed the quest/playlist fixtures and one session per scenario into one
/// database. The playlist-matching rule (from `_find_matching_playlists`):
/// a playlist matches iff its non-empty immediate set ⊆ completed AND
/// completed ⊆ immediate∪long_horizon.
///
/// Quests 1..=5 exist. Playlist P1 (immediate {1,2}) is the exact target.
/// Playlist P2 (immediate {1,2}, long_horizon {3}) ALSO matches when the
/// completed set is {1,2} (its immediate ⊆ completed and completed ⊆
/// {1,2,3}), so {1,2} matches BOTH P1 and P2 -> ambiguous. To get a clean
/// single exact match we use a completed set ({1,4}) that only P3
/// (immediate {1,4}) covers. `created_at` is pinned per playlist so the
/// `ORDER BY created_at` in `get_playlists` is deterministic across arms.
async fn seed_db(db_path: &Path) {
    let pool = open_pool(db_path).await;
    for stmt in [
        "DELETE FROM session_quest_analytics_links",
        "DELETE FROM session_quest_completions",
        "DELETE FROM quest_playlist_items",
        "DELETE FROM quest_playlists",
        "DELETE FROM quests",
        "DELETE FROM tracking_sessions",
    ] {
        sqlx::query(stmt).execute(&pool).await.expect("clear table");
    }

    // Sessions: one per scenario. ended sessions (is_active=0); the routes
    // do not guard on active-state, but the fixtures model post-session
    // linkage, so they are ended.
    for id in [S_NONE, S_QUEST, S_EXACT, S_AMBIG, S_UNCLEAN, S_DECLINE] {
        sqlx::query(
            "INSERT INTO tracking_sessions(id,started_at,ended_at,is_active,armour_cost,heal_cost,\
             dangling_cost,mob_tracking_mode,updated_at) VALUES(?,1000.0,4600.0,0,0,0,0,'mob',4600.0)",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Quests 1..=5 (id is the AUTOINCREMENT PK; pin it explicitly so both
    // arms agree on the integer ids the response stringifies).
    for qid in 1..=5_i64 {
        sqlx::query(
            "INSERT INTO quests(id,name,planet,is_active,created_at,category) \
             VALUES(?,?,'Calypso',1,1000.0,'kill')",
        )
        .bind(qid)
        .bind(format!("Quest {qid}"))
        .execute(&pool)
        .await
        .unwrap();
    }

    // Playlists with pinned ids + created_at.
    //   P1 (id 1): immediate {1,2}            (matches completed {1,2})
    //   P2 (id 2): immediate {1,2}, lh {3}    (also matches completed {1,2})
    //   P3 (id 3): immediate {1,4}            (the lone exact match for {1,4})
    for (pid, created) in [(1_i64, 1000.0), (2, 1001.0), (3, 1002.0)] {
        sqlx::query(
            "INSERT INTO quest_playlists(id,name,planet,estimated_minutes,is_active,created_at) \
             VALUES(?,?,'Calypso',30,1,?)",
        )
        .bind(pid)
        .bind(format!("Playlist {pid}"))
        .bind(created)
        .execute(&pool)
        .await
        .unwrap();
    }
    // (playlist_id, quest_id, sort_order, group_type)
    for (plid, qid, sort, group) in [
        (1_i64, 1_i64, 0_i64, "immediate"),
        (1, 2, 1, "immediate"),
        (2, 1, 0, "immediate"),
        (2, 2, 1, "immediate"),
        (2, 3, 2, "long_horizon"),
        (3, 1, 0, "immediate"),
        (3, 4, 1, "immediate"),
    ] {
        sqlx::query(
            "INSERT INTO quest_playlist_items(playlist_id,quest_id,sort_order,description,group_type) \
             VALUES(?,?,?,NULL,?)",
        )
        .bind(plid)
        .bind(qid)
        .bind(sort)
        .bind(group)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Completions per scenario session:
    //   S_NONE:    none.
    //   S_QUEST:   one quest {2}             -> single_quest (questId "2").
    //   S_EXACT:   {1,4}                     -> exact_playlist (P3, "3").
    //   S_AMBIG:   {1,2}                     -> matches P1 and P2 -> ambiguous.
    //   S_UNCLEAN: {1,5} (>1, matches no PL) -> unclean.
    //   S_DECLINE: {2} (linkable, but we decline it).
    let completions: &[(&str, &[i64])] = &[
        (S_QUEST, &[2]),
        (S_EXACT, &[1, 4]),
        (S_AMBIG, &[1, 2]),
        (S_UNCLEAN, &[1, 5]),
        (S_DECLINE, &[2]),
    ];
    for (sid, quest_ids) in completions {
        for qid in *quest_ids {
            sqlx::query(
                "INSERT INTO session_quest_completions(session_id,quest_id,completed_at) \
                 VALUES(?,?,2000.0)",
            )
            .bind(sid)
            .bind(qid)
            .execute(&pool)
            .await
            .unwrap();
        }
    }
}

async fn seed_both(arms: &Arms) {
    seed_db(&arms.native_db).await;
    seed_db(&arms.comparison_db).await;
}

async fn boot() -> (Sidecar, Sidecar, Arms) {
    let upstream = spawn_sidecar();
    let comparison = spawn_sidecar();
    wait_healthy(upstream.port).await;
    wait_healthy(comparison.port).await;

    let native_db = upstream.data_dir.path().join("entropia_orme.db");
    let pool = open_pool(&native_db).await;
    let game_data = Arc::new(
        GameDataStore::new(&repo_root().join("backend/data/snapshot")).expect("snapshot loads"),
    );
    let naive = NaiveDateTime::parse_from_str(CLOCK, "%Y-%m-%dT%H:%M:%S").expect("clock parses");
    let hydration = Arc::new(HydrationState::new(
        eo_services::db::Db::from_pool(pool),
        game_data,
        Arc::new(MockClock::new(Some(naive), 0.0)),
        upstream.data_dir.path().to_path_buf(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind substrate");
    listener.set_nonblocking(true).expect("nonblocking");
    let substrate_port = listener.local_addr().expect("addr").port();
    let state = Arc::new(
        AppState::new(
            format!("127.0.0.1:{}", upstream.port),
            substrate_port,
            ArmOverrides::empty(),
        )
        .with_hydration(hydration)
        .with_cors(CorsConfig::new(5173, None)),
    );
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
        eo_http::serve(listener, state).await.expect("serve");
    });
    wait_healthy(substrate_port).await;

    let comparison_db = comparison.data_dir.path().join("entropia_orme.db");
    let arms = Arms {
        substrate_port,
        comparison_port: comparison.port,
        native_db,
        comparison_db,
    };
    (upstream, comparison, arms)
}

const SESSION: &str = "/api/tracking/session";

fn suggestion_path(id: &str) -> String {
    format!("{SESSION}/{id}/quest-link-suggestion")
}
fn link_path(id: &str) -> String {
    format!("{SESSION}/{id}/quest-link")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_quest_link_surface_conforms_through_the_public_port() {
    let (_upstream, _comparison, arms) = boot().await;
    seed_both(&arms).await;

    // ── GET suggestion: all six computed reasons ──
    // no_completions: all link fields null.
    let v = arms.compare("GET", &suggestion_path(S_NONE), None).await;
    assert_eq!(v["suggestionType"], "none");
    assert_eq!(v["reason"], "no_completions");
    assert!(v["questId"].is_null() && v["playlistId"].is_null());

    // single_quest: questId is the stringified int, playlist null.
    let v = arms.compare("GET", &suggestion_path(S_QUEST), None).await;
    assert_eq!(v["suggestionType"], "quest");
    assert_eq!(v["reason"], "single_quest");
    assert_eq!(v["questId"], "2");
    assert_eq!(v["questName"], "Quest 2");
    assert!(v["playlistId"].is_null());

    // exact_playlist: playlistId set (P3), quest null.
    let v = arms.compare("GET", &suggestion_path(S_EXACT), None).await;
    assert_eq!(v["suggestionType"], "playlist");
    assert_eq!(v["reason"], "exact_playlist");
    assert_eq!(v["playlistId"], "3");
    assert_eq!(v["playlistName"], "Playlist 3");
    assert!(v["questId"].is_null());

    // ambiguous_playlist: matches >1 playlist -> none, all link fields null.
    let v = arms.compare("GET", &suggestion_path(S_AMBIG), None).await;
    assert_eq!(v["suggestionType"], "none");
    assert_eq!(v["reason"], "ambiguous_playlist");
    assert!(v["questId"].is_null() && v["playlistId"].is_null());

    // unclean: >1 completion matching 0 playlists -> none.
    let v = arms.compare("GET", &suggestion_path(S_UNCLEAN), None).await;
    assert_eq!(v["suggestionType"], "none");
    assert_eq!(v["reason"], "unclean");

    // ── GET 404: a session that was never seeded ──
    let v = arms.compare("GET", &suggestion_path(S_MISSING), None).await;
    assert_eq!(v["detail"], "Session not found");

    // ── GET 304 conditional leg on a 200-bearing suggestion ──
    arms.compare_conditional(&suggestion_path(S_QUEST)).await;

    // ── POST accept-quest: persists, replies the 7-field linked object ──
    let v = arms
        .compare("POST", &link_path(S_QUEST), Some(r#"{"action": "accept"}"#))
        .await;
    assert_eq!(v["status"], "linked");
    assert_eq!(v["linkType"], "quest");
    assert_eq!(v["questId"], "2");
    assert_eq!(v["questName"], "Quest 2");
    assert!(v["playlistId"].is_null());
    arms.compare_links("accept-quest").await;

    // The accepted session's re-GET now reports already_linked, echoing the
    // stored quest_id/name (the existing-row branch of the suggestion).
    let v = arms.compare("GET", &suggestion_path(S_QUEST), None).await;
    assert_eq!(v["suggestionType"], "none");
    assert_eq!(v["reason"], "already_linked");
    assert_eq!(v["questId"], "2");
    assert_eq!(v["questName"], "Quest 2");

    // ── POST accept-playlist: the playlist arm of accept ──
    let v = arms
        .compare("POST", &link_path(S_EXACT), Some(r#"{"action": "accept"}"#))
        .await;
    assert_eq!(v["status"], "linked");
    assert_eq!(v["linkType"], "playlist");
    assert_eq!(v["playlistId"], "3");
    assert!(v["questId"].is_null());
    arms.compare_links("accept-playlist").await;

    // ── POST accept when nothing is linkable -> 409 ──
    // S_AMBIG's suggestion is "none"/"ambiguous_playlist"; accept maps the
    // ValueError to 409 with the reference's exact message.
    let v = arms
        .compare("POST", &link_path(S_AMBIG), Some(r#"{"action": "accept"}"#))
        .await;
    assert_eq!(
        v["detail"],
        format!("No linkable suggestion for session {S_AMBIG}: ambiguous_playlist")
    );

    // ── POST decline: EXACTLY {sessionId, status} (the field-set make-or-
    //    break). The byte comparison in `compare` already proves the field
    //    set; assert it here too against the parsed object. ──
    let v = arms
        .compare(
            "POST",
            &link_path(S_DECLINE),
            Some(r#"{"action": "decline"}"#),
        )
        .await;
    assert_eq!(v["sessionId"], S_DECLINE);
    assert_eq!(v["status"], "declined");
    let object = v.as_object().expect("decline body is an object");
    assert_eq!(
        object.len(),
        2,
        "decline must omit the link fields entirely (exactly sessionId + status), got {object:?}"
    );
    arms.compare_links("decline").await;

    // The declined session's re-GET reports the declined reason, with the
    // stored (null) quest/playlist ids echoed through.
    let v = arms.compare("GET", &suggestion_path(S_DECLINE), None).await;
    assert_eq!(v["suggestionType"], "none");
    assert_eq!(v["reason"], "declined");

    // ── POST bad action -> 400 with the reference's detail ──
    let v = arms
        .compare(
            "POST",
            &link_path(S_NONE),
            Some(r#"{"action": "frobnicate"}"#),
        )
        .await;
    assert_eq!(v["detail"], "Action must be 'accept' or 'decline'");

    // ── POST 404: a missing session (the existence guard runs before the
    //    service, but AFTER body validation; a valid body here) ──
    let v = arms
        .compare(
            "POST",
            &link_path(S_MISSING),
            Some(r#"{"action": "decline"}"#),
        )
        .await;
    assert_eq!(v["detail"], "Session not found");

    // ── POST 422: a missing `action` field. Body validation precedes the
    //    404, so even on a missing session this is the 422, not the 404. ──
    arms.compare("POST", &link_path(S_MISSING), Some("{}"))
        .await;
}
