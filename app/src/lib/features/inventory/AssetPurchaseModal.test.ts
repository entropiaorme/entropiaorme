// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import type { InventoryItem } from '$lib/api/commands.gen';
import AssetPurchaseModal from './AssetPurchaseModal.svelte';

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

const sweat: InventoryItem = {
	id: 'asset-3',
	name: 'Vibrant Sweat',
	ttValue: 0.5,
	markupPaid: 10,
	notes: null,
	acquiredAt: '2026-11-01',
	quantity: 5000,
	divisible: true,
	purchases: [],
	origins: [],
	derivedFrom: [],
};

describe('a purchase filled in from the account readings', () => {
	it('opens with what was seen, keeps the TT seen when the count changes, and saves it', async () => {
		const onsubmit = vi.fn().mockResolvedValue(undefined);
		render(AssetPurchaseModal, {
			props: {
				item: sweat,
				prefill: {
					key: 'proposal-41',
					quantity: 1000,
					ttValue: 0.1,
					markupPaid: 3,
					acquiredAt: '2026-11-15',
				},
				onsubmit,
				oncancel: vi.fn(),
			},
		});

		expect(Number((screen.getByLabelText('Quantity') as HTMLInputElement).value)).toBe(1000);
		expect(Number((screen.getByLabelText('TT value (PED)') as HTMLInputElement).value)).toBe(0.1);
		expect((screen.getByLabelText('Bought') as HTMLInputElement).value).toBe('2026-11-15');

		await fireEvent.input(screen.getByLabelText('Quantity'), { target: { value: '900' } });
		expect(Number((screen.getByLabelText('TT value (PED)') as HTMLInputElement).value)).toBe(0.1);
		await fireEvent.click(screen.getByRole('button', { name: 'Add purchase' }));

		await waitFor(() =>
			expect(onsubmit).toHaveBeenCalledWith({
				quantity: 900,
				ttValue: 0.1,
				markupPaid: 3,
				acquiredAt: '2026-11-15',
			}),
		);
	});

	it('still follows the position for a purchase typed by hand', async () => {
		render(AssetPurchaseModal, {
			props: { item: sweat, onsubmit: vi.fn(), oncancel: vi.fn() },
		});

		await fireEvent.input(screen.getByLabelText('Quantity'), { target: { value: '1000' } });
		expect(Number((screen.getByLabelText('TT value (PED)') as HTMLInputElement).value)).toBe(0.1);
	});
});
