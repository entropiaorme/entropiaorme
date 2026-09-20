import { emit } from '@tauri-apps/api/event';
import {
	type CoordScanResult,
	createMapPin,
	getNearbyMapPin,
	getPinConfigs,
	type MapPinInput,
	type PinConfig,
	scanMapCoordinates,
} from '$lib/api';

/**
 * The cartography overlay's shared state: the current planet/map-view context
 * (synced from the main Maps surface) and the pin configurations for it (the
 * palette). Configurations are first-class per-preset data read from the
 * database, so the palette and the placed pins stay in step, and a
 * configuration's colour and special behaviour flow through to its pins.
 */

export type CartographyContext = { planet: string | null; mapViewId: number | null };

/** Broadcast the active planet/map-view context to the overlay window. */
export const CARTOGRAPHY_OVERLAY_CHANGED_EVENT = 'cartography-overlay-changed';
/**
 * The overlay asks the main surface to (re)publish the current context. The
 * broadcast is one-shot and fires on selection change, so an overlay whose
 * listener was not yet live when it fired (a pre-spawned window that WebKitGTK
 * only realises on its first show) would otherwise never learn the planet. On
 * coming alive (and on every show) the overlay requests, and the main surface
 * replies, closing that race without either side polling.
 */
export const CARTOGRAPHY_OVERLAY_CONTEXT_REQUEST = 'cartography-overlay-request-context';
/** Broadcast that placed pins changed, so the main map refreshes them. */
export const MAP_PINS_CHANGED_EVENT = 'map-pins-changed';

export const MAX_PIN_CONFIGS = 12;
export const DEFAULT_GENERIC_COLOUR = '#38bdf8';
export const DEFAULT_TREE_COLOUR = '#22c55e';
export const DEFAULT_TREE_COOLDOWN_COLOUR = '#f59e0b';

let context = $state<CartographyContext>({ planet: null, mapViewId: null });
let configs = $state<PinConfig[]>([]);

export const cartographyOverlay = {
	get context(): CartographyContext {
		return context;
	},
	get configs(): PinConfig[] {
		return configs;
	},
};

/** Reload the palette for the current context from the database. */
export async function loadCartographyConfigs(): Promise<void> {
	if (!context.planet) {
		configs = [];
		return;
	}
	try {
		configs = await getPinConfigs(context.planet, context.mapViewId);
	} catch {
		// Keep the last-good palette; a later event can restore live state.
	}
}

/** Set the context locally (the main surface owns selection). */
export function setCartographyContext(next: CartographyContext): void {
	context = next;
}

/** Main surface: set the context and broadcast it to the overlay window. */
export function broadcastCartographyContext(next: CartographyContext): void {
	context = next;
	void emit(CARTOGRAPHY_OVERLAY_CHANGED_EVENT, next);
}

/** Overlay window: ask the main surface to (re)publish the current context. */
export function requestCartographyContext(): void {
	void emit(CARTOGRAPHY_OVERLAY_CONTEXT_REQUEST);
}

/** Overlay window: adopt a broadcast context, sanitised. */
export function acceptCartographyContextBroadcast(value: unknown): CartographyContext {
	const candidate = (value ?? {}) as Partial<CartographyContext>;
	context = {
		planet:
			typeof candidate.planet === 'string' && candidate.planet.trim()
				? candidate.planet.trim().slice(0, 80)
				: null,
		mapViewId:
			typeof candidate.mapViewId === 'number' &&
			Number.isSafeInteger(candidate.mapViewId) &&
			candidate.mapViewId > 0
				? candidate.mapViewId
				: null,
	};
	return context;
}

export function cartographyScanFailureMessage(
	status: CoordScanResult['status'],
	planet: string,
): string {
	const messages: Partial<Record<CoordScanResult['status'], string>> = {
		noRegion: 'Calibrate capture in Maps first.',
		captureFailed: 'Screen capture failed.',
		engineUnavailable: 'Text recognition is unavailable.',
		unreadable: 'Coordinates could not be read.',
		implausible: `The readout is outside ${planet}.`,
	};
	return messages[status] ?? 'Coordinate scan failed.';
}

export type PinDropOutcome =
	| { kind: 'placed'; label: string }
	| { kind: 'duplicate'; input: MapPinInput; existingName: string; distance: number; label: string }
	| { kind: 'error'; message: string };

/**
 * Scan the current coordinates and drop a pin for a palette configuration.
 * Reports a nearby existing pin (for the caller to confirm) or a typed failure
 * rather than throwing on the expected legs.
 */
export async function scanAndDropPin(
	config: PinConfig,
	context: CartographyContext,
	calibratedPlanets: string[],
): Promise<PinDropOutcome> {
	const { planet, mapViewId } = context;
	if (!planet || !calibratedPlanets.includes(planet)) {
		return { kind: 'error', message: 'Choose a calibrated planet in Maps first.' };
	}
	const result = await scanMapCoordinates(planet);
	const input = cartographyPinInput(planet, mapViewId, config, result);
	if (!input) {
		return { kind: 'error', message: cartographyScanFailureMessage(result.status, planet) };
	}
	const nearby = await getNearbyMapPin(planet, mapViewId, input.lon, input.lat);
	if (nearby) {
		return {
			kind: 'duplicate',
			input,
			existingName: nearby.pin.name,
			distance: nearby.distance,
			label: config.label,
		};
	}
	await createMapPin(input);
	void emit(MAP_PINS_CHANGED_EVENT, { planet });
	return { kind: 'placed', label: config.label };
}

/** Create a confirmed duplicate pin and notify the map. */
export async function createConfirmedPin(input: MapPinInput): Promise<void> {
	await createMapPin({ ...input, allowNearby: true });
	void emit(MAP_PINS_CHANGED_EVENT, { planet: input.planet });
}

/** Build the pin-drop input for a palette configuration and a scan result. */
export function cartographyPinInput(
	planet: string,
	mapViewId: number | null,
	config: PinConfig,
	result: CoordScanResult,
): MapPinInput | null {
	if (result.status !== 'read' || result.lon == null || result.lat == null) return null;
	return {
		planet,
		lon: result.lon,
		lat: result.lat,
		// The readout no longer shows an altitude, so a scanned pin has none.
		altitude: null,
		name: config.label,
		icon: config.icon,
		kind: config.specialKind ?? 'marker',
		radiusM: config.radiusM,
		notes: null,
		sessionId: null,
		mapViewId,
		pinConfigId: config.id,
		allowNearby: false,
	};
}
