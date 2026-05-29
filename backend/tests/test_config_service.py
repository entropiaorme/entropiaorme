"""Tests for the config service's update/normalisation/save paths.

Exercises ``update`` (which applies + atomically saves) over a throwaway config
directory, covering hotbar normalisation, trifecta-preset defaulting, and the
backup-on-overwrite save branch.
"""

from pathlib import Path

from backend.services.config_service import ConfigService


def test_update_normalises_hotbar_and_defaults_trifecta_presets(tmp_path: Path):
    svc = ConfigService(tmp_path)

    # First update creates the on-disk config (no .bak yet).
    svc.update({"hotbar": {"1": 5}})
    # Second update overwrites it (exercises the backup branch) and normalises an
    # emptied trifecta-preset list back to the default preset.
    result = svc.update({"trifecta_presets": []})

    assert result.trifecta_presets  # emptied list normalised to the default preset
    assert result.hotbar  # filled to the full slot shape
    assert (tmp_path / "settings.bak").exists()  # the overwrite kept a backup


def test_get_returns_the_live_config(tmp_path: Path):
    svc = ConfigService(tmp_path)
    assert svc.get() is svc.config


def test_corrupt_config_falls_back_to_defaults(tmp_path: Path):
    """An unparseable settings file is replaced by a freshly-saved default."""
    (tmp_path / "settings.json").write_text("{ this is not json", encoding="utf-8")
    svc = ConfigService(tmp_path)
    assert svc.get().mob_tracking_mode == "mob"  # default config materialised
    assert (tmp_path / "settings.json").exists()  # default re-saved over corruption


def test_trifecta_preset_normalisation_skips_and_dedupes(tmp_path: Path):
    """Preset entries with no id, wrong type, or duplicate ids are dropped."""
    import json

    (tmp_path / "settings.json").write_text(
        json.dumps(
            {
                "trifecta_presets": [
                    {"name": "NoId"},  # missing id -> skipped
                    "not-a-dict",  # wrong type -> skipped
                    {"id": "dup", "name": "First"},
                    {"id": "dup", "name": "Second"},  # duplicate id -> skipped
                ],
                "active_trifecta_preset_id": "dup",
            }
        ),
        encoding="utf-8",
    )
    svc = ConfigService(tmp_path)
    presets = svc.get().trifecta_presets
    assert [p.id for p in presets] == ["dup"]
    assert presets[0].name == "First"
    assert svc.get().active_trifecta_preset_id == "dup"


def test_unknown_update_keys_are_ignored(tmp_path: Path):
    """clone_with_updates skips attributes the config does not define."""
    svc = ConfigService(tmp_path)
    candidate = svc.clone_with_updates({"not_a_real_field": 123, "player_name": "Mae"})
    assert candidate.player_name == "Mae"
    assert not hasattr(candidate, "not_a_real_field")


def test_reset_restores_defaults(tmp_path: Path):
    """reset() discards a prior update and re-saves the default config."""
    svc = ConfigService(tmp_path)
    svc.update({"player_name": "Mae"})
    svc.reset()
    assert svc.get().player_name == ""
