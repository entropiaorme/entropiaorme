/**
 * A weapon's attack rate under the reload speed in effect, as Equipment
 * shows it.
 *
 * The game server runs at most 100 attacks a minute. A weapon whose
 * reload-speed-buffed rate would pass that attacks 100 times a minute, and
 * each attack lands proportionally harder and costs proportionally more, so
 * damage per PEC is unchanged. The backend applies this to every stored
 * weapon (`attack_rate.rs`); the helpers here word its result and let the
 * equipment form's local preview apply the same factor before saving.
 */

import type { WeaponAttackRate } from '$lib/types/equipment';
import type { ReloadSpeedInEffect } from '$lib/types/settings';

/** Attacks a minute the server processes (mirrors the backend limit). */
export const SERVER_ATTACKS_PER_MINUTE_LIMIT = 100;

/**
 * How much each attack's damage and cost grow for a weapon of
 * `basePerMinute` under `reloadSpeedPercent`: the buffed rate over the
 * limit, or 1 within it or when the rate is unknown.
 */
export function attackRateFactor(
	basePerMinute: number | null | undefined,
	reloadSpeedPercent: number,
): number {
	const multiplier = 1 + reloadSpeedPercent / 100;
	if (
		basePerMinute == null ||
		!Number.isFinite(basePerMinute) ||
		basePerMinute <= 0 ||
		!Number.isFinite(multiplier) ||
		multiplier <= 0
	) {
		return 1;
	}
	return Math.max(1, (basePerMinute * multiplier) / SERVER_ATTACKS_PER_MINUTE_LIMIT);
}

const rate = (value: number) => `${Number(value.toFixed(1))}`;

/** The factor as a multiplier label, `×1.17`: three decimals at most, so a
 * small factor such as 1.035 is not rounded away. */
export function formatFactor(factor: number): string {
	return `×${Number(factor.toFixed(3))}`;
}

/** The attack-rate line of a weapon's detail: the value and, when the
 * server limit is holding the weapon back, the note explaining its cost. */
export function describeAttackRate(attackRate: WeaponAttackRate): {
	value: string;
	note: string | null;
} {
	const { basePerMinute, reloadSpeedPercent, buffedPerMinute, effectivePerMinute, factor } =
		attackRate;
	if (factor > 1) {
		return {
			value: `${rate(effectivePerMinute)} a minute, server limit`,
			note:
				`The ${rate(buffedPerMinute)} a minute this weapon would reach become ` +
				`${formatFactor(factor)} damage and cost per attack; damage per PEC is unchanged.`,
		};
	}
	if (reloadSpeedPercent !== 0) {
		return {
			value: `${rate(effectivePerMinute)} a minute (${rate(basePerMinute)} base, ${
				reloadSpeedPercent > 0 ? '+' : ''
			}${rate(reloadSpeedPercent)}% reload speed)`,
			note: null,
		};
	}
	return { value: `${rate(basePerMinute)} a minute`, note: null };
}

/** The Effects tab's disclosure when the declared reload speed passes the
 * game's limit for equipped items; null while every point is in effect. */
export function describeReloadLimit(reloadSpeed: ReloadSpeedInEffect | null): string | null {
	if (!reloadSpeed) return null;
	const { declaredPercent, effectivePercent, itemLimitPercent } = reloadSpeed;
	if (Math.abs(declaredPercent - effectivePercent) < 1e-9) return null;
	return (
		`Equipped items add at most ${rate(itemLimitPercent)}% reload speed, so ` +
		`${rate(effectivePercent)}% of the ${rate(declaredPercent)}% declared is in effect.`
	);
}
