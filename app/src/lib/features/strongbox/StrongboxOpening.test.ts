// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	getTrackingSnapshot: vi.fn(),
	getLootInventory: vi.fn(),
	getEquipmentInventory: vi.fn(),
	getStrongboxOpenings: vi.fn(),
	readStrongboxExport: vi.fn(),
	recordStrongboxOpening: vi.fn(),
	undoStrongboxOpening: vi.fn(),
	getMarketItemMarkups: vi.fn(),
}));
vi.mock('$lib/api', () => ({ getTrackingSnapshot: api.getTrackingSnapshot }));
vi.mock('$lib/api/inventory', () => api);
vi.mock('$lib/api/market', () => ({ getMarketItemMarkups: api.getMarketItemMarkups }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

import { createOpeningModel } from './openingModel.svelte';
import StrongboxOpening from './StrongboxOpening.svelte';

const EXPORT = [
	'Name\tQuantity\tValue(PED)',
	'Red Heart Fireworks\t1\t2.0000',
	'Skeleton Shirt White (F)\t1\t10.0000',
	'Skeleton Shirt White (M)\t1\t10.0000',
	'Halloween Strongbox\t-8\t0.0000',
	'Strongbox Key\t-8\t-80.0000',
	'Universal Ammo\t580000\t58.0000',
].join('\n');

const read = {
	boxesOpened: 8,
	keys: { count: 8, ttValue: 80 },
	ammoPed: 58,
	contents: [
		{ line: 2, itemName: 'Red Heart Fireworks', quantity: 1, valuePed: 2 },
		{ line: 3, itemName: 'Skeleton Shirt White (F)', quantity: 1, valuePed: 10 },
		{ line: 4, itemName: 'Skeleton Shirt White (M)', quantity: 1, valuePed: 10 },
	],
	unexpected: [],
	skipped: [],
	balanceGap: 0,
};

function marketItem(itemName: string, markupPct: number) {
	return {
		itemName,
		markupPct,
		marketObservedAt: 1,
		manual: null,
		preferred: 'market',
		nanocubeMarked: false,
		unitPricePed: null,
		horizon: 'month',
		salesPed: 50_000,
		recommendedPacketTt: null,
		readings: [
			{ horizon: 'week', markupPct, salesPed: 20_000 },
			{ horizon: 'month', markupPct, salesPed: 50_000 },
		],
	};
}

function mount() {
	const model = createOpeningModel();
	render(StrongboxOpening, { props: { model } });
	void model.load();
	return model;
}

beforeEach(() => {
	vi.clearAllMocks();
	api.getTrackingSnapshot.mockResolvedValue({ status: 'idle' });
	api.getLootInventory.mockResolvedValue([
		{
			itemName: 'Halloween Strongbox',
			quantity: 402,
			ttValue: 0,
			listedQuantity: 0,
			unitTt: 0,
			paidMarkup: 0,
			origins: [],
			derivedFrom: [],
		},
	]);
	api.getEquipmentInventory.mockResolvedValue([]);
	api.getStrongboxOpenings.mockResolvedValue([]);
	api.readStrongboxExport.mockResolvedValue(read);
	api.getMarketItemMarkups.mockResolvedValue({
		nanocubeMarkupPct: 102,
		items: [marketItem('Skeleton Shirt White (F)', 250), marketItem('Red Heart Fireworks', 101)],
	});
	api.recordStrongboxOpening.mockResolvedValue({ id: 'run-1' });
});

describe('the strongbox opening view', () => {
	it('will not begin a run while tracking is on', async () => {
		api.getTrackingSnapshot.mockResolvedValue({ status: 'active' });
		mount();
		await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
		expect(screen.getByText('Stop tracking to open boxes.')).not.toBeNull();
		expect((screen.getByRole('button', { name: 'Continue' }) as HTMLButtonElement).disabled).toBe(
			true,
		);
	});

	it('reads a pasted export into a review and records the run', async () => {
		mount();
		await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

		const zone = await screen.findByLabelText('Inventory Changes export');
		await fireEvent.paste(zone, { clipboardData: { getData: () => EXPORT } });

		await waitFor(() =>
			expect(api.readStrongboxExport).toHaveBeenCalledWith({
				text: EXPORT,
				boxItem: 'Halloween Strongbox',
			}),
		);
		expect(await screen.findByText('Estimated markup')).not.toBeNull();
		// The shirt with a reading is the hit and leads; the firework sells as
		// cubes and is cubed by default; the unvalued shirt is kept.
		expect(screen.getByText('1 hit worth more than cubes', { exact: false })).not.toBeNull();
		expect(screen.getByText('8 from your loot')).not.toBeNull();

		await fireEvent.click(screen.getByRole('button', { name: 'Record opening' }));
		await waitFor(() => expect(api.recordStrongboxOpening).toHaveBeenCalledTimes(1));
		const input = api.recordStrongboxOpening.mock.calls[0][0];
		expect(input.sources).toEqual([{ kind: 'held', quantity: 8 }]);
		expect(input.keyUnitTt).toBe(10);
		expect(input.ammoPed).toBe(58);
		expect(input.intake).toBe('export');
		expect(
			input.contents.map((row: { itemName: string; recycle: boolean }) => [
				row.itemName,
				row.recycle,
			]),
		).toEqual([
			['Skeleton Shirt White (F)', false],
			['Red Heart Fireworks', true],
			['Skeleton Shirt White (M)', false],
		]);
	});

	it('says why an export cannot be read, keeping the paste', async () => {
		api.readStrongboxExport.mockResolvedValue({
			...read,
			boxesOpened: 0,
			unexpected: [
				{
					row: { line: 5, itemName: 'Easter Strongbox', quantity: -8, valuePed: 0 },
					reason: 'other_box',
				},
			],
		});
		mount();
		await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
		const zone = await screen.findByLabelText('Inventory Changes export');
		await fireEvent.paste(zone, { clipboardData: { getData: () => EXPORT } });
		expect(await screen.findByText(/shows Easter Strongbox opened/)).not.toBeNull();
		expect(api.recordStrongboxOpening).not.toHaveBeenCalled();
	});

	it('takes a result by hand, working out the ammo', async () => {
		mount();
		await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
		await fireEvent.click(await screen.findByRole('button', { name: 'Enter by hand' }));

		await fireEvent.input(screen.getByLabelText('Boxes opened'), { target: { value: '4' } });
		await fireEvent.input(screen.getByLabelText('Item'), { target: { value: 'Nanocube' } });
		await fireEvent.input(screen.getByLabelText('Quantity'), { target: { value: '500' } });
		await fireEvent.input(screen.getByLabelText('TT value'), { target: { value: '5' } });

		expect(await screen.findByText('35.00')).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Review' }));
		expect(await screen.findByText('Estimated markup')).not.toBeNull();
	});
});
