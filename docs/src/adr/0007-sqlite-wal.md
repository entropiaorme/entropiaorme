# ADR-0007: SQLite with write-ahead logging

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

EntropiaOrme is a single-user desktop tool: all gameplay data (equipment, tracking sessions, kills, loot, the ledger, skill and codex progress) is owned by the one person running the app and lives on their machine. The store therefore needs to be embedded, file-backed, and dependency-free, not a separate server process.

The concurrency profile forces the harder requirement. In the Python backend a single SQLite connection is shared across the FastAPI threadpool plus several background worker threads (session and loot rows are written on the chat-log watcher thread as combat is parsed, while HTTP handlers read the same tables to serve hydration). The connection is opened with `check_same_thread=False` precisely so it can be touched from any of these threads. Under SQLite's default rollback journal a write takes an exclusive lock that blocks concurrent readers, which would stall hydration reads behind every parser write. A reader and a writer have to be able to proceed at the same time.

## Decision

Use SQLite in write-ahead-logging mode, configured identically in both backend implementations.

The Python base, `backend/db/base.py`, opens the connection and applies four session pragmas: `journal_mode = WAL`, `synchronous = NORMAL`, `busy_timeout = 5000` (a five-second wait before a lock contention raises), and `cache_size = -8000` (an 8 MB page cache). The connection is shared (`check_same_thread=False`) and guarded by a re-entrant `threading.RLock` exposed as `self.lock`. SQLite's own serialisation protects a single `execute`, but a compound, multi-step pattern (an `execute` followed by `fetchall`, or a batch inside a transaction) needs that lock held around it to keep cursor state coherent across threads. The application database (`backend/db/app_database.py`) builds on this base at schema version 33, evolving in place through a versioned forward-migration chain and refusing legacy schemas below version 28 rather than stamping a current version over a non-migrated database. The tracking tables (`backend/tracking/schema.py`) carry no version counter of their own; they rely on `CREATE TABLE IF NOT EXISTS` for fresh installs and on the application database's migrations for in-place evolution.

The native Rust persistence layer, `frontend/src-tauri/eo-services/src/db.rs`, reproduces the same surface through `sqlx`: `SqliteConnectOptions` sets WAL, `SqliteSynchronous::Normal`, a five-second busy timeout, and the same `-8000` page cache. Foreign-key enforcement is explicitly left off to match the backend's effective pragma surface, where the schema's `REFERENCES` clauses are declarative. The pool is capped at `max_connections(1)`, so every statement serialises through one connection exactly as the shared backend connection does; a cloned `Db` handle shares that single pool rather than opening a second owner.

## Consequences

- Readers do not block the writer and the writer does not block readers: WAL lets the parser write session and loot rows while hydration handlers read concurrently, which is the property the threaded backend depends on.
- `synchronous = NORMAL` under WAL trades a durability edge case (a transaction committed just before an OS crash may be lost, though the database is never corrupted) for materially fewer fsyncs, appropriate for a local analytical tool.
- The store stays embedded and file-backed: no server, no network dependency, and the data file is the user's own.
- The native port closes a documented locking gap. In the Python implementation, database-lock discipline across services is uneven: the writer path is single-producer (chat-driven writes all run on the watcher thread, ruling out writer/writer races), but much of the read path reads the bare shared connection without the lock, leaving the multi-step-read-against-concurrent-write cursor-coherency case a convention with a hole. The single-owner pool (`max_connections(1)`) closes it by construction: every statement passes through one connection behind one writer, so the discipline is structural rather than a convention.
- The two implementations are held to one configuration. `frontend/src-tauri/eo-services/src/db.rs` carries a test (`fresh_database_migrates_with_session_pragmas_in_effect`) that opens a fresh database and asserts the live pragmas: `journal_mode` is `wal`, `synchronous` is `1` (NORMAL), `cache_size` is `-8000`, `busy_timeout` is `5000`, and `foreign_keys` is `0`. A drift in any of these session pragmas fails the build.
- The native baseline is pinned to schema version 33: its migration chain reproduces the backend's fresh-install schema verbatim, adopts a backend-created version-33 database without re-running DDL, and refuses (leaving the file untouched) any database on an older schema version that the backend process still owns the upgrade for.

## Evidence

- `backend/db/base.py`
- `backend/db/app_database.py`
- `backend/tracking/schema.py`
- `frontend/src-tauri/eo-services/src/db.rs`
- `backend/architecture/README.md`
