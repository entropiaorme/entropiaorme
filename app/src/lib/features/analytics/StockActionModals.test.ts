// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeAll, describe, expect, it, vi } from 'vitest';

// The account reminder beside a conversion's amount reads the backend; here
// the account has recorded nothing.
vi.mock('$lib/api/inventory', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api/inventory')>()),
	getConversionFromAccount: vi.fn(async () => null),
}));
vi.mock('@tauri-apps/api/event', async (importOriginal) => ({
	...(await importOriginal<typeof import('@tauri-apps/api/event')>()),
	listen: vi.fn(async () => () => {}),
}));

import AdjustStockModal from './AdjustStockModal.svelte';
import SellStockModal from './SellStockModal.svelte';
import { marketOpportunity, type TreeCuttingStock } from './treeCuttingModel.svelte';

const stock: TreeCuttingStock = {
	itemName: 'Shrapnel',
	heldQty: 10_000,
	heldTt: 10,
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

describe('stock action modals', () => {
	it('defaults Sell to Auction and records a fee-free Trade from the same surface', async () => {
		const ontrade = vi.fn().mockResolvedValue(undefined);
		render(SellStockModal, {
			props: { item: stock, onlist: vi.fn(), ontrade, oncancel: vi.fn() },
		});

		expect(screen.getByText('Starting bid (PED)')).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Trade' }));
		// Stock opens in PED, how it is held; half of it is half the units.
		await fireEvent.input(screen.getByLabelText('PED to sell'), { target: { value: '5' } });
		expect(screen.getByText('5000 units')).not.toBeNull();
		await fireEvent.input(screen.getByLabelText('Sold for (PED)'), { target: { value: '6' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Record trade' }));

		await waitFor(() =>
			expect(ontrade).toHaveBeenCalledWith({
				itemName: 'Shrapnel',
				quantity: 5000,
				soldFor: 6,
				soldAt: expect.any(String),
			}),
		);
	});

	it('converts a PED amount back to source quantity and previews 101% ammo', async () => {
		const onconfirm = vi.fn().mockResolvedValue(undefined);
		render(AdjustStockModal, {
			props: {
				item: { ...stock, heldTt: 10.03 },
				mode: 'shrapnel',
				onconfirm,
				oncancel: vi.fn(),
			},
		});

		await fireEvent.input(screen.getByLabelText('PED to convert'), { target: { value: '5' } });
		expect(screen.getByText('5.05 PED ammo after conversion')).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Convert' }));
		await waitFor(() => expect(onconfirm).toHaveBeenCalledWith('Shrapnel', 4985));
	});

	it('states that Remove leaves historical TT intact', () => {
		render(AdjustStockModal, {
			props: { item: stock, mode: 'remove', onconfirm: vi.fn(), oncancel: vi.fn() },
		});
		expect(screen.getByText(/historical TT stay recorded/)).not.toBeNull();
	});

	it('removes stock with no TT by count, since it has no PED to enter', async () => {
		const onconfirm = vi.fn().mockResolvedValue(undefined);
		render(AdjustStockModal, {
			props: {
				item: { ...stock, itemName: 'Halloween Strongbox', heldQty: 12, heldTt: 0, unitTt: 0 },
				mode: 'remove',
				onconfirm,
				oncancel: vi.fn(),
			},
		});

		expect(screen.getByText('No TT, so by count')).not.toBeNull();
		expect(screen.queryByRole('button', { name: 'By PED of TT' })).toBeNull();
		await fireEvent.input(screen.getByLabelText('How many to remove'), { target: { value: '5' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
		await waitFor(() => expect(onconfirm).toHaveBeenCalledWith('Halloween Strongbox', 5));
	});

	it('removes Shrapnel by PED: a TT of 0.0001 a unit is small, not none', async () => {
		const onconfirm = vi.fn().mockResolvedValue(undefined);
		render(AdjustStockModal, {
			props: {
				item: { ...stock, heldQty: 10_000, heldTt: 1, unitTt: 0.0001 },
				mode: 'remove',
				onconfirm,
				oncancel: vi.fn(),
			},
		});

		expect(screen.getByRole('button', { name: 'All 1.00 PED' })).not.toBeNull();
		await fireEvent.input(screen.getByLabelText('PED to remove'), { target: { value: '0.25' } });
		expect(screen.getByText('2500 units')).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
		await waitFor(() => expect(onconfirm).toHaveBeenCalledWith('Shrapnel', 2500));
	});

	it('removes stock with TT by count when switched to quantity', async () => {
		const onconfirm = vi.fn().mockResolvedValue(undefined);
		render(AdjustStockModal, {
			props: { item: stock, mode: 'remove', onconfirm, oncancel: vi.fn() },
		});

		await fireEvent.click(screen.getByRole('button', { name: 'By quantity' }));
		expect(screen.getByRole('button', { name: 'All 10000' })).not.toBeNull();
		await fireEvent.input(screen.getByLabelText('How many to remove'), {
			target: { value: '300' },
		});
		expect(screen.getByText('0.30 PED')).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
		await waitFor(() => expect(onconfirm).toHaveBeenCalledWith('Shrapnel', 300));
	});

	it('refuses to remove more than is in stock', async () => {
		const onconfirm = vi.fn();
		render(AdjustStockModal, {
			props: { item: stock, mode: 'remove', onconfirm, oncancel: vi.fn() },
		});

		await fireEvent.input(screen.getByLabelText('PED to remove'), { target: { value: '11' } });
		expect(screen.getByText('More than the Shrapnel in stock.')).not.toBeNull();
		expect((screen.getByRole('button', { name: 'Remove' }) as HTMLButtonElement).disabled).toBe(
			true,
		);
	});

	it('sells stock with no TT by count, without a TT of nothing to read', () => {
		render(SellStockModal, {
			props: {
				item: { ...stock, itemName: 'Halloween Strongbox', heldQty: 12, heldTt: 0, unitTt: 0 },
				onlist: vi.fn(),
				ontrade: vi.fn(),
				oncancel: vi.fn(),
			},
		});
		expect(screen.getByText('12, no TT')).not.toBeNull();
		expect(screen.queryByText('Listing TT')).toBeNull();
	});
});
