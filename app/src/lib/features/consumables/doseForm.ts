/**
 * The Equipment form's consumable dose: what the player declares, the
 * preview over the catalogue's figures, and the request it saves as. The
 * catalogue wins wherever it has a figure; the player declares the rest,
 * and always the acquisition markup and whether a dose books its cost.
 */

import type { ConsumableDoseRequest, ConsumableEffect, ConsumableSettings } from '$lib/api';

export interface DoseFormFields {
	/** Acquisition markup, percent of TT. */
	markupPercent: number;
	/** Whether taking a dose books its cost to the session. */
	trackCost: boolean;
	/** Declared duration, minutes; used when the catalogue has none. */
	durationMinutes: number | null;
	/** Declared reload speed, percent; used when the catalogue has no effects. */
	reloadPercent: number | null;
	/** Declared TT value of one dose, PED; used when the catalogue has none. */
	ttValuePed: number | null;
}

/** A new item's starting fields: every dose booked, at 100%. */
export function blankDoseFields(): DoseFormFields {
	return {
		markupPercent: 100,
		trackCost: true,
		durationMinutes: null,
		reloadPercent: null,
		ttValuePed: null,
	};
}

/** An existing item's fields, from its stored settings. */
export function doseFieldsFrom(settings: ConsumableSettings | null): DoseFormFields {
	if (!settings) return { ...blankDoseFields(), trackCost: false };
	return {
		markupPercent: settings.markupPercent,
		trackCost: settings.trackCost,
		durationMinutes:
			settings.declaredDurationSeconds === null ? null : settings.declaredDurationSeconds / 60,
		reloadPercent: settings.declaredReloadSpeedPercent,
		ttValuePed: settings.declaredTtValuePed,
	};
}

export interface DosePreview {
	durationSeconds: number;
	effects: ConsumableEffect[];
	ttValuePed: number;
	doseCostPed: number;
	/** Which figures the catalogue supplies (the form declares the rest). */
	catalogueEffects: boolean;
	catalogueDuration: boolean;
	catalogueValue: boolean;
}

function finite(value: number | null): number | null {
	return value !== null && Number.isFinite(value) ? value : null;
}

/** What one dose will be, over the catalogue's figures and the fields. */
export function previewDose(
	catalogue: ConsumableSettings | null,
	fields: DoseFormFields,
): DosePreview {
	const catalogueEffects = catalogue?.catalogueEffects ?? false;
	const catalogueDuration = catalogue?.catalogueDuration ?? false;
	const catalogueValue = catalogue?.catalogueValue ?? false;
	const reload = finite(fields.reloadPercent);
	const effects: ConsumableEffect[] = catalogueEffects
		? (catalogue?.effects ?? [])
		: reload !== null && reload !== 0
			? [
					{
						name: reload > 0 ? 'Reload Speed Increased' : 'Reload Speed Decreased',
						strength: Math.abs(reload),
						unit: '%',
						reloadSpeedPercent: reload,
					},
				]
			: [];
	const durationSeconds = catalogueDuration
		? (catalogue?.durationSeconds ?? 0)
		: Math.max(0, (finite(fields.durationMinutes) ?? 0) * 60);
	const ttValuePed = catalogueValue
		? (catalogue?.ttValuePed ?? 0)
		: Math.max(0, finite(fields.ttValuePed) ?? 0);
	const markup = finite(fields.markupPercent) ?? 100;
	return {
		durationSeconds,
		effects,
		ttValuePed,
		doseCostPed: Math.max(0, (ttValuePed * markup) / 100),
		catalogueEffects,
		catalogueDuration,
		catalogueValue,
	};
}

/** Why the fields cannot be saved, or null when they can. Mirrors the
 * backend's bounds so the form says so before a save is refused. */
export function doseFieldsError(fields: DoseFormFields): string | null {
	const markup = finite(fields.markupPercent);
	if (markup === null || markup < 1 || markup > 100_000) {
		return 'Markup must be between 1% and 100000%';
	}
	const minutes = finite(fields.durationMinutes);
	if (fields.durationMinutes !== null && (minutes === null || minutes < 0 || minutes > 10_080)) {
		return 'Duration must be between 0 and 7 days';
	}
	const reload = finite(fields.reloadPercent);
	if (fields.reloadPercent !== null && (reload === null || reload <= -100 || reload > 100)) {
		return 'Reload speed must be above -100% and at most 100%';
	}
	const tt = finite(fields.ttValuePed);
	if (fields.ttValuePed !== null && (tt === null || tt < 0 || tt > 100_000)) {
		return 'TT value must be between 0 and 100000 PED';
	}
	return null;
}

/** The dose settings to save. Declared figures are kept even where the
 * catalogue has its own, so a catalogue that later loses a figure falls
 * back to what the player said. */
export function doseRequest(fields: DoseFormFields): ConsumableDoseRequest {
	const minutes = finite(fields.durationMinutes);
	return {
		markup_percent: fields.markupPercent,
		track_cost: fields.trackCost,
		duration_seconds: minutes === null ? null : minutes * 60,
		reload_speed_percent: finite(fields.reloadPercent),
		tt_value_ped: finite(fields.ttValuePed),
	};
}
