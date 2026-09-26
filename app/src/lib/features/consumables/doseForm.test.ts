import { describe, expect, it } from 'vitest';
import type { ConsumableSettings } from '$lib/api';
import {
	blankDoseFields,
	doseFieldsError,
	doseFieldsFrom,
	doseRequest,
	previewDose,
} from './doseForm';

const adrenaline: ConsumableSettings = {
	durationSeconds: 3600,
	effects: [{ name: 'Reload Speed Increased', strength: 10, unit: '%', reloadSpeedPercent: 10 }],
	ttValuePed: 3,
	markupPercent: 100,
	doseCostPed: 3,
	trackCost: false,
	catalogueEffects: true,
	catalogueDuration: true,
	catalogueValue: true,
	declaredReloadSpeedPercent: null,
	declaredDurationSeconds: null,
	declaredTtValuePed: null,
};

describe('the dose form', () => {
	it('books every new item by default at 100%', () => {
		expect(blankDoseFields()).toMatchObject({ markupPercent: 100, trackCost: true });
	});

	it('keeps an item saved before dose settings unbooked', () => {
		expect(doseFieldsFrom(null).trackCost).toBe(false);
	});

	it('previews a catalogue item from the catalogue, costed at the markup', () => {
		const preview = previewDose(adrenaline, {
			...blankDoseFields(),
			markupPercent: 150,
			reloadPercent: 50,
		});
		expect(preview.durationSeconds).toBe(3600);
		expect(preview.effects[0].reloadSpeedPercent).toBe(10);
		expect(preview.doseCostPed).toBeCloseTo(4.5);
	});

	it('previews a custom item from its declaration', () => {
		const preview = previewDose(null, {
			...blankDoseFields(),
			reloadPercent: 12,
			durationMinutes: 10,
			ttValuePed: 2,
			markupPercent: 110,
		});
		expect(preview.durationSeconds).toBe(600);
		expect(preview.effects).toEqual([
			{ name: 'Reload Speed Increased', strength: 12, unit: '%', reloadSpeedPercent: 12 },
		]);
		expect(preview.doseCostPed).toBeCloseTo(2.2);
	});

	it('refuses what the backend would refuse', () => {
		expect(doseFieldsError(blankDoseFields())).toBeNull();
		expect(doseFieldsError({ ...blankDoseFields(), markupPercent: 0 })).toMatch(/Markup/);
		expect(doseFieldsError({ ...blankDoseFields(), reloadPercent: -100 })).toMatch(/Reload/);
		expect(doseFieldsError({ ...blankDoseFields(), durationMinutes: -1 })).toMatch(/Duration/);
		expect(doseFieldsError({ ...blankDoseFields(), ttValuePed: -1 })).toMatch(/TT/);
	});

	it('saves declared minutes as seconds and leaves the rest null', () => {
		expect(doseRequest({ ...blankDoseFields(), durationMinutes: 10 })).toEqual({
			markup_percent: 100,
			track_cost: true,
			duration_seconds: 600,
			reload_speed_percent: null,
			tt_value_ped: null,
		});
	});

	it('round-trips a stored declaration', () => {
		const fields = doseFieldsFrom({
			...adrenaline,
			catalogueEffects: false,
			declaredDurationSeconds: 900,
			declaredReloadSpeedPercent: 8,
			declaredTtValuePed: 1.5,
			markupPercent: 120,
			trackCost: true,
		});
		expect(fields).toEqual({
			markupPercent: 120,
			trackCost: true,
			durationMinutes: 15,
			reloadPercent: 8,
			ttValuePed: 1.5,
		});
	});
});
