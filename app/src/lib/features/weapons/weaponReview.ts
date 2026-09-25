/**
 * Pure presentation logic for a session's weapon attribution: the tally of
 * how its shots were priced, the review groups, the damage-over-time
 * effects, and each stored shot and decision described in the player's
 * terms rather than the tracker's.
 */

import type {
	WeaponAttributionSummary,
	WeaponCorrectionWeapon,
	WeaponEffectCandidate,
	WeaponEffectRow,
	WeaponReviewRow,
	WeaponShot,
	WeaponShotGroup,
} from '$lib/api/weapons';
import { formatClock } from '$lib/features/healing/healingReview';

export interface ReviewGroup {
	id: WeaponShotGroup;
	label: string;
	count: number;
}

function plural(count: number, one: string, many: string): string {
	return `${count} ${count === 1 ? one : many}`;
}

/** Whether the session has anything to say about its weapons. */
export function hasWeaponEvidence(summary: WeaponAttributionSummary): boolean {
	return (
		(summary.agreed ?? 0) + (summary.evidenced ?? 0) > 0 ||
		summary.unresolved > 0 ||
		summary.evidenceShots > 0 ||
		summary.effectTicks > 0 ||
		summary.effects.length > 0 ||
		summary.reviews.length > 0
	);
}

/** The section's one-line tally: how the session's shots were priced. The
 * two live tallies are absent for a session recorded before they were kept,
 * so the line then counts only what was stored. */
export function attributionTally(summary: WeaponAttributionSummary): string {
	const parts: string[] = [];
	if (summary.agreed != null && summary.agreed > 0) {
		parts.push(`${summary.agreed} matched the hotbar`);
	}
	if (summary.evidenced != null && summary.evidenced > 0) {
		parts.push(`${summary.evidenced} by damage range`);
	}
	if (summary.unresolved > 0) {
		parts.push(plural(summary.unresolved, 'unresolved', 'unresolved'));
	}
	if (summary.effectTicks > 0) {
		parts.push(plural(summary.effectTicks, 'effect tick', 'effect ticks'));
	}
	return parts.join(' · ');
}

/** The groups review can list, the ones needing the player first. */
export function reviewGroups(summary: WeaponAttributionSummary): ReviewGroup[] {
	const groups: ReviewGroup[] = [
		{ id: 'unresolved', label: 'Unresolved', count: summary.unresolved },
		{ id: 'evidence', label: 'Overrode the hotbar', count: summary.evidenceShots },
		{ id: 'effect_tick', label: 'Effect ticks', count: summary.effectTicks },
	];
	return groups.filter((group) => group.count > 0);
}

/** A damage figure as the game prints it. */
export function formatDamage(amount: number): string {
	return amount.toFixed(1);
}

/** An open effect as a player recognises it: the weapon and when it was cast. */
export function describeEffect(
	effect: Pick<WeaponEffectCandidate, 'toolName' | 'activatedAt'> &
		Partial<Pick<WeaponEffectCandidate, 'standing'>>,
): string {
	const cast = `${effect.toolName}, cast ${formatClock(effect.activatedAt)}`;
	return effect.standing === false ? `${cast} (taken back)` : cast;
}

/** One effect's ticks in this session: how many, and the damage they did. */
export function effectTickLine(effect: Pick<WeaponEffectRow, 'ticks' | 'tickDamage'>): string {
	const damage = effect.tickDamage.toLocaleString(undefined, {
		minimumFractionDigits: 1,
		maximumFractionDigits: 1,
	});
	return `${plural(effect.ticks, 'tick', 'ticks')} · ${damage} damage`;
}

/** What the shot was, in the player's terms. */
export function describeShot(shot: WeaponShot): string {
	const fitting = shot.candidates.filter((candidate) => candidate.fits).map((c) => c.name);
	const context = shot.hotbarTool ? `hotbar: ${shot.hotbarTool}` : 'no hotbar press';
	if (shot.amount == null) {
		return `A jam, dodge, evade, or miss with no weapon known (${context})`;
	}
	const effects = shot.effectCandidates;
	if (shot.group === 'effect_tick') {
		const effect = effects.find((candidate) => candidate.windowId === shot.effectWindowId);
		if (effect) return `A tick of ${describeEffect(effect)}`;
		if (effects.length > 1) {
			const tools = [...new Set(effects.map((candidate) => candidate.toolName))];
			return `A tick shared by ${effects.length} overlapping casts of ${tools.join(' and ')}`;
		}
		return 'A tick of a cast whose session was deleted';
	}
	if (effects.length > 0) {
		const lead = fitting.length > 0 ? `Fits ${fitting.join(' and ')}, or` : 'Could be';
		return `${lead} a tick of ${effects.map(describeEffect).join(' or ')} (${context})`;
	}
	if (shot.reviewDecision === 'kept') {
		return `Fit ${fitting.join(', ') || 'another weapon'}; you kept ${shot.hotbarTool ?? 'the hotbar weapon'}`;
	}
	if (fitting.length === 0) {
		return `Fits no carried weapon (${context})`;
	}
	if (fitting.length > 1) {
		return `Fits ${fitting.join(' and ')} (${context})`;
	}
	return `Fits only ${fitting[0]} (${context})`;
}

/** Where the shot's price stands now. */
export type ShotStanding =
	| { kind: 'unpriced' }
	| { kind: 'tick' }
	| { kind: 'marked'; tool: string; correctionId: string }
	| { kind: 'assigned'; tool: string; cost: number; correctionId: string }
	| { kind: 'confirmed'; tool: string; cost: number }
	| { kind: 'priced'; tool: string; cost: number };

export function shotStanding(shot: WeaponShot): ShotStanding {
	if (shot.correctionKind === 'effect_tick' && shot.correctionId != null) {
		const effect = shot.effectCandidates.find(
			(candidate) => candidate.windowId === shot.correctionWindowId,
		);
		return {
			kind: 'marked',
			tool: effect?.toolName ?? 'an earlier cast',
			correctionId: shot.correctionId,
		};
	}
	if (shot.toolName == null) {
		return shot.group === 'effect_tick' ? { kind: 'tick' } : { kind: 'unpriced' };
	}
	if (shot.correctionId != null) {
		return {
			kind: 'assigned',
			tool: shot.toolName,
			cost: shot.costPerShot,
			correctionId: shot.correctionId,
		};
	}
	if (shot.reviewDecision === 'confirmed') {
		return { kind: 'confirmed', tool: shot.toolName, cost: shot.costPerShot };
	}
	return { kind: 'priced', tool: shot.toolName, cost: shot.costPerShot };
}

/** One live decision on a mismatch, as a sentence. */
export function describeReview(review: WeaponReviewRow): string {
	const shots = plural(review.repricedShots, 'shot', 'shots');
	return review.decision === 'confirmed'
		? `Switched to ${review.evidenceTool} from ${review.hotbarTool}; ${shots} repriced`
		: `Kept ${review.hotbarTool} over ${review.evidenceTool}; ${shots} repriced`;
}

/** The label for one weapon offered for an unpriced shot. */
export function weaponOption(
	weapon: WeaponCorrectionWeapon,
	formatCost: (value: number) => string,
): string {
	return `${weapon.name}${weapon.fits ? ', fits' : ''} · ${formatCost(weapon.costPerShotPed)} PED`;
}
