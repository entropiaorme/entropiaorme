// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	StockReconciliationDisposition,
	StockReconciliationDraft,
	StockReconciliationLine,
	StockReconciliationRevised,
	StockReconciliationUnchanged,
} from '$lib/api/commands.gen';

const api = vi.hoisted(() => ({ getTrackingSnapshot: vi.fn() }));
const account = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	planStockReconciliation: vi.fn(),
	planStockReconciliationFromPaste: vi.fn(),
	getStockReconciliationDraft: vi.fn(),
	reviseStockReconciliation: vi.fn(),
	applyStockReconciliation: vi.fn(),
	undoStockReconciliation: vi.fn(),
}));
vi.mock('$lib/api', () => ({ getTrackingSnapshot: api.getTrackingSnapshot }));
vi.mock('$lib/api/account', () => account);
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

import {
	createReconciliationModel,
	NO_READING_REFUSAL,
	TRACKING_REFUSAL,
} from './reconciliationModel.svelte';
import StockCheck from './StockCheck.svelte';

function line(
	itemName: string,
	appQuantity: number,
	observedQuantity: number,
	ped: number,
	disposition: StockReconciliationDisposition,
): StockReconciliationLine {
	return {
		itemName,
		appQuantity,
		assetQuantity: 0,
		observedQuantity,
		difference: observedQuantity - appQuantity,
		unitTt: { kind: 'exact', ped },
		disposition,
		takenBack: false,
	};
}

/** The draft as the backend proposes it, the largest difference by TT
 * first: 10 PED of ingots missing, 4 PED of Shrapnel, then the rest. */
function proposed(): StockReconciliationDraft {
	return {
		id: 'd1',
		source: { kind: 'reading', readingId: 7 },
		observedAt: '2026-11-17T10:00:00Z',
		lines: [
			line('Lysterium Ingot', 500, 300, 0.05, { kind: 'removed' }),
			line('Shrapnel', 100000, 60000, 0.0001, { kind: 'shrapnelConverted', soldAtTt: null }),
			line('Iron Ingot', 20, 30, 0.13, { kind: 'unknownOrigin' }),
			line('Animal Oil Residue', 1000, 900, 0.01, { kind: 'removed' }),
			line('Nanocube', 0, 50, 0.01, { kind: 'unknownOrigin' }),
		],
		excluded: [
			{ itemName: 'Universal Ammo', quantity: 12345, reason: 'universalAmmo' },
			{ itemName: 'Opalo (L)', quantity: 1, reason: 'equipment' },
		],
		groups: [],
		caution: null,
	};
}

/** The proposed draft with some lines set otherwise. */
function revised(
	changes: Record<string, StockReconciliationDisposition>,
): StockReconciliationDraft {
	const draft = proposed();
	return {
		...draft,
		lines: draft.lines.map((row) =>
			changes[row.itemName] ? { ...row, disposition: changes[row.itemName] } : row,
		),
	};
}

/** The backend's answer to a revision. */
function answer(
	draft: StockReconciliationDraft,
	unchanged: StockReconciliationUnchanged[] = [],
): StockReconciliationRevised {
	return { draft, unchanged };
}

/** A call the test answers when it chooses. */
function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (cause: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

function status(hasReadings = true) {
	return {
		state: 'connected',
		signal: 'connected',
		connectedAt: null,
		lastReadingAt: '2026-11-17T10:00:00Z',
		readingSession: false,
		hasConnected: true,
		confirming: null,
		signingIn: false,
		hasReadings,
	};
}

async function mount() {
	const model = createReconciliationModel();
	const ondone = vi.fn();
	render(StockCheck, { props: { model, ondone } });
	await model.load();
	return { model, ondone };
}

function decision(item: string) {
	return screen.getByRole('group', { name: `What happened to ${item}` });
}

function pressed(item: string): string | null {
	const active = within(decision(item))
		.getAllByRole('button')
		.find((button) => button.getAttribute('aria-pressed') === 'true');
	return active?.textContent?.trim() ?? null;
}

beforeEach(() => {
	vi.clearAllMocks();
	api.getTrackingSnapshot.mockResolvedValue({ status: 'idle' });
	account.getAccountStatus.mockResolvedValue(status());
	account.getStockReconciliationDraft.mockResolvedValue(proposed());
	account.planStockReconciliation.mockResolvedValue(proposed());
	account.planStockReconciliationFromPaste.mockResolvedValue(proposed());
	account.reviseStockReconciliation.mockResolvedValue(answer(proposed()));
	account.applyStockReconciliation.mockResolvedValue({
		outcome: 'applied',
		reconciliationId: 'r1',
		settled: ['Lysterium Ingot', 'Shrapnel', 'Iron Ingot', 'Animal Oil Residue'],
		refused: [{ itemName: 'Nanocube', reason: 'stockMovedSince' }],
	});
	account.undoStockReconciliation.mockResolvedValue(undefined);
});

describe('the review', () => {
	it('shows each line at what the app proposes, the largest difference first', async () => {
		await mount();

		const items = screen.getAllByRole('listitem').map((row) => row.getAttribute('aria-label'));
		expect(items).toEqual([
			'Lysterium Ingot',
			'Shrapnel',
			'Iron Ingot',
			'Animal Oil Residue',
			'Nanocube',
		]);
		expect(pressed('Lysterium Ingot')).toBe('Removed');
		expect(pressed('Shrapnel')).toBe('Converted');
		expect(pressed('Iron Ingot')).toBe('Unknown origin');
		// A surplus can only be held or left; a shortfall only removed or left.
		expect(within(decision('Iron Ingot')).queryByRole('button', { name: 'Removed' })).toBeNull();
		expect(
			within(decision('Animal Oil Residue')).queryByRole('button', { name: 'Unknown origin' }),
		).toBeNull();
		expect(
			screen.getByText(
				/2 items removed from stock, Shrapnel converted to Universal Ammo, 2 surpluses added/,
			),
		).toBeTruthy();
	});

	it('marks the large shortfall, and only it', async () => {
		await mount();
		const marked = screen.getAllByText('A large shortfall:');
		expect(marked).toHaveLength(1);
		expect(marked[0].closest('li')?.getAttribute('aria-label')).toBe('Lysterium Ingot');
		const row = screen.getByRole('listitem', { name: 'Lysterium Ingot' });
		expect(within(row).getByText('−200')).toBeTruthy();
		expect(within(row).getByText('10.00 PED')).toBeTruthy();
	});

	it('sends each change to the backend and shows the draft it answers with', async () => {
		account.reviseStockReconciliation.mockResolvedValueOnce(
			answer(revised({ 'Lysterium Ingot': { kind: 'leftAsIs' } })),
		);
		await mount();

		// The choice it already stands at is no change.
		const ingot = within(decision('Lysterium Ingot'));
		await fireEvent.click(ingot.getByRole('button', { name: 'Removed' }));
		expect(account.reviseStockReconciliation).not.toHaveBeenCalled();

		await fireEvent.click(ingot.getByRole('button', { name: 'Left as is' }));
		await waitFor(() => expect(pressed('Lysterium Ingot')).toBe('Left as is'));
		expect(account.reviseStockReconciliation).toHaveBeenCalledOnce();
		expect(account.reviseStockReconciliation).toHaveBeenCalledWith({
			draftId: 'd1',
			revision: { kind: 'set', items: ['Lysterium Ingot'], choice: 'leftAsIs' },
		});
		expect(screen.getByText(/1 left as it is/)).toBeTruthy();
	});

	it('shows a refused change beside the line, with the draft unchanged', async () => {
		account.reviseStockReconciliation.mockRejectedValueOnce(
			new Error('the Shrapnel sold at TT is more than nothing and at most the 40000 missing'),
		);
		await mount();

		const field = screen.getByRole('spinbutton', {
			name: 'How many of the missing Shrapnel sold at TT',
		});
		await fireEvent.input(field, { target: { value: '50000' } });
		const form = field.closest('form');
		if (!form) throw new Error('the amount sits in a form');
		await fireEvent.submit(form);

		expect(
			await screen.findByText(
				'The Shrapnel sold at TT is more than nothing and at most the 40000 missing.',
			),
		).toBeTruthy();
		expect(account.reviseStockReconciliation).toHaveBeenCalledWith({
			draftId: 'd1',
			revision: { kind: 'shrapnelSoldAtTt', quantity: 50000 },
		});
		expect(pressed('Shrapnel')).toBe('Converted');
	});

	it('warns when a paste came soon after a session', async () => {
		account.getStockReconciliationDraft.mockResolvedValue({
			...proposed(),
			source: { kind: 'paste' },
			caution: { kind: 'pasteSoonAfterSession', sessionEndedAt: '2026-11-17T09:58:00Z' },
		});
		await mount();
		expect(
			screen.getByText(
				/You pasted this soon after a session ended: copy your inventory afresh if it changed since\./,
			),
		).toBeTruthy();
	});

	it('says no caution when there is none', async () => {
		await mount();
		expect(screen.queryByText(/soon after a session ended/)).toBeNull();
	});

	it('says how much of the app quantity is held as an asset', async () => {
		const draft = proposed();
		draft.lines[0] = { ...draft.lines[0], assetQuantity: 120 };
		account.getStockReconciliationDraft.mockResolvedValue(draft);
		await mount();
		const row = screen.getByRole('listitem', { name: 'Lysterium Ingot' });
		expect(within(row).getAllByText(/of which 120 held as an asset/).length).toBeGreaterThan(0);
		const other = screen.getByRole('listitem', { name: 'Shrapnel' });
		expect(within(other).queryByText(/held as an asset/)).toBeNull();
	});

	it('offers an extra of inexact TT neither as unknown origin nor for a conversion', async () => {
		const draft = proposed();
		draft.lines[2] = {
			...draft.lines[2],
			unitTt: { kind: 'truncated', ped: 0.13 },
			disposition: { kind: 'leftAsIs' },
		};
		account.getStockReconciliationDraft.mockResolvedValue(draft);
		await mount();

		const iron = within(decision('Iron Ingot'));
		expect(iron.queryByRole('button', { name: 'Unknown origin' })).toBeNull();
		expect(pressed('Iron Ingot')).toBe('Left as is');
		const row = screen.getByRole('listitem', { name: 'Iron Ingot' });
		expect(
			within(row).getByRole('button', { name: 'Why it can only be left as it is' }),
		).toBeTruthy();

		await fireEvent.click(screen.getByRole('checkbox', { name: 'Select Iron Ingot' }));
		await fireEvent.click(screen.getByRole('checkbox', { name: 'Select Animal Oil Residue' }));
		const group = screen.getByRole('button', { name: 'Make one conversion' });
		expect(group.hasAttribute('disabled')).toBe(true);
		expect(
			screen.getByText(/Iron Ingot cannot join a conversion: its TT per unit is not known exactly/),
		).toBeTruthy();
	});
});

describe('changing many lines at once', () => {
	it('sets a filtered selection, and says which lines the choice did not fit', async () => {
		await mount();

		await fireEvent.input(screen.getByRole('textbox', { name: 'Filter items' }), {
			target: { value: 'ingot' },
		});
		const shown = screen.getAllByRole('listitem').map((row) => row.getAttribute('aria-label'));
		expect(shown).toEqual(['Lysterium Ingot', 'Iron Ingot']);

		await fireEvent.click(screen.getByRole('checkbox', { name: /Select all shown/ }));
		const bar = screen.getByRole('group', { name: 'Change the selected items' });
		expect(within(bar).getByText(/2 selected/)).toBeTruthy();

		await fireEvent.click(within(bar).getByRole('button', { name: 'Left as is' }));
		await waitFor(() =>
			expect(account.reviseStockReconciliation).toHaveBeenLastCalledWith({
				draftId: 'd1',
				revision: { kind: 'set', items: ['Lysterium Ingot', 'Iron Ingot'], choice: 'leftAsIs' },
			}),
		);

		// Removed fits only the missing ingots: the backend leaves the extra
		// ones alone and says why.
		account.reviseStockReconciliation.mockResolvedValueOnce(
			answer(proposed(), [
				{ itemName: 'Iron Ingot', reason: 'Iron Ingot is not missing, so nothing is removed' },
			]),
		);
		await fireEvent.click(within(bar).getByRole('button', { name: 'Removed' }));
		expect(
			await within(bar).findByText(
				'Set on 1; 1 did not fit: Iron Ingot is not missing, so nothing is removed.',
			),
		).toBeTruthy();
		expect(account.reviseStockReconciliation).toHaveBeenLastCalledWith({
			draftId: 'd1',
			revision: { kind: 'set', items: ['Lysterium Ingot', 'Iron Ingot'], choice: 'removed' },
		});
		expect(account.reviseStockReconciliation).toHaveBeenCalledTimes(2);
	});

	it('makes a selection one conversion, and sets a line apart again', async () => {
		account.reviseStockReconciliation.mockResolvedValueOnce(
			answer({
				...revised({
					'Animal Oil Residue': { kind: 'conversionGroup', group: 1 },
					Nanocube: { kind: 'conversionGroup', group: 1 },
				}),
				groups: [
					{
						group: 1,
						ttIn: 1,
						ttOut: 0.4,
						outputItem: 'Nanocube',
						outputQuantity: 40,
						remainderQuantity: 10,
					},
				],
			}),
		);
		await mount();

		await fireEvent.click(screen.getByRole('checkbox', { name: 'Select Animal Oil Residue' }));
		// One line alone is no conversion, and the bar says what it needs.
		expect(
			screen.getByRole('button', { name: 'Make one conversion' }).hasAttribute('disabled'),
		).toBe(true);
		expect(
			screen.getByText(
				'To make one conversion, select the items that went in and the one that came out.',
			),
		).toBeTruthy();
		await fireEvent.click(screen.getByRole('checkbox', { name: 'Select Nanocube' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Make one conversion' }));

		expect(await screen.findByText('Went into Nanocube')).toBeTruthy();
		expect(account.reviseStockReconciliation).toHaveBeenCalledWith({
			draftId: 'd1',
			revision: { kind: 'group', items: ['Animal Oil Residue', 'Nanocube'] },
		});
		expect(
			screen.getByText('Made from Animal Oil Residue: 1.00 PED of TT in, 0.40 PED out'),
		).toBeTruthy();
		expect(
			screen.getByText(
				'10 more Nanocube arrived some other way and is kept as stock of unknown origin.',
			),
		).toBeTruthy();
		expect(screen.getByText(/1 conversion/)).toBeTruthy();
		// The selection is spent on the conversion.
		await waitFor(() =>
			expect(screen.queryByRole('group', { name: 'Change the selected items' })).toBeNull(),
		);

		const nanocube = screen.getByRole('listitem', { name: 'Nanocube' });
		await fireEvent.click(within(nanocube).getByRole('button', { name: 'Set apart' }));
		await waitFor(() =>
			expect(account.reviseStockReconciliation).toHaveBeenLastCalledWith({
				draftId: 'd1',
				revision: { kind: 'set', items: ['Nanocube'], choice: 'unknownOrigin' },
			}),
		);
	});

	it('shows a refused conversion on the selection bar', async () => {
		account.reviseStockReconciliation.mockRejectedValueOnce(
			new Error('a conversion is one item that arrived and the missing items that went into it'),
		);
		await mount();

		await fireEvent.click(screen.getByRole('checkbox', { name: 'Select Lysterium Ingot' }));
		await fireEvent.click(screen.getByRole('checkbox', { name: 'Select Animal Oil Residue' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Make one conversion' }));

		const bar = screen.getByRole('group', { name: 'Change the selected items' });
		expect(
			await within(bar).findByText(
				'A conversion is one item that arrived and the missing items that went into it.',
			),
		).toBeTruthy();
		expect(pressed('Lysterium Ingot')).toBe('Removed');
	});
});

describe('applying', () => {
	it('says what was settled, what was left as it is, and what was left for later', async () => {
		account.getStockReconciliationDraft.mockResolvedValue(
			revised({ 'Animal Oil Residue': { kind: 'leftAsIs' } }),
		);
		await mount();

		await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
		expect(await screen.findByText(/3 items are settled against your account/)).toBeTruthy();
		expect(account.applyStockReconciliation).toHaveBeenCalledWith({ draftId: 'd1' });
		expect(screen.getByText(/1 item is left as it is/)).toBeTruthy();
		expect(screen.getByText('Animal Oil Residue.')).toBeTruthy();
		expect(screen.queryByText(/Your stock now matches/)).toBeNull();
		const left = screen.getByText('Left for the next check').parentElement;
		if (!left) throw new Error('the refused lines are listed');
		expect(within(left).getByText('Nanocube')).toBeTruthy();
		expect(
			within(left).getByText('loot or a change of it was recorded after your account was read'),
		).toBeTruthy();
	});

	it('says the stock matches only when nothing was refused or left as it is', async () => {
		account.applyStockReconciliation.mockResolvedValueOnce({
			outcome: 'applied',
			reconciliationId: 'r1',
			settled: proposed().lines.map((row) => row.itemName),
			refused: [],
		});
		await mount();
		await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
		expect(await screen.findByText(/Your stock now matches what was compared/)).toBeTruthy();
	});

	it('offers no Undo when the check recorded nothing', async () => {
		account.applyStockReconciliation.mockResolvedValueOnce({
			outcome: 'applied',
			reconciliationId: null,
			settled: [],
			refused: [{ itemName: 'Lysterium Ingot', reason: 'stockMovedSince' }],
		});
		await mount();
		await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
		expect(await screen.findByText('Nothing was recorded.')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Undo this check' })).toBeNull();
	});

	it('offers no Undo when every line was left as it is', async () => {
		const leftAlone = Object.fromEntries(
			proposed().lines.map((row) => [row.itemName, { kind: 'leftAsIs' as const }]),
		);
		account.getStockReconciliationDraft.mockResolvedValue(revised(leftAlone));
		account.applyStockReconciliation.mockResolvedValueOnce({
			outcome: 'applied',
			reconciliationId: 'r1',
			settled: proposed().lines.map((row) => row.itemName),
			refused: [],
		});
		await mount();
		await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
		expect(await screen.findByText(/5 items are left as they are/)).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Undo this check' })).toBeNull();
	});

	it('keeps the review while a session is tracked', async () => {
		account.applyStockReconciliation.mockResolvedValueOnce({ outcome: 'sessionTracked' });
		await mount();

		await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
		expect(await screen.findByText(TRACKING_REFUSAL)).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Apply' })).toBeTruthy();
	});

	it('holds the review still while applying, taking no change', async () => {
		const applying = deferred<unknown>();
		account.applyStockReconciliation.mockReturnValueOnce(applying.promise);
		const { model } = await mount();

		await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
		const review = screen.getByRole('textbox', { name: 'Filter items' }).closest('[inert]');
		expect(review).not.toBeNull();
		await model.setLine(proposed().lines[0], 'leftAsIs');
		expect(account.reviseStockReconciliation).not.toHaveBeenCalled();

		applying.resolve({ outcome: 'sessionTracked' });
		expect(await screen.findByText(TRACKING_REFUSAL)).toBeTruthy();
		expect(screen.getByRole('textbox', { name: 'Filter items' }).closest('[inert]')).toBeNull();
	});

	it('takes the whole check back with one Undo, and says so where the focus lands', async () => {
		await mount();
		await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
		await fireEvent.click(await screen.findByRole('button', { name: 'Undo this check' }));

		const said = await screen.findByText(/Taken back/);
		expect(account.undoStockReconciliation).toHaveBeenCalledWith({ id: 'r1' });
		expect(screen.queryByRole('button', { name: 'Undo this check' })).toBeNull();
		expect(screen.getByRole('heading', { name: 'Check undone' })).toBeTruthy();
		await waitFor(() => expect(document.activeElement).toBe(said));
	});

	it('says why an undo was refused', async () => {
		account.undoStockReconciliation.mockRejectedValueOnce(
			new Error('Nanocube made by it has since been sold'),
		);
		await mount();
		await fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
		await fireEvent.click(await screen.findByRole('button', { name: 'Undo this check' }));

		expect(await screen.findByText('Nanocube made by it has since been sold.')).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Undo this check' })).toBeTruthy();
		expect(screen.getByRole('heading', { name: 'Applied' })).toBeTruthy();
	});
});

describe('changes in flight', () => {
	it('lands two changes in the order made, showing the second answer', async () => {
		const first = deferred<StockReconciliationRevised>();
		const second = deferred<StockReconciliationRevised>();
		account.reviseStockReconciliation
			.mockReturnValueOnce(first.promise)
			.mockReturnValueOnce(second.promise);
		const { model } = await mount();

		void model.setLine(proposed().lines[0], 'leftAsIs');
		void model.setLine(proposed().lines[3], 'leftAsIs');
		await waitFor(() => expect(account.reviseStockReconciliation).toHaveBeenCalledTimes(1));
		expect(screen.getByText('Saving your last change...')).toBeTruthy();

		first.resolve(answer(revised({ 'Lysterium Ingot': { kind: 'leftAsIs' } })));
		await waitFor(() => expect(account.reviseStockReconciliation).toHaveBeenCalledTimes(2));
		expect(account.reviseStockReconciliation).toHaveBeenLastCalledWith({
			draftId: 'd1',
			revision: { kind: 'set', items: ['Animal Oil Residue'], choice: 'leftAsIs' },
		});
		second.resolve(
			answer(
				revised({
					'Lysterium Ingot': { kind: 'leftAsIs' },
					'Animal Oil Residue': { kind: 'leftAsIs' },
				}),
			),
		);
		await waitFor(() => expect(pressed('Animal Oil Residue')).toBe('Left as is'));
		expect(pressed('Lysterium Ingot')).toBe('Left as is');
		expect(screen.queryByText('Saving your last change...')).toBeNull();
	});

	it('applies only once the change on its way has landed', async () => {
		const change = deferred<StockReconciliationRevised>();
		account.reviseStockReconciliation.mockReturnValueOnce(change.promise);
		const { model } = await mount();

		void model.setLine(proposed().lines[0], 'leftAsIs');
		const apply = screen.getByRole('button', { name: 'Apply' });
		expect(apply.hasAttribute('disabled')).toBe(false);
		await fireEvent.click(apply);
		await waitFor(() => expect(account.reviseStockReconciliation).toHaveBeenCalledOnce());
		expect(account.applyStockReconciliation).not.toHaveBeenCalled();

		change.resolve(answer(revised({ 'Lysterium Ingot': { kind: 'leftAsIs' } })));
		await waitFor(() => expect(account.applyStockReconciliation).toHaveBeenCalledOnce());
	});

	it('drops a change still queued when the player starts again', async () => {
		const first = deferred<StockReconciliationRevised>();
		account.reviseStockReconciliation.mockReturnValueOnce(first.promise);
		const { model } = await mount();

		void model.setLine(proposed().lines[0], 'leftAsIs');
		const queued = model.setLine(proposed().lines[3], 'leftAsIs');
		await waitFor(() => expect(account.reviseStockReconciliation).toHaveBeenCalledTimes(1));
		model.startOver();
		first.reject(new Error('that reconciliation is no longer being reviewed'));
		await queued;

		expect(account.reviseStockReconciliation).toHaveBeenCalledTimes(1);
		expect(model.revisionError).toBeNull();
		expect(model.draft).toBeNull();
	});

	it("moves the focus to the line's choice after setting it apart", async () => {
		account.getStockReconciliationDraft.mockResolvedValue(
			revised({
				'Animal Oil Residue': { kind: 'conversionGroup', group: 1 },
				Nanocube: { kind: 'conversionGroup', group: 1 },
			}),
		);
		await mount();

		const nanocube = screen.getByRole('listitem', { name: 'Nanocube' });
		await fireEvent.click(within(nanocube).getByRole('button', { name: 'Set apart' }));
		await waitFor(() =>
			expect(document.activeElement).toBe(
				within(decision('Nanocube')).getByRole('button', { name: 'Unknown origin' }),
			),
		);
	});
});

describe('starting a check', () => {
	beforeEach(() => {
		account.getStockReconciliationDraft.mockResolvedValue(null);
	});

	it('compares with the latest reading', async () => {
		await mount();
		await fireEvent.click(screen.getByRole('button', { name: 'Compare with the latest reading' }));
		expect(await screen.findByRole('listitem', { name: 'Lysterium Ingot' })).toBeTruthy();
		expect(account.planStockReconciliation).toHaveBeenCalledOnce();
	});

	it('cannot compare with a reading while tracking, or before one is stored', async () => {
		api.getTrackingSnapshot.mockResolvedValue({ status: 'active' });
		const { model } = await mount();
		const reading = screen.getByRole('button', { name: 'Compare with the latest reading' });
		expect(reading.hasAttribute('disabled')).toBe(true);
		expect(screen.getAllByText(TRACKING_REFUSAL).length).toBeGreaterThan(0);
		expect(model.pasteRefusal).toBe(TRACKING_REFUSAL);

		api.getTrackingSnapshot.mockResolvedValue({ status: 'idle' });
		account.getAccountStatus.mockResolvedValue(status(false));
		await model.load();
		await waitFor(() => expect(screen.getByText(NO_READING_REFUSAL)).toBeTruthy());
		expect(model.pasteRefusal).toBeNull();
	});

	it('compares a pasted inventory, and says which line did not read', async () => {
		account.planStockReconciliationFromPaste.mockRejectedValueOnce(new Error('line 3 has no name'));
		await mount();

		const zone = screen.getByLabelText('Inventory pasted from the account site');
		await fireEvent.input(zone, { target: { value: 'Name\tQuantity\n\t4' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Compare' }));
		expect(await screen.findByText('Line 3 has no name.')).toBeTruthy();
		expect(account.planStockReconciliationFromPaste).toHaveBeenCalledWith({
			text: 'Name\tQuantity\n\t4',
		});

		await fireEvent.click(screen.getByRole('button', { name: 'Compare' }));
		expect(await screen.findByRole('listitem', { name: 'Lysterium Ingot' })).toBeTruthy();
	});

	it('says when the inventory matches, and lists what was left out on request', async () => {
		account.planStockReconciliation.mockResolvedValueOnce({ ...proposed(), lines: [] });
		await mount();
		await fireEvent.click(screen.getByRole('button', { name: 'Compare with the latest reading' }));

		expect(await screen.findByText('Your inventory matches your account.')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Apply' })).toBeNull();

		const toggle = screen.getByRole('button', { name: /Left out of the comparison \(2\)/ });
		expect(toggle.getAttribute('aria-expanded')).toBe('false');
		expect(screen.queryByText('PED, not stock')).toBeNull();
		await fireEvent.click(toggle);
		expect(screen.getByText('PED, not stock')).toBeTruthy();
		expect(screen.getByText('Equipment')).toBeTruthy();
	});
});
