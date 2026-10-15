/**
 * The market family: the manual markup-observation feed (paste preview
 * and commit) and its reads (overview, per-item history). Thin wrappers
 * over the generated typed commands.
 *
 * This is the informational market layer: estimated markup never joins
 * the ledger, the analytics aggregates, or any realised figure.
 */

import * as commands from './commands.gen';

export const previewMarketPaste = commands.marketPastePreview;
export const commitMarketPaste = commands.marketPasteCommit;
export const setMarketUnitPrice = commands.marketUnitPriceSet;
export const getMarketAuctionPacketThreshold = commands.marketAuctionPacketThreshold;
export const getMarketOverview = commands.marketOverview;
export const getMarketItemHistory = commands.marketItemHistory;
export const getMarketHarvestMarkups = commands.marketHarvestMarkups;
export const getMarketHuntMarkups = commands.marketHuntMarkups;
