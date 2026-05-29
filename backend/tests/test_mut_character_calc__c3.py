"""Mutation-hardening tests for character_calc.codex_tier_progress.

Targets cluster character_calc__c3: surviving mutants on codex_tier_progress.

  _6  : `if divisor == 0` -> `if divisor == 1`   (guard constant)  -> EQUIVALENT
  _7  : `return 0.0`      -> `return 1.0`         (dead branch)     -> EQUIVALENT
  _14 : `round(x, 4)`     -> `round(x, 5)`        (precision)       -> KILLED here

REWARD_DIVISORS only ever maps a codex category to {200, 320, 640, 1000};
no public path yields a divisor of 0 or 1, so the `divisor == 0` guard branch
is unreachable with real data and _6/_7 are behaviourally equivalent. _14 is
killed below by choosing inputs whose quotient differs between 4- and 5-decimal
rounding.
"""

from __future__ import annotations

from backend.services.character_calc import codex_tier_progress


def test_progress_is_rounded_to_four_decimals_cat2() -> None:
    # Clubs is a cat2 skill -> divisor 320.
    # (0.1 % 320) / 320 = 0.0003125
    #   round(.., 4) == 0.0003   (original)
    #   round(.., 5) == 0.00031  (mutant _14: round precision 5)
    result = codex_tier_progress("Clubs", 0.1)
    assert result == 0.0003
    # Guard against the 5-decimal mutant explicitly.
    assert result != 0.00031


def test_progress_is_rounded_to_four_decimals_cat3() -> None:
    # Telepathy is a cat3 skill -> divisor 640.
    # (0.1 % 640) / 640 = 0.00015625
    #   round(.., 4) == 0.0002   (original)
    #   round(.., 5) == 0.00016  (mutant _14)
    result = codex_tier_progress("Telepathy", 0.1)
    assert result == 0.0002
    assert result != 0.00016


def test_normal_midtier_progress_value() -> None:
    # Aim is cat1 -> divisor 200. (250 % 200) / 200 = 50/200 = 0.25
    assert codex_tier_progress("Aim", 250.0) == 0.25


def test_returns_none_for_non_codex_skill() -> None:
    assert codex_tier_progress("Definitely Not A Codex Skill", 5.0) is None
