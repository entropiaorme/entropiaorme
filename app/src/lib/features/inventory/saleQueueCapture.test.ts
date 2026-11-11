import { describe, expect, it } from 'vitest';
import type { QueuedListingDraft, ReadFigure, SaleQueueRefusal } from '$lib/api/commands.gen';
import { isCommittable } from './listingIntake';
import {
	checkMarks,
	countLine,
	draftFields,
	draftSummary,
	FIGURE_CHECK,
	fieldSources,
	nameCheck,
	overlayReplyLine,
	type QueueRead,
	refusalLine,
	rowKey,
	unreadLine,
} from './saleQueueCapture';

function figure(text: string, check = false): ReadFigure {
	return { value: Number(text), text, check };
}

/** A queued stack as the window prints it: a buyout set, the figures nothing
 * cross-checks marked. */
function draft(overrides: Partial<QueuedListingDraft> = {}): QueuedListingDraft {
	return {
		draftId: '4-0',
		itemName: { text: 'Output Amplifier Component', found: 'nowhere' },
		ttValue: figure('20.80', true),
		quantity: figure('52', true),
		startingBid: figure('42'),
		buyout: { kind: 'price', figure: figure('48') },
		auctionDays: figure('7', true),
		listingFee: figure('1.54'),
		...overrides,
	};
}

function read(overrides: Partial<QueueRead> = {}): QueueRead {
	return {
		kind: 'read',
		captureId: 4,
		readAt: '2026-11-03T19:42:07',
		drafts: [draft()],
		unread: [],
		queued: { kind: 'all' },
		...overrides,
	};
}

describe('a capture that read no queue', () => {
	it('tells the player what to change, in the game where they can', () => {
		const reasons: SaleQueueRefusal[] = [
			'gameWindowNotFound',
			'gameWindowOffScreen',
			'captureFailed',
			'captureNotAllowed',
			'sellWindowNotOpen',
			'sellWindowNotWhole',
			'queueNotVisible',
			'priceModeUnread',
			'otherPriceMode',
			'queueEmpty',
			'readerUnavailable',
		];
		const lines = reasons.map(refusalLine);
		expect(new Set(lines).size).toBe(reasons.length);
		expect(refusalLine('sellWindowNotOpen')).toBe(
			'Open the Auction sell window in the game, with its title in view',
		);
		expect(refusalLine('queueEmpty')).toBe('The sell queue is empty');
		// What did not read is not said to be covered: it may simply not have
		// read.
		expect(refusalLine('queueNotVisible')).toBe(
			'The sell queue could not be read; make sure nothing covers it',
		);
		expect(refusalLine('captureNotAllowed')).toMatch(/not allowed/);
		// Nothing tells the player where to put the window or at what scale.
		for (const line of lines) {
			expect(line).not.toMatch(/dock|corner|bottom|scale|confidence/i);
		}
	});
});

describe('how much of the queue was read', () => {
	it('says nothing when every queued item read', () => {
		expect(countLine(read())).toBeNull();
	});

	it('says how many of the queued items read and how many are out of view', () => {
		expect(countLine(read({ queued: { kind: 'some', of: 5 } }))).toBe(
			'Read 1 of 5 queued items. 4 are out of view.',
		);
		expect(countLine(read({ queued: { kind: 'some', of: 2 } }))).toBe(
			'Read 1 of 2 queued items. 1 is out of view.',
		);
		// The rest are in view but did not read: they are listed as such, not
		// called out of view.
		expect(
			countLine(
				read({
					queued: { kind: 'some', of: 2 },
					unread: [{ problems: [{ field: 'listingFee', why: 'notRead' }] }],
				}),
			),
		).toBe('Read 1 of 2 queued items.');
	});

	it('gives no queued count the window did not print readably or that the rows contradict', () => {
		expect(countLine(read({ queued: { kind: 'unread' } }))).toBe(
			"Read 1 queued item. The queue's count could not be read, so some may be out of view.",
		);
		expect(countLine(read({ queued: { kind: 'mismatched' } }))).toBe(
			'Read 1 queued item. The count shown does not match the rows in view, so some may be out of view.',
		);
	});
});

describe('a queued item that did not read whole', () => {
	it('says which figure failed and how, and nothing of what did read', () => {
		expect(
			unreadLine({
				problems: [
					{ field: 'startingBid', why: 'notRead' },
					{ field: 'listingFee', why: 'disagrees' },
				],
			}),
		).toBe('Not read: the opening bid did not read clearly');
		expect(unreadLine({ problems: [{ field: 'itemName', why: 'cutOff' }] })).toBe(
			'Not read: the item name is cut off',
		);
		expect(unreadLine({ problems: [{ field: 'listingFee', why: 'disagrees' }] })).toBe(
			"Not read: the fee does not agree with the window's other figures",
		);
		expect(unreadLine({ problems: [] })).toBe('Not read');
	});
});

describe('the floating capture button', () => {
	it('says how many items read, or why none did', () => {
		expect(overlayReplyLine({ kind: 'read', drafts: 3, unread: 0 })).toEqual({
			message: 'Read 3 queued items. Check the main window.',
			failed: false,
		});
		expect(overlayReplyLine({ kind: 'read', drafts: 1, unread: 2 })).toEqual({
			message: 'Read 1 queued item, 2 not. Check the main window.',
			failed: false,
		});
		expect(overlayReplyLine({ kind: 'read', drafts: 0, unread: 2 }).failed).toBe(true);
		expect(overlayReplyLine({ kind: 'refused', reason: 'otherPriceMode' })).toEqual({
			message: 'Switch the sell window to PED prices',
			failed: true,
		});
	});
});

describe('a draft in the form', () => {
	it('fills every figure as read and meets the checks a typed listing does', () => {
		const fields = draftFields(draft());
		expect(fields).toEqual({
			itemName: 'Output Amplifier Component',
			quantity: 52,
			ttValue: 20.8,
			auctionFee: 1.54,
			auctionDays: 7,
			startingBid: 42,
			buyout: 48,
		});
		expect(isCommittable(fields, 'auction')).toBe(true);
	});

	it('shows a switched-off buyout as no buyout, never zero, and the quantity apart from the bid', () => {
		expect(draftFields(draft({ buyout: { kind: 'off' } })).buyout).toBeNull();
		expect(draftSummary(draft({ buyout: { kind: 'off' } }))).toBe(
			'52 · opening bid 42 PED · no buyout · 7 days · fee 1.54 PED',
		);
		expect(draftSummary(draft())).toBe(
			'52 · opening bid 42 PED · buyout 48 PED · 7 days · fee 1.54 PED',
		);
	});

	it('marks the figures a misread could hide in until the player changes them', () => {
		const shown = draft();
		const fields = draftFields(shown);
		const marks = checkMarks(shown, fields, 'read');
		expect(marks).toEqual({
			itemName: nameCheck(shown.itemName),
			quantity: FIGURE_CHECK,
			ttValue: FIGURE_CHECK,
			auctionFee: null,
			auctionDays: FIGURE_CHECK,
			startingBid: null,
			buyout: null,
		});
		expect(FIGURE_CHECK).toBe(
			'Nothing on screen fully confirms this; compare it with the sell window.',
		);
		// A figure the player has typed over is theirs, and carries no mark.
		const edited = checkMarks(
			shown,
			{ ...fields, quantity: 40, itemName: 'Animal Hide' },
			'player',
		);
		expect(edited.quantity).toBeNull();
		expect(edited.itemName).toBeNull();
	});

	it('says what leaves a name unconfirmed, and marks none the item data names', () => {
		const listed = nameCheck({ text: 'Animal Hide', found: 'inventoryList' });
		const nowhere = nameCheck({ text: 'Animal Hide', found: 'nowhere' });
		expect(listed).toMatch(/inventory list but not in the app's item data/);
		expect(nowhere).toMatch(/Not found in the app's item data or the sell window's inventory list/);
		expect(nameCheck({ text: 'Electric Attack Nanochip 3', found: 'itemData' })).toBeNull();
	});

	it('keeps a name the app matched to another holding marked until the player picks', () => {
		const catalogued = draft({
			itemName: { text: 'Output Amplifier Component', found: 'itemData' },
		});
		const fields = { ...draftFields(catalogued), itemName: 'Output Amplifier Component (L)' };
		expect(checkMarks(catalogued, fields, 'matched').itemName).toMatch(/Matched to a holding/);
		expect(checkMarks(catalogued, fields, 'player').itemName).toBeNull();
	});
});

describe('where an accepted draft says its figures came from', () => {
	it('records each figure kept as read with its printed text and whether anything checked it', () => {
		const shown = draft({ buyout: { kind: 'off' } });
		const fields = { ...draftFields(shown), quantity: 40 };
		expect(fieldSources(shown, fields, 'Output Amplifier Component', 'read')).toEqual([
			{
				field: 'itemName',
				source: { kind: 'readUnchecked', text: 'Output Amplifier Component' },
			},
			{ field: 'ttValue', source: { kind: 'readUnchecked', text: '20.80' } },
			{ field: 'quantity', source: { kind: 'typed' } },
			{ field: 'startingBid', source: { kind: 'readChecked', text: '42' } },
			{ field: 'buyout', source: { kind: 'readChecked', text: null } },
			{ field: 'listingFee', source: { kind: 'readChecked', text: '1.54' } },
			{ field: 'auctionDays', source: { kind: 'readUnchecked', text: '7' } },
		]);
	});

	it('keeps the read name for a holding the app matched, and leaves out a TT value changed', () => {
		const shown = draft();
		const fields = { ...draftFields(shown), ttValue: 21 };
		const sources = fieldSources(shown, fields, 'Output Amplifier Component (L)', 'matched');
		expect(sources[0]).toEqual({
			field: 'itemName',
			source: { kind: 'matched', text: 'Output Amplifier Component' },
		});
		expect(sources.some((entry) => entry.field === 'ttValue')).toBe(false);
		// A holding the player picked or typed is theirs.
		const picked = fieldSources(shown, fields, 'Animal Hide', 'player');
		expect(picked[0]).toEqual({ field: 'itemName', source: { kind: 'typed' } });
	});

	it('records a buyout set over a switched-off one, or cleared, as typed', () => {
		const off = draft({ buyout: { kind: 'off' } });
		const set = fieldSources(off, { ...draftFields(off), buyout: 60 }, 'x', 'player');
		expect(set.find((entry) => entry.field === 'buyout')?.source).toEqual({ kind: 'typed' });
		const priced = draft();
		const cleared = fieldSources(priced, { ...draftFields(priced), buyout: null }, 'x', 'player');
		expect(cleared.find((entry) => entry.field === 'buyout')?.source).toEqual({ kind: 'typed' });
	});
});

describe('the identity of a queued row', () => {
	it('is its name and every figure as printed', () => {
		expect(rowKey(draft())).toBe(rowKey(draft({ draftId: '9-3' })));
		expect(rowKey(draft())).not.toBe(rowKey(draft({ listingFee: figure('1.55') })));
		expect(rowKey(draft())).not.toBe(rowKey(draft({ buyout: { kind: 'off' } })));
	});
});
