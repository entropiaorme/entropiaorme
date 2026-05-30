"""Mutation-hardening tests for backend.tracking.tool_inference.

Targets the surviving mutants in the tool_inference cluster of the
EntropiaOrme mutation campaign. Each test exercises the exact mutated
line and asserts the behaviour the mutation breaks.

Mutants addressed here by a focused kill:
  * match_damage__mutmut_2  (``amount <= 0 or not self._profiles`` -> ``and``)
  * match_damage__mutmut_3  (``amount <= 0`` -> ``amount < 0``)
  * _bounds__mutmut_1       (critical low ``min_damage * 2`` -> ``min_damage / 2``)

The remaining four mutants in the cluster are recorded as equivalents (see the
triage notes / campaign record); they are not killable through any public API:
  * add_weapon_profile__mutmut_1 (base_damage default 0.0 -> 1.0) and
    add_weapon_profile__mutmut_2 (cost_per_shot default 0.0 -> 1.0) and
    match_damage__mutmut_1       (critical default False -> True)
    are dead under mutmut's dispatcher trampoline: the public method binds the
    caller's arguments against the *unmutated* defaults and forwards every
    parameter explicitly, so the mutated default value is never reached.
  * match_damage__mutmut_8 (``_matches_for(amount, critical=False)`` ->
    ``critical=None``): ``critical`` is consumed only by ``_bounds`` via the
    truthiness test ``if critical:``; ``None`` and ``False`` are both falsy and
    select the identical (non-critical) bounds branch.
"""

from __future__ import annotations

from backend.tracking.tool_inference import DamageAttribution, DamageAttributor


def test_zero_amount_returns_none_even_when_a_band_covers_zero() -> None:
    """Kill match_damage__mutmut_2 and match_damage__mutmut_3.

    With a profile whose band starts at 0.0, an amount of exactly 0.0 is a
    non-positive hit. The guard ``amount <= 0 or not self._profiles`` must
    short-circuit to ``None``:

      * ``or`` -> ``and`` (mutmut_2): ``True and not profiles`` is ``False`` with
        profiles present, so the guard would fall through and the [0, 10] band
        would match 0.0, returning an attribution instead of None.
      * ``<=`` -> ``<`` (mutmut_3): ``0 < 0`` is ``False``, so the guard would
        fall through and 0.0 would match the band, returning an attribution.

    The real code returns None for both; the mutants would not.
    """
    attributor = DamageAttributor()
    attributor.add_weapon_profile(
        name="Zero", min_damage=0.0, max_damage=10.0, cost_per_shot=3.0
    )
    assert attributor.match_damage(0.0) is None
    assert attributor.match_damage(0.0, critical=True) is None


def test_negative_amount_returns_none_even_when_a_band_covers_it() -> None:
    """Reinforce match_damage__mutmut_2.

    A negative amount with profiles present and a band that spans the negative
    value must still return None. Under ``or`` -> ``and`` the guard would fall
    through (profiles are present) and the [-5, 10] band would match -2.0.
    """
    attributor = DamageAttributor()
    attributor.add_weapon_profile(
        name="Neg", min_damage=-5.0, max_damage=10.0, cost_per_shot=2.0
    )
    assert attributor.match_damage(-2.0) is None


def test_critical_low_bound_uses_multiplication_not_division() -> None:
    """Kill _bounds__mutmut_1 (critical low ``min_damage * 2`` -> ``/ 2``).

    For min=10, max=20 the critical band is [min*2, max*3] = [20, 60]. An amount
    of 10.0 sits below the correct critical low bound (20), so a critical match
    must return None. The mutant computes the low bound as ``min / 2`` = 5,
    widening the band to [5, 60], which would match 10.0 and return an
    attribution.
    """
    attributor = DamageAttributor()
    attributor.add_weapon_profile(
        name="Rifle", min_damage=10.0, max_damage=20.0, cost_per_shot=1.5
    )

    # Just below the correct critical low bound (min * 2 == 20): no crit match.
    assert attributor.match_damage(10.0, critical=True) is None

    # Sanity: at/above the correct low bound it does match, and a regular
    # (non-critical) hit inside [10, 20] is unaffected.
    crit = attributor.match_damage(20.0, critical=True)
    assert crit == DamageAttribution(tool_name="Rifle", cost_per_shot=1.5)
    assert attributor.match_damage(15.0, critical=False) == DamageAttribution(
        tool_name="Rifle", cost_per_shot=1.5
    )
