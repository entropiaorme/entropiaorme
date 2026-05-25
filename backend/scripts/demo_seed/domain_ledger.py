"""Ledger domain seeder — ledger_entries + inventory_items + ledger_presets.

Feeds the Analytics > Ledger tab: the Net Ledger Impact card (expense/markup
breakdown by tag), the entry table (Date / Description / Amount / Tag),
the Quick Entries presets, and the Inventory Ledger (inventory items with
TT + markup) including a couple of inventory_sale realised-P&L flows.
"""

from __future__ import annotations

import logging
import random
import sqlite3
import uuid
from datetime import datetime, timezone
from pathlib import Path

from backend.scripts.demo_seed.contract import CanonicalRefs

log = logging.getLogger(__name__)

_RNG_SEED = 0x1ED6_E7F7

_DAY_SECONDS = 86400

_EXPENSE_TAGS = (
    "equipment",
    "repair",
    "enhancers",
    "consumable",
    "armour",
    "transport",
    "other",
)
_MARKUP_TAGS = ("item_sale", "quest_reward", "inventory_sale", "markup", "other")


def _iso_date(epoch: float) -> str:
    return datetime.fromtimestamp(epoch, tz=timezone.utc).date().isoformat()


# Hand-curated entry shapes — each is (offset_days_back, type, description, amount, tag).
# Offsets span ~0..88 days back from demo_now to populate the 90-day window.
# Mix of expense/markup, diverse tags, fictional copy.
_LEDGER_ENTRY_TEMPLATES: tuple[tuple[int, str, str, float, str], ...] = (
    # Expense entries (8) — realistic per-event magnitudes, mostly < 20 PED.
    # Real-DB inspection showed expense avg ~4 PED, max 31; tuning toward that band.
    (88, "expense", "Repair at Twin Peaks", 8.42, "repair"),
    (82, "expense", "DE V stack, auction pickup", 28.50, "enhancers"),
    (78, "expense", "TP chip, Cape Corinth", 1.20, "transport"),
    (60, "expense", "Imperium Battle Vest, full repair", 14.20, "armour"),
    (55, "expense", "H-DNA x5", 6.25, "consumable"),
    (44, "expense", "Repair at Fort Argus", 5.83, "repair"),
    (28, "expense", "Auction listing fee", 0.40, "other"),
    (5, "expense", "Universal Ammo top-up", 18.50, "equipment"),
    # Markup entries (10): heavier weighting reflects that markup rows
    # tend to dominate expense rows in a typical hunter's ledger by roughly
    # an order of magnitude. Tags spread across item_sale, quest_reward,
    # inventory_sale (inventory sales handled below). The bare-`markup`-tagged
    # entries from an earlier shape were dropped as redundant: type=markup
    # AND tag=markup carried no extra signal.
    (85, "markup", "Sold Caboria Hide x40", 22.41, "item_sale"),
    (74, "markup", "Daily quest reward: Argonaut Hunt", 12.00, "quest_reward"),
    (66, "markup", "Loot from Codex Caboria II claim", 18.75, "quest_reward"),
    (49, "markup", "Sold animal hide bundle, auction", 28.13, "item_sale"),
    (39, "markup", "Sold Atrox Wool x80, auction", 17.62, "item_sale"),
    (33, "markup", "Daily quest reward: Atrox Cull", 18.00, "quest_reward"),
    (18, "markup", "Sold Argonaut Wool x60", 11.84, "item_sale"),
    (15, "markup", "Codex Caboria II skill claim", 7.95, "quest_reward"),
    (4, "markup", "Sold Daikiba Hide x35", 8.27, "item_sale"),
    (2, "markup", "Sold Combibo Skin x25", 11.32, "item_sale"),
)


# Inventory items — fictional inventory of persistent assets. Names overlap with
# refs.items where natural (weapons), but these are independent inventory_items
# rows, not equipment_library FKs.
_INVENTORY_ITEMS: tuple[tuple[str, float, float, str, int], ...] = (
    # (name, tt_value, markup_paid, notes, acquired_days_ago)
    # TT values match the canonical EU lore where the item is documented:
    # Korss H400 max TT ~140 PED; Herman CAP-7 (small carbiner) ~95; Hedoc
    # Mayhem, Adjusted (UL FAP) ~720. Markup tail spans 90 PED -> 2400 PED
    # to give a plausible inventory-portfolio distribution (most markup <500,
    # with one or two high-markup outliers in the >2k bucket).
    ("Korss H400", 140.00, 88.50, "Bought from Twin Peaks vendor", 78),
    ("Hedoc Mayhem, Adjusted", 720.00, 540.00, "Auction win, primary big FAP", 65),
    ("Herman CAP-7 Jungle (L)", 95.00, 42.20, "Trade with hunt buddy", 54),
    ("Sweetwater Apartment Deed", 0.00, 2400.00, "Storage hub near Twin Peaks", 110),
    ("Land Plot: Calypso #L42-North", 50.00, 720.00, "Estate auction win", 132),
    (
        "Imperium Battle Vest, Sweat-Forged Plates",
        0.00,
        280.00,
        "Synth armour set, full body",
        41,
    ),
)


# Inventory sales: realised P&L deltas posted to ledger_entries with
# tag='inventory_sale'. Each references an inventory_item name in the description.
# Amount = realised gain (final markup received minus markup_paid). Small
# positive or negative numbers to demonstrate realised P&L in both directions.
_INVENTORY_SALES: tuple[tuple[int, str, float], ...] = (
    # (offset_days_back, description, amount)
    # Realised P&L deltas: amount = final markup received minus markup_paid.
    # Sized to a plausible portfolio tail (most markup rows < 30 PED;
    # occasional large gains/losses on inventory flips).
    (47, "Sold Hedoc Mayhem, Adjusted at +145 PED over basis", 145.00),
    (21, "Sold Herman CAP-7 Jungle (L) at -6 PED vs basis (early exit)", -6.00),
)


_PRESETS: tuple[tuple[str, str, str, float, str], ...] = (
    # (name, type, description, amount, tag)
    # Amounts sized to typical per-event magnitudes in a hunter's ledger.
    ("Damage Enhancer V stack", "expense", "DE V x10", 28.50, "enhancers"),
    (
        "Sold animal hide bundle",
        "markup",
        "Animal Hide x100, auction",
        28.13,
        "item_sale",
    ),
)


class LedgerSeeder:
    name: str = "ledger"
    depends_on: tuple[str, ...] = ("core", "sessions")

    def seed(self, refs: CanonicalRefs, db: sqlite3.Connection, data_dir: Path) -> None:
        rng = random.Random(_RNG_SEED)
        demo_now = refs.timeline.demo_now

        # 1) ledger_entries — base entries.
        entry_rows = 0
        for offset_days, type_, description, amount, tag in _LEDGER_ENTRY_TEMPLATES:
            # Add small random hour-of-day jitter so dates feel natural; the
            # ISO date column only stores Y-M-D, but jitter still moves entries
            # across day boundaries near midnight UTC, which is fine.
            jitter = rng.uniform(-0.4, 0.4) * _DAY_SECONDS
            epoch = demo_now - offset_days * _DAY_SECONDS + jitter
            db.execute(
                "INSERT INTO ledger_entries (id, date, type, description, amount, tag) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (str(uuid.uuid4()), _iso_date(epoch), type_, description, amount, tag),
            )
            entry_rows += 1

        # 2) ledger_entries — inventory_sale entries (realised P&L).
        for offset_days, description, amount in _INVENTORY_SALES:
            jitter = rng.uniform(-0.4, 0.4) * _DAY_SECONDS
            epoch = demo_now - offset_days * _DAY_SECONDS + jitter
            db.execute(
                "INSERT INTO ledger_entries (id, date, type, description, amount, tag) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (
                    str(uuid.uuid4()),
                    _iso_date(epoch),
                    "markup",
                    description,
                    amount,
                    "inventory_sale",
                ),
            )
            entry_rows += 1

        # 3) inventory_items.
        inventory_rows = 0
        for name, tt_value, markup_paid, notes, acquired_days_ago in _INVENTORY_ITEMS:
            acquired_at = _iso_date(demo_now - acquired_days_ago * _DAY_SECONDS)
            db.execute(
                "INSERT INTO inventory_items (id, name, tt_value, markup_paid, notes, acquired_at, updated_at) "
                "VALUES (?, ?, ?, ?, ?, ?, NULL)",
                (str(uuid.uuid4()), name, tt_value, markup_paid, notes, acquired_at),
            )
            inventory_rows += 1

        # 4) ledger_presets.
        preset_rows = 0
        for name, type_, description, amount, tag in _PRESETS:
            db.execute(
                "INSERT INTO ledger_presets (id, name, type, description, amount, tag, updated_at) "
                "VALUES (?, ?, ?, ?, ?, ?, NULL)",
                (str(uuid.uuid4()), name, type_, description, amount, tag),
            )
            preset_rows += 1

        log.info(
            "ledger seeder: wrote %d ledger_entries, %d inventory_items, %d ledger_presets.",
            entry_rows,
            inventory_rows,
            preset_rows,
        )

    def validate_synthetic_data(self, refs: CanonicalRefs) -> list[str]:
        violations: list[str] = []

        if refs.timeline.history_window_days < 30:
            violations.append(
                f"timeline.history_window_days too small for ledger spread "
                f"(got {refs.timeline.history_window_days}, need >= 30)"
            )

        return violations


SEEDER: "LedgerSeeder" = LedgerSeeder()


# Self-test: run core + this seeder against a temp dir and print the report.
# Note: ledger declares sessions_domain as a dep, but the self-test doesn't
# register sessions; we drop the dep for the self-test to let the driver
# topo-sort succeed.
if __name__ == "__main__":
    import shutil
    import tempfile

    logging.basicConfig(
        level=logging.INFO, format="%(levelname)s %(name)s: %(message)s"
    )

    from backend.scripts.demo_seed.driver import format_report, run

    selftest_seeder = LedgerSeeder()
    selftest_seeder.depends_on = ("core",)

    tmp = Path(tempfile.mkdtemp(prefix="demoseed_ledger_"))
    try:
        report = run(tmp, extra_seeders=[selftest_seeder])
        print(format_report(report))
        if report.violations:
            raise SystemExit(1)
    finally:
        shutil.rmtree(tmp)
