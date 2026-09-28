/**
 * Display rules for the skilling hub: how the target's values read, what a
 * forecast status or sample caveat says to the player, and the compact
 * figures the hub's rows carry. Pure; the components compose over these.
 */

import type {
	SkillingForecastSample,
	SkillingForecastStatus,
	SkillingSampleWarning,
} from '$lib/api/commands.gen';
import type { CodexRankingTarget } from './codexRankingTarget';

/** A target value: profession levels to two places, HP as whole points. */
export function formatTargetValue(target: CodexRankingTarget, value: number): string {
	return target.kind === 'hp' ? String(Math.trunc(value)) : value.toFixed(2);
}

/** A goal as it reads inline: "Lv 43" or "150 HP". */
export function formatGoal(target: CodexRankingTarget, goal: number): string {
	const value = Number.isInteger(goal) ? String(goal) : goal.toFixed(2);
	return target.kind === 'hp' ? `${value} HP` : `Lv ${value}`;
}

/** Play time: minutes under an hour, one place under ten hours. */
export function formatHours(hours: number): string {
	if (hours <= 0) return '0h';
	if (hours < 1) return `${Math.max(1, Math.round(hours * 60))}m`;
	if (hours < 10) return `${hours.toFixed(1)}h`;
	return `${Math.round(hours).toLocaleString('en-GB')}h`;
}

/** Skilling PES in a ranking row: whole above 100, one place below. */
export function formatPes(value: number): string {
	return value >= 100 ? Math.round(value).toLocaleString('en-GB') : value.toFixed(1);
}

/** A target gain: HP to two places, profession levels to three (a skill's
 * share of one profession level is often a few hundredths). */
export function formatTargetGain(target: CodexRankingTarget, gain: number): string {
	return `+${gain.toFixed(target.kind === 'hp' ? 2 : 3)}`;
}

/** A signed fraction as a percentage: "+4.2%". */
export function formatLift(lift: number): string {
	const percent = lift * 100;
	// A small lift keeps two places so it never reads as zero.
	const places = Math.abs(percent) < 1 ? 2 : 1;
	return `${percent >= 0 ? '+' : ''}${percent.toFixed(places)}%`;
}

/** A skill level: two places, grouped, the way the skill tables read. */
export function formatLevel(level: number): string {
	return level.toLocaleString('en-GB', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

/** Group left-out skills by their reason: "not unlocked: A, B". */
export function groupByReason(items: { name: string; reason: string }[]): string {
	const groups = new Map<string, string[]>();
	for (const item of items) {
		groups.set(item.reason, [...(groups.get(item.reason) ?? []), item.name]);
	}
	return [...groups].map(([reason, names]) => `${reason}: ${names.join(', ')}`).join('; ');
}

/** Why a named session cannot answer, in the player's terms. */
export function statusMessage(
	status: SkillingForecastStatus,
	sessionName: string,
	targetName: string,
): string {
	switch (status) {
		case 'ready':
			return '';
		case 'reached':
			return 'Goal reached.';
		case 'does_not_train':
			return `${sessionName} does not train ${targetName}.`;
		case 'no_evidence':
			return `${sessionName} has no recorded cycling.`;
		case 'out_of_range':
			return `Out of ${sessionName}'s reach.`;
	}
}

/** A short status for a picker row. */
export function statusLabel(status: SkillingForecastStatus): string {
	switch (status) {
		case 'ready':
			return '';
		case 'reached':
			return 'Goal reached';
		case 'does_not_train':
			return 'Does not train this';
		case 'no_evidence':
			return 'No recorded cycling';
		case 'out_of_range':
			return 'Out of reach';
	}
}

/** A sample caveat, with the figure that triggered it. */
export function warningText(
	warning: SkillingSampleWarning,
	sample: SkillingForecastSample,
): string {
	switch (warning) {
		case 'thin_sessions':
			return `only ${sample.sessions} ${sample.sessions === 1 ? 'session' : 'sessions'} recorded`;
		case 'thin_hours':
			return `only ${formatHours(sample.hours)} of play recorded`;
		case 'thin_cycling':
			return `only ${Math.round(sample.cycledPed)} PED cycled`;
		case 'long_extrapolation':
			return 'the goal lies far beyond the play recorded so far';
	}
}
