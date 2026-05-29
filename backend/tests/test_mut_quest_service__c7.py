"""Mutation-hardening tests for QuestService internal helpers (cluster c7).

Each test exercises a private helper of ``backend.services.quest_service``
directly (or through a public seam) and asserts the exact behaviour that a
surviving mutant in that line would break. SQL keyword/identifier casing
mutants and a handful of value-default mutants are behaviourally equivalent
under SQLite and are recorded as equivalents rather than killed here.
"""

import logging
from pathlib import Path

import pytest

import backend.services.quest_service as qs_module
from backend.db.app_database import AppDatabase
from backend.services.quest_service import QuestService

LOGGER_NAME = "backend.services.quest_service"


@pytest.fixture
def svc(tmp_path: Path) -> QuestService:
    db = AppDatabase(tmp_path / "test.db")
    return QuestService(db)


class _FakeTime:
    """Stand-in for the ``time`` module exposing a fixed ``time()``."""

    NOW = 1_000_000.0

    @staticmethod
    def time() -> float:
        return _FakeTime.NOW


# ── _record_session_completion ────────────────────────────────────────────


def test_record_session_completion_uses_given_timestamp(svc: QuestService):
    # mutmut_4: `ts = completed_at if completed_at is None else time.time()`
    # inverts the ternary; the explicit completed_at must be persisted.
    q = svc.create_quest({"name": "Q"})
    svc._record_session_completion("sess-xyz-0001", q["id"], 12345.0)
    row = svc._conn.execute(
        "SELECT completed_at FROM session_quest_completions WHERE quest_id = ?",
        (q["id"],),
    ).fetchone()
    assert row[0] == 12345.0


def test_record_session_completion_session_log_message(
    svc: QuestService, caplog
):
    # mutmut_13..22: corrupt the session-branch log.info call (None message,
    # dropped/reordered args, changed text, or session_id[:8] -> [:9]).
    q = svc.create_quest({"name": "Q"})
    with caplog.at_level(logging.INFO, logger=LOGGER_NAME):
        svc._record_session_completion("SESSION_AB_X9", q["id"], 100.0)
    infos = [
        r for r in caplog.records
        if r.name == LOGGER_NAME and r.levelno == logging.INFO
    ]
    # getMessage() raises for the None/arg mutants and differs for the
    # text/slice mutants; the original renders exactly this.
    messages = [r.getMessage() for r in infos]
    assert f"Recorded quest {q['id']} completion in session SESSION_" in messages


def test_record_session_completion_manual_log_message(
    svc: QuestService, caplog
):
    # mutmut_23..29: corrupt the manual-branch log.info call.
    q = svc.create_quest({"name": "Q"})
    with caplog.at_level(logging.INFO, logger=LOGGER_NAME):
        svc._record_session_completion(None, q["id"], 100.0)
    infos = [
        r for r in caplog.records
        if r.name == LOGGER_NAME and r.levelno == logging.INFO
    ]
    messages = [r.getMessage() for r in infos]
    assert f"Recorded manual completion for quest {q['id']}" in messages


# ── _delete_latest_quest_claim / _delete_latest_quest_reward_entry ─────────


def test_delete_latest_quest_claim_missing_returns_false(svc: QuestService):
    # mutmut_7: `return False` -> `return True` when no claim row exists.
    assert svc._delete_latest_quest_claim(424242) is False


def test_delete_latest_quest_claim_present_returns_true_and_deletes(
    svc: QuestService,
):
    # mutmut_16: trailing `return True` -> `return False`; also proves the row
    # is actually removed (guards the DELETE statement).
    q = svc.create_quest({"name": "Skill", "reward_ped": 5.0, "reward_is_skill": True})
    svc._conn.execute(
        "INSERT INTO quest_claims (quest_id, quest_name, ped_value, claimed_at) "
        "VALUES (?, ?, ?, ?)",
        (q["id"], q["name"], 5.0, 1.0),
    )
    svc._conn.commit()
    assert svc._delete_latest_quest_claim(q["id"]) is True
    remaining = svc._conn.execute(
        "SELECT COUNT(*) FROM quest_claims WHERE quest_id = ?", (q["id"],)
    ).fetchone()[0]
    assert remaining == 0


def test_delete_latest_quest_reward_entry_missing_returns_false(svc: QuestService):
    # mutmut_7: `return False` -> `return True` when no ledger row exists.
    assert svc._delete_latest_quest_reward_entry("Nope", 1.0) is False


def test_delete_latest_quest_reward_entry_present_returns_true_and_deletes(
    svc: QuestService,
):
    # mutmut_16: trailing `return True` -> `return False`; proves the DELETE.
    svc._conn.execute(
        "INSERT INTO ledger_entries (id, date, type, description, amount, tag) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        ("e1", "2024-01-01", "markup", "Quest: Boss", 3.5, "quest_reward"),
    )
    svc._conn.commit()
    assert svc._delete_latest_quest_reward_entry("Boss", 3.5) is True
    remaining = svc._conn.execute(
        "SELECT COUNT(*) FROM ledger_entries WHERE description = ?",
        ("Quest: Boss",),
    ).fetchone()[0]
    assert remaining == 0


# ── _normalize_db_id ───────────────────────────────────────────────────────


def test_normalize_db_id_memoryview(svc: QuestService):
    # mutmut_1: `value = value.tobytes()` -> `value = None`.
    assert svc._normalize_db_id(memoryview(b"123")) == 123


def test_normalize_db_id_invalid_bytes_raises_decode_error(svc: QuestService):
    # mutmut_10/11: errors="strict" -> "XXstrictXX"/"STRICT" (unknown handler).
    # Valid utf-8 never triggers the handler, so only invalid bytes expose it:
    # strict raises UnicodeDecodeError, an unknown handler raises LookupError.
    with pytest.raises(UnicodeDecodeError):
        svc._normalize_db_id(b"\xff\xfe")


def test_normalize_db_id_float_is_int_coerced(svc: QuestService):
    # mutmut_15/16/20/21: the final reducer `int(value) if isinstance(value,
    # bool) or hasattr(value, "__int__") else value` is broken so the int()
    # coercion is skipped; a float would then pass through unchanged.
    result = svc._normalize_db_id(5.0)
    assert result == 5
    assert isinstance(result, int)


# ── _scalar_from_row ───────────────────────────────────────────────────────


class _KeyedRow:
    """A non-dict mapping: key membership via ``in`` and value via ``[key]``,
    with a distinct positional value at index 0."""

    def __init__(self, by_name: dict, by_index: list):
        self._by_name = by_name
        self._by_index = by_index

    def keys(self):
        return self._by_name.keys()

    def __contains__(self, k):
        return k in self._by_name

    def __getitem__(self, k):
        if isinstance(k, int):
            return self._by_index[k]
        return self._by_name[k]


def test_scalar_from_row_dict_uses_key(svc: QuestService):
    # mutmut_2: `row.get(key)` -> `row.get(None)`.
    assert svc._scalar_from_row({"a": 1, "b": 2}, "b") == 2


def test_scalar_from_row_keyed_mapping_uses_key_branch(svc: QuestService):
    # mutmut_3/7/8: hasattr(row, "keys") corrupted -> keys branch skipped.
    # mutmut_9: `if key in row` -> `if key not in row` -> keys branch skipped.
    # In every broken case it falls through to row[0] ("IDX") instead of the
    # keyed value ("NAME").
    row = _KeyedRow({"mob_name": "NAME"}, ["IDX"])
    assert svc._scalar_from_row(row, "mob_name") == "NAME"


# ── _row_to_quest ──────────────────────────────────────────────────────────


def test_row_to_quest_zero_cooldown_has_no_expiry(svc: QuestService):
    # mutmut_24: `cd_hours > 0` -> `cd_hours >= 0`; with cd_hours == 0 the
    # original yields no cooldown window.
    d = svc._row_to_quest(
        {"id": 1, "last_completed_at": 1000.0, "cooldown_hours": 0}
    )
    assert d["cooldown_expires_at"] is None


def test_row_to_quest_cooldown_expiry_value(svc: QuestService):
    # mutmut_29: `cd_hours * 3600` -> `* 3601` shifts the computed expiry.
    d = svc._row_to_quest(
        {"id": 1, "last_completed_at": 0.0, "cooldown_hours": 1}
    )
    assert d["cooldown_expires_at"] == "1970-01-01T01:00:00+00:00"


# ── _is_quest_cooling ──────────────────────────────────────────────────────


def test_is_quest_cooling_missing_cooldown_with_last(svc: QuestService):
    # mutmut_9: `or cd_hours is None or` -> `or cd_hours is None and`; with
    # last set and cd_hours None the original returns False (the mutant would
    # raise comparing None <= 0).
    assert svc._is_quest_cooling(
        {"last_completed_at": _FakeTime.NOW, "cooldown_hours": None}
    ) is False


def test_is_quest_cooling_missing_last_with_cooldown(svc: QuestService):
    # mutmut_10: `last is None or cd_hours is None` -> `... and ...`; with
    # last None and cd_hours positive the original returns False (the mutant
    # would fall through and raise on None + ...).
    assert svc._is_quest_cooling(
        {"last_completed_at": None, "cooldown_hours": 5}
    ) is False


def test_is_quest_cooling_zero_cooldown_not_cooling(svc: QuestService, monkeypatch):
    # mutmut_13: `cd_hours <= 0` -> `cd_hours < 0`; cd_hours == 0 must be
    # treated as "no cooldown" even when last is in the future.
    monkeypatch.setattr(qs_module, "time", _FakeTime)
    assert svc._is_quest_cooling(
        {"last_completed_at": _FakeTime.NOW + 10_000, "cooldown_hours": 0}
    ) is False


def test_is_quest_cooling_one_hour_cooldown_is_cooling(
    svc: QuestService, monkeypatch
):
    # mutmut_14: `cd_hours <= 0` -> `cd_hours <= 1`; a 1-hour cooldown that
    # just completed is still cooling.
    monkeypatch.setattr(qs_module, "time", _FakeTime)
    assert svc._is_quest_cooling(
        {"last_completed_at": _FakeTime.NOW, "cooldown_hours": 1}
    ) is True


def test_is_quest_cooling_guard_returns_false(svc: QuestService):
    # mutmut_15: guard `return False` -> `return True`.
    assert svc._is_quest_cooling(
        {"last_completed_at": None, "cooldown_hours": None}
    ) is False


def test_is_quest_cooling_multiplies_hours_to_seconds(
    svc: QuestService, monkeypatch
):
    # mutmut_17: `cd_hours * 3600` -> `cd_hours / 3600`. A quest completed
    # 100s ago with a 1h cooldown is cooling under x3600 but not under /3600.
    monkeypatch.setattr(qs_module, "time", _FakeTime)
    assert svc._is_quest_cooling(
        {"last_completed_at": _FakeTime.NOW - 100, "cooldown_hours": 1}
    ) is True


def test_is_quest_cooling_seconds_factor_exact(svc: QuestService, monkeypatch):
    # mutmut_18: `cd_hours * 3600` -> `* 3601`. With a huge cooldown placed so
    # the expiry straddles "now" only under the off-by-one factor.
    monkeypatch.setattr(qs_module, "time", _FakeTime)
    last = _FakeTime.NOW - 1_000_000 * 3600 - 500_000
    assert svc._is_quest_cooling(
        {"last_completed_at": last, "cooldown_hours": 1_000_000}
    ) is False


def test_is_quest_cooling_strict_greater_than(svc: QuestService, monkeypatch):
    # mutmut_19: `> time.time()` -> `>= time.time()`. Expiry exactly equal to
    # now is NOT cooling.
    monkeypatch.setattr(qs_module, "time", _FakeTime)
    assert svc._is_quest_cooling(
        {"last_completed_at": _FakeTime.NOW - 3600, "cooldown_hours": 1}
    ) is False


# ── _row_to_playlist ───────────────────────────────────────────────────────


def test_row_to_playlist_normalizes_id_in_place(svc: QuestService):
    # mutmut_3/4/5: the `"id" in d` guard is corrupted so normalization is
    # skipped (or applied to a missing key). mutmut_7/8: the assignment target
    # is renamed so d["id"] is left unnormalized.
    d = svc._row_to_playlist({"id": "5"})
    assert d["id"] == 5
    assert isinstance(d["id"], int)


# ── _normalize_expected_reward_markup ──────────────────────────────────────


def test_normalize_expected_reward_markup_zero_reward(svc: QuestService):
    # mutmut_5: `reward_ped <= 0` -> `reward_ped < 0`; a zero reward must
    # produce no markup.
    assert (
        svc._normalize_expected_reward_markup(0, False, 130.0) is None
    )


# ── _expected_reward_total ─────────────────────────────────────────────────


def test_expected_reward_total_zero_completions(svc: QuestService):
    # mutmut_3: `return 0` -> `return 1` in the completions<=0 guard.
    assert svc._expected_reward_total(5.0, False, 130.0, 0) == 0


def test_expected_reward_total_skill_ignores_markup(svc: QuestService):
    # mutmut_5: `reward_is_skill and reward_ped <= 0 or ...`. A skill reward
    # must use the flat reward_ped * completions, never the markup formula.
    assert svc._expected_reward_total(5.0, True, 130.0, 2) == 10.0


def test_expected_reward_total_none_markup_uses_flat(svc: QuestService):
    # mutmut_4: `... or reward_ped <= 0 and expected_markup is None`. With a
    # positive reward and None markup the original returns the flat total; the
    # mutant would reach `reward_ped * (None / 100) * completions` and raise.
    assert svc._expected_reward_total(5.0, False, None, 2) == 10.0


def test_expected_reward_total_fractional_reward_uses_markup(svc: QuestService):
    # mutmut_7: `reward_ped <= 0` -> `reward_ped <= 1`; a 0.5 PED reward with
    # markup must use the markup formula, not the flat path.
    assert svc._expected_reward_total(0.5, False, 130.0, 2) == pytest.approx(1.3)


def test_expected_reward_total_flat_multiplies(svc: QuestService):
    # mutmut_9: `reward_ped * completions` -> `reward_ped / completions` in
    # the skill/no-markup branch.
    assert svc._expected_reward_total(6.0, True, None, 2) == 12.0


def test_expected_reward_total_markup_multiplies(svc: QuestService):
    # mutmut_10: `... * completions` -> `... / completions` in the markup branch.
    assert svc._expected_reward_total(10.0, False, 200.0, 2) == 40.0


# ── _get_quest_mobs ────────────────────────────────────────────────────────


def test_get_quest_mobs_filters_empty(svc: QuestService):
    # mutmut_17: `isinstance(mob_name, str) and mob_name` -> `... or ...`; an
    # empty mob name must be filtered out (truthiness check), so it is dropped.
    q = svc.create_quest({"name": "Q"})
    svc._conn.execute(
        "INSERT INTO quest_mobs (quest_id, mob_name) VALUES (?, ?)", (q["id"], "")
    )
    svc._conn.commit()
    assert svc._get_quest_mobs(q["id"]) == []


# ── _get_playlist_items ────────────────────────────────────────────────────


def test_get_playlist_items_description_key_and_value(svc: QuestService):
    # mutmut_9/10: the "description" key is renamed in the projected dict.
    # mutmut_11: `"description": r[1]` -> `r[2]` (uses group_type as the value).
    q = svc.create_quest({"name": "Q1"})
    pl = svc.create_playlist(
        {
            "name": "PL",
            "items": [
                {
                    "quest_id": q["id"],
                    "description": "DESCVAL",
                    "group_type": "immediate",
                }
            ],
        }
    )
    items = svc._get_playlist_items(pl["id"])
    assert len(items) == 1
    assert items[0]["description"] == "DESCVAL"
    assert items[0]["group_type"] == "immediate"
