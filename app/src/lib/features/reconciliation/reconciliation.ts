/**
 * What the stock check reads off a draft: which way each line differs, what
 * it can be set to, what its difference is worth, and what applying the
 * whole draft will do. The draft and every rule that decides whether a
 * change fits live in the backend; this only reads them for display and
 * keeps a bulk change to the lines it can apply to.
 */

import type {
	StockReconciliationApplied,
	StockReconciliationChoice,
	StockReconciliationDisposition,
	StockReconciliationDraft,
	StockReconciliationExclusion,
	StockReconciliationGroup,
	StockReconciliationLine,
	StockReconciliationRefusal,
	StockReconciliationUnchanged,
} from '$lib/api/commands.gen';
import { formatQuantity, formatSeenAt, formatTt } from '$lib/utils/format';

export type Line = StockReconciliationLine;
export type Draft = StockReconciliationDraft;
export type Choice = StockReconciliationChoice;
export type Applied = StockReconciliationApplied;

/** A missing stack worth at least this much TT is read first and marked:
 * the review shows a large shortfall rather than explaining it away. */
export const LARGE_SHORTFALL_TT = 10;

/** Units are compared after the stock's own rounding. */
const EPSILON = 1e-9;

export function isShortfall(line: Line): boolean {
	return line.difference < -EPSILON;
}

export function isSurplus(line: Line): boolean {
	return line.difference > EPSILON;
}

/** Shrapnel is the one item whose shortfall may be a conversion into
 * Universal Ammo; the account and the app both call it plainly that. */
export function isShrapnel(line: Line): boolean {
	return line.itemName.trim() === 'Shrapnel';
}

/** The conversion a line belongs to, if any. */
export function groupOf(disposition: StockReconciliationDisposition): number | null {
	return disposition.kind === 'conversionGroup' ? disposition.group : null;
}

/** The TT of a line's difference, and whether it is a floor (the account's
 * values round each row down to the PEC); `null` when no TT per unit is
 * known. */
export function differenceTt(line: Line): { ped: number; floor: boolean } | null {
	if (line.unitTt.kind === 'unknown') return null;
	return {
		ped: Math.abs(line.difference) * line.unitTt.ped,
		floor: line.unitTt.kind === 'truncated',
	};
}

export function isLargeShortfall(line: Line): boolean {
	const tt = differenceTt(line);
	return isShortfall(line) && tt !== null && tt.ped >= LARGE_SHORTFALL_TT;
}

/** The difference as a person writes it: "+12", "−300". */
export function signedQuantity(value: number): string {
	return `${value >= 0 ? '+' : '−'}${formatQuantity(Math.abs(value))}`;
}

export interface ChoiceOption {
	id: Choice;
	label: string;
	title: string;
}

const REMOVED: ChoiceOption = {
	id: 'removed',
	label: 'Removed',
	title: 'The missing units leave your stock; no session loses its returns',
};
const CONVERTED: ChoiceOption = {
	id: 'shrapnelConverted',
	label: 'Converted',
	title: 'The missing Shrapnel became Universal Ammo',
};
const UNKNOWN_ORIGIN: ChoiceOption = {
	id: 'unknownOrigin',
	label: 'Unknown origin',
	title: 'The extra units are held as stock no activity looted: their TT is never income',
};
const UNKNOWN_ORIGIN_AT_ACCOUNT_TT: ChoiceOption = {
	id: 'unknownOrigin',
	label: 'Unknown origin',
	title:
		"The extra units are held as stock no activity looted, at your account's TT for them (at most a PEC a row low): their TT is never income",
};
const LEFT_AS_IS: ChoiceOption = {
	id: 'leftAsIs',
	label: 'Left as is',
	title: 'Nothing is recorded for it',
};

/** Whether the line's TT per unit is known exactly: only then can its
 * extra units be valued as stock, or the line balance a conversion. */
export function hasExactTt(line: Line): boolean {
	return line.unitTt.kind === 'exact';
}

/** Whether the line's TT per unit is known at all: exactly, or as the
 * account shows it. */
export function hasKnownTt(line: Line): boolean {
	return line.unitTt.kind !== 'unknown';
}

/** How a surplus whose TT per unit is not exact is valued, and why it
 * cannot join a conversion. */
export const INEXACT_TT_REASON =
	"Its TT per unit is not known exactly: as stock of unknown origin it is valued at your account's TT for it (at most a PEC a row low), and it cannot balance a conversion.";

/** What a line can be set to on its own: a shortfall is removed (missing
 * Shrapnel may instead have been converted), a surplus is of unknown origin
 * when its TT per unit is known (at the account's own TT when not exact),
 * and either may be left as it is. Joining a conversion is done from a
 * selection. */
export function choicesFor(line: Line): ChoiceOption[] {
	if (isSurplus(line)) {
		if (!hasKnownTt(line)) return [LEFT_AS_IS];
		return [hasExactTt(line) ? UNKNOWN_ORIGIN : UNKNOWN_ORIGIN_AT_ACCOUNT_TT, LEFT_AS_IS];
	}
	if (isShrapnel(line)) return [CONVERTED, REMOVED, LEFT_AS_IS];
	return [REMOVED, LEFT_AS_IS];
}

/** The choice a line stands at; `null` while it is part of a conversion. */
export function choiceOf(disposition: StockReconciliationDisposition): Choice | null {
	return disposition.kind === 'conversionGroup' ? null : disposition.kind;
}

/** What a line taken out of a conversion goes back to. */
export function standaloneChoice(line: Line): Choice {
	if (line.takenBack) return 'leftAsIs';
	if (isSurplus(line)) return hasKnownTt(line) ? 'unknownOrigin' : 'leftAsIs';
	return isShrapnel(line) ? 'shrapnelConverted' : 'removed';
}

/** What a bulk choice came to: "Set on 8; 2 did not fit: ...". `null` when
 * it was set on every line sent. */
export function bulkOutcome(
	sent: number,
	unchanged: StockReconciliationUnchanged[],
): string | null {
	if (unchanged.length === 0) return null;
	const shown = unchanged.slice(0, 3).map((row) => row.reason.trim().replace(/\.$/, ''));
	const more = unchanged.length - shown.length;
	if (more > 0) shown.push(`and ${formatQuantity(more)} more`);
	const set = formatQuantity(sent - unchanged.length);
	const left = formatQuantity(unchanged.length);
	return `Set on ${set}; ${left} did not fit: ${shown.join('; ')}.`;
}

/** The other items of a line's conversion. */
export function groupPartners(draft: Draft, line: Line): Line[] {
	const group = groupOf(line.disposition);
	if (group === null) return [];
	return draft.lines.filter(
		(other) => other.itemName !== line.itemName && groupOf(other.disposition) === group,
	);
}

/** How a line in a conversion reads: the missing items went into the one
 * that arrived. */
export function conversionSentence(draft: Draft, line: Line): string {
	const partners = groupPartners(draft, line);
	if (isSurplus(line)) {
		return partners.length === 1
			? `Made from ${partners[0].itemName}`
			: `Made from ${partners.length} items`;
	}
	const made = partners.find(isSurplus);
	return made ? `Went into ${made.itemName}` : 'Part of a conversion';
}

/** The figures of a line's conversion, when it is in one. */
export function groupFiguresOf(draft: Draft, line: Line): StockReconciliationGroup | null {
	const group = groupOf(line.disposition);
	if (group === null) return null;
	return draft.groups.find((figures) => figures.group === group) ?? null;
}

/** What went into a conversion and what came out, in TT. */
export function conversionTt(figures: StockReconciliationGroup): string {
	return `${formatTt(figures.ttIn)} PED of TT in, ${formatTt(figures.ttOut)} PED out`;
}

/** The part of a conversion's output its inputs cannot have paid for. */
export function remainderSentence(figures: StockReconciliationGroup): string | null {
	if (!(figures.remainderQuantity > 0)) return null;
	const extra = `${formatQuantity(figures.remainderQuantity)} more ${figures.outputItem}`;
	return `${extra} arrived some other way and is kept as stock of unknown origin.`;
}

function count(n: number, one: string, many: string): string {
	return `${formatQuantity(n)} ${n === 1 ? one : many}`;
}

/** What became of the missing Shrapnel: converted, partly sold at TT, or
 * all of it sold at TT. */
function shrapnelPhrase(soldAtTt: number | null, missing: number): string {
	if (soldAtTt === null) return 'Shrapnel converted to Universal Ammo';
	if (soldAtTt >= missing - EPSILON) return 'Shrapnel sold at TT';
	return `Shrapnel converted, ${formatQuantity(soldAtTt)} of it sold at TT`;
}

/** The items applying settled, split by what the applied draft said of
 * them: those it recorded something for, and those left as they are. */
export function splitSettled(
	settled: string[],
	draft: Draft | null,
): { changed: string[]; leftAsIs: string[] } {
	const left = new Set(
		(draft?.lines ?? [])
			.filter((line) => line.disposition.kind === 'leftAsIs')
			.map((line) => line.itemName),
	);
	return {
		changed: settled.filter((item) => !left.has(item)),
		leftAsIs: settled.filter((item) => left.has(item)),
	};
}

/** What applying the draft will do, a phrase per kind of change: "12 items
 * removed from stock, 1 conversion, 2 surpluses added". */
export function applySummary(draft: Draft): string {
	let removed = 0;
	let surpluses = 0;
	let left = 0;
	let shrapnel: { soldAtTt: number | null; missing: number } | null = null;
	const groups = new Set<number>();
	for (const line of draft.lines) {
		const { disposition } = line;
		if (disposition.kind === 'removed') removed += 1;
		else if (disposition.kind === 'unknownOrigin') surpluses += 1;
		else if (disposition.kind === 'leftAsIs') left += 1;
		else if (disposition.kind === 'shrapnelConverted') {
			shrapnel = { soldAtTt: disposition.soldAtTt, missing: Math.abs(line.difference) };
		} else groups.add(disposition.group);
	}
	const parts: string[] = [];
	if (removed > 0) parts.push(`${count(removed, 'item', 'items')} removed from stock`);
	if (shrapnel !== null) parts.push(shrapnelPhrase(shrapnel.soldAtTt, shrapnel.missing));
	if (groups.size > 0) parts.push(count(groups.size, 'conversion', 'conversions'));
	if (surpluses > 0) parts.push(`${count(surpluses, 'surplus', 'surpluses')} added`);
	if (parts.length === 0) {
		return left === 1
			? 'Nothing changes: the item is left as it is'
			: 'Nothing changes: every item is left as it is';
	}
	if (left > 0) parts.push(`${formatQuantity(left)} left as ${left === 1 ? 'it is' : 'they are'}`);
	const sentence = parts.join(', ');
	return sentence.charAt(0).toUpperCase() + sentence.slice(1);
}

/** Why an item was left out of the comparison, in the player's terms. */
export const EXCLUSION_LABEL: Record<StockReconciliationExclusion, string> = {
	universalAmmo: 'PED, not stock',
	ammo: 'Ammunition, not stock',
	equipment: 'Equipment',
	armour: 'Armour',
	plate: 'Armour plating',
	attached: 'Attached to another item',
	auction: 'Listed at auction',
	auctionWithoutListing: 'More listed at auction than the app has listings for',
	pendingProposal: 'A proposal from your account is waiting on your decision',
	consumable: 'Used up as a cost, and not held as stock',
	strongbox: 'Opened in the strongbox view',
	singleItem: 'A single item the app has never held as stock',
	sharedName: 'Two items in the app share the name',
	ttDiffers: "Its TT does not fit the item's: another item of the name, or its TT has changed",
	heldAsAsset: 'More is missing than the loose stock holds: the rest is an asset',
};

/** When a difference was accepted, in a few words. */
export function acceptedWhen(line: Line): string {
	return line.acceptedAt ? `Accepted ${formatSeenAt(line.acceptedAt)}` : 'Accepted before';
}

/** Why a line was not applied, in the player's terms. */
export const REFUSAL_LABEL: Record<StockReconciliationRefusal, string> = {
	stockMovedSince: 'loot or a change of it was recorded after your account was read',
	changedSincePlanned: 'what the app holds of it changed while you reviewed',
	groupRefused: 'another item of its conversion could not be applied',
};

/** A reason the backend gives, read as a sentence: capitalised, and ended. */
export function asSentence(message: string): string {
	const trimmed = message.trim();
	if (trimmed === '') return trimmed;
	const capital = trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
	return /[.!?:]$/.test(capital) ? capital : `${capital}.`;
}
