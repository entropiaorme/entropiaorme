// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import ConvertItemsModal from './ConvertItemsModal.svelte';
import type { ConvertibleItem } from './conversionDraft';

const items: ConvertibleItem[] = [
	{
		name: 'Halloween Strongbox',
		assetId: null,
		heldQty: 145,
		unitTt: 0,
		paidPerUnit: 0,
		origins: [{ label: 'Halloween Mayhem 2026', quantity: 145 }],
	},
];

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

function open(onsubmit = vi.fn().mockResolvedValue(undefined)) {
	render(ConvertItemsModal, {
		props: {
			open: true,
			items,
			start: { name: 'Halloween Strongbox', assetId: null },
			history: [],
			onsubmit,
		},
	});
	return onsubmit;
}

describe('ConvertItemsModal', () => {
	it('opens on the held item asking only for what it cannot work out', async () => {
		open();

		expect(await screen.findByText('of 145 held')).not.toBeNull();
		expect(screen.queryByLabelText(/TT of Halloween Strongbox/)).toBeNull();
		expect(screen.queryByLabelText(/Markup paid/)).toBeNull();
		expect(screen.queryByLabelText(/worth above TT/)).toBeNull();
	});

	it('records boxes written straight to a PED result', async () => {
		const onsubmit = open();

		await fireEvent.click(await screen.findByText('of 145 held'));
		await fireEvent.click(screen.getByRole('button', { name: '+ PED amount' }));
		await fireEvent.input(screen.getByLabelText('PED received'), { target: { value: '100' } });

		expect(await screen.findByText('+100.00 PED goes to Halloween Mayhem 2026.')).not.toBeNull();

		await fireEvent.click(screen.getByRole('button', { name: 'Convert' }));
		await waitFor(() => expect(onsubmit).toHaveBeenCalledOnce());
		expect(onsubmit.mock.calls[0][0]).toMatchObject({
			inputs: [{ kind: 'held', itemName: 'Halloween Strongbox', quantity: 145 }],
			outputs: [],
			pedAmount: 100,
		});
	});

	it('keeps Convert unavailable until there is a result', async () => {
		open();

		await fireEvent.click(await screen.findByText('of 145 held'));

		const convert = screen.getByRole('button', { name: 'Convert' }) as HTMLButtonElement;
		expect(convert.disabled).toBe(true);
	});
});
