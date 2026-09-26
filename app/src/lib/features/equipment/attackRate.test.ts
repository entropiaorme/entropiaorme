import { describe, expect, it } from 'vitest';
import {
	attackRateFactor,
	describeAttackRate,
	describeReloadLimit,
	formatFactor,
	SERVER_ATTACKS_PER_MINUTE_LIMIT,
} from './attackRate';

describe('attackRateFactor', () => {
	it('is neutral below and at the server limit', () => {
		expect(attackRateFactor(60, 15)).toBe(1);
		expect(attackRateFactor(80, 25)).toBe(1);
	});

	it('turns the rate past the limit into per-attack magnitude', () => {
		expect(attackRateFactor(90, 30)).toBeCloseTo(1.17, 12);
		expect(attackRateFactor(120, 0)).toBeCloseTo(1.2, 12);
	});

	it('is neutral when the rate is unknown or unusable', () => {
		expect(attackRateFactor(null, 30)).toBe(1);
		expect(attackRateFactor(undefined, 30)).toBe(1);
		expect(attackRateFactor(0, 30)).toBe(1);
		expect(attackRateFactor(Number.NaN, 30)).toBe(1);
		expect(attackRateFactor(90, -100)).toBe(1);
	});

	it('mirrors the backend limit', () => {
		expect(SERVER_ATTACKS_PER_MINUTE_LIMIT).toBe(100);
	});
});

describe('describeAttackRate', () => {
	it('states the catalogue rate when no reload speed applies', () => {
		expect(
			describeAttackRate({
				basePerMinute: 46,
				reloadSpeedPercent: 0,
				buffedPerMinute: 46,
				effectivePerMinute: 46,
				factor: 1,
			}),
		).toEqual({ value: '46 a minute', note: null });
	});

	it('shows the buffed rate beside its base within the limit', () => {
		expect(
			describeAttackRate({
				basePerMinute: 46,
				reloadSpeedPercent: 15,
				buffedPerMinute: 52.9,
				effectivePerMinute: 52.9,
				factor: 1,
			}),
		).toEqual({ value: '52.9 a minute (46 base, +15% reload speed)', note: null });
	});

	it('explains the limit and its per-attack cost past it', () => {
		const described = describeAttackRate({
			basePerMinute: 90,
			reloadSpeedPercent: 30,
			buffedPerMinute: 117,
			effectivePerMinute: 100,
			factor: 1.17,
		});
		expect(described.value).toBe('100 a minute, server limit');
		expect(described.note).toBe(
			'The 117 a minute this weapon would reach become ×1.17 damage and cost per attack; damage per PEC is unchanged.',
		);
	});

	it('words a slowing effect with its sign', () => {
		expect(
			describeAttackRate({
				basePerMinute: 50,
				reloadSpeedPercent: -10,
				buffedPerMinute: 45,
				effectivePerMinute: 45,
				factor: 1,
			}).value,
		).toBe('45 a minute (50 base, -10% reload speed)');
	});
});

describe('describeReloadLimit', () => {
	it('stays quiet while every declared point is in effect', () => {
		expect(describeReloadLimit(null)).toBeNull();
		expect(
			describeReloadLimit({ declaredPercent: 14, effectivePercent: 14, itemLimitPercent: 15 }),
		).toBeNull();
	});

	it('discloses the item limit when the declaration passes it', () => {
		expect(
			describeReloadLimit({ declaredPercent: 24, effectivePercent: 15, itemLimitPercent: 15 }),
		).toBe('Equipped items add at most 15% reload speed, so 15% of the 24% declared is in effect.');
	});
});

describe('formatFactor', () => {
	it('keeps up to three decimals, trimmed', () => {
		expect(formatFactor(1.1700000001)).toBe('×1.17');
		expect(formatFactor(1.2)).toBe('×1.2');
		expect(formatFactor(1.035)).toBe('×1.035');
	});
});
