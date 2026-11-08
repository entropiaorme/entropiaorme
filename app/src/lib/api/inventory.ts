/**
 * Central Inventory command surface.
 *
 * Legacy profession reads remain during the transition, so the central
 * worklist folds their existing records together with Inventory-originated
 * ones. New writes always carry the Inventory origin; provenance, not that
 * origin, decides which activity analytics may claim a loot outcome.
 */

import type {
	ActivityHistoryEntry,
	AuctionListing,
	EquipmentListingInput,
	EquipmentTradeInput,
	InventoryHoldingCandidate,
	InventorySaleDraft,
	Profession,
} from './commands.gen';
import * as commands from './commands.gen';

const ORIGINS: Profession[] = ['inventory', 'hunting', 'harvesting'];

function uniqueById<T extends { id: string }>(groups: T[][]): T[] {
	const rows = new Map<string, T>();
	for (const group of groups) for (const row of group) rows.set(row.id, row);
	return [...rows.values()];
}

export const getLootInventory = () => commands.activityStock('inventory');
export const getEquipmentInventory = commands.inventoryList;

export async function getInventoryListings(): Promise<AuctionListing[]> {
	const groups = await Promise.all(ORIGINS.map((origin) => commands.auctionListings(origin)));
	return uniqueById(groups)
		.filter((row) => row.channel === 'auction')
		.sort((a, b) => {
			if (a.status === 'pending' && b.status !== 'pending') return -1;
			if (a.status !== 'pending' && b.status === 'pending') return 1;
			return b.listedAt.localeCompare(a.listedAt) || a.id.localeCompare(b.id);
		});
}

export async function getInventoryHistory(): Promise<ActivityHistoryEntry[]> {
	const groups = await Promise.all(ORIGINS.map((origin) => commands.activityHistory(origin)));
	return uniqueById(groups).sort(
		(a, b) => b.occurredAt.localeCompare(a.occurredAt) || a.id.localeCompare(b.id),
	);
}

export const createLootListing = (input: Omit<commands.AuctionListingInput, 'profession'>) =>
	commands.auctionListingCreate({ profession: 'inventory', ...input });
export const tradeLoot = (input: Omit<commands.PrivateSaleInput, 'profession'>) =>
	commands.stockPrivateSale({ profession: 'inventory', ...input });
export const convertLoot = (input: Omit<commands.StockConversionInput, 'profession'>) =>
	commands.stockConvert({ profession: 'inventory', ...input });
export const convertItems = (input: commands.ItemConversionInput) =>
	commands.stockConvertItems(input);
export const removeLoot = (input: Omit<commands.StockRemovalInput, 'profession'>) =>
	commands.stockRemove({ profession: 'inventory', ...input });
export const convertLootShrapnel = (input: Omit<commands.ShrapnelConversionInput, 'profession'>) =>
	commands.stockShrapnelConvert({ profession: 'inventory', ...input });

export const createEquipmentListing = (input: EquipmentListingInput) =>
	commands.inventoryEquipmentListingCreate(input);
export const tradeEquipment = (input: EquipmentTradeInput) =>
	commands.inventoryEquipmentTrade(input);

export const confirmInventoryListing = commands.auctionListingConfirm;
export const expireInventoryListing = commands.auctionListingExpire;
export const revertInventorySale = commands.auctionSaleRevert;
export const undoInventoryListing = commands.auctionListingUndo;
export const undoInventoryConversion = commands.stockConversionUndo;
export const undoInventoryTrade = commands.privateSaleUndo;
export const undoInventoryRemoval = commands.stockRemovalUndo;

export const addEquipmentHolding = commands.inventoryCreate;
export const updateEquipmentHolding = commands.inventoryUpdate;
export const deleteEquipmentHolding = commands.inventoryDelete;
export const resolveInventoryDraft = (draft: InventorySaleDraft) =>
	commands.inventoryDraftResolve(draft);

/** Read the game's sale window once. Fills the form; commits nothing. */
export const captureSaleWindow = () => commands.inventorySaleWindowCapture();

/** Collect a capture taken while the form was not on screen, clearing it.
 * The overlay's button can be pressed from inside the game, so its read
 * waits here until the form opens. */
export const takeSaleWindowCapture = () => commands.inventorySaleWindowTakeCapture();

/** A reviewed transaction proposal. Manual selection and a screen-read
 * capture both cross this boundary before any accounting command is allowed
 * to run. */
export interface ResolvedInventorySaleDraft {
	draft: InventorySaleDraft;
	holding: InventoryHoldingCandidate;
	occurredAt: string | null;
}

export async function commitInventorySaleDraft({
	draft,
	holding,
	occurredAt,
}: ResolvedInventorySaleDraft): Promise<unknown> {
	if (holding.kind === 'equipment') {
		return draft.channel === 'auction'
			? createEquipmentListing({
					itemId: holding.holdingId,
					startingBid: draft.startingBid ?? 0,
					buyout: draft.buyout,
					listingFee: draft.listingFee ?? 0,
					listedAt: occurredAt,
					auctionDays: draft.auctionDays,
				})
			: tradeEquipment({
					itemId: holding.holdingId,
					soldFor: draft.finalPrice ?? 0,
					soldAt: occurredAt,
				});
	}

	if (draft.quantity === null || draft.quantity <= 0) {
		throw new Error('A loot sale needs a positive quantity');
	}
	return draft.channel === 'auction'
		? createLootListing({
				itemName: holding.name,
				quantity: draft.quantity,
				startingBid: draft.startingBid ?? 0,
				buyout: draft.buyout,
				listingFee: draft.listingFee ?? 0,
				listedAt: occurredAt,
				auctionDays: draft.auctionDays,
			})
		: tradeLoot({
				itemName: holding.name,
				quantity: draft.quantity,
				soldFor: draft.finalPrice ?? 0,
				soldAt: occurredAt,
			});
}
