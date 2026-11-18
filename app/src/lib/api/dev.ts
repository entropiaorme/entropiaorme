/**
 * Developer tools: the metrics snapshot, crash reporting, database
 * maintenance, and the auction-fee research session. Nothing gates them.
 */

import * as commands from './commands.gen';

export type {
	AuctionFeeOverlayStatus,
	AuctionFeeResearchStatus,
	HistogramSnapshot,
	MetricsSnapshot,
} from './commands.gen';

export const getDevMetrics = commands.devMetrics;
export const startAuctionFeeResearch = commands.devAuctionFeeResearchStart;
export const stopAuctionFeeResearch = commands.devAuctionFeeResearchStop;
export const getAuctionFeeResearchStatus = commands.devAuctionFeeResearchStatus;
export const captureAuctionFeeResearchSample = commands.devAuctionFeeResearchCapture;
export const getAuctionFeeResearchOverlayStatus = commands.devAuctionFeeResearchOverlayStatus;

export async function getCrashReporting(): Promise<boolean> {
	return (await commands.devCrashReporting()).crash_reporting_enabled;
}

export async function setCrashReporting(enabled: boolean): Promise<boolean> {
	return (await commands.devSetCrashReporting(enabled)).crash_reporting_enabled;
}
