// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { AuctionListing } from '$lib/types/analytics';
import AuctionListings from './AuctionListings.svelte';

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
