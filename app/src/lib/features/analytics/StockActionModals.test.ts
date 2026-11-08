// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeAll, describe, expect, it, vi } from 'vitest';
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
		await fireEvent.input(screen.getByLabelText('Quantity'), { target: { value: '5000' } });
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
});
