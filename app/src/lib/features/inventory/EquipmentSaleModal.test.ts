// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import type { InventoryItem } from '$lib/api';
import EquipmentSaleModal from './EquipmentSaleModal.svelte';

const asset: InventoryItem = {
	id: 'ore',
	name: 'Lysterium Stone',
	ttValue: 30,
	markupPaid: 6,
	notes: null,
	acquiredAt: '2026-11-01',
	quantity: 300,
	divisible: true,
	purchases: [],
	origins: [],
	derivedFrom: [],
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

function open(item: InventoryItem, ontrade = vi.fn().mockResolvedValue(undefined)) {
	render(EquipmentSaleModal, { props: { item, onlist: vi.fn(), ontrade, oncancel: vi.fn() } });
	return ontrade;
}

describe('EquipmentSaleModal', () => {
	it('sells part of a stack by count, every unit by default', async () => {
		const ontrade = open(asset);

		await fireEvent.click(screen.getByRole('button', { name: 'Trade' }));
		await fireEvent.input(screen.getByLabelText('How many to sell'), { target: { value: '100' } });
		// Read out beneath the field, and as the TT the sale carries.
		expect(screen.getAllByText('10.00 PED')).toHaveLength(2);
		await fireEvent.click(screen.getByRole('button', { name: 'Record trade' }));

		await waitFor(() =>
			expect(ontrade).toHaveBeenCalledWith(
				expect.objectContaining({ itemId: 'ore', quantity: 100 }),
			),
		);
	});

	it('sells part of a stack by the PED of TT it comes to', async () => {
		const ontrade = open(asset);

		await fireEvent.click(screen.getByRole('button', { name: 'Trade' }));
		await fireEvent.click(screen.getByRole('button', { name: 'By PED of TT' }));
		await fireEvent.input(screen.getByLabelText('PED to sell'), { target: { value: '12.5' } });
		expect(screen.getByText('125 units')).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Record trade' }));

		await waitFor(() =>
			expect(ontrade).toHaveBeenCalledWith(
				expect.objectContaining({ itemId: 'ore', quantity: 125 }),
			),
		);
	});

	it('offers no PED for a stack with no TT', () => {
		open({ ...asset, name: 'Strongbox', ttValue: 0, markupPaid: 24, quantity: 12 });

		expect(screen.getByText('No TT, so by count')).not.toBeNull();
		expect(screen.queryByRole('button', { name: 'By PED of TT' })).toBeNull();
	});
});
