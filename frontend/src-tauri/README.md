# Rust workspace

Cargo workspace for the desktop application's Rust side.

## Members

- **`entropia-orme/`**: the Tauri shell. Window chrome, the overlay windows, and the backend sidecar lifecycle. The only member coupled to the Tauri toolchain; `tauri.conf.json` and its dev overlays live here (see that directory's README).
- **`eo-http/`**: HTTP substrate for the native backend (router and middleware).
- **`eo-services/`**: domain services behind that HTTP surface.
- **`eo-wire/`**: wire-format contracts (response and event types, serialisation).

The `eo-*` members are the landing zone for the native backend port; see `backend/architecture/PORT-READINESS.md` and `backend/architecture/PORTING-RULEBOOK.md` for the plan and the porting rules. They are deliberately Tauri-free, and CI keeps them that way structurally: a Linux job without the Tauri toolchain's system stack builds and tests them in isolation, so a GUI dependency creeping into backend code fails the gate rather than landing silently.

## Workspace-level files

- **`Cargo.toml`**: the virtual manifest. Shared dependency versions live in `[workspace.dependencies]`, including the Tauri minor pin (see the comment there before bumping).
- **`Cargo.lock`**: the single lockfile for all members.
- **`.cargo/audit.toml`** / **`deny.toml`**: the dependency audit and supply-chain policies enforced in CI; review both together on any Tauri bump.
- **`.sqlx/`**: offline query metadata for compile-time-checked SQL (consumed with `SQLX_OFFLINE=true` in CI). Empty until persistence code lands; regenerate with `cargo sqlx prepare` whenever a query or the schema changes.

Commands for the CI gates that cover this workspace are documented in `TESTING.md` ("Rust workspace checks").
