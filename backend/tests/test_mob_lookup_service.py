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

    # Pin the full projected result set, fields, dedup, and prefix-priority sort:
    # neither display starts with "atrox", so both fall to the alphabetical tier.
    atrox = svc.search_mob_names("atrox")
    assert atrox == [
        {"display": "Old Atrox", "species": "Atrox", "maturity": "Old"},
        {"display": "Young Atrox", "species": "Atrox", "maturity": "Young"},
    ]

    berycled = svc.search_mob_names("berycled")
    assert berycled == [{"display": "Berycled", "species": "Berycled", "maturity": ""}]

    # Multi-token query exercises the all-parts matcher: "Old Atrox" is rejected
    # because "young" is absent, leaving only the Young Atrox row.
    young_atrox = svc.search_mob_names("young atrox")
    assert young_atrox == [
        {"display": "Young Atrox", "species": "Atrox", "maturity": "Young"}
    ]


def test_search_empty_query_returns_nothing():
    svc = MobLookupService(_game_data([]))
    assert svc.search_mob_names("   ") == []


def test_has_mob_name_checks_exact_pair():
    mobs = [{"species": {"name": "Atrox"}, "maturities": [{"name": "Young"}]}]
    svc = MobLookupService(_game_data(mobs))

    assert svc.has_mob_name("Atrox", "Young") is True
    assert svc.has_mob_name("Nonexistent", "Young") is False


def test_has_mob_name_branches():
    mobs = [
        {
            "species": {"name": "Atrox"},
            "maturities": [{"name": "Young"}, {"name": "Old"}],
        },
        {"name": "Berycled", "maturities": []},  # no maturities -> bare species
    ]
    svc = MobLookupService(_game_data(mobs))

    assert svc.has_mob_name("", "Young") is False  # empty species short-circuits
    assert svc.has_mob_name("Atrox", "Young") is True  # exact maturity match
    assert svc.has_mob_name("Atrox", "Ancient") is False  # maturities present, no match
    assert (
        svc.has_mob_name("Berycled", "") is True
    )  # no-maturity species, empty maturity
    assert svc.has_mob_name("Berycled", "Young") is False  # no-maturity species, named
