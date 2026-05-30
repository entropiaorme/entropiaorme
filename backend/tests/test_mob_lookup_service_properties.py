"""Property-based tests for the mob-name lookup service.

Covers ``backend.services.mob_lookup_service.MobLookupService``: the
autocomplete search (``search_mob_names``) and the exact-pair validator
(``has_mob_name``). Both are pure synchronous reads over the bundled mobs
catalogue, so the catalogue is supplied directly as a lightweight stub.
"""

from types import SimpleNamespace

from hypothesis import given
from hypothesis import strategies as st

from backend.services.mob_lookup_service import MobLookupService

# Catalogue tokens. Mixed case and overlap let the search/match predicates
# exercise both the substring branch and the per-token branch.
_NAMES = st.text(
    alphabet="AbcXyz ",
    min_size=0,
    max_size=8,
)
_MATURITY_NAMES = st.text(alphabet="AbcYoung", min_size=0, max_size=6)


def _game_data(mobs):
    return SimpleNamespace(get_entities=lambda kind: mobs if kind == "mobs" else [])


def _mob(species_name, maturity_names):
    return {
        "species": {"name": species_name},
        "maturities": [{"name": m} for m in maturity_names],
    }


# A generated catalogue entry: a species (sometimes nested, sometimes bare
# ``name``) with zero or more maturities, mirroring the two shapes the real
# snapshot uses.
_MOB = st.builds(
    _mob,
    species_name=_NAMES,
    maturity_names=st.lists(_MATURITY_NAMES, min_size=0, max_size=4),
)
_CATALOGUE = st.lists(_MOB, min_size=0, max_size=8)
_QUERY = st.text(alphabet="AbcXyz Young", min_size=0, max_size=8)


def _matches(query, display):
    """Replicate the per-row match gate the service applies before append."""
    q = query.strip().lower()
    q_parts = [part for part in q.split() if part]
    display_lower = display.lower()
    return q in display_lower or all(part in display_lower for part in q_parts)


# --- search_mob_names ---


@given(_CATALOGUE, _QUERY, st.integers(min_value=1, max_value=20))
def test_results_bounded_by_limit(catalogue, query, limit):
    svc = MobLookupService(_game_data(catalogue))
    results = svc.search_mob_names(query, limit=limit)
    assert len(results) <= limit


@given(_CATALOGUE, st.text(alphabet=" \t\n", min_size=0, max_size=5))
def test_empty_query_yields_empty(catalogue, blank):
    # Precondition: query.strip() == "". Whitespace-only (or truly empty)
    # queries short-circuit before the catalogue is ever consulted.
    svc = MobLookupService(_game_data(catalogue))
    assert svc.search_mob_names(blank) == []


@given(_CATALOGUE, _QUERY)
def test_every_result_matches_query(catalogue, query):
    svc = MobLookupService(_game_data(catalogue))
    results = svc.search_mob_names(query)
    for row in results:
        assert _matches(query, row["display"])


@given(_CATALOGUE, _QUERY)
def test_display_is_canonical_join(catalogue, query):
    svc = MobLookupService(_game_data(catalogue))
    for row in svc.search_mob_names(query):
        species = row["species"]
        maturity = row["maturity"]
        expected = f"{maturity} {species}" if maturity else species
        assert row["display"] == expected


@given(_CATALOGUE, _QUERY)
def test_leading_trailing_whitespace_is_ignored_in_search(catalogue, query):
    # needs-qualification invariant: strip() removes only LEADING/TRAILING
    # whitespace, so padded queries behave identically to trimmed ones.
    # Internal whitespace is NOT normalised and is deliberately excluded here.
    svc = MobLookupService(_game_data(catalogue))
    trimmed = svc.search_mob_names(query)
    padded = svc.search_mob_names(f"  \t{query}\n ")
    assert trimmed == padded


# --- has_mob_name ---


@given(
    _CATALOGUE,
    st.text(alphabet="AbcYoung \t", min_size=0, max_size=6),
    st.text(alphabet=" \t\n", min_size=0, max_size=4),
)
def test_has_mob_name_empty_species_false(catalogue, maturity, blank_species):
    # An empty (or whitespace-only) species short-circuits to False before any
    # catalogue iteration, for any maturity argument.
    svc = MobLookupService(_game_data(catalogue))
    assert svc.has_mob_name(blank_species, maturity) is False


@given(
    _CATALOGUE,
    st.text(alphabet="AbcXyz", min_size=1, max_size=6),
    st.text(alphabet="AbcYoung", min_size=0, max_size=6),
)
def test_has_mob_name_leading_trailing_whitespace_ignored(catalogue, species, maturity):
    # Padded species/maturity compare equal to their trimmed forms; only
    # leading/trailing padding is generated (internal whitespace is preserved
    # by strip() and is out of scope for this invariant).
    svc = MobLookupService(_game_data(catalogue))
    plain = svc.has_mob_name(species, maturity)
    padded = svc.has_mob_name(f"  {species} ", f"\t{maturity}  ")
    assert plain == padded
