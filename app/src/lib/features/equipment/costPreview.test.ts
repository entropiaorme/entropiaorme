import { describe, expect, it } from 'vitest';
import { type CostPreviewInput, type PreviewComponent, previewCostPerUse } from './costPreview';

// Expected values are hand-computed with the backend cost engine's constants
// (src-tauri/eo-services/src/cost_engine.rs): each damage-enhancer slot adds
// 10% to weapon decay and ammo, markup applies to decay only as percent / 100,
// and ammo always carries a markup multiplier of 1.

const weapon: PreviewComponent = { decay: 2.0, ammoBurn: 1.0, isLimited: false };
const limitedWeapon: PreviewComponent = { decay: 2.0, ammoBurn: 1.0, isLimited: true };
const amp: PreviewComponent = { decay: 0.5, ammoBurn: 0.2, isLimited: false };
const limitedAmp: PreviewComponent = { decay: 0.5, ammoBurn: 0.2, isLimited: true };
const scope: PreviewComponent = { decay: 0.3, ammoBurn: 0, isLimited: false };
const limitedScope: PreviewComponent = { decay: 0.3, ammoBurn: 0, isLimited: true };
const implant: PreviewComponent = {
	decay: 0,
	ammoBurn: 0,
	isLimited: true,
	absorptionPercent: 20,
};
const extender: PreviewComponent = {
	decay: 0,
	ammoBurn: 0,
	isLimited: true,
	absorptionPercent: 20,
};

function input(overrides: Partial<CostPreviewInput> = {}): CostPreviewInput {
	return {
		weapon,
		amp: null,
		scope: null,
		absorber: null,
		implant: null,
		markupPercent: 100,
		ampMarkupPercent: 100,
		scopeMarkupPercent: 100,
		absorberMarkupPercent: 100,
		implantMarkupPercent: 100,
		damageEnhancers: 0,
		reloadSpeedPercent: 0,
		...overrides,
	};
}

describe('previewCostPerUse', () => {
	it('returns null while no weapon is selected', () => {
		expect(previewCostPerUse(input({ weapon: null }))).toBeNull();
		expect(previewCostPerUse(input({ weapon: null, amp, scope }))).toBeNull();
	});

	it('sums decay and ammo for an unlimited weapon alone', () => {
		expect(previewCostPerUse(input())).toBeCloseTo(3.0, 10);
	});

	it('applies markup to a limited weapon decay but never to its ammo', () => {
		expect(previewCostPerUse(input({ weapon: limitedWeapon, markupPercent: 150 }))).toBeCloseTo(
			4.0,
			10,
		);
	});

	it('leaves an unlimited weapon untouched by the markup input', () => {
		expect(previewCostPerUse(input({ markupPercent: 150 }))).toBeCloseTo(3.0, 10);
	});

	it('scales weapon decay and ammo by 10% per enhancer slot', () => {
		expect(previewCostPerUse(input({ damageEnhancers: 2 }))).toBeCloseTo(3.6, 10);
	});

	it('compounds markup and enhancers on the weapon decay only', () => {
		expect(
			previewCostPerUse(input({ weapon: limitedWeapon, markupPercent: 150, damageEnhancers: 2 })),
		).toBeCloseTo(4.8, 10);
	});

	it('adds amp decay and ammo, with enhancers left off the amp', () => {
		expect(previewCostPerUse(input({ amp, damageEnhancers: 2 }))).toBeCloseTo(4.3, 10);
	});

	it('applies the amp markup input to a limited amp decay only', () => {
		expect(previewCostPerUse(input({ amp: limitedAmp, ampMarkupPercent: 200 }))).toBeCloseTo(
			4.2,
			10,
		);
	});

	it('adds scope decay with its own markup input for limited scopes', () => {
		expect(previewCostPerUse(input({ scope }))).toBeCloseTo(3.3, 10);
		expect(previewCostPerUse(input({ scope: limitedScope, scopeMarkupPercent: 120 }))).toBeCloseTo(
			3.36,
			10,
		);
	});

	it('routes implant and absorber shares out of weapon decay at their own markups', () => {
		// Implant 20% of 2.0 = 0.4 @ 1.10; extender/absorber 20% of the 1.6
		// remainder = 0.32 @ 1.08; weapon keeps 1.28 @ 15.0; ammo 1.0.
		expect(
			previewCostPerUse(
				input({
					weapon: limitedWeapon,
					markupPercent: 1500,
					implant,
					implantMarkupPercent: 110,
					absorber: extender,
					absorberMarkupPercent: 108,
				}),
			),
		).toBeCloseTo(1.28 * 15 + 0.4 * 1.1 + 0.32 * 1.08 + 1.0, 10);
	});

	it('scales absorption shares with enhancers and ignores shareless devices', () => {
		// Enhancer mult 1.2: scaled decay 2.4; implant 0.48; weapon 1.92;
		// ammo 1.2. An unlimited implant prices its share at par.
		expect(
			previewCostPerUse(input({ damageEnhancers: 2, implant: { ...implant, isLimited: false } })),
		).toBeCloseTo(1.92 + 0.48 + 1.2, 10);
		expect(
			previewCostPerUse(input({ implant: { ...implant, absorptionPercent: null } })),
		).toBeCloseTo(3.0, 10);
	});

	it('applies the markup as given, with no floor of its own', () => {
		// The form's input clamps markup to a minimum of 100; the formula
		// itself applies whatever it receives.
		expect(previewCostPerUse(input({ weapon: limitedWeapon, markupPercent: 0 }))).toBeCloseTo(
			1.0,
			10,
		);
	});

	it('grows every line by the attack-rate factor past the server limit', () => {
		const fast: PreviewComponent = { ...weapon, usesPerMinute: 90 };
		// 2.0 decay + 1.0 ammo + 0.5 amp decay + 0.2 amp ammo = 3.7, then x1.17.
		expect(previewCostPerUse(input({ weapon: fast, amp, reloadSpeedPercent: 30 }))).toBeCloseTo(
			3.7 * 1.17,
			10,
		);
	});

	it('leaves a weapon within the limit, or without a rate, at its own cost', () => {
		expect(
			previewCostPerUse(
				input({ weapon: { ...weapon, usesPerMinute: 60 }, reloadSpeedPercent: 30 }),
			),
		).toBeCloseTo(3.0, 10);
		expect(previewCostPerUse(input({ reloadSpeedPercent: 30 }))).toBeCloseTo(3.0, 10);
	});
});
