// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { StockConversionFromAccount } from '$lib/api/commands.gen';
import {
	marketOpportunity,
	type TreeCuttingStock,
} from '$lib/features/analytics/treeCuttingModel.svelte';
import { formatTime } from '$lib/utils/format';

const api = vi.hoisted(() => ({
	getConversionFromAccount: vi.fn(),
	getSaleFromAccount: vi.fn(),
	getAuctionListings: vi.fn(),
}));
const frames = vi.hoisted(() => ({ handlers: [] as Array<() => void> }));

vi.mock('$lib/api/inventory', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api/inventory')>()),
	getConversionFromAccount: api.getConversionFromAccount,
	getSaleFromAccount: api.getSaleFromAccount,
}));
vi.mock('$lib/api/analytics', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api/analytics')>()),
	getAuctionListings: api.getAuctionListings,
}));
vi.mock('@tauri-apps/api/event', async (importOriginal) => ({
	...(await importOriginal<typeof import('@tauri-apps/api/event')>()),
	listen: vi.fn(async (topic: string, handler: () => void) => {
		if (topic === 'account:updated') frames.handlers.push(handler);
		return () => {};
	}),
}));

import AdjustStockModal from '$lib/features/analytics/AdjustStockModal.svelte';
import ConvertStockModal from '$lib/features/analytics/ConvertStockModal.svelte';
import SellStockModal from '$lib/features/analytics/SellStockModal.svelte';
import { listedListing, listedStep } from './__fixtures__/activity';
import AccountRecordHint from './AccountRecordHint.svelte';
import ConvertItemsModal from './ConvertItemsModal.svelte';
import CreateListingModal from './CreateListingModal.svelte';

function stock(itemName: string, heldQty: number, heldTt: number): TreeCuttingStock {
	return {
		itemName,
		heldQty,
		heldTt,
		listedQty: 0,
		readings: [],
		opportunity: marketOpportunity(undefined, 100.6),
		markupPct: null,
		markupHorizon: null,
		tier: 'illiquid',
		effectiveMarkupPct: 100.6,
		markupBasis: 'nanocube',
		floored: true,
		salesPed: null,
		weeklySalesPed: null,
		recommendedPacketTt: null,
		marketItem: null,
	};
}

const shrapnel = stock('Shrapnel', 100_000, 10);
const wood = stock('Ash Wood', 400, 20);

/** Recorded five minutes ago: inside the hour whenever the suite runs. */
function recentRecord(
	overrides: Partial<StockConversionFromAccount> = {},
): StockConversionFromAccount {
	return {
		id: 'conversion-7',
		quantity: 12_000,
		ttValue: 1.2,
		recordedAt: new Date(Date.now() - 5 * 60 * 1000).toISOString(),
		...overrides,
	};
}

function shrapnelHint(record: StockConversionFromAccount): string {
	return `Your account recorded a conversion of 12000 Shrapnel (1.20 PED TT) at ${formatTime(record.recordedAt)}; record this only if it is another one.`;
}

/** A read the test answers when it chooses. */
function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((res) => {
		resolve = res;
	});
	return { promise, resolve };
}

/** The account topic fires, as the backend's does once a record is made. */
async function frame() {
	await waitFor(() => expect(frames.handlers.length).toBeGreaterThan(0));
	for (const handler of frames.handlers) handler();
}

beforeAll(() => {
	Element.prototype.animate = function animate() {
		const animation = {
			cancel() {},
			finish() {},
			effect: null,
			currentTime: 0,
			playState: 'finished',
			onfinish: null as (() => void) | null,
			oncancel: null as (() => void) | null,
		};
		queueMicrotask(() => animation.onfinish?.());
		return animation as unknown as Animation;
	};
});

beforeEach(() => {
	vi.clearAllMocks();
	frames.handlers.length = 0;
	api.getConversionFromAccount.mockResolvedValue(null);
	api.getSaleFromAccount.mockResolvedValue(null);
	api.getAuctionListings.mockResolvedValue([]);
});

describe('a manual form beside what the account recorded', () => {
	it('shows the Shrapnel the account converted within the hour, announced, and still converts', async () => {
		const record = recentRecord();
		api.getConversionFromAccount.mockResolvedValue(record);
		const onconfirm = vi.fn().mockResolvedValue(undefined);
		render(AdjustStockModal, {
			props: { item: shrapnel, mode: 'shrapnel', onconfirm, oncancel: vi.fn() },
		});

		const hint = await screen.findByRole('status');
		expect(hint.textContent?.trim()).toBe(shrapnelHint(record));
		expect(api.getConversionFromAccount).toHaveBeenCalledWith({ itemName: 'Shrapnel' });
		// The amount is described by it.
		expect(screen.getByLabelText('PED to convert').getAttribute('aria-describedby')).toBe(hint.id);
		// A reminder, never a refusal: a second conversion is often real.
		await fireEvent.click(screen.getByRole('button', { name: 'Convert' }));
		await waitFor(() => expect(onconfirm).toHaveBeenCalledWith('Shrapnel', 100_000));
	});

	it('shows nothing when the account recorded no conversion of the item', async () => {
		render(AdjustStockModal, {
			props: { item: shrapnel, mode: 'shrapnel', onconfirm: vi.fn(), oncancel: vi.fn() },
		});

		await waitFor(() =>
			expect(api.getConversionFromAccount).toHaveBeenCalledWith({ itemName: 'Shrapnel' }),
		);
		expect(screen.queryByTestId('account-record-hint')).toBeNull();
	});

	it('shows nothing for one more than an hour old by the time it is shown', async () => {
		api.getConversionFromAccount.mockResolvedValue(
			recentRecord({ recordedAt: new Date(Date.now() - 61 * 60 * 1000).toISOString() }),
		);
		render(AdjustStockModal, {
			props: { item: shrapnel, mode: 'shrapnel', onconfirm: vi.fn(), oncancel: vi.fn() },
		});

		await waitFor(() => expect(api.getConversionFromAccount).toHaveBeenCalledTimes(1));
		await Promise.resolve();
		expect(screen.queryByTestId('account-record-hint')).toBeNull();
	});

	it('does not ask for a removal', async () => {
		render(AdjustStockModal, {
			props: { item: shrapnel, mode: 'remove', onconfirm: vi.fn(), oncancel: vi.fn() },
		});

		await screen.findByRole('button', { name: 'Remove' });
		expect(api.getConversionFromAccount).not.toHaveBeenCalled();
		expect(screen.queryByTestId('account-record-hint')).toBeNull();
	});

	it('appears when the account records one while the form is open', async () => {
		render(AdjustStockModal, {
			props: { item: shrapnel, mode: 'shrapnel', onconfirm: vi.fn(), oncancel: vi.fn() },
		});
		await waitFor(() => expect(api.getConversionFromAccount).toHaveBeenCalledTimes(1));
		expect(screen.queryByTestId('account-record-hint')).toBeNull();

		const record = recentRecord();
		api.getConversionFromAccount.mockResolvedValue(record);
		await frame();
		expect(await screen.findByText(shrapnelHint(record))).toBeTruthy();
	});

	it('attaches to the account topic before its first read', async () => {
		const order: string[] = [];
		const { listen } = await import('@tauri-apps/api/event');
		vi.mocked(listen).mockImplementationOnce(async (_topic, handler) => {
			order.push('listen');
			frames.handlers.push(handler as () => void);
			return () => {};
		});
		api.getConversionFromAccount.mockImplementation(async () => {
			order.push('read');
			return null;
		});
		render(AccountRecordHint, { props: { kind: 'conversion', itemName: 'Shrapnel', id: 'hint' } });

		await waitFor(() => expect(order).toEqual(['listen', 'read']));
	});

	it('never shows an answer for the item the form has moved away from', async () => {
		const first = deferred<StockConversionFromAccount | null>();
		api.getConversionFromAccount.mockReturnValueOnce(first.promise);
		const wood = recentRecord({ quantity: 400, ttValue: 20 });
		api.getConversionFromAccount.mockResolvedValue(wood);
		const { rerender } = render(AccountRecordHint, {
			props: { kind: 'conversion', itemName: 'Shrapnel', id: 'hint' },
		});
		await waitFor(() => expect(api.getConversionFromAccount).toHaveBeenCalledTimes(1));

		// The form moves on while the first read is still out.
		await rerender({ kind: 'conversion', itemName: 'Ash Wood', id: 'hint' });
		first.resolve(recentRecord());
		await waitFor(() =>
			expect(api.getConversionFromAccount).toHaveBeenLastCalledWith({ itemName: 'Ash Wood' }),
		);
		expect(
			await screen.findByText(
				`Your account recorded a conversion of 400 Ash Wood (20.00 PED TT) at ${formatTime(wood.recordedAt)}; record this only if it is another one.`,
			),
		).toBeTruthy();
		expect(screen.queryByText(/12000 Shrapnel/)).toBeNull();
	});

	it('shows the recycling the account recorded beside the recycling form', async () => {
		const record = recentRecord({ quantity: 400, ttValue: 20 });
		api.getConversionFromAccount.mockResolvedValue(record);
		render(ConvertStockModal, {
			props: { item: wood, onconvert: vi.fn(), oncancel: vi.fn() },
		});

		expect(
			await screen.findByText(
				`Your account recorded a conversion of 400 Ash Wood (20.00 PED TT) at ${formatTime(record.recordedAt)}; record this only if it is another one.`,
			),
		).toBeTruthy();
		expect(api.getConversionFromAccount).toHaveBeenCalledWith({ itemName: 'Ash Wood' });
	});

	it('shows it under a held line of the conversion form', async () => {
		api.getConversionFromAccount.mockResolvedValue(recentRecord({ quantity: 400, ttValue: 20 }));
		render(ConvertItemsModal, {
			props: {
				open: true,
				items: [
					{
						name: 'Ash Wood',
						assetId: null,
						heldQty: 400,
						unitTt: 0.05,
						paidPerUnit: 0,
						origins: [],
					},
				],
				start: { name: 'Ash Wood', assetId: null },
				history: [],
				onsubmit: vi.fn(),
			},
		});

		expect(await screen.findByTestId('account-record-hint')).toBeTruthy();
		expect(api.getConversionFromAccount).toHaveBeenCalledWith({ itemName: 'Ash Wood' });
	});

	it('shows a Trade Terminal sale beside the trade form, and no sale beside a listing', async () => {
		const record = recentRecord({ id: 'trade-3', quantity: 400, ttValue: 20 });
		api.getSaleFromAccount.mockResolvedValue(record);
		render(SellStockModal, {
			props: { item: wood, onlist: vi.fn(), ontrade: vi.fn(), oncancel: vi.fn() },
		});
		await screen.findByRole('button', { name: 'List on auction' });
		expect(api.getSaleFromAccount).not.toHaveBeenCalled();

		await fireEvent.click(screen.getByRole('button', { name: 'Trade' }));
		expect(
			await screen.findByText(
				`Your account recorded a Trade Terminal sale of 400 Ash Wood (20.00 PED TT) at ${formatTime(record.recordedAt)}; record this only if it is another one.`,
			),
		).toBeTruthy();
		expect(api.getSaleFromAccount).toHaveBeenCalledWith({ itemName: 'Ash Wood' });
		expect(api.getConversionFromAccount).not.toHaveBeenCalled();
	});

	/** Ash Wood the readings saw listed five minutes ago. */
	function listedWood() {
		const to = new Date(Date.now() - 5 * 60 * 1000).toISOString();
		return listedListing({
			itemName: 'Ash Wood',
			quantity: 400,
			ttValue: 20,
			listedFromAccount: listedStep({ to }),
		});
	}

	function listingHint(to: string): string {
		return `Your account recorded an auction listing of 400 Ash Wood (20.00 PED TT) at ${formatTime(to)}; record this only if it is another one.`;
	}

	it('shows a listing the account created beside the activity listing form', async () => {
		const made = listedWood();
		api.getAuctionListings.mockResolvedValue([made]);
		render(SellStockModal, {
			props: { item: wood, onlist: vi.fn(), ontrade: vi.fn(), oncancel: vi.fn() },
		});
		expect(await screen.findByText(listingHint(made.listedFromAccount?.to ?? ''))).toBeTruthy();
		expect(api.getAuctionListings).toHaveBeenCalledWith('inventory');
		expect(api.getSaleFromAccount).not.toHaveBeenCalled();
	});

	it('shows it beside the item of the inventory listing form, and nothing for one listed by hand', async () => {
		const made = listedWood();
		api.getAuctionListings.mockResolvedValue([made]);
		render(CreateListingModal, {
			props: {
				open: true,
				holdings: [
					{
						kind: 'loot',
						holdingId: 'Ash Wood',
						name: 'Ash Wood',
						score: 1,
						heldQty: 400,
					},
				],
				onresolve: vi.fn().mockResolvedValue({ candidates: [], resolved: null }),
				oncapture: vi.fn(),
				oncollect: vi.fn().mockResolvedValue(null),
				onsubmit: vi.fn(),
			},
		});
		const item = screen.getByRole('combobox');
		await fireEvent.input(item, { target: { value: 'Ash Wood' } });
		expect(await screen.findByText(listingHint(made.listedFromAccount?.to ?? ''))).toBeTruthy();
	});
});
