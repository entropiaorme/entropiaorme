// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AuctionListing } from '$lib/types/analytics';

const account = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getProposals: vi.fn(),
	linkProposals: vi.fn(),
	dismissProposal: vi.fn(),
}));
vi.mock('$lib/api/account', () => account);
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

import { returned, sold } from '$lib/features/strongbox/__fixtures__/proposals';
import AuctionListings from './AuctionListings.svelte';
import { createListingProposals } from './listingProposals.svelte';

/** Twelve strongboxes sold at 3 PED each, two of them beyond tracked stock. */
const boxes: AuctionListing = {
	id: 'boxes',
	itemName: 'Halloween Strongbox',
	quantity: 12,
	attributedQty: 10,
	unattributedQty: 2,
	ttValue: 0,
	attributedTt: 0,
	startingBid: 30,
	buyout: 36,
	listingFee: 0.5,
	listedAt: '2026-11-01',
	status: 'sold',
	finalPrice: 36,
	saleFee: 1,
	resolvedAt: '2026-11-02',
	subjectKind: 'loot',
	inventoryItemId: null,
	costBasis: null,
	channel: 'auction',
	auctionDays: 3,
	expiresAt: null,
	activityNetMarkup: 28.75,
	grossMarkup: 36,
	recordedFromAccount: null,
	listedFromAccount: null,
};

describe('a listing of an item with no TT', () => {
	it('reads it by count and price each, never as a rate on TT', () => {
		render(AuctionListings, { props: { open: [], resolved: [boxes], onresolve: vi.fn() } });

		expect(screen.getByText('12 × Halloween Strongbox')).not.toBeNull();
		expect(screen.getByText('No TT')).not.toBeNull();
		expect(screen.getByText('Quantity')).not.toBeNull();
		expect(screen.getByText('Each')).not.toBeNull();
		expect(screen.getByText('3.00')).not.toBeNull();
		expect(screen.queryByText('MU rate')).toBeNull();
		expect(screen.queryByText('Listing TT')).toBeNull();
		expect(document.body.textContent).not.toMatch(/NaN/);
	});
});

/** Shrapnel on auction, its outcome not yet told. */
const shrapnel: AuctionListing = {
	...boxes,
	id: 'shrapnel',
	itemName: 'Shrapnel',
	quantity: 100_000,
	attributedQty: 100_000,
	unattributedQty: 0,
	ttValue: 10,
	startingBid: 10.5,
	buyout: 12,
	status: 'pending',
	finalPrice: null,
	saleFee: null,
	resolvedAt: null,
	activityNetMarkup: null,
	grossMarkup: null,
};

async function withProposals(list: unknown[]) {
	account.getProposals.mockResolvedValue(list);
	const proposals = createListingProposals();
	await proposals.refresh();
	return proposals;
}

describe('a listing the account shows the outcome of', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		account.linkProposals.mockResolvedValue(undefined);
		account.dismissProposal.mockResolvedValue(undefined);
	});

	it('opens the sale with the price the PED Card showed, and marks it once recorded', async () => {
		const proposals = await withProposals([sold('shrapnel', 12.5)]);
		const onresolve = vi.fn().mockResolvedValue(undefined);
		render(AuctionListings, { props: { open: [shrapnel], resolved: [], onresolve, proposals } });

		expect(screen.getByText(/Your account shows this left the auction between/)).not.toBeNull();
		expect(
			screen.getByText(/PED Card rose by 12.50 PED between those two readings/),
		).not.toBeNull();
		expect(screen.getByText('Your account shows what became of it:')).not.toBeNull();
		const price = (await screen.findByLabelText('Sold for (PED)')) as HTMLInputElement;
		expect(price.value).toBe('12.5');

		const confirm = screen.getByRole('button', { name: 'Confirm sale' });
		await fireEvent.click(confirm);
		await fireEvent.click(confirm);
		await waitFor(() => expect(account.linkProposals).toHaveBeenCalledTimes(1));
		expect(onresolve).toHaveBeenCalledTimes(1);
		expect(onresolve.mock.calls[0][1]).toMatchObject({ sold: true, finalPrice: 12.5, saleFee: 0 });
		expect(account.linkProposals).toHaveBeenCalledWith({ ids: [21], recordId: 'shrapnel' });
	});

	it('asks for the price when the Card could not show it', async () => {
		const proposals = await withProposals([sold('shrapnel', null)]);
		render(AuctionListings, {
			props: { open: [shrapnel], resolved: [], onresolve: vi.fn(), proposals },
		});
		const price = (await screen.findByLabelText('Sold for (PED)')) as HTMLInputElement;
		expect(price.value).toBe('');
		await waitFor(() => expect(document.activeElement).toBe(price));
		expect(
			(screen.getByRole('button', { name: 'Confirm sale' }) as HTMLButtonElement).disabled,
		).toBe(true);
	});

	it('makes "It expired" the action for one that came back, with the day it was seen', async () => {
		const proposals = await withProposals([returned('shrapnel')]);
		const onresolve = vi.fn().mockResolvedValue(undefined);
		render(AuctionListings, { props: { open: [shrapnel], resolved: [], onresolve, proposals } });
		expect(screen.getByText(/came back from the auction between/)).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'It expired' }));
		await waitFor(() => expect(account.linkProposals).toHaveBeenCalledTimes(1));
		expect(onresolve.mock.calls[0][1].sold).toBe(false);
		expect(onresolve.mock.calls[0][1].resolvedAt).toMatch(/^2026-11-1[45]$/);
	});

	it('leaves a proposal unmarked when the other outcome is recorded, and dismisses on "Not this listing"', async () => {
		const proposals = await withProposals([returned('shrapnel')]);
		const onresolve = vi.fn().mockResolvedValue(undefined);
		const view = render(AuctionListings, {
			props: { open: [shrapnel], resolved: [], onresolve, proposals },
		});
		const reads = account.getProposals.mock.calls.length;
		await fireEvent.click(screen.getByRole('button', { name: 'It sold' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Confirm sale' }));
		await waitFor(() => expect(account.getProposals.mock.calls.length).toBe(reads + 1));
		expect(onresolve).toHaveBeenCalledTimes(1);
		expect(account.linkProposals).not.toHaveBeenCalled();
		view.unmount();

		render(AuctionListings, { props: { open: [shrapnel], resolved: [], onresolve, proposals } });
		await fireEvent.click(screen.getByRole('button', { name: 'Not this listing' }));
		await waitFor(() => expect(account.dismissProposal).toHaveBeenCalledWith({ id: 22 }));
	});

	it('shows nothing of the account for a listing it says nothing about', async () => {
		const proposals = await withProposals([sold('another', 3)]);
		render(AuctionListings, {
			props: { open: [shrapnel], resolved: [], onresolve: vi.fn(), proposals },
		});
		expect(screen.queryByText(/Your account shows/)).toBeNull();
		expect(screen.queryByLabelText('Sold for (PED)')).toBeNull();
	});
});

/** Shrapnel the account saw listed, its starting bid inferred from the fee. */
function made(): AuctionListing {
	return {
		...shrapnel,
		id: 'made',
		startingBid: 5.31,
		buyout: null,
		listingFee: 0.51,
		listedFromAccount: {
			readingId: 45,
			from: '2026-11-15T10:04:00Z',
			to: '2026-11-15T10:05:00Z',
			recordedAt: '2026-11-15T10:05:05Z',
			cardChange: -0.51,
			basis: 'inferred',
			inferredBid: { low: 5.21, high: 5.41 },
		},
	};
}

describe('a listing the account created', () => {
	it('shows its starting bid as the range its fee reads back as, never one figure', async () => {
		account.getProposals.mockResolvedValue([]);
		render(AuctionListings, { props: { open: [made()], resolved: [], onresolve: vi.fn() } });
		expect(screen.getByText('Starting bid (inferred)')).toBeTruthy();
		expect(screen.getByText('5.21 to 5.41')).toBeTruthy();
		expect(screen.queryByText('5.31')).toBeNull();
		expect(screen.getByTestId('listed-from-account').textContent).toContain(
			'Starting bid inferred from the fee: between 5.21 and 5.41 PED.',
		);
	});

	it('opens its sale with the price empty and focused, never the inferred bid', async () => {
		account.getProposals.mockResolvedValue([]);
		render(AuctionListings, { props: { open: [made()], resolved: [], onresolve: vi.fn() } });
		await fireEvent.click(screen.getByRole('button', { name: 'It sold' }));
		const price = screen.getByLabelText('Sold for (PED)') as HTMLInputElement;
		expect(price.value).toBe('');
		await waitFor(() => expect(document.activeElement).toBe(price));
		expect(
			(screen.getByRole('button', { name: 'Confirm sale' }) as HTMLButtonElement).disabled,
		).toBe(true);
	});

	it('still opens a listing the player entered at its starting bid', async () => {
		account.getProposals.mockResolvedValue([]);
		const typed: AuctionListing = { ...made(), buyout: null, listedFromAccount: null };
		render(AuctionListings, { props: { open: [typed], resolved: [], onresolve: vi.fn() } });
		await fireEvent.click(screen.getByRole('button', { name: 'It sold' }));
		expect((screen.getByLabelText('Sold for (PED)') as HTMLInputElement).value).toBe('5.31');
	});
});
