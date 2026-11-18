import { describe, expect, it } from 'vitest';
import {
	entry,
	listing,
	listingRow,
	opening,
	recycling,
	sale,
	shrapnel,
	step,
	waitingOpening,
} from './__fixtures__/activity';
import {
	type AccountEvent,
	accountEventShowsUnder,
	accountEventUndo,
	isNewSince,
	newestShown,
	openingFlow,
	recordedAt,
	recyclingFlow,
	reviewedEventShowsUnder,
	saleFlow,
	shrapnelFlow,
	splitHistory,
	waitingOf,
} from './accountActivity';

describe('the history split by who decided', () => {
	it('puts what the readings recorded, what was reviewed and what was entered apart', () => {
		const typed = entry({ id: 'typed' });
		const checked = entry({ id: 'checked', reconciliationId: 'r1', kind: 'removal' });
		const split = splitHistory([typed, checked, shrapnel(), recycling()], [opening()], []);
		expect(split.yours.map((row) => row.id)).toEqual(['typed']);
		expect(split.reviewed.map((event) => event.kind).sort()).toEqual(['check', 'opening']);
		expect(split.account.map((event) => event.kind)).toEqual(['recycling', 'shrapnel']);
	});

	it("gathers a step's sales to the Trade Terminal into one event", () => {
		const split = splitHistory(
			[sale('s1', 'Animal Hide', 0.5), sale('s2', 'Wool', 0.3), sale('s3', 'Wool', 0.3, 50)],
			[],
			[],
		);
		const sales = split.account.filter((event) => event.kind === 'sale');
		expect(sales).toHaveLength(2);
		const both = sales.find((event) => event.key === 'sale:43');
		expect(both?.kind === 'sale' && both.entries.map((row) => row.id)).toEqual(['s1', 's2']);
	});

	it("gathers a check's records into one event dated by its latest", () => {
		const split = splitHistory(
			[
				entry({ id: 'a', reconciliationId: 'r1', occurredAt: '2026-11-14' }),
				entry({ id: 'b', reconciliationId: 'r1', occurredAt: '2026-11-15' }),
			],
			[],
			[],
		);
		expect(split.reviewed).toHaveLength(1);
		const [check] = split.reviewed;
		expect(check.kind === 'check' && check.entries).toHaveLength(2);
		expect(check.at).toBe('2026-11-15');
	});

	it('reads a listing outcome beside its listing, sold or came back', () => {
		const sold = listingRow();
		const back = listingRow({ id: 'listing-2', status: 'returned', netMarkup: null });
		const split = splitHistory([sold, back], [], [listing()]);
		const kinds = Object.fromEntries(split.account.map((event) => [event.key, event]));
		expect(kinds['listingSold:listing-1']).toMatchObject({ listing: { id: 'listing-1' } });
		expect(kinds['listingReturned:listing-2']).toMatchObject({ listing: null });
	});

	it('orders the account events newest seen first', () => {
		const older = shrapnel({ recordedFromAccount: step(1, '2026-11-15T08:00:00Z') });
		const newer = recycling({ recordedFromAccount: step(2, '2026-11-15T09:00:00Z') });
		const split = splitHistory([older, newer], [], []);
		expect(split.account.map((event) => event.kind)).toEqual(['recycling', 'shrapnel']);
	});
});

describe('the Loot and Assets filter over events', () => {
	it('shows an event under each side one of its records touched', () => {
		const [event] = splitHistory([shrapnel()], [], []).account;
		expect(accountEventShowsUnder(event, 'loot')).toBe(true);
		expect(accountEventShowsUnder(event, 'equipment')).toBe(false);
	});

	it('shows an opening under Loot only', () => {
		const [event] = splitHistory([], [opening()], []).reviewed;
		expect(reviewedEventShowsUnder(event, 'loot')).toBe(true);
		expect(reviewedEventShowsUnder(event, 'equipment')).toBe(false);
	});
});

describe('what is new', () => {
	const [event] = splitHistory([shrapnel()], [], []).account;

	it('is what the account recorded after how far the player had seen', () => {
		expect(recordedAt(event)).toBe('2026-11-15T10:01:05.000Z');
		expect(isNewSince(recordedAt(event), '2026-11-15T10:00:00Z')).toBe(true);
		expect(isNewSince(recordedAt(event), '2026-11-15T10:01:05Z')).toBe(false);
	});

	it('is nothing while how far the player has seen is unknown', () => {
		expect(isNewSince(recordedAt(event), null)).toBe(false);
	});

	it('moves the mark to the newest shown, events and waiting alike', () => {
		expect(newestShown([event], [])).toBe('2026-11-15T10:01:05.000Z');
		expect(newestShown([event], [waitingOpening('2026-11-15T11:01:05Z')])).toBe(
			'2026-11-15T11:01:05Z',
		);
		expect(newestShown([], [])).toBeNull();
	});

	it('lists what waits newest found first', () => {
		const older = waitingOpening('2026-11-15T09:00:00Z');
		const newer = { ...waitingOpening('2026-11-15T10:00:00Z'), id: 8 };
		expect(waitingOf([older, newer]).map((p) => p.id)).toEqual([8, 7]);
	});
});

describe('each event laid out as what went in and what came out', () => {
	it('balances a recycling at nothing when the Nanocubes are worth what went in', () => {
		const flow = recyclingFlow(recycling());
		expect(flow.inputs.map((line) => line.name)).toEqual(['Lysterium Ingot', 'Animal Hide']);
		expect(flow.outputs).toEqual([{ name: 'Nanocube', quantity: 150, tt: 1.5 }]);
		expect(flow.balance).toBeCloseTo(0, 10);
	});

	it("shows Shrapnel to ammo with the Trade Terminal's 1% on top", () => {
		expect(shrapnelFlow(shrapnel())).toEqual({
			shrapnel: 100_000,
			tt: 10,
			ammo: 10.1,
			bonus: expect.closeTo(0.1, 10),
		});
	});

	it('adds up what a step sold at TT', () => {
		const flow = saleFlow([sale('s1', 'Animal Hide', 0.5), sale('s2', 'Wool', 0.3)]);
		expect(flow.items).toHaveLength(2);
		expect(flow.tt).toBeCloseTo(0.8, 10);
	});

	it('keeps what an opening kept apart from what it recycled', () => {
		expect(openingFlow(opening())).toEqual({
			kept: [{ name: 'Pumpkin Hat (C)', quantity: 1, tt: 0.2 }],
			nanocubes: 40,
			recycledTt: 0.4,
		});
	});
});

describe('the way back from an event', () => {
	const only = (rows: Parameters<typeof splitHistory>[0], listings = [listing()]) =>
		splitHistory(rows, [], listings).account[0] as AccountEvent;

	it('offers each kind its own Undo, saying the readings will not record it again', () => {
		for (const [event, action] of [
			[only([recycling()]), 'Undo recycling'],
			[only([shrapnel()]), 'Undo conversion'],
			[only([sale('s1', 'Wool', 0.3)]), 'Undo sale'],
			[only([listingRow()]), 'Undo sale'],
		] as const) {
			const undo = accountEventUndo(event);
			expect(undo).toMatchObject({ kind: 'available', action });
			expect(undo.kind === 'available' && undo.sentence).toContain('will not record this again');
		}
	});

	it('holds what something since depends on, saying why', () => {
		const held = accountEventUndo(
			only([shrapnel({ canDelete: false, undoBlockedReason: 'The ammo was spent since.' })]),
		);
		expect(held).toEqual({ kind: 'held', reason: 'The ammo was spent since.' });
	});

	it('undoes what it can of a sale, keeping what something since depends on', () => {
		const undo = accountEventUndo(
			only([sale('s1', 'Wool', 0.3), sale('s2', 'Hide', 0.5, 43, { canDelete: false })]),
		);
		expect(undo.kind === 'available' && undo.sentence).toContain(
			'Those something else depends on stay.',
		);
	});

	it('puts loot that came back on auction again, and holds an asset', () => {
		const loot = only([listingRow({ status: 'returned' })]);
		expect(accountEventUndo(loot)).toMatchObject({ action: 'Put back on auction' });
		const asset = only([listingRow({ status: 'returned', subjectKind: 'equipment' })]);
		expect(accountEventUndo(asset)).toEqual({
			kind: 'held',
			reason: 'An asset that came back is held again; list it anew instead.',
		});
	});

	it('says an event already taken back is undone', () => {
		expect(accountEventUndo(only([shrapnel({ undone: true })]))).toEqual({ kind: 'undone' });
	});
});
