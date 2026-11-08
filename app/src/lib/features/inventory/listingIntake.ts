/**
 * The listing-intake draft: what the game's sale window says, before any of
 * it becomes a transaction.
 *
 * The TT value is read, never worked out from what we hold. Our per-unit
 * figure is an average over recorded loot, and it drifts: a stack the game
 * values at 336.49 came to 336.34 by that route. A percentage being a
 * fraction out is cosmetic, but the TT value is what a markup is measured
 * against, so it is taken from the window and typed only when there is no
 * window to read.
 *
 * The markup percentages are not among them. In game they are a read-only
 * consequence of the TT value and the two bids, so they are derived here the
 * same way rather than recorded: a figure the player cannot set is not a
 * figure to ask them for, and one fewer thing to read off the screen later.
 *
 * One wrinkle that follows from deriving it. The window rounds the TT it
 * displays to two decimals but computes its own percentage against the
 * unrounded value, so a percentage derived from the shown TT can differ from
 * the shown percentage in the last decimal (275.48 and 9276 give 3367.21%
 * where the window says 3367.10%, its TT really being 275.4889). Both are
 * display figures and nothing accounts on either, so the derivation stands.
 *
 * This module is deliberately pure and intake-neutral. A typed form and a
 * screen capture fill the same shape and meet the same checks, so a misread
 * cannot reach the ledger by a path a typo could not.
 */

/** One thing the player holds, with what the app already knows about it: the
 * per-unit TT it was recorded at, so a quantity is enough to say what a sale
 * is worth, and the quantity held, so an overrun is visible. */
export interface HoldingOption {
	kind: string;
	holdingId: string;
	name: string;
	score: number;
	heldQty: number | null;
	/** Markup paid per unit that a sale of this holding must recover. */
	paidPerUnit?: number;
}

/** What the sale window states, as read or typed. Percentages are the game's
 * own display form: 102.5 means 102.50%. */
export interface ListingDraftFields {
	itemName: string;
	quantity: number | null;
	ttValue: number | null;
	auctionFee: number | null;
	auctionDays: number | null;
	startingBid: number | null;
	buyout: number | null;
}

export interface DraftIssue {
	/** Which field the reader should look at first. */
	field: keyof ListingDraftFields;
	/** `blocking` stops the commit; `advisory` is worth saying and no more. */
	severity: 'blocking' | 'advisory';
	message: string;
}

export const EMPTY_DRAFT: ListingDraftFields = {
	itemName: '',
	quantity: null,
	ttValue: null,
	auctionFee: null,
	auctionDays: null,
	startingBid: null,
	buyout: null,
};

/** The markup a bid represents against a TT value, in the game's percentage
 * form. `null` when TT is missing or zero, where the ratio says nothing. */
export function impliedMarkupPct(bid: number | null, ttValue: number | null): number | null {
	if (bid === null || ttValue === null || ttValue <= 0) return null;
	return (bid / ttValue) * 100;
}

/**
 * Everything wrong with a draft, worst first. An empty list means it can be
 * committed; any `blocking` entry means it cannot.
 */
export function draftIssues(draft: ListingDraftFields, channel: 'auction' | 'trade'): DraftIssue[] {
	const issues: DraftIssue[] = [];

	if (draft.itemName.trim() === '') {
		issues.push({ field: 'itemName', severity: 'blocking', message: 'Name the item being sold.' });
	}
	if (draft.quantity === null || draft.quantity <= 0) {
		issues.push({
			field: 'quantity',
			severity: 'blocking',
			message: 'Quantity must be above zero.',
		});
	}
	if (draft.ttValue !== null && draft.ttValue < 0) {
		issues.push({
			field: 'ttValue',
			severity: 'blocking',
			message: 'TT value cannot be negative.',
		});
	}

	if (channel === 'auction') {
		if (draft.startingBid === null || draft.startingBid <= 0) {
			issues.push({
				field: 'startingBid',
				severity: 'blocking',
				message: 'An auction needs a starting bid.',
			});
		}
		if (draft.auctionFee !== null && draft.auctionFee < 0) {
			issues.push({
				field: 'auctionFee',
				severity: 'blocking',
				message: 'A fee cannot be negative.',
			});
		}
		if (
			draft.auctionDays !== null &&
			(draft.auctionDays <= 0 || !Number.isInteger(draft.auctionDays))
		) {
			issues.push({
				field: 'auctionDays',
				severity: 'blocking',
				message: 'A listing runs for a whole number of days.',
			});
		}
		// A buyout under the starting bid is not a misreading we can resolve
		// for the player, and the game would not have accepted it, so it is
		// reported as the contradiction it is.
		if (draft.buyout !== null && draft.startingBid !== null && draft.buyout < draft.startingBid) {
			issues.push({
				field: 'buyout',
				severity: 'blocking',
				message: 'The buyout is below the starting bid.',
			});
		}

		// Not an error: selling at or under TT is a real (if grim) choice, and
		// the recycler is often the better one. Worth saying once, not blocking.
		const impliedSb = impliedMarkupPct(draft.startingBid, draft.ttValue);
		if (impliedSb !== null && impliedSb < 100) {
			issues.push({
				field: 'startingBid',
				severity: 'advisory',
				message: 'The starting bid is below TT, so a sale at that price loses value.',
			});
		}
	} else if (draft.buyout === null || draft.buyout <= 0) {
		// A trade has one price and no fee, bid, or duration.
		issues.push({
			field: 'buyout',
			severity: 'blocking',
			message: 'A trade needs the price it sold for.',
		});
	}

	return issues.sort((a, b) =>
		a.severity === b.severity ? 0 : a.severity === 'blocking' ? -1 : 1,
	);
}

/** Whether the draft may cross the commit boundary. */
export function isCommittable(draft: ListingDraftFields, channel: 'auction' | 'trade'): boolean {
	return !draftIssues(draft, channel).some((issue) => issue.severity === 'blocking');
}

/** The net a sale at this price would leave, after the fee already spent. Not
 * a realised figure: nothing is realised until an auction actually closes. */
export function previewNetMarkup(
	draft: ListingDraftFields,
	channel: 'auction' | 'trade',
): number | null {
	const price = channel === 'trade' ? draft.buyout : (draft.buyout ?? draft.startingBid);
	if (price === null || draft.ttValue === null) return null;
	const fee = channel === 'auction' ? (draft.auctionFee ?? 0) : 0;
	return price - draft.ttValue - fee;
}

/** What one read of the sale window said, in the shape the capture command
 * answers. Declared structurally rather than imported so this module stays
 * free of the generated wire types, as the rest of it is. */
export interface CapturedSaleWindow {
	observedName: string | null;
	quantity: number | null;
	ttValue: number | null;
	listingFee: number | null;
	auctionDays: number | null;
	startingBid: number | null;
	buyout: number | null;
}

/** A draft filled from a captured window.
 *
 * A capture replaces the form rather than merging into it. It is one look at
 * one window, so combining it with half-typed values would produce a listing
 * matching neither the screen nor the player's own entry. Fields the read
 * refused arrive null and stay null: an empty box asks to be filled, while a
 * plausible wrong number does not. */
export function capturedDraft(read: CapturedSaleWindow): ListingDraftFields {
	return {
		itemName: read.observedName ?? '',
		quantity: read.quantity,
		ttValue: read.ttValue,
		auctionFee: read.listingFee,
		auctionDays: read.auctionDays,
		startingBid: read.startingBid,
		buyout: read.buyout,
	};
}
