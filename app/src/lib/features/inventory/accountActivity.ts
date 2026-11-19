/**
 * The Inventory's history split by who decided, as events the player reads
 * at a glance. Pure.
 *
 * - **From your account**: what the readings recorded on their own, one
 *   event for each thing a step did (its recycling, its Shrapnel
 *   conversion, its sales to the Trade Terminal together, each listing it
 *   created, each listing it settled), and what they found that waits for
 *   the player's decision. A listing the readings created is the account's
 *   from its making; once the player resolves it, that outcome is theirs
 *   too.
 * - **Reviewed**: what the player applied after a review, each a stock
 *   check or a strongbox opening as one event.
 * - **Your entries**: what the player entered by hand, row by row as
 *   before.
 *
 * What is new is read against how far the player has seen the account's
 * activity: an event recorded, or a proposal found, after that instant.
 */
import type {
	AccountProposal,
	ActivityHistoryEntry,
	AuctionListing,
	ConversionLine,
	InferredBid,
	RecordedFromAccount,
	StrongboxOpeningEntry,
} from '$lib/api/commands.gen';

export type HistorySpace = 'account' | 'reviewed' | 'yours';
export type HoldingSide = 'loot' | 'equipment';

/** One thing the account recorded on its own, as the player reads it. */
export type AccountEvent =
	| { kind: 'recycling'; key: string; step: RecordedFromAccount; entry: ActivityHistoryEntry }
	| { kind: 'shrapnel'; key: string; step: RecordedFromAccount; entry: ActivityHistoryEntry }
	| { kind: 'sale'; key: string; step: RecordedFromAccount; entries: ActivityHistoryEntry[] }
	| {
			kind: 'listed' | 'listingSold' | 'listingReturned';
			key: string;
			step: RecordedFromAccount;
			entry: ActivityHistoryEntry;
			listing: AuctionListing | null;
	  };

/** One thing the player applied after a review. */
export type ReviewedEvent =
	| {
			kind: 'check';
			key: string;
			reconciliationId: string;
			entries: ActivityHistoryEntry[];
			at: string;
	  }
	| { kind: 'opening'; key: string; opening: StrongboxOpeningEntry; at: string };

export interface HistorySplit {
	account: AccountEvent[];
	reviewed: ReviewedEvent[];
	yours: ActivityHistoryEntry[];
}

const UNIVERSAL_AMMO = /^universal ammo$/i;

/** Whether a conversion turned Shrapnel into Universal Ammo: an outcome in
 * PED, never stock. */
function isShrapnelConversion(entry: ActivityHistoryEntry): boolean {
	return entry.pedAmount !== null || UNIVERSAL_AMMO.test(entry.targetItem ?? '');
}

/** Which event of its step an entry's outcome belongs to (a listing the
 * readings created is an event of its own beside it). */
function familyOf(entry: ActivityHistoryEntry): Exclude<AccountEvent['kind'], 'listed'> {
	if (entry.kind === 'conversion') return isShrapnelConversion(entry) ? 'shrapnel' : 'recycling';
	if (entry.kind === 'trade') return 'sale';
	return entry.status === 'sold' ? 'listingSold' : 'listingReturned';
}

/** The instant an ISO date and time stands for, in milliseconds; never NaN. */
function instant(iso: string | null | undefined): number {
	const at = iso ? Date.parse(iso) : Number.NaN;
	return Number.isFinite(at) ? at : 0;
}

/** Split the history by who decided. A step's sales come together as one
 * event; a stock check's records come together as one. Newest first. */
export function splitHistory(
	entries: readonly ActivityHistoryEntry[],
	openings: readonly StrongboxOpeningEntry[],
	listings: readonly AuctionListing[],
): HistorySplit {
	const account = new Map<string, AccountEvent>();
	const checks = new Map<string, Extract<ReviewedEvent, { kind: 'check' }>>();
	const yours: ActivityHistoryEntry[] = [];
	for (const entry of entries) {
		if (entry.reconciliationId) {
			const id = entry.reconciliationId;
			const check = checks.get(id);
			if (check) {
				check.entries.push(entry);
				if (entry.occurredAt > check.at) check.at = entry.occurredAt;
			} else {
				checks.set(id, {
					kind: 'check',
					key: `check:${id}`,
					reconciliationId: id,
					entries: [entry],
					at: entry.occurredAt,
				});
			}
			continue;
		}
		const listed = entry.listedFromAccount;
		if (listed) {
			const key = `listed:${entry.id}`;
			const listing = listings.find((row) => row.id === entry.id) ?? null;
			account.set(key, { kind: 'listed', key, step: listed, entry, listing });
		}
		const step = entry.recordedFromAccount;
		if (!step) {
			// A listing the readings created waits on the auction, or was
			// taken back, as theirs alone; an outcome the player recorded
			// is theirs.
			const theirsAlone = listed && (entry.status === 'pending' || entry.undone);
			if (!theirsAlone) yours.push(entry);
			continue;
		}
		const kind = familyOf(entry);
		if (kind === 'sale') {
			const key = `sale:${step.readingId}`;
			const sale = account.get(key);
			if (sale?.kind === 'sale') sale.entries.push(entry);
			else account.set(key, { kind, key, step, entries: [entry] });
		} else if (kind === 'listingSold' || kind === 'listingReturned') {
			const key = `${kind}:${entry.id}`;
			const listing = listings.find((row) => row.id === entry.id) ?? null;
			account.set(key, { kind, key, step, entry, listing });
		} else {
			const key = `${kind}:${step.readingId}:${entry.id}`;
			account.set(key, { kind, key, step, entry });
		}
	}
	const reviewed: ReviewedEvent[] = [
		...checks.values(),
		...openings.map((opening) => ({
			kind: 'opening' as const,
			key: `opening:${opening.id}`,
			opening,
			at: opening.openedAt,
		})),
	];
	return {
		account: [...account.values()].sort(
			(a, b) => instant(b.step.to) - instant(a.step.to) || a.key.localeCompare(b.key),
		),
		reviewed: reviewed.sort((a, b) => b.at.localeCompare(a.at) || a.key.localeCompare(b.key)),
		yours,
	};
}

/** Whether an entry shows under the Loot or the Assets side. A conversion
 * can span both sides, so it shows on either side it touched. */
export function entryShowsUnder(entry: ActivityHistoryEntry, side: HoldingSide): boolean {
	return (
		entry.subjectKind === side ||
		(side === 'equipment' &&
			entry.conversionLines.some(
				(line) => line.holding === 'asset' || line.acquisition === 'asset',
			))
	);
}

export function accountEventEntries(event: AccountEvent): ActivityHistoryEntry[] {
	return event.kind === 'sale' ? event.entries : [event.entry];
}

export function accountEventShowsUnder(event: AccountEvent, side: HoldingSide): boolean {
	return accountEventEntries(event).some((entry) => entryShowsUnder(entry, side));
}

export function reviewedEventShowsUnder(event: ReviewedEvent, side: HoldingSide): boolean {
	if (event.kind === 'opening') return side === 'loot';
	return event.entries.some((entry) => entryShowsUnder(entry, side));
}

/** When the account recorded an event: its latest record. A listing it
 * created was recorded when it was made, whatever became of it since. */
export function recordedAt(event: AccountEvent): string {
	if (event.kind === 'listed') return event.step.recordedAt;
	return accountEventEntries(event)
		.map((entry) => entry.recordedFromAccount?.recordedAt ?? event.step.to)
		.reduce((latest, at) => (instant(at) > instant(latest) ? at : latest), event.step.to);
}

/** Whether something recorded or found at `at` is new against how far the
 * player had seen; nothing is new before that is known. */
export function isNewSince(at: string, seenThrough: string | null): boolean {
	return seenThrough !== null && instant(at) > instant(seenThrough);
}

/** The newest instant among what the player is shown: how far they have
 * now seen. `null` when nothing is shown. */
export function newestShown(
	events: readonly AccountEvent[],
	waiting: readonly AccountProposal[],
): string | null {
	let newest: string | null = null;
	for (const at of [...events.map(recordedAt), ...waiting.map((p) => p.detectedAt)]) {
		if (newest === null || instant(at) > instant(newest)) newest = at;
	}
	return newest;
}

/** What waits for the player's decision, newest found first. */
export function waitingOf(proposals: readonly AccountProposal[]): AccountProposal[] {
	return [...proposals].sort(
		(a, b) => instant(b.detectedAt) - instant(a.detectedAt) || b.id - a.id,
	);
}

/** One line of what went in or came out. */
export interface FlowLine {
	name: string;
	quantity: number | null;
	tt: number;
}

function lines(entry: ActivityHistoryEntry, side: ConversionLine['side']): FlowLine[] {
	return entry.conversionLines
		.filter((line) => line.side === side)
		.map((line) => ({ name: line.itemName, quantity: line.quantity, tt: line.ttValue }));
}

/** What a recycling took and the Nanocubes it made, with the balance
 * (zero when the TT closes, as the readings require). */
export function recyclingFlow(entry: ActivityHistoryEntry): {
	inputs: FlowLine[];
	outputs: FlowLine[];
	balance: number;
} {
	const inputs = lines(entry, 'input');
	const outputs = lines(entry, 'output');
	const taken = inputs.length
		? inputs
		: [{ name: entry.itemName, quantity: entry.quantity, tt: entry.ttValue }];
	const made = outputs.length
		? outputs
		: [{ name: entry.targetItem ?? 'Nanocube', quantity: null, tt: entry.ttValue }];
	const sum = (rows: FlowLine[]) => rows.reduce((total, row) => total + row.tt, 0);
	return { inputs: taken, outputs: made, balance: sum(made) - sum(taken) };
}

/** What a Shrapnel conversion took and the Universal Ammo it paid, with
 * the Trade Terminal's 1% on top. */
export function shrapnelFlow(entry: ActivityHistoryEntry): {
	shrapnel: number;
	tt: number;
	ammo: number;
	bonus: number;
} {
	const ammo = entry.pedAmount ?? entry.ttValue;
	return { shrapnel: entry.quantity, tt: entry.ttValue, ammo, bonus: ammo - entry.ttValue };
}

/** What a step sold to the Trade Terminal, and the TT it fetched. */
export function saleFlow(entries: readonly ActivityHistoryEntry[]): {
	items: FlowLine[];
	tt: number;
} {
	const items = entries.map((entry) => ({
		name: entry.itemName,
		quantity: entry.quantity,
		tt: entry.ttValue,
	}));
	return { items, tt: items.reduce((total, row) => total + row.tt, 0) };
}

/** What a listing the readings created put on the auction, the fee it
 * cost, and the starting bids that fee reads back as. The fee is the
 * listing's own, or the Card's fall in its step. */
export function listedFlow(
	entry: ActivityHistoryEntry,
	listing: AuctionListing | null,
	step: RecordedFromAccount,
): { item: FlowLine; fee: number; bids: InferredBid | null } {
	return {
		item: { name: entry.itemName, quantity: entry.quantity, tt: entry.ttValue },
		fee: listing?.listingFee ?? Math.abs(step.cardChange ?? 0),
		bids: step.inferredBid,
	};
}

/** What an opening put in and took out. */
export function openingFlow(opening: StrongboxOpeningEntry): {
	kept: FlowLine[];
	nanocubes: number;
	recycledTt: number;
} {
	const kept = opening.contents
		.filter((item) => item.nanocubes === null)
		.map((item) => ({ name: item.itemName, quantity: item.quantity, tt: item.ttValue }));
	const recycled = opening.contents.filter((item) => item.nanocubes !== null);
	return {
		kept,
		nanocubes: recycled.reduce((total, item) => total + (item.nanocubes ?? 0), 0),
		recycledTt: recycled.reduce((total, item) => total + item.ttValue, 0),
	};
}

/** What taking an event back would do, or why it cannot be; `none` when
 * the event is not the place to take it back (a listing the readings
 * created, once it has an outcome: that outcome's own surfaces do). */
export type EventUndo =
	| { kind: 'available'; action: string; sentence: string }
	| { kind: 'held'; reason: string }
	| { kind: 'undone' }
	| { kind: 'none' };

/** What the readings do once a record of theirs is taken back. */
const NOT_AGAIN = 'Your account readings will not record this again.';

/** Taking back one thing the account recorded: each kind has its own way
 * back, and says what it does. */
export function accountEventUndo(event: AccountEvent): EventUndo {
	const entries = accountEventEntries(event);
	if (entries.every((entry) => entry.undone)) return { kind: 'undone' };
	const blocked = (entry: ActivityHistoryEntry): EventUndo => ({
		kind: 'held',
		reason: entry.undoBlockedReason ?? 'Something recorded since depends on this.',
	});
	switch (event.kind) {
		case 'recycling':
			return event.entry.canDelete
				? {
						kind: 'available',
						action: 'Undo recycling',
						sentence: `The items come back and the Nanocubes go. ${NOT_AGAIN}`,
					}
				: blocked(event.entry);
		case 'shrapnel':
			return event.entry.canDelete
				? {
						kind: 'available',
						action: 'Undo conversion',
						sentence: `The Shrapnel comes back and the ammo it paid is unmade. ${NOT_AGAIN}`,
					}
				: blocked(event.entry);
		case 'sale': {
			const standing = event.entries.filter((entry) => !entry.undone);
			const open = standing.filter((entry) => entry.canDelete);
			if (open.length === 0) return blocked(standing[0]);
			const some = open.length < standing.length ? ' Those something else depends on stay.' : '';
			return {
				kind: 'available',
				action: 'Undo sale',
				sentence: `What was sold comes back to your stock.${some} ${NOT_AGAIN}`,
			};
		}
		case 'listed':
			if (event.entry.status !== 'pending') return { kind: 'none' };
			return event.entry.canDelete
				? {
						kind: 'available',
						action: 'Undo listing',
						sentence: `The items come back to your stock and the fee is unmade. ${NOT_AGAIN}`,
					}
				: blocked(event.entry);
		case 'listingSold':
			return event.entry.canRevertSale
				? {
						kind: 'available',
						action: 'Undo sale',
						sentence: `The listing goes back on auction and its sale is unmade. ${NOT_AGAIN}`,
					}
				: blocked(event.entry);
		case 'listingReturned':
			return event.entry.subjectKind === 'loot'
				? {
						kind: 'available',
						action: 'Put back on auction',
						sentence: `The listing waits on the auction again, as if it never came back. ${NOT_AGAIN}`,
					}
				: {
						kind: 'held',
						reason: 'An asset that came back is held again; list it anew instead.',
					};
	}
}
