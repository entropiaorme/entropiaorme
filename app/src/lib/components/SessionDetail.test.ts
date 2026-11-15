// @vitest-environment happy-dom

import { render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import * as api from '$lib/api';
import type { SessionDetail as SessionDetailType } from '$lib/types/tracking';
import SessionDetail from './SessionDetail.svelte';

// The in-development channel, switchable per test: a published build hides
// surfaces still in development.
const channel = vi.hoisted(() => ({ visible: true }));
vi.mock('$lib/inDevelopment/channel', () => ({
	isInDevelopmentVisible: (stable: boolean) => !stable,
	inDevelopment: {
		get visible() {
			return channel.visible;
		},
	},
}));

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
	getMobDetections: vi.fn(async () => ({ sessionId: 's1', ended: true, mobs: [] })),
	settleMobDetection: vi.fn(),
	getToolSightings: vi.fn(async () => ({ sessionId: 's1', ended: true, sightings: [] })),
	settleToolSighting: vi.fn(),
	undoToolSighting: vi.fn(),
	setLibraryEnabled: vi.fn(),
	getTrackingSnapshot: vi.fn(),
}));

// The account readings: no measure unless a test gives the session one.
const account = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	disconnectAccount: vi.fn(),
	getSessionMeasure: vi.fn(),
	getMeasuredSessions: vi.fn(),
}));
vi.mock('$lib/api/account', () => account);

// The protection and tracking bridges: capture the listeners so a test can
// fire a write or the tracker moving on.
const protectionListeners: Array<() => void> = [];
const trackingListeners: Array<() => void> = [];
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (topic: string, handler: () => void) => {
		if (topic === 'protection:updated') protectionListeners.push(handler);
		if (topic === 'tracking:session:updated') trackingListeners.push(handler);
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
	trackingListeners.length = 0;
	channel.visible = true;
	account.getSessionMeasure.mockResolvedValue(null);
});

// The name is a stamp of the session definition's name, not a label of
// this instance: identity comes from the definition, so the record shows
// what was recorded and offers no way to retype it. A mis-recorded
// session is corrected by moving it to another definition.
describe('the quest rewards a session handed in', () => {
	it('lists each reward with its lines, apart from the loot', () => {
		render(SessionDetail, {
			props: {
				detail: detail({
					questRewards: [
						{
							completionId: 225,
							questName: 'AI Daily: Hanna Hendrix',
							completedAt: 1_794_762_846,
							items: [
								{
									itemName: 'Universal Ammo',
									quantity: 136_177,
									valuePed: 13.61,
									accounting: 'liquid',
								},
								{
									itemName: 'Blazar Fragment',
									quantity: 103,
									valuePed: 0.001,
									accounting: 'stock',
								},
							],
						},
					],
				}),
			},
		});
		const section = screen.getByTestId('session-quest-rewards');
		expect(section.textContent).toContain('AI Daily: Hanna Hendrix');
		expect(section.textContent).toContain('13.61 PED');
		expect(section.textContent).toContain('Universal Ammo');
		// An item with no TT shows how many there are, never 0.00.
		expect(section.textContent).toContain('\u00d7103');
	});

	it('shows nothing for a session that handed nothing in', () => {
		render(SessionDetail, { props: { detail: detail({ questRewards: [] }) } });
		expect(screen.queryByTestId('session-quest-rewards')).toBeNull();
	});
});

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

describe('the mob attribution', () => {
	it('says how sure each mob is, and marks an unlisted one until it is confirmed', () => {
		render(SessionDetail, {
			props: {
				detail: detail({
					mobBreakdown: [
						{ currentName: 'Gloomrat', originalName: null, killCount: 2, certainty: 'unlisted' },
						{ currentName: 'Old Atrox', originalName: null, killCount: 1, certainty: 'probable' },
						{
							currentName: 'Snarl',
							originalName: null,
							killCount: 1,
							certainty: 'unlisted_probable',
						},
						{ currentName: 'Young Atrox', originalName: null, killCount: 3, certainty: 'recorded' },
					],
				}),
			},
		});
		expect(screen.getAllByText('not in the mob list, unconfirmed')).toHaveLength(1);
		expect(screen.getAllByText('probably')).toHaveLength(1);
		expect(screen.getAllByText('probably; not in the mob list, unconfirmed')).toHaveLength(1);
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

describe('the after-session reviews in a build that hides surfaces in development', () => {
	const sighting = {
		id: 't1',
		sighted: { kind: 'bundled', name: 'Sollomate Opalo', tool: 'weapon', shots: 212 },
		firstSeen: 1000,
		lastSeen: 2000,
		settled: null,
	} as const;

	it('still shows the detected tools, and follows a running session', async () => {
		channel.visible = false;
		const read = vi.mocked(api.getToolSightings);
		read.mockImplementation(async () => ({ sessionId: 's1', ended: false, sightings: [sighting] }));
		try {
			render(SessionDetail, { props: { detail: detail() } });
			expect(await screen.findByText('Sollomate Opalo')).toBeTruthy();
			expect(screen.getByText('Detected tools')).toBeTruthy();
			expect(screen.queryByText('In development')).toBeNull();
			const before = read.mock.calls.length;

			await waitFor(() => expect(trackingListeners.length).toBeGreaterThan(0));
			for (const handler of trackingListeners) handler();
			await waitFor(() => expect(read.mock.calls.length).toBeGreaterThan(before));
		} finally {
			read.mockImplementation(async () => ({ sessionId: 's1', ended: true, sightings: [] }));
		}
	});

	it('still reads mob detection', async () => {
		channel.visible = false;
		render(SessionDetail, { props: { detail: detail() } });
		await waitFor(() => expect(api.getMobDetections).toHaveBeenCalled());
		expect(api.getToolSightings).toHaveBeenCalled();
	});

	it('reads mob detection wherever surfaces in development show too', async () => {
		render(SessionDetail, { props: { detail: detail() } });
		await waitFor(() => expect(api.getMobDetections).toHaveBeenCalled());
		expect(api.getToolSightings).toHaveBeenCalled();
	});
});

describe('what the account readings measured', () => {
	const measure = {
		sessionId: 's1',
		sealed: true,
		coverage: {
			level: 'complete',
			opening: 'read',
			closing: 'read',
			readingCount: 10,
			gapCount: 0,
		},
		ammo: [],
		decay: [],
		repairs: [],
		otherConsumption: [],
		unexplainedDepartures: [],
		gained: [],
		gainedTotal: { ped: 11.5, precision: 'exact' },
		gainedPossiblyStrongbox: { ped: 0, precision: 'exact' },
		events: [],
	} as const;

	it('adds nothing to the detail of a session the account never read', async () => {
		render(SessionDetail, { props: { detail: detail() } });
		await waitFor(() => expect(account.getSessionMeasure).toHaveBeenCalledWith('s1'));
		expect(screen.queryByText('Measured from the account')).toBeNull();
	});

	it('stands the measure beside the tracked figures, after the cost summary', async () => {
		account.getSessionMeasure.mockResolvedValue(measure);
		render(SessionDetail, { props: { detail: detail() } });
		const block = await screen.findByTestId('account-measure');
		expect(block.textContent).toContain('Measured from the account');
		const loot = within(block).getByText('Loot').closest('tr');
		if (!loot) throw new Error('no loot row');
		expect(loot.textContent).toContain('12.00');
		expect(loot.textContent).toContain('11.5000');
		expect(loot.textContent).toContain('-0.5000');
		// The tracked summary itself is unchanged.
		expect(screen.getByText('Loot TT').closest('div')?.parentElement?.textContent).toContain(
			'12.00',
		);
	});

	it('is not read at all in a build that hides surfaces in development', async () => {
		channel.visible = false;
		render(SessionDetail, { props: { detail: detail() } });
		await waitFor(() => expect(api.getMobDetections).toHaveBeenCalled());
		expect(account.getSessionMeasure).not.toHaveBeenCalled();
	});
});
