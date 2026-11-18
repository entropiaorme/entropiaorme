import { describe, expect, it, vi } from 'vitest';
import type { QueuedListingDraft, SaleQueueCapture } from '$lib/api/commands.gen';
import { createSaleQueueDrafts } from './saleQueueDrafts.svelte';

function draft(captureId: number, row: number, name: string): QueuedListingDraft {
	const figure = (text: string) => ({ value: Number(text), text, check: false });
	return {
		draftId: `${captureId}-${row}`,
		itemName: { text: name, found: 'itemData' },
		ttValue: figure('20.80'),
		quantity: figure('1'),
		startingBid: figure('42'),
		buyout: { kind: 'off' },
		auctionDays: figure('7'),
		listingFee: figure('1.54'),
	};
}

function capture(captureId: number, names: string[]): SaleQueueCapture {
	return {
		kind: 'read',
		captureId,
		readAt: '2026-11-03T19:42:07',
		drafts: names.map((name, row) => draft(captureId, row, name)),
		unread: [],
		queued: { kind: 'all' },
	};
}

function refused(captureId: number): SaleQueueCapture {
	return { kind: 'refused', captureId, reason: 'sellWindowNotOpen' };
}

/** A promise the test settles when it chooses, for a capture still out. */
function pending<T>() {
	let settle!: (value: T) => void;
	const promise = new Promise<T>((resolve) => {
		settle = resolve;
	});
	return { promise, settle };
}

function source() {
	return {
		capture: vi.fn<() => Promise<SaleQueueCapture>>(),
		collect: vi.fn<() => Promise<SaleQueueCapture | null>>().mockResolvedValue(null),
	};
}

const names = (drafts: ReturnType<typeof createSaleQueueDrafts>) =>
	drafts.entries.map((entry) => [entry.draft.itemName.text, entry.state]);

describe('a capture of the sell queue', () => {
	it('offers a draft per queued item, the first under review', async () => {
		const io = source();
		io.capture.mockResolvedValue(capture(1, ['Animal Hide', 'Mind Essence']));
		const drafts = createSaleQueueDrafts(io);
		await drafts.capture();
		expect(names(drafts)).toEqual([
			['Animal Hide', 'pending'],
			['Mind Essence', 'pending'],
		]);
		expect(drafts.selectedId).toBe('1-0');
		expect(drafts.read?.readAt).toBe('2026-11-03T19:42:07');
	});

	it('replaces an earlier capture whole; a refusal leaves the drafts and says why', async () => {
		const io = source();
		io.capture
			.mockResolvedValueOnce(capture(1, ['Animal Hide', 'Mind Essence']))
			.mockResolvedValueOnce(capture(2, ['Skyripper Loyalist (L)']))
			.mockResolvedValueOnce(refused(3))
			.mockResolvedValueOnce(capture(4, ['Animal Hide']));
		const drafts = createSaleQueueDrafts(io);
		await drafts.capture();
		await drafts.capture();
		expect(names(drafts)).toEqual([['Skyripper Loyalist (L)', 'pending']]);
		await drafts.capture();
		expect(names(drafts)).toEqual([['Skyripper Loyalist (L)', 'pending']]);
		expect(drafts.selectedId).toBe('2-0');
		expect(drafts.refusal).toBe('sellWindowNotOpen');
		// The next capture that reads clears the refusal.
		await drafts.capture();
		expect(drafts.refusal).toBeNull();
		expect(names(drafts)).toEqual([['Animal Hide', 'pending']]);
	});

	it('does not show on opening a refusal held from before, nor one after closing', async () => {
		const io = source();
		io.collect.mockResolvedValueOnce(refused(1)).mockResolvedValueOnce(refused(2));
		const drafts = createSaleQueueDrafts(io);
		await drafts.collect(true);
		expect(drafts.refusal).toBeNull();
		// One arriving while the form is open is shown, and closing clears it.
		await drafts.collect();
		expect(drafts.refusal).toBe('sellWindowNotOpen');
		drafts.reset();
		expect(drafts.refusal).toBeNull();
	});

	it('never lets an older capture replace a newer one, whichever button took it', async () => {
		const io = source();
		const own = pending<SaleQueueCapture>();
		io.capture.mockReturnValueOnce(own.promise);
		io.collect.mockResolvedValueOnce(capture(7, ['Mind Essence']));
		const drafts = createSaleQueueDrafts(io);
		const out = drafts.capture();
		// The floating button's capture, started later, arrives first.
		await drafts.collect();
		own.settle(capture(6, ['Animal Hide']));
		await out;
		expect(names(drafts)).toEqual([['Mind Essence', 'pending']]);
	});

	it('drops a capture still out when the form closes', async () => {
		const io = source();
		const out = pending<SaleQueueCapture>();
		io.capture.mockReturnValueOnce(out.promise);
		const drafts = createSaleQueueDrafts(io);
		const capturing = drafts.capture();
		drafts.reset();
		out.settle(capture(1, ['Animal Hide']));
		await capturing;
		expect(drafts.entries).toEqual([]);
		expect(drafts.capturing).toBe(false);
	});

	it('says when the capture itself failed to answer', async () => {
		const io = source();
		io.capture.mockRejectedValueOnce(new Error('sale queue capture task failed'));
		const drafts = createSaleQueueDrafts(io);
		await drafts.capture();
		expect(drafts.failure).toBe('sale queue capture task failed');
		expect(drafts.entries).toEqual([]);
	});
});

describe('accepting and discarding drafts', () => {
	async function ready() {
		const io = source();
		io.capture.mockResolvedValue(capture(1, ['Animal Hide', 'Mind Essence', 'Animal Oil']));
		const drafts = createSaleQueueDrafts(io);
		await drafts.capture();
		return { io, drafts };
	}

	it('lists the draft it names and moves on to the next one', async () => {
		const { drafts } = await ready();
		const save = vi.fn().mockResolvedValue(undefined);
		expect(await drafts.accept('1-0', save)).toBeNull();
		expect(save).toHaveBeenCalledTimes(1);
		expect(names(drafts)[0]).toEqual(['Animal Hide', 'recorded']);
		expect(drafts.selectedId).toBe('1-1');
		// Recorded is final: it is not recorded twice.
		expect(await drafts.accept('1-0', save)).toBe('That draft is already recorded');
		expect(save).toHaveBeenCalledTimes(1);
	});

	it('refuses a draft from an earlier capture', async () => {
		const { drafts } = await ready();
		const save = vi.fn();
		expect(await drafts.accept('0-0', save)).toBe('That draft is from an earlier capture');
		expect(drafts.select('0-0')).toBe('That draft is from an earlier capture');
		expect(save).not.toHaveBeenCalled();
	});

	it('refuses every other action at once while a draft is being saved', async () => {
		const { io, drafts } = await ready();
		const saving = pending<void>();
		const accepted = drafts.accept('1-0', () => saving.promise);
		expect(drafts.saving).toBe(true);
		const busy = 'A listing is being saved; wait for it to finish';
		expect(await drafts.accept('1-0', vi.fn())).toBe(busy);
		expect(await drafts.accept('1-1', vi.fn())).toBe(busy);
		expect(drafts.discard('1-0')).toBe(busy);
		expect(drafts.select('1-1')).toBe(busy);
		expect(await drafts.capture()).toBe(busy);
		expect(io.capture).toHaveBeenCalledTimes(1);

		// A capture from the floating button waits for the save to end.
		io.collect.mockResolvedValueOnce(capture(2, ['Skyripper Loyalist (L)']));
		await drafts.collect();
		expect(names(drafts)[0]).toEqual(['Animal Hide', 'saving']);
		saving.settle();
		expect(await accepted).toBeNull();
		expect(names(drafts)).toEqual([['Skyripper Loyalist (L)', 'pending']]);
	});

	it('brings a recorded row back from a deferred capture as recorded', async () => {
		const { io, drafts } = await ready();
		const saving = pending<void>();
		const accepted = drafts.accept('1-0', () => saving.promise);
		// The game still queues the row being recorded: the capture taken
		// meanwhile brings it back.
		io.collect.mockResolvedValueOnce(capture(2, ['Animal Hide', 'Mind Essence']));
		await drafts.collect();
		saving.settle();
		await accepted;
		expect(names(drafts)).toEqual([
			['Animal Hide', 'recorded'],
			['Mind Essence', 'pending'],
		]);
		expect(drafts.selectedId).toBe('2-1');
		expect(await drafts.accept('2-0', vi.fn())).toBe('That draft is already recorded');
	});

	it('remembers recorded rows by count, across captures and closing the form', async () => {
		const io = source();
		io.capture
			.mockResolvedValueOnce(capture(1, ['Animal Hide', 'Animal Hide']))
			.mockResolvedValueOnce(capture(2, ['Animal Hide', 'Animal Hide', 'Mind Essence']));
		const drafts = createSaleQueueDrafts(io);
		await drafts.capture();
		await drafts.accept('1-0', vi.fn().mockResolvedValue(undefined));
		drafts.reset();
		await drafts.capture();
		// Two identical rows queued and one recorded: one comes back
		// recorded, the other to review.
		expect(names(drafts)).toEqual([
			['Animal Hide', 'recorded'],
			['Animal Hide', 'pending'],
			['Mind Essence', 'pending'],
		]);
		expect(drafts.selectedId).toBe('2-1');
	});

	it('keeps a failed save over its own draft until the player moves on', async () => {
		const { io, drafts } = await ready();
		let fail!: (cause: Error) => void;
		const saving = new Promise<void>((_, reject) => {
			fail = reject;
		});
		const accepted = drafts.accept('1-0', () => saving);
		io.collect.mockResolvedValueOnce(capture(2, ['Mind Essence']));
		await drafts.collect();
		fail(new Error('disk full'));
		await expect(accepted).rejects.toThrow('disk full');
		// The draft the failure is about still stands under review.
		expect(names(drafts)[0]).toEqual(['Animal Hide', 'pending']);
		expect(drafts.selectedId).toBe('1-0');
		// The player's next action takes the waiting capture.
		drafts.next();
		expect(names(drafts)).toEqual([['Mind Essence', 'pending']]);
	});

	it('keeps a draft whose save failed, to try again', async () => {
		const { drafts } = await ready();
		await expect(
			drafts.accept('1-0', () => Promise.reject(new Error('disk full'))),
		).rejects.toThrow('disk full');
		expect(names(drafts)[0]).toEqual(['Animal Hide', 'pending']);
		expect(drafts.saving).toBe(false);
	});

	it('puts a discarded draft aside and reviews the next', async () => {
		const { drafts } = await ready();
		expect(drafts.discard('1-0')).toBeNull();
		expect(names(drafts)).toEqual([
			['Mind Essence', 'pending'],
			['Animal Oil', 'pending'],
		]);
		expect(drafts.selectedId).toBe('1-1');
		expect(drafts.select(null)).toBeNull();
		drafts.next();
		expect(drafts.selectedId).toBe('1-1');
		drafts.select(null);
		expect(drafts.selected).toBeNull();
		expect(drafts.hasPending).toBe(true);
	});
});
