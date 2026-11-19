/**
 * History rows, openings and listings as the backend sends them, for the
 * account activity space's tests. Each builder takes overrides.
 */
import type {
	AccountProposal,
	ActivityHistoryEntry,
	AuctionListing,
	RecordedFromAccount,
	StrongboxOpeningEntry,
} from '$lib/api/commands.gen';
import { seven } from '$lib/features/strongbox/__fixtures__/proposals';

/** The step a record from the account was seen in. */
export function step(
	readingId: number,
	to: string,
	overrides: Partial<RecordedFromAccount> = {},
): RecordedFromAccount {
	const end = Date.parse(to);
	return {
		readingId,
		from: new Date(end - 60_000).toISOString(),
		to,
		recordedAt: new Date(end + 5_000).toISOString(),
		cardChange: null,
		basis: 'exact',
		inferredBid: null,
		...overrides,
	};
}

/** A history row the player entered by hand: one Animal Hide sold. */
export function entry(overrides: Partial<ActivityHistoryEntry> = {}): ActivityHistoryEntry {
	return {
		id: 'row-1',
		subjectKind: 'loot',
		channel: 'trade',
		kind: 'trade',
		status: 'sold',
		itemName: 'Animal Hide',
		targetItem: null,
		occurredAt: '2026-11-15',
		quantity: 1,
		ttValue: 0.5,
		netMarkup: null,
		activityNetMarkup: null,
		canRevertSale: false,
		canDelete: true,
		undoBlockedReason: null,
		undone: false,
		conversionLines: [],
		pedAmount: null,
		itemised: false,
		derivedFrom: [],
		recordedFromAccount: null,
		listedFromAccount: null,
		reconciliationId: null,
		...overrides,
	};
}

/** 100 000 Shrapnel the readings saw turned into 101 000 ammo. */
export function shrapnel(overrides: Partial<ActivityHistoryEntry> = {}): ActivityHistoryEntry {
	return entry({
		id: 'conversion-shrapnel',
		channel: 'conversion',
		kind: 'conversion',
		status: 'converted',
		itemName: 'Shrapnel',
		targetItem: 'Universal Ammo',
		quantity: 100_000,
		ttValue: 10,
		pedAmount: 10.1,
		recordedFromAccount: step(41, '2026-11-15T10:01:00Z'),
		...overrides,
	});
}

/** Two items the readings saw recycled into 150 Nanocubes. */
export function recycling(overrides: Partial<ActivityHistoryEntry> = {}): ActivityHistoryEntry {
	return entry({
		id: 'conversion-recycle',
		channel: 'conversion',
		kind: 'conversion',
		status: 'converted',
		itemName: 'Lysterium Ingot',
		targetItem: 'Nanocube',
		quantity: 100,
		ttValue: 1.5,
		conversionLines: [
			{
				side: 'input',
				itemName: 'Lysterium Ingot',
				quantity: 100,
				ttValue: 1,
				acquisition: 'held',
				markup: null,
				holding: 'loot',
			},
			{
				side: 'input',
				itemName: 'Animal Hide',
				quantity: 1,
				ttValue: 0.5,
				acquisition: 'held',
				markup: null,
				holding: 'loot',
			},
			{
				side: 'output',
				itemName: 'Nanocube',
				quantity: 150,
				ttValue: 1.5,
				acquisition: null,
				markup: null,
				holding: 'loot',
			},
		],
		recordedFromAccount: step(42, '2026-11-15T10:02:00Z'),
		...overrides,
	});
}

/** One item the readings saw sold to the Trade Terminal in `readingId`'s step. */
export function sale(
	id: string,
	itemName: string,
	ttValue: number,
	readingId = 43,
	overrides: Partial<ActivityHistoryEntry> = {},
): ActivityHistoryEntry {
	return entry({
		id,
		itemName,
		ttValue,
		recordedFromAccount: step(readingId, '2026-11-15T10:03:00Z'),
		...overrides,
	});
}

/** A listing of ten Animal Hide the readings saw sell, and its row. */
export function listing(overrides: Partial<AuctionListing> = {}): AuctionListing {
	return {
		id: 'listing-1',
		itemName: 'Animal Hide',
		quantity: 10,
		attributedQty: 10,
		unattributedQty: 0,
		ttValue: 5,
		attributedTt: 5,
		startingBid: 7,
		buyout: 8,
		listingFee: 0.5,
		listedAt: '2026-11-14',
		status: 'sold',
		finalPrice: 8,
		saleFee: 0.25,
		resolvedAt: '2026-11-15',
		subjectKind: 'loot',
		inventoryItemId: null,
		costBasis: null,
		channel: 'auction',
		auctionDays: 3,
		expiresAt: null,
		activityNetMarkup: 2.25,
		grossMarkup: 3,
		recordedFromAccount: step(44, '2026-11-15T10:04:00Z'),
		listedFromAccount: null,
		...overrides,
	};
}

export function listingRow(overrides: Partial<ActivityHistoryEntry> = {}): ActivityHistoryEntry {
	return entry({
		id: 'listing-1',
		channel: 'auction',
		kind: 'listing',
		status: 'sold',
		quantity: 10,
		ttValue: 5,
		netMarkup: 2.25,
		canRevertSale: true,
		recordedFromAccount: step(44, '2026-11-15T10:04:00Z'),
		...overrides,
	});
}

/** Three Halloween Strongboxes opened: ammo back, one item kept and one
 * recycled. */
export function opening(overrides: Partial<StrongboxOpeningEntry> = {}): StrongboxOpeningEntry {
	return {
		id: 'opening-1',
		boxItem: 'Halloween Strongbox',
		boxCount: 3,
		boxMarkupPaid: 0,
		keyTt: 3,
		keyMarkupPaid: 0.6,
		ammoPed: 2.4,
		contents: [
			{ itemName: 'Pumpkin Hat (C)', quantity: 1, ttValue: 0.2, nanocubes: null },
			{ itemName: 'Old Bones', quantity: 4, ttValue: 0.4, nanocubes: 40 },
		],
		intake: 'export',
		openedAt: '2026-11-15T09:00:00Z',
		undone: false,
		...overrides,
	};
}

/** A strongbox opening the readings found, waiting to be reviewed, found
 * at `detectedAt`. */
export function waitingOpening(detectedAt = '2026-11-15T11:01:05Z'): AccountProposal {
	return { ...seven(), detectedAt };
}

/** The step ten Animal Hide were seen listed in: the Card fell by the
 * 0.51 fee, which reads back as any starting bid from 5.21 to 5.41. */
export function listedStep(overrides: Partial<RecordedFromAccount> = {}): RecordedFromAccount {
	return step(45, '2026-11-15T10:05:00Z', {
		cardChange: -0.51,
		basis: 'inferred',
		inferredBid: { low: 5.21, high: 5.41 },
		...overrides,
	});
}

/** A listing the readings created, still on the auction. */
export function listedListing(overrides: Partial<AuctionListing> = {}): AuctionListing {
	return listing({
		id: 'listing-made',
		startingBid: 5.31,
		buyout: null,
		listingFee: 0.51,
		listedAt: '2026-11-15',
		status: 'pending',
		finalPrice: null,
		saleFee: null,
		resolvedAt: null,
		auctionDays: null,
		activityNetMarkup: null,
		grossMarkup: null,
		recordedFromAccount: null,
		listedFromAccount: listedStep(),
		...overrides,
	});
}

/** Its history row. */
export function listedRow(overrides: Partial<ActivityHistoryEntry> = {}): ActivityHistoryEntry {
	return listingRow({
		id: 'listing-made',
		status: 'pending',
		netMarkup: null,
		canRevertSale: false,
		occurredAt: '2026-11-15',
		recordedFromAccount: null,
		listedFromAccount: listedStep(),
		...overrides,
	});
}
