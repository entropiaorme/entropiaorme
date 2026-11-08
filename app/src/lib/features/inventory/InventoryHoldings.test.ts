// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import {
	marketOpportunity,
	type TreeCuttingStock,
} from '$lib/features/analytics/treeCuttingModel.svelte';
import InventoryHoldings from './InventoryHoldings.svelte';

describe('inventory assets', () => {
	const lootProps = (loot: TreeCuttingStock[]) => ({
		kind: 'loot' as const,
		onkindchange: vi.fn(),
		confidenceMode: 'liquid' as const,
		onconfidencechange: vi.fn(),
		packetFeeSharePct: 10,
		onpacketfeesharechange: vi.fn(),
		loot,
		equipment: [],
		onsellloot: vi.fn(),
		onconvert: vi.fn(),
		onremove: vi.fn(),
		onshrapnel: vi.fn(),
		onaddequipment: vi.fn(),
		oncreatelisting: vi.fn(),
		oneditequipment: vi.fn(),
		onsellequipment: vi.fn(),
		ondeleteequipment: vi.fn(),
	});

	it('uses the shared inventory layout and compact actions for assets', async () => {
		const onedit = vi.fn();
		const onsell = vi.fn();
		const onremove = vi.fn();
		const equipment = {
			id: 'equipment-1',
			name: 'Ares Ring, Improved',
			ttValue: 25,
			markupPaid: 75,
			notes: 'Tier 4',
			acquiredAt: '2026-08-01',
		};

		render(InventoryHoldings, {
			props: {
				kind: 'equipment',
				onkindchange: vi.fn(),
				confidenceMode: 'liquid',
				onconfidencechange: vi.fn(),
				packetFeeSharePct: 10,
				onpacketfeesharechange: vi.fn(),
				loot: [],
				equipment: [equipment],
				onsellloot: vi.fn(),
				onconvert: vi.fn(),
				onremove: vi.fn(),
				onshrapnel: vi.fn(),
				onaddequipment: vi.fn(),
				oncreatelisting: vi.fn(),
				oneditequipment: onedit,
				onsellequipment: onsell,
				ondeleteequipment: onremove,
			},
		});

		const utility = screen.getByTestId('equipment-utility-strip');
		expect(within(utility).getByLabelText('Find an item')).not.toBeNull();
		expect(within(utility).getByRole('button', { name: 'Add asset' })).not.toBeNull();

		const table = screen.getByRole('table', { name: 'Assets' });
		expect(
			within(table)
				.getAllByRole('columnheader')
				.map((header) => header.textContent),
		).toEqual(['Asset', 'TT', 'MU paid', 'Total cost', 'Acquired', 'Actions']);
		expect(screen.getByText('Ares Ring, Improved')).not.toBeNull();
		const assetCell = screen.getByText('Ares Ring, Improved').parentElement;
		const actionCell = screen.getByRole('button', { name: 'Edit' }).parentElement;
		expect(assetCell?.classList.contains('flex-1')).toBe(true);
		expect(actionCell?.classList.contains('min-w-[5.25rem]')).toBe(true);
		expect(actionCell?.classList.contains('w-[5.25rem]')).toBe(false);

		await fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Sell' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Remove' }));

		expect(onedit).toHaveBeenCalledWith(equipment);
		expect(onsell).toHaveBeenCalledWith(equipment);
		expect(onremove).toHaveBeenCalledWith(equipment);
	});

	it('lets loot actions expand by yielding space from the item column', () => {
		render(InventoryHoldings, {
			props: {
				kind: 'loot',
				onkindchange: vi.fn(),
				confidenceMode: 'liquid',
				onconfidencechange: vi.fn(),
				packetFeeSharePct: 10,
				onpacketfeesharechange: vi.fn(),
				loot: [
					{
						itemName: 'Animal Muscle Oil',
						heldQty: 100,
						heldTt: 1,
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
					},
				],
				equipment: [],
				onsellloot: vi.fn(),
				onconvert: vi.fn(),
				onremove: vi.fn(),
				onshrapnel: vi.fn(),
				onaddequipment: vi.fn(),
				oncreatelisting: vi.fn(),
				oneditequipment: vi.fn(),
				onsellequipment: vi.fn(),
				ondeleteequipment: vi.fn(),
			},
		});

		const actionCell = screen.getByRole('button', { name: 'Sell' }).parentElement;
		expect(actionCell?.classList.contains('min-w-[7rem]')).toBe(true);
		expect(actionCell?.classList.contains('w-[7rem]')).toBe(false);
	});

	it('keeps packet readiness to one quiet white-or-green column', () => {
		const opportunity = {
			...marketOpportunity(undefined, 100.6),
			efficientBatchTt: 2,
		};
		const row = {
			heldQty: 100,
			listedQty: 0,
			readings: [],
			opportunity,
			markupPct: 110,
			markupHorizon: 'week',
			tier: 'liquid' as const,
			effectiveMarkupPct: 110,
			markupBasis: 'market' as const,
			floored: false,
			salesPed: 1_000,
			weeklySalesPed: 1_000,
			recommendedPacketTt: 2,
			marketItem: null,
		};
		render(InventoryHoldings, {
			props: lootProps([
				{ ...row, itemName: 'Still accumulating', heldTt: 1 },
				{ ...row, itemName: 'Ready packet', heldTt: 3 },
			]),
		});

		expect(screen.getByText('Packet TT')).not.toBeNull();
		const accumulating = screen.getByText('Still accumulating').closest('li');
		const ready = screen.getByText('Ready packet').closest('li');
		expect(accumulating).toBeInstanceOf(HTMLElement);
		expect(ready).toBeInstanceOf(HTMLElement);
		if (!(accumulating instanceof HTMLElement) || !(ready instanceof HTMLElement)) return;
		expect(within(accumulating).getByText('2.00').className).toContain('text-text');
		expect(within(accumulating).getByText('2.00').className).not.toContain('text-positive');
		expect(within(ready).getByText('2.00').className).toContain('text-positive');
	});

	it('shows Shrapnel market MU as context but applies the fixed 101% conversion value', async () => {
		render(InventoryHoldings, {
			props: lootProps([
				{
					itemName: 'Shrapnel',
					heldQty: 10_000,
					heldTt: 10,
					listedQty: 0,
					readings: [{ horizon: 'week', markupPct: 100.9, salesPed: 20_000 }],
					opportunity: marketOpportunity(
						{
							itemName: 'Shrapnel',
							markupPct: 100.9,
							unitPricePed: null,
							marketObservedAt: 1_700_000_000,
							manual: null,
							preferred: 'market',
							nanocubeMarked: false,
							horizon: 'week',
							salesPed: 20_000,
							recommendedPacketTt: null,
							readings: [{ horizon: 'week', markupPct: 100.9, salesPed: 20_000 }],
						},
						100.6,
					),
					markupPct: 100.9,
					markupHorizon: 'week',
					tier: 'liquid',
					effectiveMarkupPct: 101,
					markupBasis: 'shrapnel_conversion',
					floored: false,
					salesPed: 20_000,
					weeklySalesPed: 20_000,
					recommendedPacketTt: null,
					marketItem: null,
				},
			]),
		});

		const shrapnelRow = screen.getByText('Shrapnel').closest('li');
		expect(shrapnelRow).toBeInstanceOf(HTMLElement);
		if (!(shrapnelRow instanceof HTMLElement)) return;
		const observed = within(shrapnelRow).getByText('100.9%');
		expect(observed.className).toContain('line-through');
		expect(within(shrapnelRow).getByText('101.0%')).not.toBeNull();
		const info = screen.getByRole('button', { name: 'Fixed Shrapnel conversion value' });
		expect(info.textContent).toBe('i');
		await fireEvent.mouseEnter(info);
		expect(screen.getByText(/last seen market markup on Shrapnel was 100.9%/i)).not.toBeNull();
		expect(screen.getByText(/1% gain then enters Realised Net/i)).not.toBeNull();
	});

	it('offers preset and custom packet fee caps beside markup confidence', async () => {
		const onchange = vi.fn();
		render(InventoryHoldings, {
			props: { ...lootProps([]), onpacketfeesharechange: onchange },
		});

		expect(screen.getByText('Fee cap')).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: '5%' }));
		expect(onchange).toHaveBeenCalledWith(5);

		await fireEvent.click(screen.getByRole('button', { name: 'Custom' }));
		const input = screen.getByRole('spinbutton', { name: 'Custom fee cap percentage' });
		await fireEvent.input(input, { target: { value: '8.5' } });
		await fireEvent.blur(input);
		expect(onchange).toHaveBeenLastCalledWith(8.5);
	});
});
