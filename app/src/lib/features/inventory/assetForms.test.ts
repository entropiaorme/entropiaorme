import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AssetPurchase, InventoryItem } from '$lib/api/commands.gen';

const inventory = vi.hoisted(() => ({ addAssetPurchase: vi.fn() }));
vi.mock('$lib/api/inventory', () => inventory);

import { createAssetForms } from './assetForms.svelte';
import type { PurchaseProposal, PurchaseProposals } from './purchaseProposals.svelte';

const sweat: InventoryItem = {
	id: 'asset-9',
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

const bought = {
	id: 41,
	between: { from: '2026-11-15T09:10:00Z', to: '2026-11-15T09:11:00Z' },
	outcome: {
		kind: 'purchase',
		itemName: 'Vibrant Sweat',
		units: 1000,
		tt: { ped: 0.1, precision: 'wholePec' },
		markup: { ped: 3, precision: 'wholePec' },
	},
} as unknown as PurchaseProposal;

const input = { quantity: 1000, ttValue: 0.1, markupPaid: 3, acquiredAt: '2026-11-15' };

function setup() {
	const record = vi.fn((_proposal: PurchaseProposal, save: () => Promise<InventoryItem>) => save());
	const options = {
		proposals: { record } as unknown as PurchaseProposals,
		addPurchase: vi.fn().mockResolvedValue(undefined),
		updatePurchase: vi.fn().mockResolvedValue(undefined),
		refresh: vi.fn().mockResolvedValue(undefined),
	};
	return { forms: createAssetForms(options), options, record };
}

beforeEach(() => {
	vi.clearAllMocks();
	inventory.addAssetPurchase.mockResolvedValue(sweat);
});

describe('the asset forms', () => {
	it('opens a blank asset, or the one being corrected', () => {
		const { forms } = setup();
		forms.editAsset(sweat);
		expect(forms.assetOpen).toBe(true);
		expect(forms.assetToEdit).toBe(sweat);
		expect(forms.around).toBeNull();

		forms.addAsset();
		expect(forms.assetToEdit).toBeNull();
		expect(forms.assetPrefill).toBeNull();
	});

	it('buys more of an asset, or corrects a purchase of it', async () => {
		const { forms, options } = setup();
		forms.buyMore(sweat);
		await forms.savePurchase(input);
		expect(options.addPurchase).toHaveBeenCalledWith(sweat, input);

		const purchase = { id: 7 } as AssetPurchase;
		forms.editPurchase(sweat, purchase);
		expect(forms.purchase).toBe(purchase);
		await forms.savePurchase(input);
		expect(options.updatePurchase).toHaveBeenCalledWith(7, input);

		forms.closePurchase();
		expect(forms.purchaseItem).toBeNull();
	});

	it('records a proposed purchase into an asset as the proposal', async () => {
		const { forms, options, record } = setup();
		forms.recordProposal(bought, { kind: 'asset', asset: sweat });
		expect(forms.purchaseItem).toBe(sweat);
		expect(forms.purchasePrefill?.quantity).toBe(1000);

		await forms.savePurchase(input);
		expect(record).toHaveBeenCalledWith(bought, expect.any(Function));
		expect(inventory.addAssetPurchase).toHaveBeenCalledWith('asset-9', input);
		expect(options.addPurchase).not.toHaveBeenCalled();
	});

	it('records a proposed new asset as the proposal, then lets go of it', async () => {
		const { forms, options, record } = setup();
		forms.recordProposal(bought, { kind: 'new' });
		expect(forms.assetOpen).toBe(true);
		expect(forms.assetToEdit).toBeNull();
		expect(forms.assetPrefill?.name).toBe('Vibrant Sweat');

		const save = vi.fn().mockResolvedValue(sweat);
		await forms.around?.(save);
		expect(record).toHaveBeenCalledWith(bought, save);

		forms.assetSaved();
		expect(forms.around).toBeNull();
		expect(options.refresh).toHaveBeenCalledOnce();
	});

	it('leaves a proposal that names no single asset alone', () => {
		const { forms } = setup();
		forms.recordProposal(bought, { kind: 'several' });
		expect(forms.assetOpen).toBe(false);
		expect(forms.purchaseItem).toBeNull();
	});
});
