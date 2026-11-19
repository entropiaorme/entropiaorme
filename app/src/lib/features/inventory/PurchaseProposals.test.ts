// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountProposal, AssetPurchase, InventoryItem } from '$lib/api/commands.gen';

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

import PurchaseProposals from './PurchaseProposals.svelte';
import {
	createPurchaseProposals,
	type PurchaseProposal,
	purchaseDraft,
	purchaseTarget,
	recordIdOf,
} from './purchaseProposals.svelte';

/** A thousand Vibrant Sweat: 0.10 PED of TT for 3.10 PED, seen outside
 * every session. */
function bought(overrides: Partial<AccountProposal> = {}): PurchaseProposal {
	return {
		id: 41,
		readingId: 52,
		previousReadingId: 51,
		between: { from: '2026-11-15T09:10:00Z', to: '2026-11-15T09:11:00Z' },
		sessionId: null,
		possiblyRecorded: false,
		outcome: {
			kind: 'purchase',
			itemName: 'Vibrant Sweat',
			units: 1000,
			tt: { ped: 0.1, precision: 'wholePec' },
			paid: { ped: 3.1, precision: 'exact' },
			markup: { ped: 3, precision: 'wholePec' },
			arrivedIn: { kind: 'carried' },
		},
		...overrides,
	} as PurchaseProposal;
}

function asset(id: string, name: string, divisible = true): InventoryItem {
	return {
		id,
		name,
		ttValue: 0.5,
		markupPaid: 10,
		notes: null,
		acquiredAt: '2026-11-01',
		quantity: 5000,
		divisible,
		purchases: [],
		origins: [],
		derivedFrom: [],
	};
}

/** A read the test answers when it chooses. */
function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (cause: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

const RECORD = 'Record purchase of 1000 Vibrant Sweat';
function purchase(id: number, acquiredAt: string): AssetPurchase {
	return {
		id,
		quantity: 1000,
		ttValue: 0.1,
		markupPaid: 3,
		acquiredAt,
		fromConversion: false,
		editable: true,
	};
}

/** The asset as saved: the purchase just recorded has the highest id, though purchases are
 * listed by date (it was bought earlier than one already held). */
const saved: InventoryItem = {
	...asset('asset-9', 'Vibrant Sweat'),
	purchases: [purchase(12, '2026-11-01'), purchase(7, '2026-11-10')],
};

async function show(assets: InventoryItem[] = []) {
	const onrecorded = vi.fn().mockResolvedValue(undefined);
	const onrecord = vi.fn();
	const model = createPurchaseProposals({ onrecorded });
	await model.refresh();
	render(PurchaseProposals, { props: { model, assets, onrecord } });
	return { model, onrecorded, onrecord };
}

beforeEach(() => {
	vi.clearAllMocks();
	account.getProposals.mockResolvedValue([bought()]);
	account.claimProposals.mockResolvedValue(undefined);
	account.releaseProposals.mockResolvedValue(undefined);
	account.linkProposals.mockResolvedValue(undefined);
	account.dismissProposal.mockResolvedValue(undefined);
});

describe('what looks like a purchase in the readings', () => {
	it('shows the item, its TT, the markup paid above it, and where it would go', async () => {
		await show();
		expect(account.getProposals).toHaveBeenCalledWith('purchase');
		expect(screen.getByText('Looks like a purchase')).toBeTruthy();
		expect(screen.getByText('1000 Vibrant Sweat')).toBeTruthy();
		expect(screen.getByText(/0\.10 PED TT, 3\.00\s+PED markup paid, as a new asset/)).toBeTruthy();
	});

	it('shows nothing while none waits', async () => {
		account.getProposals.mockResolvedValue([]);
		await show();
		expect(screen.queryByRole('region', { name: 'Purchases read from your account' })).toBeNull();
	});

	it('names the asset it adds to, matching only one that can be bought more of', async () => {
		await show([asset('asset-3', 'vibrant sweat')]);
		expect(screen.getByText(/adds to your vibrant sweat/)).toBeTruthy();

		const proposal = bought();
		expect(purchaseTarget(proposal, [asset('asset-5', 'Vibrant Sweat', false)])).toEqual({
			kind: 'new',
		});
		expect(
			purchaseTarget(proposal, [
				asset('asset-5', 'Vibrant Sweat', false),
				asset('asset-3', 'Vibrant Sweat'),
			]),
		).toEqual({ kind: 'asset', asset: asset('asset-3', 'Vibrant Sweat') });
	});

	it('says to add it by hand when two assets that can take it share its name', async () => {
		await show([asset('asset-3', 'Vibrant Sweat'), asset('asset-4', 'Vibrant Sweat')]);
		expect(screen.getByText(/add it from the right asset's row, then dismiss this/)).toBeTruthy();
		expect(screen.queryByRole('button', { name: RECORD })).toBeNull();
		expect(screen.getByRole('button', { name: 'Dismiss 1000 Vibrant Sweat' })).toBeTruthy();
	});

	it('opens the asset form filled in, saving nothing from the row', async () => {
		const held = asset('asset-3', 'Vibrant Sweat');
		const { onrecord } = await show([held]);
		await fireEvent.click(screen.getByRole('button', { name: RECORD }));

		expect(onrecord).toHaveBeenCalledWith(bought(), { kind: 'asset', asset: held });
		expect(account.claimProposals).not.toHaveBeenCalled();
		expect(purchaseDraft(bought())).toEqual({
			key: 'proposal-41',
			name: 'Vibrant Sweat',
			quantity: 1000,
			ttValue: 0.1,
			markupPaid: 3,
			acquiredAt: '2026-11-15',
		});
	});

	it('asks before recording one that may be recorded already', async () => {
		account.getProposals.mockResolvedValue([bought({ possiblyRecorded: true })]);
		const { onrecord } = await show();
		expect(screen.getByText('This may already be recorded; check your assets first.')).toBeTruthy();

		await fireEvent.click(screen.getByRole('button', { name: RECORD }));
		expect(onrecord).not.toHaveBeenCalled();
		expect(screen.getByText(/record it again only if it is not there/)).toBeTruthy();

		await fireEvent.click(screen.getByRole('button', { name: 'Record again: 1000 Vibrant Sweat' }));
		expect(onrecord).toHaveBeenCalledTimes(1);
	});

	it('keeps a dismissed row still until the list read again drops it', async () => {
		await show();
		const reread = deferred<AccountProposal[]>();
		account.getProposals.mockReturnValue(reread.promise);
		await fireEvent.click(screen.getByRole('button', { name: 'Dismiss 1000 Vibrant Sweat' }));

		await waitFor(() => expect(account.dismissProposal).toHaveBeenCalledWith({ id: 41 }));
		await waitFor(() =>
			expect(
				(screen.getByRole('button', { name: 'Dismiss 1000 Vibrant Sweat' }) as HTMLButtonElement)
					.disabled,
			).toBe(true),
		);
		expect(screen.getByText('1000 Vibrant Sweat')).toBeTruthy();
		expect((screen.getByRole('button', { name: RECORD }) as HTMLButtonElement).disabled).toBe(true);

		reread.resolve([]);
		await waitFor(() => expect(screen.queryByText('1000 Vibrant Sweat')).toBeNull());
	});

	it('says when a recorded purchase could not be marked done, and marks it without saving again', async () => {
		account.linkProposals.mockRejectedValue(new Error('the link failed'));
		const { model } = await show();
		const save = vi.fn().mockResolvedValue(saved);
		await model.record(bought(), save);

		expect(await screen.findByText('Recorded in Vibrant Sweat, but not marked done.')).toBeTruthy();
		expect(screen.queryByRole('button', { name: RECORD })).toBeNull();

		account.linkProposals.mockResolvedValue(undefined);
		await fireEvent.click(
			screen.getByRole('button', { name: 'Mark as recorded: 1000 Vibrant Sweat' }),
		);
		await waitFor(() => expect(model.isSettled(41)).toBe(true));
		expect(account.linkProposals).toHaveBeenLastCalledWith({ ids: [41], recordId: '12' });
		expect(save).toHaveBeenCalledTimes(1);
		expect(model.unlinkedOf(41)).toBeNull();
	});
});

describe('recording a purchase through the asset form', () => {
	function model(onrecorded = vi.fn().mockResolvedValue(undefined)) {
		return { proposals: createPurchaseProposals({ onrecorded }), onrecorded };
	}

	it('claims before the save and links what was saved after it', async () => {
		const order: string[] = [];
		account.claimProposals.mockImplementation(async () => void order.push('claim'));
		account.linkProposals.mockImplementation(async () => void order.push('link'));
		const save = vi.fn(async () => {
			order.push('save');
			return saved;
		});
		const { proposals, onrecorded } = model();

		await expect(proposals.record(bought(), save)).resolves.toBe(saved);
		expect(order).toEqual(['claim', 'save', 'link']);
		expect(account.claimProposals).toHaveBeenCalledWith({ ids: [41], retake: [] });
		expect(account.linkProposals).toHaveBeenCalledWith({ ids: [41], recordId: '12' });
		expect(onrecorded).toHaveBeenCalled();
		expect(proposals.isSettled(41)).toBe(true);
		expect(proposals.recording).toBeNull();
	});

	it('releases the claim and rejects when the save fails, so the form says why', async () => {
		const { proposals } = model();
		const failed = new Error('The asset could not be saved');

		await expect(proposals.record(bought(), () => Promise.reject(failed))).rejects.toBe(failed);
		expect(account.releaseProposals).toHaveBeenCalledWith({ ids: [41] });
		expect(account.linkProposals).not.toHaveBeenCalled();
		expect(proposals.isSettled(41)).toBe(false);
		expect(proposals.recording).toBeNull();
	});

	it('keeps the claim of one possibly recorded when its save fails', async () => {
		const { proposals } = model();
		const possibly = bought({ possiblyRecorded: true });

		await expect(proposals.record(possibly, () => Promise.reject(new Error('no')))).rejects.toThrow(
			'no',
		);
		expect(account.claimProposals).toHaveBeenCalledWith({ ids: [41], retake: [41] });
		expect(account.releaseProposals).not.toHaveBeenCalled();
	});

	it('saves nothing when the claim is refused', async () => {
		account.claimProposals.mockRejectedValue(new Error('that proposal is no longer open'));
		const { proposals } = model();
		const save = vi.fn();

		await expect(proposals.record(bought(), save)).rejects.toThrow('no longer open');
		expect(save).not.toHaveBeenCalled();
	});

	it('records one at a time, and dismisses nothing meanwhile', async () => {
		const { proposals } = model();
		const pendingSave = deferred<InventoryItem>();
		const first = proposals.record(bought(), () => pendingSave.promise);
		await waitFor(() => expect(proposals.recording).toBe(41));

		const other = bought({ id: 42 });
		await expect(proposals.record(other, vi.fn())).rejects.toThrow('already being recorded');
		await proposals.dismiss(other);
		expect(account.dismissProposal).not.toHaveBeenCalled();

		pendingSave.resolve(saved);
		await first;
		// Recorded once, never again from here.
		await expect(proposals.record(bought(), vi.fn())).rejects.toThrow('already being recorded');
		expect(account.claimProposals).toHaveBeenCalledTimes(1);
	});

	it('keeps a saved purchase linked when the inventory cannot be read again, never saving it twice', async () => {
		const { proposals } = model(vi.fn().mockRejectedValue(new Error('the database is busy')));
		account.getProposals.mockRejectedValue(new Error('the database is busy'));
		const save = vi.fn().mockResolvedValue(saved);

		await expect(proposals.record(bought(), save)).resolves.toBe(saved);
		expect(account.linkProposals).toHaveBeenCalledWith({ ids: [41], recordId: '12' });
		expect(account.releaseProposals).not.toHaveBeenCalled();
		await expect(proposals.record(bought(), save)).rejects.toThrow('already being recorded');
		expect(save).toHaveBeenCalledTimes(1);
	});

	it('offers a purchase an undo opened again for recording in the same sitting', async () => {
		const { proposals } = model();
		account.getProposals.mockResolvedValue([]);
		await proposals.record(bought(), async () => saved);
		await waitFor(() => expect(proposals.isSettled(41)).toBe(false));

		// The purchase taken back: the proposal is listed again.
		account.getProposals.mockResolvedValue([bought()]);
		await proposals.refresh();
		expect(proposals.pending.map((proposal) => proposal.id)).toEqual([41]);
		await expect(proposals.record(bought(), async () => saved)).resolves.toBe(saved);
		expect(account.claimProposals).toHaveBeenCalledTimes(2);
	});

	it('keeps where it was recorded when the link fails', async () => {
		account.linkProposals.mockRejectedValue(new Error('down'));
		const { proposals } = model();

		await proposals.record(bought(), async () => saved);
		expect(account.linkProposals).toHaveBeenCalledTimes(2);
		expect(proposals.unlinkedOf(41)).toEqual({ recordId: '12', assetName: 'Vibrant Sweat' });
		expect(proposals.isSettled(41)).toBe(false);
	});
});

describe('what a recorded purchase links to', () => {
	it('is the purchase just recorded: the highest id, not the latest date or the last listed', () => {
		expect(recordIdOf(saved)).toBe('12');
	});

	it('refuses an asset that holds no purchase', () => {
		expect(() => recordIdOf(asset('asset-1', 'Empty'))).toThrow(
			'Empty holds no purchase to link to',
		);
	});
});
