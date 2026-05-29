"""Mutation-hardening tests for backend.services.mob_lookup_service.

Targets the surviving mutants from the EntropiaOrme mutation campaign for the
`search_mob_names` and `has_mob_name` reducers. Each test exercises the exact
mutated line and asserts the behaviour the mutation would break.
"""

from types import SimpleNamespace

from backend.services.mob_lookup_service import MobLookupService


def _game_data(mobs):
    return SimpleNamespace(
        get_entities=lambda kind: mobs if kind == "mobs" else []
    )


# --------------------------------------------------------------------------
# search_mob_names: default limit (mutmut_1: limit=10 -> 11)
# --------------------------------------------------------------------------
def test_search_default_limit_is_ten():
    # 12 distinct species all matching "mob"; default limit must cap at 10,
    # not 11. None start with "mob" so the prefix tier is uniform and the
    # alphabetical sort is deterministic.
    mobs = [{"name": f"Mob{i:02d}", "maturities": []} for i in range(12)]
    svc = MobLookupService(_game_data(mobs))

    out = svc.search_mob_names("mob")
    assert len(out) == 10
    # The first 10 alphabetically: Mob00 .. Mob09.
    assert [r["display"] for r in out] == [f"Mob{i:02d}" for i in range(10)]
    # An explicit limit still works (guards the parameter is actually used).
    assert len(svc.search_mob_names("mob", limit=3)) == 3


# --------------------------------------------------------------------------
# search_mob_names: empty-species fallback (mutmut_24: "" -> "XXXX")
# A mob with neither a species name nor a top-level name must be skipped.
# --------------------------------------------------------------------------
def test_search_skips_mob_with_no_species_or_name():
    mobs = [
        {"species": {}, "maturities": []},  # no species name, no name -> ""
        {"name": "Atrox", "maturities": []},
    ]
    svc = MobLookupService(_game_data(mobs))

    # The blank mob contributes nothing. A search for the fallback token must
    # find no row (mutant would synthesise a "XXXX" species).
    assert svc.search_mob_names("xxxx") == []
    # And only the real mob is returned for its own query.
    assert svc.search_mob_names("atrox") == [
        {"display": "Atrox", "species": "Atrox", "maturity": ""}
    ]


# --------------------------------------------------------------------------
# search_mob_names: blank-species skip must `continue`, not `break`
# (mutmut_26: continue -> break)
# --------------------------------------------------------------------------
def test_search_blank_species_continues_to_later_mobs():
    mobs = [
        {"species": {}, "maturities": []},  # blank species, must be skipped
        {"name": "Atrox", "maturities": []},  # later mob must still be reached
    ]
    svc = MobLookupService(_game_data(mobs))

    # `break` would abort the loop at the blank mob and never see Atrox.
    assert svc.search_mob_names("atrox") == [
        {"display": "Atrox", "species": "Atrox", "maturity": ""}
    ]


# --------------------------------------------------------------------------
# search_mob_names: no-maturity dedup key
# mutmut_33: key=(species,"") -> None  (collapses distinct species)
# mutmut_44: seen.add(key) -> seen.add(None)  (breaks dedup of same species)
# --------------------------------------------------------------------------
def test_search_no_maturity_distinct_species_not_collapsed():
    # Two distinct no-maturity species both matching "a". With key=None both
    # would share one dedup slot and only the first would survive.
    mobs = [
        {"name": "Aaa", "maturities": []},
        {"name": "Abb", "maturities": []},
    ]
    svc = MobLookupService(_game_data(mobs))

    out = svc.search_mob_names("a")
    assert {r["display"] for r in out} == {"Aaa", "Abb"}
    assert len(out) == 2


def test_search_no_maturity_same_species_deduped():
    # Same no-maturity species appears twice; the dedup key must suppress the
    # second. With seen.add(None) the real key never lands in `seen`, so the
    # duplicate would be appended.
    mobs = [
        {"name": "Atrox", "maturities": []},
        {"name": "Atrox", "maturities": []},
    ]
    svc = MobLookupService(_game_data(mobs))

    assert svc.search_mob_names("atrox") == [
        {"display": "Atrox", "species": "Atrox", "maturity": ""}
    ]


# --------------------------------------------------------------------------
# search_mob_names: no-maturity key value collision with empty-maturity entry
# mutmut_34: key=(species,"") -> (species,"XXXX")
# A no-maturity mob and a maturity-with-blank-name mob for the same species
# both project to the bare species display and must dedup to one row.
# --------------------------------------------------------------------------
def test_search_no_maturity_key_dedups_against_blank_maturity():
    mobs = [
        {"name": "Atrox", "maturities": []},  # no-maturity -> key (Atrox,"")
        {"name": "Atrox", "maturities": [{"name": ""}]},  # blank maturity -> (Atrox,"")
    ]
    svc = MobLookupService(_game_data(mobs))

    # Both rows project to display "Atrox" with maturity "". They must collapse
    # to a single result. With key (Atrox,"XXXX") the no-maturity row would not
    # share the maturity row's key, yielding two identical-display rows.
    assert svc.search_mob_names("atrox") == [
        {"display": "Atrox", "species": "Atrox", "maturity": ""}
    ]


# --------------------------------------------------------------------------
# search_mob_names: no-maturity matcher is an OR
# mutmut_40: (q in display) or all(parts) -> and
# A multi-word species whose tokens all appear (out of order) must match via
# the all-parts arm even though the full query is not a contiguous substring.
# --------------------------------------------------------------------------
def test_search_no_maturity_all_parts_matcher_or_arm():
    mobs = [{"name": "Big Bird", "maturities": []}]
    svc = MobLookupService(_game_data(mobs))

    # "bird big" is not a substring of "big bird" but both tokens are present.
    # OR keeps it; the AND mutant would drop it (q-substring arm is False).
    assert svc.search_mob_names("bird big") == [
        {"display": "Big Bird", "species": "Big Bird", "maturity": ""}
    ]


# --------------------------------------------------------------------------
# search_mob_names: no-maturity branch must `continue`, not `break`
# (mutmut_53: continue -> break)
# --------------------------------------------------------------------------
def test_search_no_maturity_continues_to_later_mobs():
    mobs = [
        {"name": "Aaa", "maturities": []},  # no-maturity branch taken
        {"name": "Abb", "maturities": []},  # later mob must still be reached
    ]
    svc = MobLookupService(_game_data(mobs))

    out = svc.search_mob_names("a")
    # `break` would stop after the first no-maturity mob, dropping Abb.
    assert {r["display"] for r in out} == {"Aaa", "Abb"}


# --------------------------------------------------------------------------
# search_mob_names: maturity name fallback (mutmut_59: "" -> "XXXX")
# A maturity entry with no name yields the bare species display, not "XXXX X".
# --------------------------------------------------------------------------
def test_search_maturity_blank_name_yields_bare_species():
    mobs = [{"name": "Atrox", "maturities": [{"name": None}]}]
    svc = MobLookupService(_game_data(mobs))

    # Blank maturity -> maturity "" -> display "Atrox" (no leading token).
    assert svc.search_mob_names("atrox") == [
        {"display": "Atrox", "species": "Atrox", "maturity": ""}
    ]
    # The mutant would build display "XXXX Atrox"; assert that is absent.
    assert svc.search_mob_names("xxxx") == []


# --------------------------------------------------------------------------
# search_mob_names: maturity skip condition De Morgan
# mutmut_66: (q not in display) and (not all parts) -> or
# Multi-word query whose tokens all appear out of order must be kept.
# --------------------------------------------------------------------------
def test_search_maturity_all_parts_matcher_keeps_row():
    mobs = [{"species": {"name": "Atrox"}, "maturities": [{"name": "Young"}]}]
    svc = MobLookupService(_game_data(mobs))

    # display "Young Atrox": query "atrox young" is not a contiguous substring
    # but both tokens appear. Original keeps (skip-condition is False); the OR
    # mutant would skip it because the q-substring arm is True (q not in).
    assert svc.search_mob_names("atrox young") == [
        {"display": "Young Atrox", "species": "Atrox", "maturity": "Young"}
    ]


# --------------------------------------------------------------------------
# search_mob_names: maturity skip must `continue`, not `break`
# (mutmut_71: continue -> break)
# A non-matching maturity must not abort scanning the remaining maturities.
# --------------------------------------------------------------------------
def test_search_maturity_skip_continues_to_later_maturities():
    mobs = [
        {
            "species": {"name": "Atrox"},
            "maturities": [{"name": "Old"}, {"name": "Young"}],
        }
    ]
    svc = MobLookupService(_game_data(mobs))

    # Query "young atrox": "Old Atrox" fails the all-parts test (no "young")
    # and is skipped; "Young Atrox" must still be reached. `break` would abort
    # the maturity loop after the first miss and return nothing.
    assert svc.search_mob_names("young atrox") == [
        {"display": "Young Atrox", "species": "Atrox", "maturity": "Young"}
    ]


# --------------------------------------------------------------------------
# search_mob_names: maturity dedup add (mutmut_72: seen.add(key) -> add(None))
# The same species/maturity appearing in two mobs must dedup to one row.
# --------------------------------------------------------------------------
def test_search_maturity_same_pair_deduped():
    mobs = [
        {"species": {"name": "Atrox"}, "maturities": [{"name": "Young"}]},
        {"species": {"name": "Atrox"}, "maturities": [{"name": "Young"}]},
    ]
    svc = MobLookupService(_game_data(mobs))

    # With seen.add(None) the (Atrox, Young) key never enters `seen`, so the
    # second copy would be appended.
    assert svc.search_mob_names("young atrox") == [
        {"display": "Young Atrox", "species": "Atrox", "maturity": "Young"}
    ]


# --------------------------------------------------------------------------
# search_mob_names: prefix-priority sort
# mutmut_82: (0 if startswith else 1) -> (1 if startswith else 1)
# mutmut_84: display.lower().startswith(q) -> .upper().startswith(q)
# A prefix match must sort ahead of a non-prefix match even when the non-prefix
# match is alphabetically earlier.
# --------------------------------------------------------------------------
def test_search_prefix_match_sorts_before_alphabetical():
    mobs = [
        # "Armax" contains "rax"? no. Use a substring-but-not-prefix match that
        # is alphabetically earlier than the prefix match.
        {"name": "Aakra", "maturities": []},   # contains "akr", not a prefix of "kr"
        {"name": "Kraax", "maturities": []},   # starts with "kra" -> prefix match
    ]
    svc = MobLookupService(_game_data(mobs))

    out = svc.search_mob_names("kra")
    # Both contain "kra". "Aakra" sorts first alphabetically, but "Kraax" is a
    # prefix match and must come first. The mutants flatten the prefix tier to
    # a constant, so the order would become pure-alphabetical (Aakra, Kraax).
    assert [r["display"] for r in out] == ["Kraax", "Aakra"]


# --------------------------------------------------------------------------
# has_mob_name: cached-species fallback (mutmut_21: "" -> "XXXX")
# A mob with no species/name must not be matchable as species "XXXX".
# --------------------------------------------------------------------------
def test_has_mob_name_blank_mob_not_matched_as_fallback():
    mobs = [{"species": {}, "maturities": []}]  # cached_species -> ""
    svc = MobLookupService(_game_data(mobs))

    # Original: cached_species "" != "XXXX" -> skip -> False.
    # Mutant: cached_species "XXXX" == "XXXX" -> no-maturity branch -> True.
    assert svc.has_mob_name("XXXX", "") is False


# --------------------------------------------------------------------------
# has_mob_name: maturity-name fallback (mutmut_36: "" -> "XXXX")
# A maturity entry with no name represents the empty maturity and must match
# an empty-maturity query.
# --------------------------------------------------------------------------
def test_has_mob_name_blank_maturity_entry_matches_empty():
    mobs = [{"species": {"name": "Atrox"}, "maturities": [{"name": None}]}]
    svc = MobLookupService(_game_data(mobs))

    # Original: ("" ).strip() == "" -> True.
    # Mutant: "XXXX".strip() == "" -> False -> falls through to return False.
    assert svc.has_mob_name("Atrox", "") is True
