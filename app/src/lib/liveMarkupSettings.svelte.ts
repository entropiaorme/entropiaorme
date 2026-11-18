/**
 * How the live session figures read markup and expected return.
 *
 * Two choices belong to the session in play rather than to planning:
 *
 * - The confidence threshold: which market readings the live markup
 *   figures trust. Items below it are valued at the Nanocube markup, the
 *   way Inventory's confidence toggle values them, but the live surfaces
 *   remember their own choice.
 * - The looter basis the live expected figure is read at. Nothing yet
 *   tells the app which class of mob is being hunted, so the player
 *   declares it; the declaration is the dashboard's own, separate from the
 *   planning basis Equipment and Market read at.
 *
 * Both persist like the stat selection and broadcast across windows (the
 * overlay is its own Tauri window), so the dashboard and the overlay
 * always read the same way.
 */

import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { ConfidenceMode } from '$lib/features/analytics/treeCuttingModel.svelte';
import { DEFAULT_LOOTER_BASIS, LOOTER_BASES, type LooterBasis } from './looterBasis.svelte';
import { getPreference, setPreference } from './preferences';

export interface LiveMarkupSettings {
	confidence: ConfidenceMode;
	looterBasis: LooterBasis;
}

const KEY_CONFIDENCE = 'liveMarkupConfidence';
const KEY_LOOTER_BASIS = 'liveLooterBasis';

/** Cross-window broadcast, so a change on one surface moves the other. */
export const LIVE_MARKUP_SETTINGS_CHANGED_EVENT = 'live-markup-settings-changed';

/** The live default matches Inventory's: trust liquid and middling
 * markets, value the rest at Nanocube. */
export const DEFAULT_LIVE_CONFIDENCE: ConfidenceMode = 'liquidMiddling';

const CONFIDENCE_MODES: readonly ConfidenceMode[] = ['liquid', 'liquidMiddling', 'all'];

function sanitiseConfidence(value: unknown): ConfidenceMode {
	return CONFIDENCE_MODES.includes(value as ConfidenceMode)
		? (value as ConfidenceMode)
		: DEFAULT_LIVE_CONFIDENCE;
}

function sanitiseBasis(value: unknown): LooterBasis {
	return LOOTER_BASES.includes(value as LooterBasis)
		? (value as LooterBasis)
		: DEFAULT_LOOTER_BASIS;
}

let confidence = $state<ConfidenceMode>(DEFAULT_LIVE_CONFIDENCE);
let looterBasis = $state<LooterBasis>(DEFAULT_LOOTER_BASIS);

export const liveMarkupSettings = {
	get confidence(): ConfidenceMode {
		return confidence;
	},
	get looterBasis(): LooterBasis {
		return looterBasis;
	},
};

export async function initLiveMarkupSettings(): Promise<void> {
	const [storedConfidence, storedBasis] = await Promise.all([
		getPreference<unknown>(KEY_CONFIDENCE, DEFAULT_LIVE_CONFIDENCE),
		getPreference<unknown>(KEY_LOOTER_BASIS, DEFAULT_LOOTER_BASIS),
	]);
	confidence = sanitiseConfidence(storedConfidence);
	looterBasis = sanitiseBasis(storedBasis);
}

function broadcast(): void {
	const payload: LiveMarkupSettings = { confidence, looterBasis };
	void emit(LIVE_MARKUP_SETTINGS_CHANGED_EVENT, payload);
}

export async function setLiveConfidence(value: ConfidenceMode): Promise<void> {
	confidence = sanitiseConfidence(value);
	broadcast();
	await setPreference(KEY_CONFIDENCE, confidence);
}

export async function setLiveLooterBasis(value: LooterBasis): Promise<void> {
	looterBasis = sanitiseBasis(value);
	broadcast();
	await setPreference(KEY_LOOTER_BASIS, looterBasis);
}

/** Follow the other window's changes; returns the detach function. */
export function followLiveMarkupSettings(): Promise<UnlistenFn> {
	return listen<Partial<LiveMarkupSettings>>(LIVE_MARKUP_SETTINGS_CHANGED_EVENT, (event) => {
		confidence = sanitiseConfidence(event.payload?.confidence);
		looterBasis = sanitiseBasis(event.payload?.looterBasis);
	});
}
