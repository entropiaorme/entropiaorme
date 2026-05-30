"""Property tests for the damage-based weapon attributor.

These complement the worked examples in ``test_tool_inference``: each property
below pins one structural guarantee of ``DamageAttributor`` over generated
inputs rather than a single hand-picked case.

The attributor maps an observed damage amount back to one of the configured
trifecta weapons by checking which weapons' damage bands contain the amount and
picking the narrowest band (ties broken by name). Critical hits widen each band
by the ``[2.0, 3.0]`` multiplier, with a preference for a known big-weapon
regular hit when a small weapon could also have crit.
"""

from __future__ import annotations

from hypothesis import given
from hypothesis import strategies as st

from backend.tracking.tool_inference import (
    DamageAttribution,
    DamageAttributor,
)

# Finite, positive damage figures: the only values the parser feeds in.
_damage = st.floats(min_value=0.1, max_value=1e4, allow_nan=False, allow_infinity=False)
# Names are dict keys for the profile store; keep them distinct and simple.
_name = st.text(
    alphabet=st.characters(min_codepoint=65, max_codepoint=90), min_size=1, max_size=6
)
_cost = st.floats(min_value=0.0, max_value=1e3, allow_nan=False, allow_infinity=False)


def _profile_dict(draw, name: str) -> dict:
    low = draw(_damage)
    span = draw(st.floats(min_value=0.0, max_value=1e4))
    return {
        "name": name,
        "min_damage": low,
        "max_damage": low + span,
        "cost_per_shot": draw(_cost),
    }


@st.composite
def _profiles(draw) -> list[dict]:
    """Generate one to three weapon profiles with distinct names."""
    names = draw(st.lists(_name, min_size=1, max_size=3, unique=True))
    return [_profile_dict(draw, name) for name in names]


def _build(profiles: list[dict]) -> DamageAttributor:
    attributor = DamageAttributor()
    for profile in profiles:
        attributor.add_weapon_profile(**profile)
    return attributor


# --- non_positive_or_empty_returns_none ---------------------------------------


@given(
    amount=st.floats(max_value=0.0, allow_nan=False, allow_infinity=False),
    profiles=_profiles(),
    critical=st.booleans(),
)
def test_non_positive_amount_returns_none(
    amount: float, profiles: list[dict], critical: bool
) -> None:
    # The amount guard runs before any critical-flag branching, so a
    # non-positive amount yields None regardless of profiles or the flag.
    attributor = _build(profiles)
    assert attributor.match_damage(amount, critical=critical) is None


@given(
    amount=_damage,
    critical=st.booleans(),
)
def test_empty_profiles_return_none(amount: float, critical: bool) -> None:
    # With no profiles configured, every amount short-circuits to None.
    assert DamageAttributor().match_damage(amount, critical=critical) is None


# --- cost_and_name_come_from_selected_profile ---------------------------------


@given(
    profiles=_profiles(),
    amount=_damage,
    critical=st.booleans(),
)
def test_attribution_fields_belong_to_one_configured_profile(
    profiles: list[dict], amount: float, critical: bool
) -> None:
    # A returned attribution's name and cost are a matched pair lifted from a
    # single configured profile: no cross-profile field leak.
    attributor = _build(profiles)
    result = attributor.match_damage(amount, critical=critical)
    if result is not None:
        by_name = {p["name"]: p for p in profiles}
        assert result.tool_name in by_name
        assert result.cost_per_shot == by_name[result.tool_name]["cost_per_shot"]


# --- big_weapon_preference_under_small_crit_ambiguity -------------------------


@given(
    # Small weapon: regular [s_min, s_max]; crit band [s_min*2, s_max*3].
    s_min=st.floats(min_value=1.0, max_value=50.0),
    s_span=st.floats(min_value=0.0, max_value=50.0),
    # Big weapon: regular band [b_min, b_max].
    b_min=st.floats(min_value=1.0, max_value=50.0),
    b_span=st.floats(min_value=0.0, max_value=200.0),
    pick=st.floats(min_value=0.0, max_value=1.0),
)
def test_big_weapon_regular_wins_over_small_weapon_crit(
    s_min: float, s_span: float, b_min: float, b_span: float, pick: float
) -> None:
    # When a critical-hit amount sits in a big weapon's regular band AND a
    # small weapon could also have crit-matched it, attribution routes to the
    # narrowest big-weapon regular match (the known big-weapon shot is
    # preferred over the small weapon's wider crit possibility).
    s_max = s_min + s_span
    b_max = b_min + b_span
    # Overlap of the small crit band [s_min*2, s_max*3] with the big regular
    # band [b_min, b_max]. Only run the property when such an overlap exists.
    lo = max(s_min * 2.0, b_min)
    hi = min(s_max * 3.0, b_max)
    if lo > hi:
        return
    amount = lo + pick * (hi - lo)

    attributor = _build(
        [
            {
                "name": "Big",
                "min_damage": b_min,
                "max_damage": b_max,
                "cost_per_shot": 5.0,
                "role": "big_weapon",
            },
            {
                "name": "Small",
                "min_damage": s_min,
                "max_damage": s_max,
                "cost_per_shot": 1.0,
                "role": "small_weapon",
            },
        ]
    )
    result = attributor.match_damage(amount, critical=True)
    assert result is not None
    assert result.tool_name == "Big"


# --- base_damage_fallback_never_zero_when_max_positive ------------------------


@given(
    name=_name,
    min_damage=st.floats(
        min_value=0.0, max_value=1e4, allow_nan=False, allow_infinity=False
    ),
    max_span=st.floats(min_value=0.1, max_value=1e4),
)
def test_base_damage_falls_back_to_positive_max(
    name: str, min_damage: float, max_span: float
) -> None:
    # base_damage omitted (defaults to 0.0) with a positive max_damage: the
    # `base_damage or max_damage` fallback stores the positive max, never 0.0.
    max_damage = min_damage + max_span  # strictly positive
    attributor = DamageAttributor()
    attributor.add_weapon_profile(
        name=name, min_damage=min_damage, max_damage=max_damage
    )
    stored = attributor._profiles[name]
    assert stored.base_damage == max_damage
    assert stored.base_damage != 0.0


# --- clear_empties_all_profiles -----------------------------------------------


@given(
    profiles=_profiles(),
    amount=_damage,
    critical=st.booleans(),
)
def test_clear_drops_every_profile(
    profiles: list[dict], amount: float, critical: bool
) -> None:
    # After clear(), no profile remains and every amount attributes to None
    # until a profile is re-added.
    attributor = _build(profiles)
    attributor.clear()
    assert attributor._profiles == {}
    assert attributor.match_damage(amount, critical=critical) is None

    re_added = profiles[0]
    attributor.add_weapon_profile(**re_added)
    mid = (re_added["min_damage"] + re_added["max_damage"]) / 2.0
    assert attributor.match_damage(mid) == DamageAttribution(
        tool_name=re_added["name"], cost_per_shot=re_added["cost_per_shot"]
    )
