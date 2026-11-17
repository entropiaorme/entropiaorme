import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { ACCOUNT_TOPIC, undoArrivalUnknownOrigin, undoStockReconciliation } from '$lib/api/account';
import type {
	ActivityHistoryEntry,
	AssetPurchaseInput,
	AuctionListing,
	EquipmentListingInput,
	EquipmentTradeInput,
	InventoryHoldingCandidate,
	InventoryItem,
	InventorySaleDraft,
	ItemConversionInput,
	ListingFieldSourceInput,
	MarketHarvestData,
	StockPosition,
	StrongboxOpeningEntry,
} from '$lib/api/commands.gen';
import {
	addAssetPurchase,
	captureSaleWindow,
	commitInventorySaleDraft,
	confirmInventoryListing,
	convertItems,
	convertLoot,
	convertLootShrapnel,
	deleteAssetPurchase,
	deleteEquipmentHolding,
	expireInventoryListing,
	getEquipmentInventory,
	getInventoryHistory,
	getInventoryListings,
	getLootInventory,
	getStrongboxOpenings,
	removeLoot,
	reopenInventoryListing,
	resolveInventoryDraft,
	revertInventorySale,
	takeSaleWindowCapture,
	undoInventoryConversion,
	undoInventoryListing,
	undoInventoryRemoval,
	undoInventoryTrade,
	undoStrongboxOpening,
	updateAssetPurchase,
} from '$lib/api/inventory';
import {
	getMarketAuctionPacketThreshold,
	getMarketItemMarkups,
	MARKET_TOPIC,
} from '$lib/api/market';
import type {
	ActivityListingDraft,
	ActivityTradeDraft,
	TreeCuttingStock,
} from '$lib/features/analytics/treeCuttingModel.svelte';
import {
	type ConfidenceMode,
	effectiveItemMarkup,
	manualValuation,
	marketOpportunity,
	NANOCUBE_FALLBACK_MARKUP,
	opportunityTier,
} from '$lib/features/analytics/treeCuttingModel.svelte';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { describeError } from '$lib/view/errorState';
import { entryShowsUnder } from './accountActivity';
import type { ConvertibleItem } from './conversionDraft';
import type { ListingDraftFields } from './listingIntake';

export type InventoryKind = 'loot' | 'equipment';
export type InventoryView = 'holdings' | 'listings' | 'history';

/** The market signals for exactly the items held: every holding gets its
 * reading (and the player's own markup) whether or not tracked play ever
 * looted it. A failed read leaves the holdings unvalued rather than
 * failing the page. */
async function holdingsMarket(rows: StockPosition[]): Promise<MarketHarvestData | null> {
	return getMarketItemMarkups(rows.map((row) => row.itemName)).catch(() => null);
}

function stockRow(
	row: StockPosition,
	market: MarketHarvestData | null,
	confidenceMode: ConfidenceMode,
	packetGrossMarkupPed: number | null,
): TreeCuttingStock {
	const marketItem = market?.items.find((item) => item.itemName === row.itemName);
	const nanocubeMarkup = market?.nanocubeMarkupPct ?? NANOCUBE_FALLBACK_MARKUP;
	const opportunity = market ? marketOpportunity(marketItem, nanocubeMarkup) : null;
	const manual = manualValuation(marketItem, row.quantity, row.ttValue);
	const applied = effectiveItemMarkup(row.itemName, opportunity, nanocubeMarkup, confidenceMode, {
		manual,
	});
	// A packet is sized against the markup the stock would sell at: the
	// player's own value when it stands, else the market reading.
	const sellingMarkup = applied?.basis === 'manual' ? applied.markupPct : marketItem?.markupPct;
	const premium = sellingMarkup ? sellingMarkup / 100 - 1 : 0;
	const recommendedPacketTt =
		packetGrossMarkupPed !== null && premium > 0
			? Math.ceil((packetGrossMarkupPed / premium) * 100 - 1e-9) / 100
			: null;
	return {
		unitTt: row.unitTt,
		paidMarkup: row.paidMarkup,
		origins: row.origins,
		derivedFrom: row.derivedFrom,
		itemName: row.itemName,
		heldQty: row.quantity,
		heldTt: row.ttValue,
		listedQty: row.listedQuantity,
		readings: (marketItem?.readings ?? []).map((reading) => ({
			horizon: reading.horizon,
			markupPct: reading.markupPct,
			salesPed: reading.salesPed,
		})),
		opportunity,
		markupPct: marketItem?.markupPct ?? null,
		markupHorizon: marketItem?.horizon ?? null,
		tier: opportunity ? opportunityTier(opportunity) : null,
		effectiveMarkupPct: applied?.markupPct ?? null,
		markupBasis: applied?.basis ?? null,
		floored: applied?.basis === 'nanocube',
		salesPed: marketItem?.salesPed ?? null,
		weeklySalesPed:
			marketItem?.readings.find((reading) => reading.horizon === 'week')?.salesPed ?? null,
		recommendedPacketTt,
		marketItem: marketItem ?? null,
	};
}

function manualDraft(
	channel: 'auction' | 'trade',
	name: string,
	quantity: number,
	values: {
		startingBid?: number;
		buyout?: number | null;
		listingFee?: number;
		finalPrice?: number;
	},
): InventorySaleDraft {
	return {
		draftId: crypto.randomUUID(),
		source: 'manual',
		channel,
		observedName: name,
		quantity,
		startingBid: values.startingBid ?? null,
		buyout: values.buyout ?? null,
		listingFee: values.listingFee ?? null,
		finalPrice: values.finalPrice ?? null,
		auctionDays: null,
		confidence: null,
	};
}

export function createInventoryModel() {
	let kind = $state<InventoryKind>('loot');
	let view = $state<InventoryView>('holdings');
	let confidenceMode = $state<ConfidenceMode>('liquidMiddling');
	let packetFeeSharePct = $state(10);
	let packetGrossMarkupPed = $state<number | null>(null);
	let loading = $state(true);
	let historyLoading = $state(false);
	let historyLoaded = $state(false);
	let error = $state<string | null>(null);
	let positions = $state<StockPosition[]>([]);
	let market = $state<MarketHarvestData | null>(null);
	let equipment = $state<InventoryItem[]>([]);
	let listings = $state<AuctionListing[]>([]);
	let history = $state<ActivityHistoryEntry[]>([]);
	/** The strongbox openings recorded, read with the history: each is an
	 * event the player applied after a review. */
	let openings = $state<StrongboxOpeningEntry[]>([]);
	let packetThresholdRequest = 0;
	/** Each moves on with every read of its part (the stock, listings and
	 * assets; the history), so a read that answers after a later one
	 * started never overwrites it. */
	let stockRequest = 0;
	let historyRequest = 0;

	async function load() {
		loading = true;
		error = null;
		const request = ++stockRequest;
		try {
			const [positionRows, equipmentRows, listingRows, packetThreshold] = await Promise.all([
				getLootInventory(),
				getEquipmentInventory(),
				getInventoryListings(),
				getMarketAuctionPacketThreshold(packetFeeSharePct),
			]);
			packetFeeSharePct = packetThreshold.maxFeeSharePct;
			packetGrossMarkupPed = packetThreshold.grossMarkupPed;
			if (request !== stockRequest) return;
			const nextMarket = await holdingsMarket(positionRows);
			if (request !== stockRequest) return;
			positions = positionRows;
			market = nextMarket;
			equipment = equipmentRows;
			listings = listingRows;
		} catch (cause) {
			error = describeError(cause, 'Failed to load inventory');
		} finally {
			loading = false;
		}
	}

	async function setPacketFeeSharePct(value: number) {
		const request = ++packetThresholdRequest;
		error = null;
		try {
			const threshold = await getMarketAuctionPacketThreshold(value);
			if (request !== packetThresholdRequest) return;
			packetFeeSharePct = threshold.maxFeeSharePct;
			packetGrossMarkupPed = threshold.grossMarkupPed;
			error = null;
		} catch (cause) {
			if (request !== packetThresholdRequest) return;
			error = describeError(cause, 'Failed to model the packet fee cap');
		}
	}

	async function loadHistory() {
		historyLoading = true;
		error = null;
		const request = ++historyRequest;
		try {
			const [rows, opened] = await Promise.all([getInventoryHistory(), getStrongboxOpenings()]);
			if (request !== historyRequest) return;
			history = rows;
			openings = opened;
			historyLoaded = true;
		} catch (cause) {
			error = describeError(cause, 'Failed to load inventory history');
			throw cause;
		} finally {
			historyLoading = false;
		}
	}

	async function refresh(includeHistory = historyLoaded) {
		const request = ++stockRequest;
		const asked = includeHistory ? ++historyRequest : null;
		const [positionRows, equipmentRows, listingRows, historyRows, opened] = await Promise.all([
			getLootInventory(),
			getEquipmentInventory(),
			getInventoryListings(),
			includeHistory ? getInventoryHistory() : Promise.resolve(null),
			includeHistory ? getStrongboxOpenings() : Promise.resolve(null),
		]);
		if (historyRows && asked === historyRequest) history = historyRows;
		if (opened && asked === historyRequest) openings = opened;
		if (request !== stockRequest) return;
		// A sale or conversion can bring an item into holdings or take
		// the last of one out, so the market is read for what is held now.
		const nextMarket = await holdingsMarket(positionRows);
		if (request !== stockRequest) return;
		positions = positionRows;
		market = nextMarket;
		equipment = equipmentRows;
		listings = listingRows;
	}

	// The account readings record conversions and Trade Terminal sales on
	// their own, and announce each on the account topic. Every frame re-reads
	// the stock, listings and any history already shown, coalesced as every
	// snapshot read is; a read that fails keeps what is shown, and the next
	// frame reads again.
	const accountFrames = createSnapshotStore<true>(ACCOUNT_TOPIC, async () => {
		await refresh();
		return true;
	});

	/** Re-read the holdings' market signals: a paste, a manual markup, or
	 * a mark committed here or in another window. */
	async function refreshMarket() {
		market = await holdingsMarket(positions);
	}

	/** Follow market writes from any window, and what the account readings
	 * record; returns the detach function. */
	async function subscribe(): Promise<UnlistenFn> {
		const attached = await Promise.allSettled([
			listen(MARKET_TOPIC, () => void refreshMarket()),
			accountFrames.subscribe(),
		]);
		const stops = attached.flatMap((result) =>
			result.status === 'fulfilled' ? [result.value] : [],
		);
		const failed = attached.find((result) => result.status === 'rejected');
		if (failed) {
			// All or nothing: a listener that did attach is not left behind.
			for (const stop of stops) stop();
			throw failed.reason;
		}
		return () => {
			for (const stop of stops) stop();
		};
	}

	async function withRefresh(work: () => Promise<unknown>, message: string) {
		error = null;
		try {
			await work();
			await refresh();
		} catch (cause) {
			error = describeError(cause, message);
			throw cause;
		}
	}

	async function listLoot(input: ActivityListingDraft) {
		const draft = manualDraft('auction', input.itemName, input.quantity, input);
		await withRefresh(
			() =>
				commitInventorySaleDraft({
					draft,
					holding: { kind: 'loot', holdingId: input.itemName, name: input.itemName, score: 100 },
					occurredAt: input.listedAt ?? null,
				}),
			'Failed to create the listing',
		);
	}

	async function sellLootByTrade(input: ActivityTradeDraft) {
		const draft = manualDraft('trade', input.itemName, input.quantity, {
			finalPrice: input.soldFor,
		});
		await withRefresh(
			() =>
				commitInventorySaleDraft({
					draft,
					holding: { kind: 'loot', holdingId: input.itemName, name: input.itemName, score: 100 },
					occurredAt: input.soldAt,
				}),
			'Failed to record the trade',
		);
	}

	async function listEquipment(input: EquipmentListingInput) {
		const item = equipment.find((row) => row.id === input.itemId);
		if (!item) throw new Error('That asset is no longer available');
		const draft = manualDraft('auction', item.name, input.quantity ?? item.quantity, input);
		await withRefresh(
			() =>
				commitInventorySaleDraft({
					draft,
					holding: { kind: 'equipment', holdingId: item.id, name: item.name, score: 100 },
					occurredAt: input.listedAt ?? null,
				}),
			'Failed to create the listing',
		);
	}

	async function sellEquipmentByTrade(input: EquipmentTradeInput) {
		const item = equipment.find((row) => row.id === input.itemId);
		if (!item) throw new Error('That asset is no longer available');
		const draft = manualDraft('trade', item.name, input.quantity ?? item.quantity, {
			finalPrice: input.soldFor,
		});
		await withRefresh(
			() =>
				commitInventorySaleDraft({
					draft,
					holding: { kind: 'equipment', holdingId: item.id, name: item.name, score: 100 },
					occurredAt: input.soldAt ?? null,
				}),
			'Failed to record the trade',
		);
	}

	/** Candidate holdings for a name read or typed off the game's sell window.
	 * Conservative by construction: a winner comes back only for a match with
	 * no plausible rival. */
	async function resolveDraftName(name: string, channel: 'auction' | 'trade') {
		const resolution = await resolveInventoryDraft({
			...manualDraft(channel, name, 0, {}),
			quantity: null,
		});
		return { candidates: resolution.candidates, resolved: resolution.resolved };
	}

	/** Commit an intake draft against the holding the user reviewed. A typed
	 * draft and a captured one cross the same boundary here; a captured one
	 * also says where each of its figures came from, which an auction listing
	 * records beside itself. */
	async function createFromDraft({
		fields,
		channel,
		holding,
		occurredAt,
		fieldSources = null,
	}: {
		fields: ListingDraftFields;
		channel: 'auction' | 'trade';
		holding: InventoryHoldingCandidate;
		occurredAt: string | null;
		fieldSources?: ListingFieldSourceInput[] | null;
	}) {
		const draft: InventorySaleDraft = {
			draftId: crypto.randomUUID(),
			source: 'manual',
			channel,
			observedName: holding.name,
			quantity: fields.quantity,
			startingBid: fields.startingBid,
			buyout: fields.buyout,
			listingFee: fields.auctionFee,
			finalPrice: channel === 'trade' ? fields.buyout : null,
			auctionDays: channel === 'auction' ? fields.auctionDays : null,
			confidence: null,
		};
		await withRefresh(
			() =>
				commitInventorySaleDraft({
					draft,
					holding,
					occurredAt,
					fieldSources: channel === 'auction' ? fieldSources : null,
				}),
			channel === 'auction' ? 'Failed to create the listing' : 'Failed to record the trade',
		);
	}

	async function resolveListing(
		listingId: string,
		outcome:
			| { sold: true; finalPrice: number; saleFee: number; resolvedAt?: string }
			| { sold: false; resolvedAt?: string },
	) {
		await withRefresh(
			() =>
				outcome.sold
					? confirmInventoryListing({
							listingId,
							finalPrice: outcome.finalPrice,
							saleFee: outcome.saleFee,
							resolvedAt: outcome.resolvedAt ?? null,
						})
					: expireInventoryListing({ listingId, resolvedAt: outcome.resolvedAt ?? null }),
			'Failed to resolve the listing',
		);
	}

	async function undo(entry: ActivityHistoryEntry, revertSale = false) {
		await withRefresh(async () => {
			const listing = listings.find((row) => row.id === entry.id);
			if (listing || entry.subjectKind === 'equipment') {
				if (revertSale) await revertInventorySale({ id: entry.id });
				else await undoInventoryListing({ id: entry.id });
			} else if (entry.kind === 'conversion') {
				await undoInventoryConversion({ id: entry.id });
			} else if (entry.kind === 'trade') {
				await undoInventoryTrade({ id: entry.id });
			} else if (entry.kind === 'removal') {
				await undoInventoryRemoval({ id: entry.id });
			} else if (entry.kind === 'unknownOrigin') {
				// Stock held as of unknown origin: taking it back leaves the
				// arrival it came from open to decide again.
				await undoArrivalUnknownOrigin({ id: entry.id });
			}
		}, 'Failed to undo that entry');
	}

	/** Undo a stock check as a whole: every record it made is taken back. */
	async function undoCheck(reconciliationId: string) {
		await withRefresh(
			() => undoStockReconciliation({ id: reconciliationId }),
			'Failed to undo the stock check',
		);
	}

	/** Take a strongbox opening back whole: the boxes and keys return and
	 * everything they became is unmade. */
	async function undoOpening(openingId: string) {
		await withRefresh(
			() => undoStrongboxOpening({ id: openingId }),
			'Failed to undo the opening',
		);
	}

	/** Put a listing that came back from the auction on it again. */
	async function reopenListing(listingId: string) {
		await withRefresh(
			() => reopenInventoryListing({ id: listingId }),
			'Failed to put the listing back on auction',
		);
	}

	async function recycle(itemName: string, quantity: number) {
		await withRefresh(
			() =>
				convertLoot({
					sourceItem: itemName,
					targetItem: 'Nanocube',
					quantity,
					convertedAt: null,
				}),
			'Failed to convert the stock',
		);
	}

	/** Record a hand-in, an opened box, or anything else that turned some
	 * things into others. */
	async function convert(request: ItemConversionInput) {
		await withRefresh(() => convertItems(request), 'Failed to record the conversion');
	}

	async function remove(itemName: string, quantity: number) {
		await withRefresh(
			() => removeLoot({ itemName, quantity, removedAt: null }),
			'Failed to remove the stock',
		);
	}

	async function shrapnel(quantity: number) {
		await withRefresh(
			() => convertLootShrapnel({ quantity, convertedAt: null }),
			'Failed to convert the Shrapnel',
		);
	}

	/** Record buying more of an asset. */
	async function addPurchase(item: InventoryItem, purchase: AssetPurchaseInput) {
		await withRefresh(
			() => addAssetPurchase(item.id, purchase),
			`Failed to add the purchase of ${item.name}`,
		);
	}

	/** Correct one of an asset's purchases. */
	async function updatePurchase(purchaseId: number, purchase: AssetPurchaseInput) {
		await withRefresh(
			() => updateAssetPurchase(purchaseId, purchase),
			'Failed to change the purchase',
		);
	}

	/** Take back one of an asset's purchases. */
	async function deletePurchase(purchaseId: number) {
		await withRefresh(() => deleteAssetPurchase(purchaseId), 'Failed to remove the purchase');
	}

	async function deleteEquipment(item: InventoryItem) {
		await withRefresh(() => deleteEquipmentHolding(item.id), `Failed to remove ${item.name}`);
	}

	const visibleListings = $derived(
		listings.filter((row) =>
			kind === 'equipment' ? row.subjectKind === 'equipment' : row.subjectKind === 'loot',
		),
	);
	// A conversion can span both sides of Inventory, so it shows on either
	// side it touched.
	const visibleHistory = $derived(history.filter((row) => entryShowsUnder(row, kind)));
	const convertibleItems = $derived<ConvertibleItem[]>([
		...positions.map((row) => ({
			name: row.itemName,
			assetId: null,
			heldQty: row.quantity,
			unitTt: row.unitTt,
			paidPerUnit: row.quantity > 0 ? row.paidMarkup / row.quantity : 0,
			origins: row.origins,
		})),
		...equipment.map((row) => ({
			name: row.name,
			assetId: row.id,
			heldQty: row.quantity,
			unitTt: row.ttValue / row.quantity,
			paidPerUnit: row.markupPaid / row.quantity,
			divisible: row.divisible,
			origins: row.origins,
		})),
	]);
	const loot = $derived.by(() =>
		positions
			.map((position) => stockRow(position, market, confidenceMode, packetGrossMarkupPed))
			.sort((a, b) => b.heldTt - a.heldTt || a.itemName.localeCompare(b.itemName)),
	);
	// Everything currently held, in one list, for the intake typeahead: the
	// sell window names an item, not a profession or an asset class.
	const holdingOptions = $derived([
		...loot.map((row) => ({
			kind: 'loot',
			holdingId: row.itemName,
			name: row.itemName,
			score: 100,
			heldQty: row.heldQty,
			paidPerUnit: row.heldQty > 0 ? (row.paidMarkup ?? 0) / row.heldQty : 0,
		})),
		...equipment.map((row) => ({
			kind: 'equipment',
			holdingId: row.id,
			name: row.name,
			score: 100,
			heldQty: row.quantity,
			paidPerUnit: row.markupPaid / row.quantity,
		})),
	]);

	return {
		get kind() {
			return kind;
		},
		set kind(value: InventoryKind) {
			kind = value;
		},
		get view() {
			return view;
		},
		set view(value: InventoryView) {
			view = value;
		},
		get confidenceMode() {
			return confidenceMode;
		},
		set confidenceMode(value: ConfidenceMode) {
			confidenceMode = value;
		},
		get packetFeeSharePct() {
			return packetFeeSharePct;
		},
		get loading() {
			return loading;
		},
		get historyLoading() {
			return historyLoading;
		},
		get historyLoaded() {
			return historyLoaded;
		},
		get error() {
			return error;
		},
		get loot() {
			return loot;
		},
		get equipment() {
			return equipment;
		},
		get listings() {
			return visibleListings;
		},
		get openListings() {
			return visibleListings.filter((row) => row.status === 'pending');
		},
		get resolvedListings() {
			return visibleListings.filter((row) => row.status !== 'pending');
		},
		get history() {
			return visibleHistory;
		},
		get holdingOptions() {
			return holdingOptions;
		},
		get convertibleItems() {
			return convertibleItems;
		},
		get allHistory() {
			return history;
		},
		/** Every listing, on either side: the history's events read their
		 * price and fees from it. */
		get allListings() {
			return listings;
		},
		get openings() {
			return openings;
		},
		get lootTt() {
			return loot.reduce((sum, row) => sum + row.heldTt, 0);
		},
		get equipmentTt() {
			return equipment.reduce((sum, row) => sum + row.ttValue, 0);
		},
		get equipmentBasis() {
			return equipment.reduce((sum, row) => sum + row.ttValue + row.markupPaid, 0);
		},
		load,
		loadHistory,
		refresh,
		refreshMarket,
		subscribe,
		setPacketFeeSharePct,
		listLoot,
		sellLootByTrade,
		resolveDraftName,
		captureSaleWindow,
		takeSaleWindowCapture,
		createFromDraft,
		listEquipment,
		sellEquipmentByTrade,
		resolveListing,
		undo,
		undoCheck,
		undoOpening,
		reopenListing,
		recycle,
		convert,
		remove,
		shrapnel,
		deleteEquipment,
		addPurchase,
		updatePurchase,
		deletePurchase,
	};
}

export type InventoryModel = ReturnType<typeof createInventoryModel>;
