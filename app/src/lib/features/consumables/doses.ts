/**
 * Pure derivations over the dose readout. Every countdown is a projection of
 * a dose's stored end: the caller passes `now` (epoch seconds) and owns the
 * ticking, so nothing here keeps time of its own.
 */

import type { ConsumableDose, ConsumableEffect, ConsumableOption, ReloadSpeedNow } from '$lib/api';

/** How long an ended dose stays on the readouts, offering a re-dose. */
export const RECENTLY_ENDED_SECONDS = 60;

export type DoseState = 'running' | 'ended' | 'removed';

/** Where a dose stands at `now`. */
export function doseState(dose: ConsumableDose, now: number): DoseState {
	if (dose.removedAt !== null) return 'removed';
	return now < dose.endsAt ? 'running' : 'ended';
}

/** Whole seconds left in a dose's run at `now`; 0 once it has ended. */
export function remainingSeconds(dose: ConsumableDose, now: number): number {
	return Math.max(0, Math.ceil(dose.endsAt - now));
}

/** A countdown as the readouts print it: `h:mm:ss` past an hour, else `m:ss`. */
export function formatCountdown(seconds: number): string {
	const total = Math.max(0, Math.floor(seconds));
	const h = Math.floor(total / 3600);
	const m = Math.floor((total % 3600) / 60);
	const s = total % 60;
	const ss = s.toString().padStart(2, '0');
	if (h > 0) return `${h}:${m.toString().padStart(2, '0')}:${ss}`;
	return `${m}:${ss}`;
}

/** A dose length in words: `8 s`, `10 min`, `1 h`, `1 h 30 min`. Zero is `Instant`. */
export function formatDuration(seconds: number): string {
	if (!(seconds > 0)) return 'Instant';
	if (seconds < 60) return `${Math.round(seconds)} s`;
	const minutes = Math.round(seconds / 60);
	if (minutes < 60) return `${minutes} min`;
	const h = Math.floor(minutes / 60);
	const m = minutes % 60;
	return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

function formatSigned(value: number): string {
	const rounded = Math.round(value * 100) / 100;
	return `${rounded > 0 ? '+' : ''}${rounded}`;
}

/** One effect as a short line: `Reload speed +10%`, else the printed name and strength. */
export function describeEffect(effect: ConsumableEffect): string {
	if (effect.reloadSpeedPercent !== null) {
		return `Reload speed ${formatSigned(effect.reloadSpeedPercent)}%`;
	}
	if (effect.strength === null) return effect.name;
	const unit = effect.unit === '%' ? '%' : effect.unit ? ` ${effect.unit}` : '';
	return `${effect.name} ${effect.strength}${unit}`;
}

/** The effects of a dose or item as one line, reload speed first. */
export function describeEffects(effects: readonly ConsumableEffect[]): string {
	const ordered = [...effects].sort(
		(a, b) => Number(b.reloadSpeedPercent !== null) - Number(a.reloadSpeedPercent !== null),
	);
	return ordered.map(describeEffect).join(', ');
}

/** The rows a live readout shows at `now`: running doses soonest-ending first,
 * then doses that ended within the last minute (offered for a re-dose), most
 * recent first. Removed doses never show here. `includeOnUse` keeps a
 * healing tool's automatic buffs (the dashboard shows them; the overlay,
 * which is for actions, does not). */
export function liveRows(
	doses: readonly ConsumableDose[],
	now: number,
	includeOnUse: boolean,
): ConsumableDose[] {
	const shown = doses.filter(
		(dose) => dose.removedAt === null && (includeOnUse || dose.source !== 'on_use'),
	);
	const running = shown
		.filter((dose) => doseState(dose, now) === 'running')
		.sort((a, b) => a.endsAt - b.endsAt);
	const ended = shown
		.filter(
			(dose) =>
				doseState(dose, now) === 'ended' &&
				!dose.replaced &&
				now - dose.endsAt <= RECENTLY_ENDED_SECONDS &&
				dose.source !== 'on_use',
		)
		.sort((a, b) => b.endsAt - a.endsAt);
	return [...running, ...ended];
}

/** When the next dose on show ends or leaves the readout, for a re-read. */
export function nextBoundary(doses: readonly ConsumableDose[], now: number): number | null {
	const boundaries = doses
		.filter((dose) => dose.removedAt === null)
		.flatMap((dose) => [dose.endsAt, dose.endsAt + RECENTLY_ENDED_SECONDS])
		.filter((at) => at > now);
	return boundaries.length > 0 ? Math.min(...boundaries) : null;
}

/** The consumable a dose was taken of, when it is still configured. */
export function optionFor(
	options: readonly ConsumableOption[],
	dose: ConsumableDose,
): ConsumableOption | null {
	return options.find((option) => option.equipmentId === dose.equipmentId) ?? null;
}

/** A dose's cost as the readouts say it: the booked figure, `not booked` for
 * a tracked item taken outside a session, or `untracked`. */
export function describeDoseCost(dose: ConsumableDose): string {
	if (dose.costPed > 0) return `${dose.costPed.toFixed(2)} PED`;
	if (dose.source === 'on_use') return 'paid with the heal';
	if (!dose.costTracked) return 'cost not tracked';
	return dose.sessionId === null ? 'outside a session' : 'no cost';
}

/** How a dose started, in words. */
export function describeSource(dose: ConsumableDose): string {
	switch (dose.source) {
		case 'hotbar':
			return 'Hotbar key';
		case 'manual':
			return 'Started by hand';
		case 'on_use':
			return 'Heal buff';
	}
}

/** The reload speed in effect as one line, with where it comes from when
 * both sources contribute or a limit holds it back. */
export function describeReloadSpeed(reload: ReloadSpeedNow): string {
	const inEffect = `${formatSigned(reload.inEffectPercent)}%`;
	const parts: string[] = [];
	if (reload.equippedPercent !== 0) parts.push(`items ${formatSigned(reload.equippedPercent)}%`);
	if (reload.consumedPercent !== 0) parts.push(`doses ${formatSigned(reload.consumedPercent)}%`);
	const printed = reload.equippedPercent + reload.consumedPercent;
	const held = Math.abs(printed - reload.inEffectPercent) > 1e-9;
	const detail =
		parts.length > 1 || held ? ` (${parts.join(', ')}${held ? ', held at the limit' : ''})` : '';
	return `Reload speed ${inEffect}${detail}`;
}
