// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { InventoryItem } from '$lib/api/commands.gen';

const api = vi.hoisted(() => ({
	addEquipmentHolding: vi.fn(),
	updateEquipmentHolding: vi.fn(),
}));
vi.mock('$lib/api/inventory', () => api);

import InventoryItemFormModal from './InventoryItemFormModal.svelte';

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

// Each test counts only the saves it made itself.
beforeEach(() => {
	vi.clearAllMocks();
});

describe('adding an asset with no TT', () => {
	it('holds a stack worth its markup alone, entered in PED and read per unit', async () => {
		api.addEquipmentHolding.mockResolvedValue({ id: 'boxes' });
		const onsaved = vi.fn();
		render(InventoryItemFormModal, { props: { open: true, onsaved } });

		await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'Strongbox' } });
		await fireEvent.input(screen.getByLabelText('Quantity'), { target: { value: '12' } });

		// No TT means no percentage of it: the field is PED only.
		expect(screen.queryByRole('button', { name: '%' })).toBeNull();
		expect(screen.getByText('No TT, so all of it is markup')).not.toBeNull();
		await fireEvent.input(screen.getByLabelText('MU paid, all units in PED'), {
			target: { value: '24' },
		});
		expect(screen.getByText('2.00 PED each')).not.toBeNull();
		expect(screen.getByText('2.00 each')).not.toBeNull();

		const add = screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement;
		expect(add.disabled).toBe(false);
		await fireEvent.click(add);
		await waitFor(() =>
			expect(api.addEquipmentHolding).toHaveBeenCalledWith({
				name: 'Strongbox',
				quantity: 12,
				tt_value: 0,
				markup_paid: 24,
				notes: null,
			}),
		);
		expect(onsaved).toHaveBeenCalled();
	});

	it('shows a markup typed as a percentage in PED once the TT goes, so what is saved is in view', async () => {
		render(InventoryItemFormModal, { props: { open: true, onsaved: vi.fn() } });
		await fireEvent.input(screen.getByLabelText(/^TT value/), { target: { value: '5' } });
		await fireEvent.input(screen.getByLabelText('MU paid as a percentage of TT'), {
			target: { value: '150' },
		});
		expect(screen.getByText('2.50 PED')).not.toBeNull();

		await fireEvent.input(screen.getByLabelText(/^TT value/), { target: { value: '0' } });
		const ped = screen.getByLabelText('MU paid in PED') as HTMLInputElement;
		expect(Number(ped.value)).toBe(2.5);

		// Typed in PED there, it stays PED when a TT comes back.
		await fireEvent.input(ped, { target: { value: '4' } });
		await fireEvent.input(screen.getByLabelText(/^TT value/), { target: { value: '2' } });
		expect(Number((screen.getByLabelText('MU paid in PED') as HTMLInputElement).value)).toBe(4);
		expect(screen.getByText('300.00%')).not.toBeNull();
	});
});

describe('adding an asset filled in from the account readings', () => {
	const prefill = {
		name: 'Vibrant Sweat',
		quantity: 1000,
		ttValue: 0.1,
		markupPaid: 3,
		acquiredAt: '2026-11-15',
	};

	it('opens with what was seen, the day included, and saves it through the caller', async () => {
		const asset = { id: 'asset-9', name: 'Vibrant Sweat' };
		api.addEquipmentHolding.mockResolvedValue(asset);
		const around = vi.fn(async (save: () => Promise<InventoryItem>) => save());
		const onsaved = vi.fn();
		render(InventoryItemFormModal, { props: { open: true, prefill, around, onsaved } });

		expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe('Vibrant Sweat');
		expect((screen.getByLabelText('Bought') as HTMLInputElement).value).toBe('2026-11-15');
		await fireEvent.input(screen.getByLabelText('Bought'), { target: { value: '2026-11-14' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Add' }));

		await waitFor(() => expect(onsaved).toHaveBeenCalledWith(asset));
		expect(around).toHaveBeenCalledTimes(1);
		expect(api.addEquipmentHolding).toHaveBeenCalledWith({
			name: 'Vibrant Sweat',
			quantity: 1000,
			tt_value: 0.1,
			markup_paid: 3,
			notes: null,
			acquired_at: '2026-11-14',
		});
	});

	it('shows why when the caller refuses, and stays open', async () => {
		const around = vi.fn(async (): Promise<InventoryItem> => {
			throw new Error('This purchase is already being recorded.');
		});
		const onsaved = vi.fn();
		render(InventoryItemFormModal, { props: { open: true, prefill, around, onsaved } });

		await fireEvent.click(screen.getByRole('button', { name: 'Add' }));
		expect(await screen.findByText('This purchase is already being recorded.')).toBeTruthy();
		expect(onsaved).not.toHaveBeenCalled();
		expect(api.addEquipmentHolding).not.toHaveBeenCalled();
	});
});
