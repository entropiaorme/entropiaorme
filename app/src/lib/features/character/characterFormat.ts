/**
 * Display rules for the character stats tables: the signed gain since the
 * last scan and its tone, and a profession level with its progress.
 */

import { NO_DATA } from '$lib/utils/format';

// Gain is shown with sign + 2dp; near-zero collapses to '0.00' so it doesn't
// flicker between '+0.00' and '-0.00'. Null = no anchor on record.
export function formatGain(gain: number | null): string {
	if (gain === null) return NO_DATA;
	if (Math.abs(gain) < 0.005) return '0.00';
	return (gain > 0 ? '+' : '') + gain.toFixed(2);
}

export function gainColorClass(gain: number | null): string {
	if (gain === null || Math.abs(gain) < 0.005) return 'text-text-tertiary';
	return gain > 0 ? 'text-success' : 'text-warning';
}

export function formatProfLevel(level: number | null): string {
	if (level === null) return NO_DATA;
	return `${Math.floor(level)} (${((level % 1) * 100).toFixed(1)}%)`;
}
