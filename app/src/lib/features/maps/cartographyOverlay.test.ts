import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PinConfig } from '$lib/api';

const getPinConfigs = vi.fn();
const scanMapCoordinates = vi.fn();
const getNearbyMapPin = vi.fn();
const createMapPin = vi.fn();
const emit = vi.fn();

vi.mock('$lib/api', () => ({
	getPinConfigs: (...args: unknown[]) => getPinConfigs(...args),
	scanMapCoordinates: (...args: unknown[]) => scanMapCoordinates(...args),
	getNearbyMapPin: (...args: unknown[]) => getNearbyMapPin(...args),
	createMapPin: (...args: unknown[]) => createMapPin(...args),
}));
vi.mock('@tauri-apps/api/event', () => ({
	emit: (...args: unknown[]) => emit(...args),
}));

type Mod = typeof import('./cartographyOverlay.svelte');

async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./cartographyOverlay.svelte');
}

function treeConfig(overrides: Partial<PinConfig> = {}): PinConfig {
	return {
		id: 1,
		planet: 'Arkadia',
		mapViewId: null,
		label: 'Tree',
		category: 'special',
		specialKind: 'tree',
		icon: '🌳',
		radiusM: null,
		colour: '#22c55e',
		cooldownColour: '#f59e0b',
		ordinal: 0,
		createdAt: 1,
		placedCount: 0,
		...overrides,
	};
}

beforeEach(() => {
	getPinConfigs.mockReset().mockResolvedValue([]);
	scanMapCoordinates.mockReset();
	getNearbyMapPin.mockReset().mockResolvedValue(null);
	createMapPin.mockReset().mockResolvedValue({});
	emit.mockReset().mockResolvedValue(undefined);
});

describe('cartography overlay context', () => {
	it('sanitises a broadcast context', async () => {
		const { acceptCartographyContextBroadcast } = await loadModule();
		expect(acceptCartographyContextBroadcast({ planet: ' Calypso ', mapViewId: 42 })).toEqual({
			planet: 'Calypso',
			mapViewId: 42,
		});
		expect(acceptCartographyContextBroadcast({ planet: '  ', mapViewId: -3 })).toEqual({
			planet: null,
			mapViewId: null,
		});
		expect(acceptCartographyContextBroadcast(null)).toEqual({ planet: null, mapViewId: null });
	});

	it('broadcasts the context to every window', async () => {
		const { broadcastCartographyContext, cartographyOverlay, CARTOGRAPHY_OVERLAY_CHANGED_EVENT } =
			await loadModule();
		broadcastCartographyContext({ planet: 'Arkadia', mapViewId: 7 });
		expect(cartographyOverlay.context).toEqual({ planet: 'Arkadia', mapViewId: 7 });
		expect(emit).toHaveBeenCalledWith(CARTOGRAPHY_OVERLAY_CHANGED_EVENT, {
			planet: 'Arkadia',
			mapViewId: 7,
		});
	});

	it('requests the current context from the main surface', async () => {
		const { requestCartographyContext, CARTOGRAPHY_OVERLAY_CONTEXT_REQUEST } = await loadModule();
		requestCartographyContext();
		expect(emit).toHaveBeenCalledWith(CARTOGRAPHY_OVERLAY_CONTEXT_REQUEST);
	});

	it('loads the palette for the current context', async () => {
		getPinConfigs.mockResolvedValue([treeConfig()]);
		const { acceptCartographyContextBroadcast, loadCartographyConfigs, cartographyOverlay } =
			await loadModule();
		acceptCartographyContextBroadcast({ planet: 'Arkadia', mapViewId: null });
		await loadCartographyConfigs();
		expect(getPinConfigs).toHaveBeenCalledWith('Arkadia', null);
		expect(cartographyOverlay.configs).toHaveLength(1);
		expect(cartographyOverlay.configs[0].label).toBe('Tree');
	});

	it('clears the palette when no planet is selected', async () => {
		const { loadCartographyConfigs, cartographyOverlay } = await loadModule();
		await loadCartographyConfigs();
		expect(getPinConfigs).not.toHaveBeenCalled();
		expect(cartographyOverlay.configs).toEqual([]);
	});
});

describe('cartographyPinInput', () => {
	it('maps a scan and a configuration to the typed pin input', async () => {
		const { cartographyPinInput } = await loadModule();
		expect(
			cartographyPinInput('Arkadia', 42, treeConfig({ id: 9 }), {
				kind: 'read',
				lon: 61_234,
				lat: 75_456,
				source: 'radar',
			}),
		).toEqual({
			planet: 'Arkadia',
			lon: 61_234,
			lat: 75_456,
			// The readout carries no altitude, so a scanned pin has none.
			altitude: null,
			name: 'Tree',
			icon: '🌳',
			kind: 'tree',
			radiusM: null,
			notes: null,
			sessionId: null,
			mapViewId: 42,
			pinConfigId: 9,
			allowNearby: false,
		});
	});

	it('uses the marker kind for a generic configuration', async () => {
		const { cartographyPinInput } = await loadModule();
		const input = cartographyPinInput(
			'Arkadia',
			null,
			treeConfig({ category: 'generic', specialKind: null, icon: '📍' }),
			{ kind: 'read', lon: 10_100, lat: 10_200, source: 'region' },
		);
		expect(input?.kind).toBe('marker');
	});

	it('refuses a refused scan, which carries no coordinates', async () => {
		const { cartographyPinInput } = await loadModule();
		expect(
			cartographyPinInput('Arkadia', null, treeConfig(), {
				kind: 'refused',
				refusal: { reason: 'notRead', lon: 'covered', lat: null },
			}),
		).toBeNull();
	});
});

describe('scanAndDropPin', () => {
	it('pins what was read, on the map it was read for', async () => {
		scanMapCoordinates.mockResolvedValue({
			kind: 'read',
			lon: 20_000,
			lat: 21_000,
			source: 'radar',
		});
		const { acceptCartographyContextBroadcast, scanAndDropPin } = await loadModule();
		const context = acceptCartographyContextBroadcast({ planet: 'Arkadia', mapViewId: 3 });
		const outcome = await scanAndDropPin(treeConfig(), context, ['Arkadia']);
		expect(outcome).toEqual({ kind: 'placed', label: 'Tree' });
		expect(createMapPin).toHaveBeenCalledWith(
			expect.objectContaining({ planet: 'Arkadia', mapViewId: 3, lon: 20_000, lat: 21_000 }),
		);
	});

	it('says why a refused read pinned nothing', async () => {
		scanMapCoordinates.mockResolvedValue({
			kind: 'refused',
			refusal: { reason: 'notRead', lon: null, lat: 'covered' },
		});
		const { acceptCartographyContextBroadcast, scanAndDropPin } = await loadModule();
		const context = acceptCartographyContextBroadcast({ planet: 'Arkadia', mapViewId: null });
		expect(await scanAndDropPin(treeConfig(), context, ['Arkadia'])).toEqual({
			kind: 'error',
			message: 'The coordinates are covered by another window.',
		});
		expect(createMapPin).not.toHaveBeenCalled();
	});

	it('drops a read that lands after another planet was opened', async () => {
		let land: (value: unknown) => void = () => {};
		scanMapCoordinates.mockReturnValue(new Promise((resolve) => (land = resolve)));
		const { acceptCartographyContextBroadcast, scanAndDropPin } = await loadModule();
		const context = acceptCartographyContextBroadcast({ planet: 'Arkadia', mapViewId: null });
		const dropping = scanAndDropPin(treeConfig(), context, ['Arkadia', 'Calypso']);
		acceptCartographyContextBroadcast({ planet: 'Calypso', mapViewId: null });
		land({ kind: 'read', lon: 20_000, lat: 21_000, source: 'radar' });
		const outcome = await dropping;
		expect(outcome.kind).toBe('error');
		expect(getNearbyMapPin).not.toHaveBeenCalled();
		expect(createMapPin).not.toHaveBeenCalled();
	});
});
