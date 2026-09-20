import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PinConfig } from '$lib/api';

const getPinConfigs = vi.fn();
const emit = vi.fn();

vi.mock('$lib/api', () => ({
	getPinConfigs: (...args: unknown[]) => getPinConfigs(...args),
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
				status: 'read',
				lon: 61_234,
				lat: 75_456,
				rawText: 'LON 61234 | LAT 75456',
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
			{ status: 'read', lon: 100, lat: 200, rawText: null },
		);
		expect(input?.kind).toBe('marker');
	});

	it('refuses a read result without both coordinates', async () => {
		const { cartographyPinInput } = await loadModule();
		expect(
			cartographyPinInput('Arkadia', null, treeConfig(), {
				status: 'read',
				lon: null,
				lat: 75_456,
				rawText: null,
			}),
		).toBeNull();
	});
});
