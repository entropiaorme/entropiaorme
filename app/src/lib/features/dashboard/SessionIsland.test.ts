// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

// The island's own surface is what is under test: the session picker and the
// Tauri-backed seams are stubbed.
vi.mock('$lib/features/sessions/DefinitionPicker.svelte', async () => ({
	default: (await import('$lib/components/Divider.svelte')).default,
}));
vi.mock('$lib/api', () => ({
	startTracking: vi.fn(),
	toggleOverlay: vi.fn(async () => {}),
}));
vi.mock('$lib/stores/trackingStore.svelte', () => ({ hydrate: vi.fn() }));
vi.mock('$lib/statsScope.svelte', () => ({ setStatsScope: vi.fn(async () => {}) }));
vi.mock('$lib/liveMarkupSettings.svelte', () => ({
	liveMarkupSettings: { looterBasis: 'three_looter_mean', confidence: 'medium' },
}));

import type { TrackingSnapshot } from '$lib/api';
import type { DefinitionsModel } from '$lib/features/sessions/definitionsModel.svelte';
import SessionIsland from './SessionIsland.svelte';
import type { StatsGridModel } from './statsGridModel.svelte';

const statsGrid = { scope: 'instance', enabledStats: [] } as unknown as StatsGridModel;
const definitions = { definitions: [], error: null } as unknown as DefinitionsModel;

function mount(status: TrackingSnapshot) {
	return render(SessionIsland, {
		props: { status, statsGrid, definitions, onReview: vi.fn() },
	});
}

const deaf: TrackingSnapshot = {
	status: 'active',
	started_at: '2026-11-13T10:00:00',
	hotbarKeysEnabled: true,
	hotbarListenerActive: false,
};

describe('the hotbar listener note', () => {
	it('says weapon costs follow damage ranges while the screen does not name the tool, as it always did', () => {
		mount(deaf);
		expect(screen.getByText('Hotbar key listener not active')).toBeTruthy();
		expect(
			screen.getByText(
				'Your hotbar presses are not reaching the app, so weapon costs follow damage ranges alone. Check the hotbar key listener in Settings.',
			),
		).toBeTruthy();
	});

	it('says the same with reading turned on but the screen not read, since the keys are in use', () => {
		mount({
			...deaf,
			toolSource: {
				readingScreen: false,
				notReading: 'stopped',
				manual: false,
				held: { kind: 'notRead' },
			},
		});
		expect(screen.getByText(/weapon costs follow damage ranges alone/)).toBeTruthy();
	});

	it('says only consumable timers are affected while the screen names the tool', () => {
		mount({
			...deaf,
			toolSource: {
				readingScreen: true,
				notReading: null,
				manual: false,
				held: { kind: 'nothing' },
			},
		});
		expect(screen.getByText(/consumables pressed on it do not start their timers/)).toBeTruthy();
		expect(screen.queryByText(/damage ranges/)).toBeNull();
	});
});
