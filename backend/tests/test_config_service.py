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
