// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MarketHarvestItem } from '$lib/api';

vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	setManualMarkup: vi.fn(async () => ({})),
	clearManualMarkup: vi.fn(async () => true),
}));

import * as api from '$lib/api';
import ManualMarkupEditor from './ManualMarkupEditor.svelte';

const mocked = vi.mocked(api);

function market(overrides: Partial<MarketHarvestItem> = {}): MarketHarvestItem {
	return {
		itemName: 'Robot Component',
		markupPct: 112.5,
		marketObservedAt: Date.now() / 1000 - 3 * 86_400,
		manual: null,
		preferred: 'market',
		nanocubeMarked: false,
		unitPricePed: null,
		horizon: 'week',
		salesPed: 1_000,
		recommendedPacketTt: null,
		readings: [],
		...overrides,
	};
}

async function open(props: Record<string, unknown>) {
	const onChanged = vi.fn();
	render(ManualMarkupEditor, {
		itemName: 'Robot Component',
		quantity: 4,
		ttValue: 2,
		market: market(),
		applied: { markupPct: 112.5, basis: 'market' },
		onChanged,
		...props,
	});
	await fireEvent.click(screen.getByRole('button', { name: 'Update markup for Robot Component' }));
	return onChanged;
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('ManualMarkupEditor', () => {
	it('says what the item is valued at now and starts from the market reading', async () => {
		await open({});
		expect(screen.getByTestId('markup-in-use').textContent).toContain("Using the market's 112.5%");
		expect(screen.getByTestId('markup-in-use').textContent).toContain('3d ago');
		expect((screen.getByLabelText('Markup percent') as HTMLInputElement).value).toBe('112.5');
	});

	it('saves a per-unit entry with the row unit TT, and previews it as a markup', async () => {
		const onChanged = await open({});
		await fireEvent.click(screen.getByRole('button', { name: 'Markup as PED above TT per unit' }));
		const field = screen.getByLabelText('PED above TT per unit') as HTMLInputElement;
		await fireEvent.input(field, { target: { value: '0.25' } });
		// 0.5 PED TT per unit plus 0.25: 150% of TT.
		expect(screen.getByText('= 150.0% of TT')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Save' }));
		await waitFor(() => expect(onChanged).toHaveBeenCalled());
		expect(mocked.setManualMarkup).toHaveBeenCalledWith({
			itemName: 'Robot Component',
			kind: 'ped_per_unit',
			value: 0.25,
			unitTtPed: 0.5,
		});
	});

	it('refuses a value that is not a markup without writing', async () => {
		await open({});
		const field = screen.getByLabelText('Markup percent') as HTMLInputElement;
		await fireEvent.input(field, { target: { value: '0' } });
		expect((screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement).disabled).toBe(true);
		expect(mocked.setManualMarkup).not.toHaveBeenCalled();
	});

	it('clears a standing entry back to the import', async () => {
		const onChanged = await open({
			market: market({
				manual: {
					kind: 'percent',
					value: 140,
					unitTtPed: null,
					markupPct: 140,
					recordedAt: Date.now() / 1000,
				},
				preferred: 'manual',
			}),
			applied: { markupPct: 140, basis: 'manual' },
		});
		expect(screen.getByTestId('markup-in-use').textContent).toContain('Using your 140.0%');
		await fireEvent.click(screen.getByRole('button', { name: 'Clear mine' }));
		await waitFor(() => expect(onChanged).toHaveBeenCalled());
		expect(mocked.clearManualMarkup).toHaveBeenCalledWith('Robot Component');
	});
});
