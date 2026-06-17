import { afterEach, describe, expect, it, vi } from 'vitest';
import type { TrackingStatus } from '$lib/api';
import { ALL_STAT_IDS, getStatDef, STAT_DEFS, type StatId } from '$lib/statsRegistry';

// Pure render functions over a TrackingStatus. Each stat only ever reads the
// handful of fields it needs, so fixtures are deliberately minimal: status
// 'active' plus exactly the fields under test.

// The registry's empty placeholder is the em-dash glyph; written as an escape
// so this file passes the authoring lint, which bans the literal character.
const EMPTY = { value: '\u2014', color: 'text-text' } as const;

/**
 * Build a minimal active TrackingStatus with the supplied overlay fields.
 * Overrides are intentionally typed loosely (per-field unknown) so tests can
 * feed null into fields the public type declares as `number | undefined`,
 * exercising the runtime `!= null` guards the render functions rely on.
 */
function active(fields: Partial<Record<keyof TrackingStatus, unknown>> = {}): TrackingStatus {
	return { status: 'active', ...fields } as TrackingStatus;
}

/** Render a stat by id through its STAT_DEFS entry. */
function render(id: StatId, status: TrackingStatus | null) {
	return STAT_DEFS[id].render(status);
}

afterEach(() => {
	vi.useRealTimers();
});

describe('inactive gate', () => {
	// Every stat short-circuits to EMPTY unless status === 'active'.
	for (const id of ALL_STAT_IDS) {
		it(`${id}: render(null) === EMPTY`, () => {
			expect(render(id, null)).toEqual(EMPTY);
		});

		it(`${id}: render(idle) === EMPTY`, () => {
			// 'idle' (and 'unavailable') are non-active states.
			expect(render(id, { status: 'idle' } as TrackingStatus)).toEqual(EMPTY);
		});
	}
});

describe('cycled / loot_tt / rate / pes / kills / globals / hofs', () => {
	it('cycled renders formatPed(cost) with fallback 0', () => {
		expect(render('cycled', active({ cost: 12.5 }))).toEqual({
			value: '12.50',
			color: 'text-text',
		});
		expect(render('cycled', active())).toEqual({ value: '0.00', color: 'text-text' });
	});

	it('loot_tt renders formatPed(returns) with fallback 0', () => {
		expect(render('loot_tt', active({ returns: 8 }))).toEqual({
			value: '8.00',
			color: 'text-text',
		});
		expect(render('loot_tt', active())).toEqual({ value: '0.00', color: 'text-text' });
	});

	it('rate renders formatPercent(returnRate) with fallback 0', () => {
		expect(render('rate', active({ returnRate: 0.917 }))).toEqual({
			value: '91.7%',
			color: 'text-text',
		});
		expect(render('rate', active())).toEqual({ value: '0.0%', color: 'text-text' });
	});

	it('pes renders formatPed(pes) with fallback 0', () => {
		expect(render('pes', active({ pes: 3.456 }))).toEqual({
			value: '3.46',
			color: 'text-text',
		});
		expect(render('pes', active())).toEqual({ value: '0.00', color: 'text-text' });
	});

	it('kills_count / globals_count / hofs_count render String(value) with fallback 0', () => {
		expect(render('kills_count', active({ kill_count: 7 }))).toEqual({
			value: '7',
			color: 'text-text',
		});
		expect(render('kills_count', active())).toEqual({ value: '0', color: 'text-text' });
		expect(render('globals_count', active({ globalsCount: 2 }))).toEqual({
			value: '2',
			color: 'text-text',
		});
		expect(render('globals_count', active())).toEqual({ value: '0', color: 'text-text' });
		expect(render('hofs_count', active({ hofsCount: 1 }))).toEqual({
			value: '1',
			color: 'text-text',
		});
		expect(render('hofs_count', active())).toEqual({ value: '0', color: 'text-text' });
	});
});

describe('net: sign + colour', () => {
	it('positive net -> +sign, text-positive', () => {
		expect(render('net', active({ returns: 110, cost: 100 }))).toEqual({
			value: '+10.00',
			color: 'text-positive',
		});
	});

	it('exactly zero (returns === cost) -> +0.00, text-positive', () => {
		// net === 0 takes the >= 0 branch, so it gets the '+' sign and positive colour.
		expect(render('net', active({ returns: 100, cost: 100 }))).toEqual({
			value: '+0.00',
			color: 'text-positive',
		});
	});

	it('negative net -> text-negative, single formatPed "-" sign (no extra "+")', () => {
		// sign === '' for negatives; the '-' comes from formatPed/toFixed itself.
		expect(render('net', active({ returns: 90, cost: 100 }))).toEqual({
			value: '-10.00',
			color: 'text-negative',
		});
	});

	it('missing returns/cost default to 0 -> +0.00 positive', () => {
		expect(render('net', active())).toEqual({ value: '+0.00', color: 'text-positive' });
	});
});

describe('divide-by-zero guards (denominator <= 0 -> EMPTY)', () => {
	it('pes_per_100: cost <= 0 -> EMPTY; positive cost -> (pes/cost*100).toFixed(2)', () => {
		expect(render('pes_per_100', active({ pes: 5, cost: 0 }))).toEqual(EMPTY);
		expect(render('pes_per_100', active({ pes: 5, cost: -1 }))).toEqual(EMPTY);
		// 2 / 50 * 100 = 4.00
		expect(render('pes_per_100', active({ pes: 2, cost: 50 }))).toEqual({
			value: '4.00',
			color: 'text-text',
		});
	});

	it('avg_cost_per_kill: kill_count <= 0 -> EMPTY; positive -> formatPed(cost/kills)', () => {
		expect(render('avg_cost_per_kill', active({ cost: 100, kill_count: 0 }))).toEqual(EMPTY);
		expect(render('avg_cost_per_kill', active({ cost: 100, kill_count: -3 }))).toEqual(EMPTY);
		expect(render('avg_cost_per_kill', active({ cost: 100, kill_count: 4 }))).toEqual({
			value: '25.00',
			color: 'text-text',
		});
	});

	it('avg_damage: kill_count <= 0 -> EMPTY; positive -> formatPed(damage/kills)', () => {
		expect(render('avg_damage', active({ damageDealtTotal: 500, kill_count: 0 }))).toEqual(EMPTY);
		expect(render('avg_damage', active({ damageDealtTotal: 500, kill_count: 5 }))).toEqual({
			value: '100.00',
			color: 'text-text',
		});
		// damageDealtTotal absent -> falls back to 0 over a positive kill count.
		expect(render('avg_damage', active({ kill_count: 5 }))).toEqual({
			value: '0.00',
			color: 'text-text',
		});
	});

	it('crit_rate: shotsFiredTotal <= 0 -> EMPTY; positive -> formatPercent(crits/shots)', () => {
		expect(render('crit_rate', active({ criticalHitsTotal: 5, shotsFiredTotal: 0 }))).toEqual(
			EMPTY,
		);
		expect(render('crit_rate', active({ criticalHitsTotal: 5, shotsFiredTotal: -2 }))).toEqual(
			EMPTY,
		);
		// 10 / 200 = 0.05 -> 5.0%
		expect(render('crit_rate', active({ criticalHitsTotal: 10, shotsFiredTotal: 200 }))).toEqual({
			value: '5.0%',
			color: 'text-text',
		});
	});

	it('dpp: weaponCost <= 0 -> EMPTY', () => {
		expect(render('dpp', active({ weaponCost: 0, weaponDamageDealt: 250 }))).toEqual(EMPTY);
		expect(render('dpp', active({ weaponCost: -1, weaponDamageDealt: 250 }))).toEqual(EMPTY);
	});
});

describe('!= null guards (0 is valid; null/undefined -> EMPTY)', () => {
	it('latest_kill_loot: 0 valid, null/undefined -> EMPTY', () => {
		expect(render('latest_kill_loot', active({ latestKillLoot: 0 }))).toEqual({
			value: '0.00',
			color: 'text-text',
		});
		expect(render('latest_kill_loot', active({ latestKillLoot: 7.5 }))).toEqual({
			value: '7.50',
			color: 'text-text',
		});
		expect(render('latest_kill_loot', active({ latestKillLoot: null }))).toEqual(EMPTY);
		expect(render('latest_kill_loot', active())).toEqual(EMPTY);
	});

	it('multiplier_last: 0 -> formatMultiplier(0) "0.00x" (NOT empty); null -> EMPTY', () => {
		expect(render('multiplier_last', active({ multiplierLast: 0 }))).toEqual({
			value: '0.00x',
			color: 'text-text',
		});
		// >= 10 uses 1 decimal place.
		expect(render('multiplier_last', active({ multiplierLast: 25 }))).toEqual({
			value: '25.0x',
			color: 'text-text',
		});
		expect(render('multiplier_last', active({ multiplierLast: null }))).toEqual(EMPTY);
		expect(render('multiplier_last', active())).toEqual(EMPTY);
	});

	it('multiplier_avg: 0 valid, null/undefined -> EMPTY', () => {
		expect(render('multiplier_avg', active({ multiplierAvg: 0 }))).toEqual({
			value: '0.00x',
			color: 'text-text',
		});
		expect(render('multiplier_avg', active({ multiplierAvg: 1.5 }))).toEqual({
			value: '1.50x',
			color: 'text-text',
		});
		expect(render('multiplier_avg', active({ multiplierAvg: null }))).toEqual(EMPTY);
		expect(render('multiplier_avg', active())).toEqual(EMPTY);
	});

	it('multiplier_max: 0 valid, null/undefined -> EMPTY', () => {
		expect(render('multiplier_max', active({ multiplierMax: 0 }))).toEqual({
			value: '0.00x',
			color: 'text-text',
		});
		expect(render('multiplier_max', active({ multiplierMax: 12.34 }))).toEqual({
			value: '12.3x',
			color: 'text-text',
		});
		expect(render('multiplier_max', active({ multiplierMax: null }))).toEqual(EMPTY);
		expect(render('multiplier_max', active())).toEqual(EMPTY);
	});

	it('max_damage: 0 valid, null/undefined -> EMPTY', () => {
		expect(render('max_damage', active({ maxDamage: 0 }))).toEqual({
			value: '0.00',
			color: 'text-text',
		});
		expect(render('max_damage', active({ maxDamage: 88.2 }))).toEqual({
			value: '88.20',
			color: 'text-text',
		});
		expect(render('max_damage', active({ maxDamage: null }))).toEqual(EMPTY);
		expect(render('max_damage', active())).toEqual(EMPTY);
	});
});

describe('dpp: PEC conversion', () => {
	it('dpp = weaponDamage / (weaponCost * 100); cost 1, dmg 250 -> 2.50', () => {
		expect(render('dpp', active({ weaponCost: 1, weaponDamageDealt: 250 }))).toEqual({
			value: '2.50',
			color: 'text-text',
		});
	});

	it('weaponDamageDealt falls back to damageDealtTotal when undefined', () => {
		// weaponDamageDealt absent -> uses damageDealtTotal: 250 / (1 * 100) = 2.50.
		expect(render('dpp', active({ weaponCost: 1, damageDealtTotal: 250 }))).toEqual({
			value: '2.50',
			color: 'text-text',
		});
	});

	it('weaponDamageDealt takes precedence over damageDealtTotal when present', () => {
		// 250 / (1 * 100) = 2.50, ignoring the damageDealtTotal of 999.
		expect(
			render('dpp', active({ weaponCost: 1, weaponDamageDealt: 250, damageDealtTotal: 999 })),
		).toEqual({ value: '2.50', color: 'text-text' });
	});

	it('neither damage field present -> 0 / (cost*100) = 0.00', () => {
		expect(render('dpp', active({ weaponCost: 1 }))).toEqual({
			value: '0.00',
			color: 'text-text',
		});
	});
});

describe('avg_dps: wall-clock dependent', () => {
	const NOW = new Date('2026-06-03T12:00:00.000Z');

	it('no started_at -> EMPTY', () => {
		vi.useFakeTimers();
		vi.setSystemTime(NOW);
		expect(render('avg_dps', active({ damageDealtTotal: 1000 }))).toEqual(EMPTY);
	});

	it('invalid started_at (NaN time) -> EMPTY', () => {
		vi.useFakeTimers();
		vi.setSystemTime(NOW);
		expect(render('avg_dps', active({ damageDealtTotal: 1000, started_at: 'not-a-date' }))).toEqual(
			EMPTY,
		);
	});

	it('elapsed <= 0 (started_at in the future) -> EMPTY', () => {
		// elapsedSeconds clamps to Math.max(0, ...), so a future start yields 0 -> EMPTY.
		vi.useFakeTimers();
		vi.setSystemTime(NOW);
		const future = new Date(NOW.getTime() + 60_000).toISOString();
		expect(render('avg_dps', active({ damageDealtTotal: 1000, started_at: future }))).toEqual(
			EMPTY,
		);
	});

	it('elapsed exactly 0 (started_at === now) -> EMPTY', () => {
		vi.useFakeTimers();
		vi.setSystemTime(NOW);
		expect(
			render('avg_dps', active({ damageDealtTotal: 1000, started_at: NOW.toISOString() })),
		).toEqual(EMPTY);
	});

	it('positive elapsed -> formatPed(damageDealtTotal / elapsed)', () => {
		vi.useFakeTimers();
		vi.setSystemTime(NOW);
		// started 100s before now; 1000 damage / 100s = 10.00.
		const started = new Date(NOW.getTime() - 100_000).toISOString();
		expect(render('avg_dps', active({ damageDealtTotal: 1000, started_at: started }))).toEqual({
			value: '10.00',
			color: 'text-text',
		});
	});

	it('positive elapsed, missing damage -> 0.00', () => {
		vi.useFakeTimers();
		vi.setSystemTime(NOW);
		const started = new Date(NOW.getTime() - 100_000).toISOString();
		expect(render('avg_dps', active({ started_at: started }))).toEqual({
			value: '0.00',
			color: 'text-text',
		});
	});
});

describe('getStatDef', () => {
	it("known id 'cycled' -> its def", () => {
		expect(getStatDef('cycled')).toBe(STAT_DEFS.cycled);
	});

	it("genuinely-unknown id 'does_not_exist' -> null", () => {
		expect(getStatDef('does_not_exist')).toBeNull();
	});

	// getStatDef has an Object.hasOwn own-property guard, so prototype-chain
	// keys resolve to null rather than the inherited Object members they would
	// otherwise expose typed as StatDef.
	it("'__proto__' -> null (own-property guard)", () => {
		expect(getStatDef('__proto__')).toBeNull();
	});

	it("'constructor' -> null (own-property guard)", () => {
		expect(getStatDef('constructor')).toBeNull();
	});
});

describe('drift guard', () => {
	it('ALL_STAT_IDS deep-equals Object.keys(STAT_DEFS)', () => {
		expect(ALL_STAT_IDS).toEqual(Object.keys(STAT_DEFS));
	});

	it('ALL_STAT_IDS has length 19', () => {
		expect(ALL_STAT_IDS).toHaveLength(19);
	});
});
