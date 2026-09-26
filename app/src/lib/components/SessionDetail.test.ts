// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { SessionDetail as SessionDetailType } from '$lib/types/tracking';
import SessionDetail from './SessionDetail.svelte';

vi.mock('$lib/api', () => ({
	ApiError: class ApiError extends Error {},
	PROTECTION_TOPIC: 'protection:updated',
	HEALING_TOPIC: 'healing:updated',
	WEAPONS_TOPIC: 'weapons:updated',
	CONSUMABLES_TOPIC: 'consumables:updated',
	getSessionDoses: vi.fn(async () => []),
	removeConsumableDose: vi.fn(),
	restoreConsumableDose: vi.fn(),
	activateLootItem: vi.fn(),
	assignWeaponShot: vi.fn(),
	getWeaponCorrectionWeapons: vi.fn(),
	getWeaponShots: vi.fn(),
	undoWeaponAssignment: vi.fn(),
	correctHealing: vi.fn(),
	getHealingCorrectionTools: vi.fn(),
	getHealingOutputs: vi.fn(),
	undoHealingCorrection: vi.fn(),
	deactivateLootItem: vi.fn(),
	getProtectionSessionStatus: vi.fn(async () => ({ unrecordedHits: 0 })),
	getSessionDetail: vi.fn(),
	renameSessionMob: vi.fn(),
	restoreSessionMob: vi.fn(),
}));

// The protection bridge: capture the listener so a test can fire a write.
const protectionListeners: Array<() => void> = [];
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (topic: string, handler: () => void) => {
		if (topic === 'protection:updated') protectionListeners.push(handler);
		return () => {};
	}),
}));

function detail(overrides: Partial<SessionDetailType> = {}): SessionDetailType {
	return {
		sessionId: 's1',
		sessionName: null,
		summary: {
			cost: 10,
			returns: 12,
			pes: 1,
			net: 2,
			returnRate: 1.2,
			kills: 5,
			duration: 600,
			costBreakdown: {
				weaponCost: 10,
				healCost: 0,
				enhancerCost: 0,
				armourCost: 0,
				harvestCost: 0,
			},
		},
		harvest: { swings: 0, successes: 0, lootTt: 0, cost: 0 },
		mobEntryMode: 'mob',
		notableEvents: [],
		lootBreakdown: [],
		deactivatedLootBreakdown: [],
		mobBreakdown: [],
		effectiveLoot: 12,
		toolStats: [],
		skillGains: [],
		...overrides,
	} as SessionDetailType;
}

beforeEach(() => {
	vi.clearAllMocks();
	protectionListeners.length = 0;
});

// The name is a stamp of the session definition's name, not a label of
// this instance: identity comes from the definition, so the record shows
// what was recorded and offers no way to retype it. A mis-recorded
// session is corrected by moving it to another definition.
describe('the recorded session name', () => {
	it('shows the recorded name, and Unnamed when there is none', () => {
		const { unmount } = render(SessionDetail, {
			props: { detail: detail({ sessionName: 'Ark Monura Instance' }) },
		});
		expect(screen.getByText('Ark Monura Instance')).toBeTruthy();
		unmount();

		render(SessionDetail, { props: { detail: detail() } });
		expect(screen.getByText('Unnamed')).toBeTruthy();
	});

	it('offers no rename affordance', () => {
		render(SessionDetail, { props: { detail: detail({ sessionName: 'Wrong Name' }) } });
		expect(screen.queryByText('Rename')).toBeNull();
		expect(screen.queryByLabelText('Session name')).toBeNull();
	});
});

// Armour is recorded when the player repairs, possibly sessions later, so a
// session with hits and no recording yet must not read as zero armour cost.
describe('the armour standing', () => {
	it('reads as not recorded while hits await a recording', async () => {
		const api = await import('$lib/api');
		vi.mocked(api.getProtectionSessionStatus).mockResolvedValueOnce({ unrecordedHits: 42 });
		render(SessionDetail, { props: { detail: detail() } });
		expect(await screen.findByText('not recorded yet')).toBeTruthy();
	});

	it('re-reads the cost and standing when a recording lands elsewhere', async () => {
		const api = await import('$lib/api');
		vi.mocked(api.getProtectionSessionStatus)
			.mockResolvedValueOnce({ unrecordedHits: 42 })
			.mockResolvedValue({ unrecordedHits: 0 });
		const recorded = detail();
		recorded.summary.costBreakdown.armourCost = 2.25;
		vi.mocked(api.getSessionDetail).mockResolvedValue(recorded);
		render(SessionDetail, { props: { detail: detail() } });
		expect(await screen.findByText('not recorded yet')).toBeTruthy();

		await vi.waitFor(() => expect(protectionListeners).toHaveLength(1));
		protectionListeners[0]();
		expect(await screen.findByText('2.25')).toBeTruthy();
		expect(api.getSessionDetail).toHaveBeenCalledWith('s1');
		expect(screen.queryByText('not recorded yet')).toBeNull();
	});

	it('shows the recorded cost once a recording covers the session', async () => {
		const recorded = detail();
		recorded.summary.costBreakdown.armourCost = 1.5;
		render(SessionDetail, { props: { detail: recorded } });
		expect(await screen.findByText('1.50')).toBeTruthy();
		expect(screen.queryByText('not recorded yet')).toBeNull();
	});
});
