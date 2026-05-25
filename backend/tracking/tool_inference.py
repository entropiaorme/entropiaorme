"""Damage-based weapon attribution for configured trifecta profiles."""

from __future__ import annotations

from dataclasses import dataclass

CRITICAL_DAMAGE_MIN = 2.0
CRITICAL_DAMAGE_MAX = 3.0


@dataclass(frozen=True)
class DamageAttribution:
    tool_name: str
    cost_per_shot: float


@dataclass(frozen=True)
class _WeaponDamageProfile:
    name: str
    min_damage: float
    max_damage: float
    base_damage: float
    cost_per_shot: float
    role: str | None = None


@dataclass(frozen=True)
class _DamageMatch:
    profile: _WeaponDamageProfile
    low: float
    high: float

    @property
    def width(self) -> float:
        return self.high - self.low


class DamageAttributor:
    """Attribute combat damage against the two configured trifecta weapons."""

    def __init__(self):
        self._profiles: dict[str, _WeaponDamageProfile] = {}

    def clear(self) -> None:
        self._profiles.clear()

    def add_weapon_profile(
        self,
        *,
        name: str,
        min_damage: float,
        max_damage: float,
        base_damage: float = 0.0,
        cost_per_shot: float = 0.0,
        role: str | None = None,
    ) -> None:
        self._profiles[name] = _WeaponDamageProfile(
            name=name,
            min_damage=min_damage,
            max_damage=max_damage,
            base_damage=base_damage or max_damage,
            cost_per_shot=cost_per_shot,
            role=role,
        )

    def match_damage(
        self,
        amount: float,
        *,
        critical: bool = False,
    ) -> DamageAttribution | None:
        if amount <= 0 or not self._profiles:
            return None

        regular_matches = self._matches_for(amount, critical=False)
        critical_matches = self._matches_for(amount, critical=True)

        if critical:
            selected = self._prefer_known_crit_pattern(
                regular_matches,
                critical_matches,
            ) or self._narrowest(critical_matches)
        else:
            selected = self._narrowest(regular_matches)

        if selected is None:
            return None
        return DamageAttribution(
            tool_name=selected.profile.name,
            cost_per_shot=selected.profile.cost_per_shot,
        )

    def _matches_for(self, amount: float, *, critical: bool) -> list[_DamageMatch]:
        matches = []
        for profile in self._profiles.values():
            low, high = self._bounds(profile, critical=critical)
            if low <= amount <= high:
                matches.append(_DamageMatch(profile=profile, low=low, high=high))
        return matches

    def _bounds(
        self,
        profile: _WeaponDamageProfile,
        *,
        critical: bool,
    ) -> tuple[float, float]:
        if critical:
            return (
                profile.min_damage * CRITICAL_DAMAGE_MIN,
                profile.max_damage * CRITICAL_DAMAGE_MAX,
            )
        return profile.min_damage, profile.max_damage

    def _prefer_known_crit_pattern(
        self,
        regular_matches: list[_DamageMatch],
        critical_matches: list[_DamageMatch],
    ) -> _DamageMatch | None:
        small_weapon_can_crit = any(
            match.profile.role == "small_weapon" for match in critical_matches
        )
        if not small_weapon_can_crit:
            return None
        big_regular_matches = [
            match for match in regular_matches if match.profile.role == "big_weapon"
        ]
        return self._narrowest(big_regular_matches)

    def _narrowest(self, matches: list[_DamageMatch]) -> _DamageMatch | None:
        if not matches:
            return None
        return min(matches, key=lambda match: (match.width, match.profile.name))
