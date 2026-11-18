/**
 * A weapon's declared damage-over-time effect, as the equipment form edits
 * it and the library shows it.
 *
 * A paid cast of such a weapon may land an initial hit, then its effect
 * ticks for a while. The ticks are outcomes of that one cast, so they cost
 * nothing and never count as shots. The catalogue carries no figures for
 * the effect, so the player declares them from what the game prints.
 */

import type {
	WeaponEffectMode,
	WeaponEffectProfileDto,
	WeaponEffectRequest,
} from '$lib/api/commands.gen';

/** The form's pattern choice: `direct` is a weapon with no effect. */
export type WeaponEffectForm = 'direct' | WeaponEffectMode;

export const WEAPON_EFFECT_OPTIONS: { id: WeaponEffectForm; label: string }[] = [
	{ id: 'direct', label: 'Direct' },
	{ id: 'over_time', label: 'Over time' },
	{ id: 'compound', label: 'Hit + over time' },
];

/** The longest effect the backend accepts, in seconds. */
export const MAX_EFFECT_SECONDS = 600;

/** The largest hit or tick the backend accepts. */
export const MAX_EFFECT_DAMAGE = 100_000;

export interface WeaponEffectFields {
	mode: WeaponEffectForm;
	hitMin: number | null;
	hitMax: number | null;
	durationSeconds: number | null;
	tickMin: number | null;
	tickMax: number | null;
	tickSeconds: number | null;
}

function set(value: number | null): value is number {
	return value !== null && Number.isFinite(value);
}

/** Why a damage range cannot be used, or null when it can. */
function rangeProblem(min: number, max: number, what: string): string | null {
	if (min < 0) return `The ${what} minimum cannot be negative`;
	if (max <= 0) return `The ${what} maximum must be above zero`;
	if (min > max) return `The ${what} minimum must not exceed its maximum`;
	if (max > MAX_EFFECT_DAMAGE) return `A ${what} is at most 100,000 damage`;
	return null;
}

/** Why the declared effect cannot be saved yet, or null when it can. */
export function effectFormProblem(fields: WeaponEffectFields): string | null {
	if (fields.mode === 'direct') return null;
	if (fields.mode === 'compound') {
		if (!set(fields.hitMin) || !set(fields.hitMax)) return 'Enter the initial hit range';
		const hit = rangeProblem(fields.hitMin, fields.hitMax, 'hit');
		if (hit) return hit;
	}
	if (!set(fields.durationSeconds) || fields.durationSeconds <= 0)
		return 'Enter how long the effect lasts';
	if (fields.durationSeconds > MAX_EFFECT_SECONDS) return 'An effect lasts at most ten minutes';
	if (!set(fields.tickMin) || !set(fields.tickMax)) return 'Enter the tick range';
	const tick = rangeProblem(fields.tickMin, fields.tickMax, 'tick');
	if (tick) return tick;
	if (
		fields.tickSeconds !== null &&
		(!Number.isFinite(fields.tickSeconds) || fields.tickSeconds <= 0)
	) {
		return 'The tick cadence must be above zero';
	}
	return null;
}

/** The request's effect, or null for a weapon with none. Call only once
 * `effectFormProblem` is null. */
export function effectRequest(fields: WeaponEffectFields): WeaponEffectRequest | null {
	if (fields.mode === 'direct') return null;
	return {
		mode: fields.mode,
		hit_min: fields.mode === 'compound' ? fields.hitMin : null,
		hit_max: fields.mode === 'compound' ? fields.hitMax : null,
		duration_seconds: fields.durationSeconds ?? 0,
		tick_min: fields.tickMin ?? 0,
		tick_max: fields.tickMax ?? 0,
		tick_seconds: fields.tickSeconds,
	};
}

/** The form's fields for a stored profile (or a weapon with none). */
export function effectFields(profile: WeaponEffectProfileDto | null): WeaponEffectFields {
	if (!profile) {
		return {
			mode: 'direct',
			hitMin: null,
			hitMax: null,
			durationSeconds: null,
			tickMin: null,
			tickMax: null,
			tickSeconds: null,
		};
	}
	return {
		mode: profile.mode,
		hitMin: profile.hitMin,
		hitMax: profile.hitMax,
		durationSeconds: profile.durationSeconds,
		tickMin: profile.tickMin,
		tickMax: profile.tickMax,
		tickSeconds: profile.tickSeconds,
	};
}

function figure(value: number): string {
	return Number.isInteger(value) ? String(value) : value.toFixed(1);
}

/** The effect in one line, as the library lists it. */
export function describeEffectProfile(profile: WeaponEffectProfileDto): string {
	const ticks = `ticks ${figure(profile.tickMin)}–${figure(profile.tickMax)} for ${figure(profile.durationSeconds)} s`;
	if (profile.mode === 'compound' && profile.hitMin !== null && profile.hitMax !== null) {
		return `Hit ${figure(profile.hitMin)}–${figure(profile.hitMax)}, then ${ticks}`;
	}
	return `Over time: ${ticks}`;
}

/** The range a paid cast's own outcome prints: the initial hit, or the
 * first tick of an effect with only ticks. Attribution checks the weapon's
 * shots against it in place of the catalogue's damage. */
export function activationRange(profile: WeaponEffectProfileDto): { min: number; max: number } {
	if (profile.mode === 'compound' && profile.hitMin !== null && profile.hitMax !== null) {
		return { min: profile.hitMin, max: profile.hitMax };
	}
	return { min: profile.tickMin, max: profile.tickMax };
}
