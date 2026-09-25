/**
 * Pure presentation logic for a session's healing evidence: what each paid
 * use's row says, which uncosted heals review offers, and how an output is
 * described in the player's terms rather than the tracker's.
 */

import type {
	HealingActivationRow,
	HealingCorrectionTool,
	HealingOutput,
	HealingOutputClassification,
	HealingSessionSummary,
} from '$lib/api/healing';

/** Where a paid use stands after any correction. */
export type ActivationStanding = 'paid' | 'notPaid' | 'corrected';

export function activationStanding(row: HealingActivationRow): ActivationStanding {
	if (row.superseded) return 'notPaid';
	if (row.provenance === 'corrected') return 'corrected';
	return 'paid';
}

/** The classifications review can list: the heals that carry no cost. */
export type UncostedClassification = Exclude<HealingOutputClassification, 'direct'>;

export interface ReviewFilter {
	id: UncostedClassification;
	label: string;
	count: number;
}

/** The uncosted heal groups with anything in them, unresolved first: those
 * are the ones most likely to be a missed paid use. */
export function reviewFilters(healing: HealingSessionSummary): ReviewFilter[] {
	const filters: ReviewFilter[] = [
		{ id: 'unattributed', label: 'Unresolved', count: healing.unattributedOutputs },
		{ id: 'effect', label: 'Effect ticks', count: healing.effectOutputs },
		{ id: 'passive', label: 'Lifesteal', count: healing.passiveOutputs },
	];
	return filters.filter((filter) => filter.count > 0);
}

export function uncostedCount(healing: HealingSessionSummary): number {
	return healing.unattributedOutputs + healing.effectOutputs + healing.passiveOutputs;
}

function plural(count: number, one: string, many: string): string {
	return `${count} ${count === 1 ? one : many}`;
}

/** The section's one-line tally. Ticks are never counted as uses. */
export function evidenceTally(healing: HealingSessionSummary): string {
	const parts = [plural(healing.activationCount, 'paid use', 'paid uses')];
	if (healing.effectOutputs > 0)
		parts.push(plural(healing.effectOutputs, 'effect tick', 'effect ticks'));
	if (healing.passiveOutputs > 0) parts.push(`${healing.passiveOutputs} lifesteal`);
	if (healing.unattributedOutputs > 0) parts.push(`${healing.unattributedOutputs} unresolved`);
	return parts.join(' · ');
}

/** A heal amount in the game's own unit. */
export function formatHeal(amount: number): string {
	const rounded = Math.round(amount * 10) / 10;
	return `${Number.isInteger(rounded) ? rounded.toFixed(0) : rounded.toFixed(1)} HP`;
}

/** What an uncosted heal was, in the player's terms. */
export function describeOutput(output: HealingOutput): string {
	if (output.correction?.kind === 'notPaidUse') return 'Its paid use was taken back';
	if (output.correction?.kind === 'paidUse') return 'Tick of a use you marked as paid';
	switch (output.classification) {
		case 'effect':
			return output.toolName
				? `Tick of ${output.toolName}`
				: 'Tick of more than one running effect';
		case 'passive':
			return 'Came with damage dealt';
		case 'unattributed':
			return 'No paid use explains it';
		case 'direct':
			return output.toolName ? `Paid use of ${output.toolName}` : 'Paid use';
	}
}

/** The label for one item offered to bill a heal as its paid use. */
export function toolOption(
	tool: HealingCorrectionTool,
	formatCost: (value: number) => string,
): string {
	return `${tool.name} · ${formatCost(tool.costPerUsePed)} PED`;
}

/** A time of day to the second, in the player's locale. */
export function formatClock(epoch: number): string {
	return new Date(epoch * 1000).toLocaleTimeString([], {
		hour: '2-digit',
		minute: '2-digit',
		second: '2-digit',
	});
}
