/**
 * A session's measured costs item by item, and its overhead, in the
 * player's words.
 *
 * The readings bill each item's use to one bucket: one of Cycled's (the
 * cost of performing the activity) or one of the overhead categories (what
 * the session cost beyond it: travel, field logistics, and what the app
 * cannot place yet). Overhead is in the session's Net, never in Cycled.
 * Nothing here sums a cost of its own: every figure is the backend's.
 *
 * An item the app cannot place is the player's to place, once, for every
 * session; the choices offered are the plain ones a player would name.
 */

import type {
	CostBreakdown,
	CostPlacementAwaiting,
	CostPlacementCategory,
	CostReason,
	MeasuredArmourPart,
	MeasuredCostBucket,
	MeasuredCostDetail,
	MeasuredCostGroup,
	MeasuredCostLine,
	MeasuredCostUnpriced,
	MeasuredPrecision,
} from '$lib/api/commands.gen';
import { formatPercent, NO_DATA } from '$lib/utils/format';
import { formatMeasured, formatUnits } from './measure';

/** Every bucket's name, in the order the detail lists them. */
export const BUCKET_LABELS: Record<MeasuredCostBucket, string> = {
	weapon: 'Weapon',
	enhancer: 'Enhancers',
	harvesting: 'Harvesting',
	healing: 'Healing',
	armour: 'Armour',
	consumables: 'Consumables',
	travel: 'Travel',
	fieldLogistics: 'Field logistics',
	unclassified: 'Unclassified',
};

/** The overhead categories, in the order they are listed. */
export const OVERHEAD_BUCKETS = ['travel', 'fieldLogistics', 'unclassified'] as const;
export type OverheadBucket = (typeof OVERHEAD_BUCKETS)[number];

function isOverhead(bucket: MeasuredCostBucket): bucket is OverheadBucket {
	return (OVERHEAD_BUCKETS as readonly MeasuredCostBucket[]).includes(bucket);
}

/** One overhead category of a session: its amount, the lines behind it,
 * and why it is withheld from the figures, where it is. */
export interface OverheadCategory {
	bucket: OverheadBucket;
	label: string;
	amount: number;
	lines: MeasuredCostLine[];
	reason: CostReason | null;
}

function linesOf(measured: MeasuredCostDetail | null, bucket: MeasuredCostBucket) {
	return measured?.buckets.find((group) => group.bucket === bucket)?.lines ?? [];
}

/** The session's overhead by category: only the categories holding an
 * amount or a line, or withheld with a reason, in the fixed order. */
export function overheadCategories(breakdown: CostBreakdown): OverheadCategory[] {
	return OVERHEAD_BUCKETS.map((bucket) => ({
		bucket,
		label: BUCKET_LABELS[bucket],
		amount: breakdown.overhead[bucket],
		lines: linesOf(breakdown.measured, bucket),
		reason: breakdown.overhead.reasons[bucket],
	})).filter(
		(category) => category.amount !== 0 || category.lines.length > 0 || category.reason !== null,
	);
}

/** What a category that bills nothing says in place of a figure: withheld
 * with a reason, or holding only items to place; `null` for a category
 * with a figure to show. */
export function overheadWithheld(category: OverheadCategory): 'reason' | 'toPlace' | null {
	if (category.reason !== null) return 'reason';
	if (category.amount === 0 && category.lines.length > 0) return 'toPlace';
	return null;
}

/** Whether the session has any overhead to show. */
export function hasOverhead(breakdown: CostBreakdown): boolean {
	return overheadCategories(breakdown).length > 0;
}

/** Cycled's measured buckets, item by item; the overhead categories are
 * shown with the overhead, never here. */
export function cycledGroups(measured: MeasuredCostDetail | null): MeasuredCostGroup[] {
	return (measured?.buckets ?? []).filter(
		(group) => !isOverhead(group.bucket) && group.lines.length > 0,
	);
}

// ── A line in words ─────────────────────────────────────────────────────

/** Why a line is charged nothing. */
export const UNPRICED: Record<MeasuredCostUnpriced, string> = {
	boughtThroughLedger: 'not charged here: you record its purchase in the ledger',
	limitedSetUnconfirmed: "no price yet: its set's pieces need confirming",
	notPlaced: 'not charged: place it if it was used up',
};

function markup(percent: number): string {
	return `${formatPercent(percent / 100)} markup`;
}

/** What a line measured, in a few words: the units used, or the TT lost,
 * and any markup applied. */
export function lineQuantity(line: MeasuredCostLine): string {
	const units = line.units === null ? null : formatUnits(line.units);
	const plural = (n: number) => (Math.abs(n) === 1 ? 'unit' : 'units');
	switch (line.kind) {
		case 'ammoBurned':
			return units === null ? 'burned' : `${units} ${plural(line.units ?? 0)} burned`;
		case 'consumed': {
			const used = units === null ? 'used up' : `${units} ${plural(line.units ?? 0)} used`;
			return line.markupPercent !== null ? `${used}, at ${markup(line.markupPercent)}` : used;
		}
		case 'decay':
			return 'TT lost';
		case 'limitedDecay':
			return line.markupPercent !== null
				? `TT lost, priced at its set's ${markup(line.markupPercent)}`
				: 'TT lost';
	}
}

const PRECISION_TITLE: Record<MeasuredPrecision, string> = {
	exact: 'Exact',
	wholePec: 'Read from values the site rounds down to the PEC',
};

/** A line's figure: its amount at the precision it is known to, or why it
 * has none. */
export function lineFigure(line: MeasuredCostLine): { text: string; title: string | null } {
	if (line.amount === null) {
		return { text: line.unpriced ? UNPRICED[line.unpriced] : NO_DATA, title: null };
	}
	const precision = line.precision ?? 'exact';
	return {
		text: formatMeasured({ ped: line.amount, precision }),
		title: PRECISION_TITLE[precision],
	};
}

/** One part of the session's armour, in words, and what it billed. */
export function armourPartLine(part: MeasuredArmourPart): {
	label: string;
	detail: string;
	cost: { text: string; title: string | null };
} {
	const lost = formatMeasured({ ped: part.lost, precision: part.lostPrecision });
	let label: string;
	let detail = `${lost} TT lost`;
	switch (part.part) {
		case 'unlimited':
			label = 'Unlimited armour and plates';
			break;
		case 'limitedSet':
			label = part.setName ?? 'A limited set';
			if (part.markupPercent !== null) detail += `, at ${markup(part.markupPercent)}`;
			break;
		case 'unconfirmed':
			label = 'Limited pieces in no confirmed set';
			break;
	}
	const cost =
		part.cost === null
			? { text: UNPRICED.limitedSetUnconfirmed, title: null }
			: {
					text: formatMeasured({ ped: part.cost, precision: part.costPrecision ?? 'exact' }),
					title: PRECISION_TITLE[part.costPrecision ?? 'exact'],
				};
	return { label, detail, cost };
}

/** Whether any figure Cycled's item-by-item groups list rests on values
 * the site rounds down to the PEC: the overhead's lines, listed with the
 * overhead, are not among them. */
export function costsListWholePec(measured: MeasuredCostDetail | null): boolean {
	if (!measured) return false;
	return (
		cycledGroups(measured).some((group) =>
			group.lines.some((line) => line.precision === 'wholePec'),
		) ||
		measured.armourParts.some(
			(part) => part.lostPrecision === 'wholePec' || part.costPrecision === 'wholePec',
		)
	);
}

/** What a line says when a ledger expense pegged to the session may record
 * the same cost by hand, quoting the entry as the player wrote it. Neither
 * figure is changed. */
export function peggedLedgerNote(description: string | null): string {
	return description
		? `You also recorded this in the ledger: ${description}`
		: 'You also recorded this in the ledger, pegged to this session';
}

// ── Placing an item ─────────────────────────────────────────────────────

/** Where the player may place an item, as the menu offers it. */
export interface PlaceChoice {
	label: string;
	category: CostPlacementCategory;
	group: 'cycled' | 'overhead' | 'none';
	/** The stored categories that read as this choice. */
	covers: readonly CostPlacementCategory[];
}

/** The choices the menu offers, in order. A healing item used up by the
 * unit is costed as one by its use, so Healing needs no second choice. */
export const PLACE_CHOICES: readonly PlaceChoice[] = [
	{
		label: 'Weapon',
		category: 'weapon',
		group: 'cycled',
		covers: ['weapon', 'amplifier', 'attachment', 'mindforceImplant'],
	},
	{
		label: 'Harvesting',
		category: 'harvestingTool',
		group: 'cycled',
		covers: ['harvestingTool'],
	},
	{
		label: 'Healing',
		category: 'healingTool',
		group: 'cycled',
		covers: ['healingTool', 'healingConsumable'],
	},
	{
		label: 'Armour',
		category: 'unlimitedArmour',
		group: 'cycled',
		covers: ['unlimitedArmour', 'unlimitedPlate'],
	},
	{ label: 'Consumable', category: 'consumable', group: 'cycled', covers: ['consumable'] },
	{ label: 'Travel', category: 'travel', group: 'overhead', covers: ['travel'] },
	{
		label: 'Field logistics',
		category: 'fieldLogistics',
		group: 'overhead',
		covers: ['fieldLogistics'],
	},
	{ label: 'Not a cost', category: 'notACost', group: 'none', covers: ['notACost'] },
];

/** How a stored placement reads. */
export const PLACEMENT_LABELS: Record<CostPlacementCategory, string> = {
	weapon: 'Weapon',
	amplifier: 'Weapon (amplifier)',
	attachment: 'Weapon (attachment)',
	mindforceImplant: 'Weapon (mindforce implant)',
	harvestingTool: 'Harvesting',
	healingTool: 'Healing',
	healingConsumable: 'Healing',
	unlimitedArmour: 'Armour',
	unlimitedPlate: 'Armour (plate)',
	consumable: 'Consumable',
	travel: 'Travel',
	fieldLogistics: 'Field logistics',
	notACost: 'Not a cost',
};

/** What an item awaiting a placement did across the sessions it was in:
 * the TT it lost wearing, and the units of it that left. */
export function awaitingSummary(entry: CostPlacementAwaiting): string {
	const parts = [`${entry.sessions} ${entry.sessions === 1 ? 'session' : 'sessions'}`];
	if (entry.lost !== null) parts.push(`${formatMeasured(entry.lost)} TT lost`);
	if (entry.units !== null) {
		const noun = Math.abs(entry.units) === 1 ? 'unit' : 'units';
		parts.push(`${formatUnits(entry.units)} ${noun} left`);
	}
	return parts.join(', ');
}

/** Whether a choice is the one an item is placed in now. */
export function isCurrentChoice(
	choice: PlaceChoice,
	current: CostPlacementCategory | null,
): boolean {
	return current !== null && choice.covers.includes(current);
}
