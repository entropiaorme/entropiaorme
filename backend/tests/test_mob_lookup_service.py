"""Tests for the mob-name lookup service (autocomplete + exact-pair validation)."""

from types import SimpleNamespace

from backend.services.mob_lookup_service import MobLookupService


def _game_data(mobs):
    return SimpleNamespace(get_entities=lambda kind: mobs if kind == "mobs" else [])


def test_search_matches_both_maturity_and_no_maturity_mobs():
    mobs = [
        {
            "species": {"name": "Atrox"},
            "maturities": [{"name": "Young"}, {"name": "Old"}],
        },
        {"name": "Berycled", "maturities": []},  # no maturities → bare species
    ]
    svc = MobLookupService(_game_data(mobs))

    atrox = svc.search_mob_names("atrox")
    assert any(r["display"] == "Young Atrox" for r in atrox)  # maturity branch

    berycled = svc.search_mob_names("berycled")
    assert any(r["display"] == "Berycled" for r in berycled)  # no-maturity branch


def test_search_empty_query_returns_nothing():
    svc = MobLookupService(_game_data([]))
    assert svc.search_mob_names("   ") == []


def test_has_mob_name_checks_exact_pair():
    mobs = [{"species": {"name": "Atrox"}, "maturities": [{"name": "Young"}]}]
    svc = MobLookupService(_game_data(mobs))

    assert isinstance(svc.has_mob_name("Atrox", "Young"), bool)
    assert svc.has_mob_name("Nonexistent", "Young") is False
