// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountProposal, InventoryItem } from '$lib/api/commands.gen';
import type { TreeCuttingStock } from '$lib/features/analytics/treeCuttingModel.svelte';

const account = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getProposals: vi.fn(),
	claimProposals: vi.fn(),
	releaseProposals: vi.fn(),
	linkProposals: vi.fn(),
	dismissProposal: vi.fn(),
}));

vi.mock('$lib/api/account', () => account);
vi.mock('@tauri-apps/api/event', async (importOriginal) => ({
	...(await importOriginal<typeof import('@tauri-apps/api/event')>()),
	listen: vi.fn(async () => () => {}),
}));

import SaleProposals from './SaleProposals.svelte';
import {
	createSaleProposals,
	type SaleProposal,
	saleStart,
	saleTargetFor,
} from './saleProposals.svelte';

/** Forty Weapon Cells, 0.40 PED of TT, sold to a player as the Card rose by
 * 6.40 PED, seen outside every session. */
function sold(overrides: Partial<AccountProposal> = {}): SaleProposal {
	return {
		id: 61,
		readingId: 72,
		previousReadingId: 71,
		between: { from: '2026-11-18T16:51:53Z', to: '2026-11-18T16:56:54Z' },
		sessionId: null,
		possiblyRecorded: false,
		detectedAt: '2026-11-18T16:57:00Z',
		outcome: {
			kind: 'sale',
			itemName: 'Weapon Cells',
			units: 40,
			tt: { ped: 0.4, precision: 'wholePec' },
			received: { ped: 6.4, precision: 'exact' },
			markup: { ped: 6, precision: 'wholePec' },
			leftFrom: { kind: 'carried' },
		},
		...overrides,
	} as SaleProposal;
}

function stock(itemName: string): TreeCuttingStock {
	return {
		itemName,
		heldQty: 100,
		heldTt: 1,
		listedQty: 0,
		readings: [],
		opportunity: null,
		markupPct: null,
		markupHorizon: null,
		tier: null,
		effectiveMarkupPct: null,
		markupBasis: null,
		floored: false,
		salesPed: null,
		weeklySalesPed: null,
		recommendedPacketTt: null,
		marketItem: null,
	};
}

function asset(id: string, name: string): InventoryItem {
	return {
		id,
		name,
		ttValue: 0.4,
		markupPaid: 2,
		notes: null,
		acquiredAt: '2026-11-01',
		quantity: 40,
		divisible: true,
		purchases: [],
		origins: [],
		derivedFrom: [],
	};
}

const RECORD = 'Record sale of 40 Weapon Cells';

async function show(loot: TreeCuttingStock[] = [], assets: InventoryItem[] = []) {
	const onrecord = vi.fn();
	const model = createSaleProposals();
	await model.refresh();
	render(SaleProposals, { props: { model, loot, assets, onrecord } });
	return { model, onrecord };
}

beforeEach(() => {
	vi.clearAllMocks();
	account.getProposals.mockResolvedValue([sold()]);
	account.claimProposals.mockResolvedValue(undefined);
	account.releaseProposals.mockResolvedValue(undefined);
	account.linkProposals.mockResolvedValue(undefined);
	account.dismissProposal.mockResolvedValue(undefined);
});

describe('what looks like a sale to a player in the readings', () => {
	it('shows the item, its TT, what the Card rose by, and where it is from', async () => {
		await show([stock('Weapon Cells')]);
		expect(account.getProposals).toHaveBeenCalledWith('sale');
		expect(screen.getByText('Looks like a sale to a player')).toBeTruthy();
		expect(screen.getByText('40 Weapon Cells')).toBeTruthy();
		expect(
			screen.getByText(/0\.40 PED TT, the Card rose by\s+6\.40 PED, from your loot/),
		).toBeTruthy();
	});

	it('shows nothing while none waits', async () => {
		account.getProposals.mockResolvedValue([]);
		await show();
		expect(screen.queryByRole('region', { name: 'Sales read from your account' })).toBeNull();
	});

	it('records from the loot or the one asset of its name, and from neither when both or none hold it', () => {
		const cells = stock('weapon cells');
		const held = asset('asset-3', 'Weapon Cells');
		expect(saleTargetFor('Weapon Cells', [cells], [])).toEqual({ kind: 'loot', stock: cells });
		expect(saleTargetFor('Weapon Cells', [], [held])).toEqual({ kind: 'asset', asset: held });
		expect(saleTargetFor('Weapon Cells', [cells], [held])).toEqual({ kind: 'several' });
		expect(saleTargetFor('Weapon Cells', [], [held, asset('asset-4', 'Weapon Cells')])).toEqual({
			kind: 'several',
		});
		expect(saleTargetFor('Weapon Cells', [stock('Iron Stone')], [])).toEqual({ kind: 'none' });
	});

	it('offers no recording where the app cannot choose, only dismissing', async () => {
		await show();
		expect(screen.getByText(/not held here/)).toBeTruthy();
		expect(screen.queryByRole('button', { name: RECORD })).toBeNull();
		expect(screen.getByRole('button', { name: 'Dismiss 40 Weapon Cells' })).toBeTruthy();
	});

	it('opens the trade form filled in, saving nothing from the row', async () => {
		const cells = stock('Weapon Cells');
		const { onrecord } = await show([cells]);
		await fireEvent.click(screen.getByRole('button', { name: RECORD }));
		expect(onrecord).toHaveBeenCalledWith(sold(), { kind: 'loot', stock: cells });
		expect(account.claimProposals).not.toHaveBeenCalled();
		expect(saleStart(sold())).toEqual({
			key: 'proposal-61',
			quantity: 40,
			soldFor: 6.4,
			soldAt: '2026-11-18',
		});
	});

	it('asks before recording one that may be recorded already', async () => {
		account.getProposals.mockResolvedValue([sold({ possiblyRecorded: true })]);
		const { onrecord } = await show([stock('Weapon Cells')]);
		await fireEvent.click(screen.getByRole('button', { name: RECORD }));
		expect(onrecord).not.toHaveBeenCalled();
		await fireEvent.click(screen.getByRole('button', { name: 'Record again: 40 Weapon Cells' }));
		expect(onrecord).toHaveBeenCalledTimes(1);
	});
});

describe('recording a sale through its proposal', () => {
	it('claims it, saves through the trade form, then links it to the trade recorded', async () => {
		const model = createSaleProposals();
		await model.refresh();
		model.begin(sold());
		expect(model.start?.soldFor).toBe(6.4);
		const save = vi.fn().mockResolvedValue('trade-9');
		await model.around(save)({ quantity: 40 });

		expect(account.claimProposals).toHaveBeenCalledWith({ ids: [61], retake: [] });
		expect(save).toHaveBeenCalledWith({ quantity: 40 });
		expect(account.linkProposals).toHaveBeenCalledWith({ ids: [61], recordId: 'trade-9' });
		expect(model.isSettled(61)).toBe(true);
	});

	it('releases the claim when the save fails, and the form shows why', async () => {
		const model = createSaleProposals();
		await model.refresh();
		model.begin(sold());
		const save = vi.fn().mockRejectedValue(new Error('not held'));
		await expect(model.around(save)({})).rejects.toThrow('not held');
		expect(account.releaseProposals).toHaveBeenCalledWith({ ids: [61] });
		expect(account.linkProposals).not.toHaveBeenCalled();
	});

	it('saves as by hand when no proposal is open, and after the form closes', async () => {
		const model = createSaleProposals();
		await model.refresh();
		const save = vi.fn().mockResolvedValue('trade-10');
		await model.around(save)({});
		model.begin(sold());
		model.end();
		await model.around(save)({});
		expect(save).toHaveBeenCalledTimes(2);
		expect(account.claimProposals).not.toHaveBeenCalled();
		expect(model.start).toBeNull();
	});

	it('keeps a recorded sale whose link failed, to mark it done without saving again', async () => {
		account.linkProposals.mockRejectedValue(new Error('gone'));
		await show([stock('Weapon Cells')]);
		const model = createSaleProposals();
		await model.refresh();
		model.begin(sold());
		await model.around(vi.fn().mockResolvedValue('trade-11'))({});
		expect(model.isUnlinked(61)).toBe(true);

		account.linkProposals.mockResolvedValue(undefined);
		await model.markRecorded(sold());
		await waitFor(() => expect(model.isSettled(61)).toBe(true));
		expect(account.linkProposals).toHaveBeenLastCalledWith({ ids: [61], recordId: 'trade-11' });
	});

	it('keeps a saved trade linked when the list cannot be read again, never saving it twice', async () => {
		const model = createSaleProposals();
		await model.refresh();
		account.getProposals.mockRejectedValue(new Error('the database is busy'));
		model.begin(sold());
		const save = vi.fn().mockResolvedValue('trade-12');
		await model.around(save)({});

		expect(account.linkProposals).toHaveBeenCalledWith({ ids: [61], recordId: 'trade-12' });
		expect(account.releaseProposals).not.toHaveBeenCalled();
		expect(model.isSettled(61)).toBe(true);
		await expect(model.around(save)({})).rejects.toThrow('already being recorded');
		expect(save).toHaveBeenCalledTimes(1);
		expect(account.claimProposals).toHaveBeenCalledTimes(1);
	});

	it('offers a sale an undo opened again for recording in the same sitting', async () => {
		const model = createSaleProposals();
		await model.refresh();
		model.begin(sold());
		const save = vi.fn().mockResolvedValueOnce('trade-13').mockResolvedValueOnce('trade-14');
		account.getProposals.mockResolvedValue([]);
		await model.around(save)({});
		await waitFor(() => expect(model.pending).toEqual([]));
		expect(model.isSettled(61)).toBe(false);

		// The trade taken back: the proposal is listed again.
		account.getProposals.mockResolvedValue([sold()]);
		await model.refresh();
		expect(model.pending.map((proposal) => proposal.id)).toEqual([61]);
		await model.around(save)({});
		expect(account.linkProposals).toHaveBeenLastCalledWith({ ids: [61], recordId: 'trade-14' });
	});
});
