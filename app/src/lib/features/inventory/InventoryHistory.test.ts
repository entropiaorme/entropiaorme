// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountProposal } from '$lib/api/commands.gen';

const account = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getProposals: vi.fn(),
	getActivityMarks: vi.fn(),
	markActivitySeen: vi.fn(),
	claimProposals: vi.fn(),
	releaseProposals: vi.fn(),
	linkProposals: vi.fn(),
	dismissProposal: vi.fn(),
}));

vi.mock('$lib/api/account', () => account);
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('@tauri-apps/api/event', async (importOriginal) => ({
	...(await importOriginal<typeof import('@tauri-apps/api/event')>()),
	listen: vi.fn(async () => () => {}),
}));

import { activityMarks } from '$lib/features/account/activityMarks.svelte';
import { entry, opening, recycling, shrapnel, step, waitingOpening } from './__fixtures__/activity';
import InventoryHistory from './InventoryHistory.svelte';
import type { InventoryModel } from './inventoryModel.svelte';
import { createPurchaseProposals } from './purchaseProposals.svelte';

/** The recycling was seen after the player last looked; the Shrapnel
 * before. One opening waits for review. */
const SEEN = '2026-11-15T10:01:30Z';

function model(overrides: Partial<Record<string, unknown>> = {}) {
	const typed = entry({ id: 'typed', itemName: 'Wool' });
	const checked = entry({ id: 'checked', reconciliationId: 'r1', kind: 'removal' });
	const rows = [typed, checked, shrapnel(), recycling()];
	const state = {
		kind: 'loot',
		allHistory: rows,
		history: rows,
		openings: [opening()],
		allListings: [],
		historyLoaded: true,
		historyLoading: false,
		undo: vi.fn().mockResolvedValue(undefined),
		undoCheck: vi.fn().mockResolvedValue(undefined),
		undoOpening: vi.fn().mockResolvedValue(undefined),
		reopenListing: vi.fn().mockResolvedValue(undefined),
		...overrides,
	};
	return state as unknown as InventoryModel & typeof state;
}

let detach: (() => void) | null = null;

async function show(inventory = model(), waiting: AccountProposal[] = [waitingOpening()]) {
	account.getProposals.mockImplementation(async (kind: string | null) =>
		kind === null ? waiting : [],
	);
	detach = activityMarks.connect();
	await waitFor(() => expect(activityMarks.current).not.toBeNull());
	const purchases = createPurchaseProposals({ onrecorded: vi.fn() });
	render(InventoryHistory, {
		props: {
			model: inventory,
			purchases,
			assets: [],
			onrecordpurchase: vi.fn(),
			onlistings: vi.fn(),
		},
	});
	return inventory;
}

beforeEach(() => {
	vi.clearAllMocks();
	account.getActivityMarks.mockResolvedValue({ seenThrough: SEEN, unseen: 2 });
	account.markActivitySeen.mockResolvedValue(undefined);
});

afterEach(() => {
	detach?.();
	detach = null;
});

describe('the history split by who decided', () => {
	it('opens on what the account did: what waits, then each event it recorded', async () => {
		await show();
		const space = await screen.findByTestId('account-space');
		const waiting = within(space).getByRole('region', { name: 'Waiting for you' });
		expect(within(waiting).getByText(/Halloween Strongbox opened/)).toBeTruthy();
		const events = within(space).getAllByTestId('account-event');
		expect(events.map((event) => event.dataset.kind)).toEqual(['recycling', 'shrapnel']);
	});

	it('shows each review as one event, and the hand entries alone', async () => {
		await show();
		await fireEvent.click(screen.getByRole('button', { name: /^Reviewed/ }));
		const reviewed = screen.getAllByTestId('reviewed-event');
		expect(reviewed.map((event) => event.dataset.kind).sort()).toEqual(['check', 'opening']);

		await fireEvent.click(screen.getByRole('button', { name: /^Your entries/ }));
		expect(screen.queryByTestId('reviewed-event')).toBeNull();
		expect(screen.getByText('Wool')).toBeTruthy();
	});

	it('keeps what waits in view whichever side the filter shows', async () => {
		const inventory = await show(model({ kind: 'equipment' }));
		const space = await screen.findByTestId('account-space');
		expect(within(space).getByRole('region', { name: 'Waiting for you' })).toBeTruthy();
		expect(within(space).queryAllByTestId('account-event')).toHaveLength(0);
		expect(inventory.kind).toBe('equipment');
	});
});

describe('what is new from the account', () => {
	it('marks what came after the player last looked, and counts it with what waits', async () => {
		await show();
		await screen.findByTestId('account-space');
		const recycled = screen
			.getAllByTestId('account-event')
			.find((event) => event.dataset.kind === 'recycling') as HTMLElement;
		expect(within(recycled).getByTestId('event-new')).toBeTruthy();
		const converted = screen
			.getAllByTestId('account-event')
			.find((event) => event.dataset.kind === 'shrapnel') as HTMLElement;
		expect(within(converted).queryByTestId('event-new')).toBeNull();
		// One opening waiting, one event new.
		expect(
			screen.getByRole('button', { name: 'From your account, 2 new or waiting' }),
		).toBeTruthy();
	});

	it('tells the app the player has seen up to the newest thing shown', async () => {
		await show();
		await waitFor(() =>
			expect(account.markActivitySeen).toHaveBeenCalledWith({ through: '2026-11-15T11:01:05Z' }),
		);
	});

	it('moves the mark only as far as the newest event when nothing waits', async () => {
		await show(model(), []);
		const newest = step(42, '2026-11-15T10:02:00Z').recordedAt;
		await waitFor(() => expect(account.markActivitySeen).toHaveBeenCalledWith({ through: newest }));
	});
});

describe('taking an event back from the space', () => {
	it('undoes a recycling through the record it made', async () => {
		const inventory = await show();
		const recycled = (await screen.findAllByTestId('account-event')).find(
			(event) => event.dataset.kind === 'recycling',
		) as HTMLElement;
		await fireEvent.click(within(recycled).getByRole('button', { name: 'Undo' }));
		await fireEvent.click(within(recycled).getByRole('button', { name: 'Undo recycling' }));
		expect(inventory.undo).toHaveBeenCalledWith(
			expect.objectContaining({ id: 'conversion-recycle' }),
			false,
		);
	});

	it('undoes an opening whole', async () => {
		const inventory = await show();
		await fireEvent.click(screen.getByRole('button', { name: /^Reviewed/ }));
		const opened = screen
			.getAllByTestId('reviewed-event')
			.find((event) => event.dataset.kind === 'opening') as HTMLElement;
		await fireEvent.click(within(opened).getByRole('button', { name: 'Undo' }));
		await fireEvent.click(within(opened).getByRole('button', { name: 'Undo the opening' }));
		expect(inventory.undoOpening).toHaveBeenCalledWith('opening-1');
	});
});
