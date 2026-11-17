// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '$lib/api/client';
import type { CostPlacements as Placements } from '$lib/api/commands.gen';
import { exact } from './__fixtures__/measure';

const api = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getCostPlacements: vi.fn(),
	placeCostItem: vi.fn(),
	clearCostPlacement: vi.fn(),
}));
const frames = vi.hoisted(() => ({ handlers: [] as Array<() => void> }));

vi.mock('$lib/api/account', () => api);
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (topic: string, handler: () => void) => {
		if (topic === 'account:updated') frames.handlers.push(handler);
		return () => {};
	}),
}));

import CostPlacements from './CostPlacements.svelte';

const AWAITING: Placements = {
	awaiting: [
		{ item: 'Odd Thing', sessions: 3, lost: exact(0.1234), units: null },
		{ item: 'Strange Stack', sessions: 1, lost: null, units: 40 },
	],
	placed: [],
};

/** Choose a placement from the open menu by its label. */
async function choose(label: string) {
	const option = screen
		.getAllByRole('menuitemradio')
		.find((candidate) => candidate.textContent?.trim() === label);
	if (!option) throw new Error(`no ${label} choice`);
	await fireEvent.click(option);
}

beforeEach(() => {
	vi.clearAllMocks();
	frames.handlers.length = 0;
});

describe('the items to place, in Settings', () => {
	it('says there is nothing to place when every item is placed', async () => {
		api.getCostPlacements.mockResolvedValue({ awaiting: [], placed: [] });
		render(CostPlacements, { props: { connected: true } });
		expect((await screen.findByTestId('nothing-to-place')).textContent).toBe('Nothing to place');
		expect(screen.getByText('Items to place')).toBeTruthy();
		expect(screen.queryByTestId('placed-items')).toBeNull();
	});

	it('lists each item with its sessions, the TT it lost and the units that left', async () => {
		api.getCostPlacements.mockResolvedValue(AWAITING);
		render(CostPlacements, { props: { connected: true } });
		const list = await screen.findByTestId('awaiting-placement');
		const rows = within(list).getAllByRole('listitem');
		expect(rows[0].textContent).toContain('Odd Thing');
		expect(rows[0].textContent).toContain('3 sessions, 0.1234 TT lost');
		expect(rows[1].textContent).toContain('1 session, 40 units left');
		expect(within(rows[0]).getByRole('button', { name: 'Place Odd Thing' })).toBeTruthy();
	});

	it('places an item for every session, then lists it as placed, with focus on it', async () => {
		api.getCostPlacements.mockResolvedValue(AWAITING);
		api.placeCostItem.mockResolvedValue({
			placements: {
				awaiting: [AWAITING.awaiting[1]],
				placed: [
					{ item: 'Odd Thing', category: 'fieldLogistics', placedAt: '2026-11-16T20:00:00Z' },
				],
			},
			pendingSessions: [],
		});
		render(CostPlacements, { props: { connected: true } });
		await fireEvent.click(await screen.findByRole('button', { name: 'Place Odd Thing' }));
		expect(screen.getByText('Remembered for every session')).toBeTruthy();
		await choose('Field logistics');
		expect(api.placeCostItem).toHaveBeenCalledWith({
			item: 'Odd Thing',
			category: 'fieldLogistics',
		});
		const placed = await screen.findByTestId('placed-items');
		const trigger = within(placed).getByRole('button', {
			name: 'Odd Thing: placed as Field logistics; change or clear',
		});
		await waitFor(() => expect(document.activeElement).toBe(trigger));
		expect(within(screen.getByTestId('awaiting-placement')).queryByText('Odd Thing')).toBeNull();
		expect(screen.getByTestId('placement-status').textContent?.trim()).toBe(
			'Placed as Field logistics',
		);
	});

	it('takes every Place menu out of reach while one placement is saving, showing which', async () => {
		api.getCostPlacements.mockResolvedValue(AWAITING);
		api.placeCostItem.mockReturnValue(new Promise(() => {}));
		render(CostPlacements, { props: { connected: true } });
		await fireEvent.click(await screen.findByRole('button', { name: 'Place Odd Thing' }));
		await choose('Weapon');
		const saving = screen.getByRole('button', { name: 'Place Odd Thing' });
		await waitFor(() => expect(saving.textContent).toContain('Placing'));
		expect(saving.hasAttribute('disabled')).toBe(true);
		const other = screen.getByRole('button', { name: 'Place Strange Stack' });
		expect(other.hasAttribute('disabled')).toBe(true);
		expect(other.textContent).not.toContain('Placing');
		expect(api.placeCostItem).toHaveBeenCalledTimes(1);
	});

	it('stays reachable after disconnecting while it holds a placement, and not otherwise', async () => {
		api.getCostPlacements.mockResolvedValue({
			awaiting: [],
			placed: [{ item: 'Mind Essence', category: 'travel', placedAt: '2026-11-16T20:00:00Z' }],
		});
		const { unmount } = render(CostPlacements, { props: { connected: false } });
		expect(await screen.findByTestId('placed-items')).toBeTruthy();
		unmount();
		api.getCostPlacements.mockResolvedValue({ awaiting: [], placed: [] });
		render(CostPlacements, { props: { connected: false } });
		await waitFor(() => expect(api.getCostPlacements).toHaveBeenCalledTimes(2));
		await new Promise((settle) => setTimeout(settle, 0));
		expect(screen.queryByTestId('cost-placements')).toBeNull();
	});

	it("says why a placement failed, with the backend's reason, beside the item", async () => {
		api.getCostPlacements.mockResolvedValue(AWAITING);
		api.placeCostItem.mockRejectedValue(new ApiError('badRequest', 'An item name is required'));
		render(CostPlacements, { props: { connected: true } });
		await fireEvent.click(await screen.findByRole('button', { name: 'Place Odd Thing' }));
		await choose('Weapon');
		const alert = await screen.findByRole('alert');
		expect(alert.textContent).toContain('An item name is required');
		expect(alert.closest('li')?.textContent).toContain('Odd Thing');
	});

	it('clears a placement made, on request', async () => {
		api.getCostPlacements.mockResolvedValue({
			awaiting: [],
			placed: [{ item: 'Mind Essence', category: 'travel', placedAt: '2026-11-16T20:00:00Z' }],
		});
		api.clearCostPlacement.mockResolvedValue({
			placements: { awaiting: [], placed: [] },
			pendingSessions: [],
		});
		render(CostPlacements, { props: { connected: true } });
		await fireEvent.click(
			await screen.findByRole('button', {
				name: 'Mind Essence: placed as Travel; change or clear',
			}),
		);
		await fireEvent.click(screen.getByRole('menuitem', { name: 'Clear placement' }));
		expect(api.clearCostPlacement).toHaveBeenCalledWith('Mind Essence');
		await waitFor(() => expect(screen.queryByTestId('placed-items')).toBeNull());
	});

	it('says why when the list cannot be read', async () => {
		api.getCostPlacements.mockRejectedValue(new Error('the account connection was not composed'));
		render(CostPlacements, { props: { connected: true } });
		expect(await screen.findByText('the account connection was not composed')).toBeTruthy();
	});

	it('re-reads the list when the account topic fires', async () => {
		api.getCostPlacements.mockResolvedValueOnce({ awaiting: [], placed: [] });
		api.getCostPlacements.mockResolvedValue(AWAITING);
		render(CostPlacements, { props: { connected: true } });
		await screen.findByTestId('nothing-to-place');
		await waitFor(() => expect(frames.handlers.length).toBeGreaterThan(0));
		for (const handler of frames.handlers) handler();
		expect(await screen.findByTestId('awaiting-placement')).toBeTruthy();
	});
});
