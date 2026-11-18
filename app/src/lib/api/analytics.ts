/**
 * The analytics family: the Overview, Hunting, and Tree Cutting aggregates, the
 * ledger (entries and presets), and the inventory ledger. Thin
 * wrappers over the generated typed commands.
 */

import type { LedgerItem } from './commands.gen';
import * as commands from './commands.gen';

export async function getAnalyticsOverview(period: string = 'all') {
	return commands.analyticsOverview(period);
}

/** The whole-ledger per-tag summary for a period, independent of the
 * paginated entry list: the Net Ledger Impact card's source of truth. */
export const getLedgerSummary = commands.ledgerSummary;

export const getAnalyticsHunting = commands.analyticsHunting;
export async function getAnalyticsHarvest(period: string = 'all') {
	return commands.analyticsHarvest(period);
}
export async function getAnalyticsHuntingActivity(period: string = 'all') {
	return commands.analyticsHuntingActivity(period);
}
export const getLedgerPresets = commands.ledgerPresetsList;
export const getInventoryItems = commands.inventoryList;

export const addLedgerEntry = commands.ledgerCreate;
export const deleteLedgerEntry = commands.ledgerDelete;
export const addLedgerPreset = commands.ledgerPresetCreate;
export const deleteLedgerPreset = commands.ledgerPresetDelete;
// Current holdings and the auction lifecycle over them: operational
// position context for sale and recycling actions. It does not influence
// holding-independent market opportunity. Each read is scoped to the
// activity family whose tab is asking.
export const getActivityStock = commands.activityStock;
export const getHarvestRealisedMarkup = commands.harvestRealisedMarkup;
export const getHuntingRealisedMarkup = commands.huntingRealisedMarkup;
export const getAuctionListings = commands.auctionListings;
export const createAuctionListing = commands.auctionListingCreate;
export const confirmAuctionListing = commands.auctionListingConfirm;
export const expireAuctionListing = commands.auctionListingExpire;
export const convertStock = commands.stockConvert;
export const sellStockPrivately = commands.stockPrivateSale;
export const removeStock = commands.stockRemove;
export const convertShrapnel = commands.stockShrapnelConvert;
// What the activity has done to its stock, and the ways back out of it.
export const getActivityHistory = commands.activityHistory;
export const revertAuctionSale = commands.auctionSaleRevert;
export const undoAuctionListing = commands.auctionListingUndo;
export const undoStockConversion = commands.stockConversionUndo;
export const undoPrivateSale = commands.privateSaleUndo;
export const undoStockRemoval = commands.stockRemovalUndo;

export const addInventoryItem = commands.inventoryCreate;
export const updateInventoryItem = commands.inventoryUpdate;
export const deleteInventoryItem = commands.inventoryDelete;
export const sellInventoryItem = commands.inventorySell;

/** One keyset page of ledger entries plus the cursor for the next page
 * (null on the last page) and the whole-ledger row count. Frontend-owned
 * reshape of the generated `LedgerPage` (`entries` reads as `items` at
 * the consumer). */
export interface LedgerPage {
	items: LedgerItem[];
	nextCursor: string | null;
	total: number;
}

export async function getLedgerEntries(cursor?: string, limit?: number): Promise<LedgerPage> {
	const page = await commands.ledgerList(cursor ?? null, limit ?? null);
	return {
		items: page.entries,
		nextCursor: page.nextCursor ?? null,
		total: page.total,
	};
}
