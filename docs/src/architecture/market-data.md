# Market data pipeline

The market layer tracks what loot items actually trade for, without ever
letting an estimate masquerade as measured profit. Every observation is
entered on the player's own machine, by a paste or by hand, and lands in a
quarantined informational layer; the accounting boundary it sits behind is
[ADR-0024](../adr/0024-market-informational-layer.md).

## The local feed

The game builds a market ledger in-game and copies it to the clipboard
as a tab-separated table: one row per item, five time horizons (day,
week, month, year, decade), each with a markup percentage and a sales
volume. The user pastes that export into the Market page, reviews the
parse, and commits it.

- `eo-services/src/market_paste.rs` parses the paste fail-soft: rows it
  cannot read are skipped and reported, a no-sales window (`N/A`)
  becomes a null markup rather than a zero, and sales volumes normalise
  to PED. The committed result is one `market_submissions` row recording the
  paste, and one `market_observations` row per item and horizon (migration
  `0005_market_observations.sql`; column detail in the
  [schema reference](database-schema.md)).
- `eo-services/src/market_service.rs` serves the reads: each observed
  item's latest readings with when they were observed, per-item history,
  and the market signals a surface valuing loot asks for (an activity's
  loot items, or exactly the items it names). `eo-api/src/market.rs` adds a
  fee-efficient packet threshold: through the measured listing-fee curve in
  `eo-services/src/auction_fee.rs`, the smallest gross markup whose listing
  fee stays within a chosen share of it (ten per cent by default), from
  which an item's packet TT follows by its premium. It deliberately does
  not constrain the parcel by traded volume. Scoping the loot items reads
  the accounting tables; the reverse direction is forbidden (see the
  boundary below).
- Beside the pasted observations, the player can enter an item's markup by
  hand (a percentage, or PED above TT per unit), and whichever of the two
  was recorded more recently is the one the item is valued at; an item can
  also be marked to be valued at the Nanocube markup in the live session
  figures (migration `0070_market_manual_markups.sql`).
- The paste surface also accepts, behind a disclosure, a manual absolute
  PED-per-unit observation for zero-TT and unit-priced items. These append
  to `market_unit_price_observations`, and the latest quote is served with
  the item's other signals. The quote is informational only, just like
  percentage markup.
- `eo-api/src/market.rs` is the typed command family over those reads,
  consumed by the Market page (`app/src/lib/features/market/`).

## Where observations come from

Market observations enter only by the player's paste or hand entry. The
app downloads no market feed and uploads none: the shared service
recorded in [ADR-0025](../adr/0025-central-market-data-service.md) is no
longer read or written by the app, the webview may reach no external
origin (`connect-src 'self'` in
`app/src-tauri/entropia-orme/tauri.conf.json`), and nothing is fetched
from the game itself for the market layer.

## The accounting boundary

Estimated markup and absolute unit price are informational data classes. They
never join the ledger, the accounting aggregates, or any realised rate; value becomes
profit-and-loss only when a confirmed sale is entered in the ledger. The
boundary is one-directional (the market layer may read accounting data;
the accounting surfaces may never reference the market layer) and
mechanically enforced by the `market-isolation` CI guard
(`app/src-tauri/xtask/src/market_isolation.rs`), which scans down to raw
SQL so a table reference cannot slip past the module seam. Any future
markup display follows the same recipe: compute on the market side, join
in the frontend, label as an estimate.
