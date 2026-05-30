"""Property-based tests for the settings configuration service.

Covers ``backend.services.config_service.ConfigService``: the normalisation,
update, clone, and persistence surface. Each property generates a valid update
payload (the shape the settings PATCH router is typed to deliver: string hotbar
slot keys mapping to ``int | None``, and trifecta presets as JSON-style dicts),
applies it through the public API over a throwaway data directory, and asserts
a structural invariant of the resulting config.

The service is not event-driven: the tracker, reducers, parser, and event bus
only ever read this config, never mutate its structure. So spanning the update
payload space exercises these invariants far more thoroughly than replaying a
fixed gameplay script would.
"""

from __future__ import annotations

import tempfile
from dataclasses import asdict
from pathlib import Path

from hypothesis import given
from hypothesis import strategies as st

from backend.services.config_service import (
    HOTBAR_SLOTS,
    ConfigService,
    active_trifecta_preset,
)

# A hotbar update: a partial map over the canonical slot keys. The PATCH router
# is typed ``dict[str, int | None]``, so values are an item id or an empty slot.
_HOTBAR = st.dictionaries(
    keys=st.sampled_from(HOTBAR_SLOTS),
    values=st.one_of(st.none(), st.integers(min_value=1, max_value=9999)),
    max_size=len(HOTBAR_SLOTS),
)

# Preset ids that survive normalisation (non-blank after strip). A pool of
# repeated tokens makes duplicate-id collisions likely, exercising the dedup.
_PRESET_ID = st.sampled_from(["a", "b", "c", "alpha", "beta"])
_PRESET_NAME = st.text(alphabet="AbcXyz ", min_size=0, max_size=8)
_WEAPON_ID = st.one_of(st.none(), st.integers(min_value=1, max_value=9999))


def _preset_dict(preset_id, name, small, big, heal):
    return {
        "id": preset_id,
        "name": name,
        "small_weapon_id": small,
        "big_weapon_id": big,
        "heal_id": heal,
    }


_PRESET = st.builds(
    _preset_dict,
    preset_id=_PRESET_ID,
    name=_PRESET_NAME,
    small=_WEAPON_ID,
    big=_WEAPON_ID,
    heal=_WEAPON_ID,
)
_PRESET_LIST = st.lists(_PRESET, min_size=0, max_size=6)


def _updates_strategy():
    """A partial-update payload mixing the structurally-normalised fields with
    a couple of plain scalar fields and an unknown key the service must ignore."""
    return st.fixed_dictionaries(
        {},
        optional={
            "hotbar": _HOTBAR,
            "trifecta_presets": _PRESET_LIST,
            "active_trifecta_preset_id": st.one_of(
                st.none(), _PRESET_ID, st.just("ghost")
            ),
            "player_name": st.text(alphabet="AbcXyz ", min_size=0, max_size=10),
            "not_a_real_field": st.integers(),
        },
    )


# --- hotbar_full_canonical_shape ---


@given(_HOTBAR)
def test_update_yields_full_canonical_hotbar(hotbar):
    # After any partial hotbar update the stored hotbar has exactly the fixed
    # 1-9,0 slot set; supplied slots keep their value, unsupplied slots are None.
    with tempfile.TemporaryDirectory() as data_dir:
        svc = ConfigService(Path(data_dir))
        result = svc.update({"hotbar": dict(hotbar)})
        assert set(result.hotbar.keys()) == set(HOTBAR_SLOTS)
        for slot in HOTBAR_SLOTS:
            assert result.hotbar[slot] == hotbar.get(slot)


# --- trifecta_ids_unique ---


@given(_PRESET_LIST)
def test_normalised_preset_ids_are_unique(presets):
    # The dedup keeps first occurrence per id, so the surviving preset ids are
    # pairwise distinct regardless of how many collisions the input carried.
    with tempfile.TemporaryDirectory() as data_dir:
        svc = ConfigService(Path(data_dir))
        result = svc.update({"trifecta_presets": presets})
        ids = [preset.id for preset in result.trifecta_presets]
        assert len(ids) == len(set(ids))
        assert result.trifecta_presets  # never empty: falls back to the default


# --- active_preset_id_resolvable ---


@given(_updates_strategy())
def test_active_preset_id_always_resolvable(updates):
    # Whatever the payload (including a dangling active id, an emptied preset
    # list, or no trifecta keys at all) the active id resolves to a real preset.
    with tempfile.TemporaryDirectory() as data_dir:
        svc = ConfigService(Path(data_dir))
        result = svc.update(dict(updates))
        assert active_trifecta_preset(result) is not None
        ids = {preset.id for preset in result.trifecta_presets}
        assert result.active_trifecta_preset_id in ids


# --- clone_does_not_mutate_live_config ---


@given(_updates_strategy())
def test_clone_does_not_mutate_live_config(updates):
    # clone_with_updates is the dry-run path: it must leave the live config (and
    # all its mutable substructures) untouched and object-distinct from the clone.
    with tempfile.TemporaryDirectory() as data_dir:
        svc = ConfigService(Path(data_dir))
        before = asdict(svc.config)
        candidate = svc.clone_with_updates(dict(updates))
        assert asdict(svc.config) == before
        assert candidate.hotbar is not svc.config.hotbar
        assert candidate.trifecta_presets is not svc.config.trifecta_presets
        assert candidate.loot_filter_blacklist is not svc.config.loot_filter_blacklist


# --- load_save_roundtrip_idempotent ---


@given(_updates_strategy())
def test_save_load_roundtrip_is_a_fixed_point(updates):
    # Precondition (per the qualified invariant): the persisted config must have
    # already been normalised by the public API, NOT a hand-authored raw file.
    # Under that precondition, reloading reproduces an equal config and a second
    # save reproduces byte-identical known-key JSON.
    with tempfile.TemporaryDirectory() as data_dir:
        path = Path(data_dir)
        svc = ConfigService(path)
        saved = svc.update(dict(updates))
        saved_snapshot = asdict(saved)

        reloaded = ConfigService(path)
        assert asdict(reloaded.config) == saved_snapshot

        # Re-saving the unchanged config is idempotent on the known-key
        # projection (the merge only carries forward unknown third-party keys).
        before_text = (path / "settings.json").read_text(encoding="utf-8")
        reloaded._save(reloaded.config)
        after_text = (path / "settings.json").read_text(encoding="utf-8")
        assert before_text == after_text
