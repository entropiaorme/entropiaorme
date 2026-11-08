/**
 * The market family: the manual markup-observation feed (paste preview
 * and commit), the player's own markup entries and live Nanocube marks,
 * and their reads (overview, per-item history, activity and by-name
 * markups). Thin wrappers over the generated typed commands.
 *
 * This is the informational market layer: estimated markup never joins
 * the ledger, the analytics aggregates, or any realised figure.
 */

import * as commands from './commands.gen';

/**
 * The Tauri-bus topic the shell's event bridge emits after any market write
 * commits (the colon form of the `market.updated` wire topic): a paste, a
 * quote, a manual markup, or a Nanocube mark. A pure trigger for every
 * surface valuing loot at market markup to re-read.
 */
export const MARKET_TOPIC = 'market:updated';

export const previewMarketPaste = commands.marketPastePreview;
export const commitMarketPaste = commands.marketPasteCommit;
export const setMarketUnitPrice = commands.marketUnitPriceSet;
export const getMarketAuctionPacketThreshold = commands.marketAuctionPacketThreshold;
export const getMarketOverview = commands.marketOverview;
export const getMarketItemHistory = commands.marketItemHistory;
export const getMarketHarvestMarkups = commands.marketHarvestMarkups;
export const getMarketHuntMarkups = commands.marketHuntMarkups;
export const getMarketItemMarkups = commands.marketItemMarkups;
export const setManualMarkup = commands.marketManualMarkupSet;
export const clearManualMarkup = commands.marketManualMarkupClear;
export const setNanocubeMark = commands.marketNanocubeMarkSet;
