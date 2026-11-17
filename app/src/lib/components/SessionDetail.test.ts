// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import * as api from '$lib/api';
import {
	minutes as minuteSeries,
	step as minuteStep,
	missing as missingLoot,
} from '$lib/features/account/__fixtures__/minutes';
import type { SessionDetail as SessionDetailType } from '$lib/types/tracking';
import SessionDetail from './SessionDetail.svelte';

const NO_OVERHEAD_REASONS = { travel: null, fieldLogistics: null, unclassified: null };

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
	getMeasuredSummary: vi.fn(),
	getCostPlacements: vi.fn(),
	placeCostItem: vi.fn(),
	clearCostPlacement: vi.fn(),
	// No step to read minute by minute unless a test gives the session some.
	getSessionMinutes: vi.fn(),
	recordArrivalUnknownOrigin: vi.fn(),
	undoArrivalUnknownOrigin: vi.fn(),
	unignoreArrival: vi.fn(),
	dismissProposal: vi.fn(),
	claimProposals: vi.fn(),
	releaseProposals: vi.fn(),
	linkProposals: vi.fn(),
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
			costBasis: 'tracked',
			costBreakdown: {
				weaponCost: 10,
				healCost: 0,
				consumableCost: 0,
				enhancerCost: 0,
				armourCost: 0,
				harvestCost: 0,
				tracked: {
					weaponCost: 10,
					healCost: 0,
					consumableCost: 0,
					enhancerCost: 0,
					armourCost: 0,
					harvestCost: 0,
				},
				basis: {
					weaponCost: 'tracked',
					healCost: 'tracked',
					consumableCost: 'tracked',
					enhancerCost: 'tracked',
					armourCost: 'tracked',
					harvestCost: 'tracked',
				},
				reasons: {
					weaponCost: 'noMeasuredCosts',
					healCost: 'noMeasuredCosts',
					consumableCost: 'noMeasuredCosts',
					enhancerCost: 'noMeasuredCosts',
					armourCost: 'noMeasuredCosts',
					harvestCost: 'noMeasuredCosts',
				},
				overheadCost: 0,
				overhead: { travel: 0, fieldLogistics: 0, unclassified: 0, reasons: NO_OVERHEAD_REASONS },
				measured: null,
				armourUnrecorded: false,
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
	account.getCostPlacements.mockResolvedValue({ awaiting: [], placed: [] });
	account.getSessionMinutes.mockResolvedValue(minuteSeries());
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
								{
									itemName: 'Halloween Strongbox',
									quantity: 2,
									valuePed: 0,
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
		// A TT of a tenth of a PEC is small, not none: it reads in PED, legibly.
		expect(section.textContent).toContain('0.0010');
		expect(section.textContent).not.toContain('\u00d7103');
		// An item with no TT shows how many there are, never 0.00.
		expect(section.textContent).toContain('\u00d72');
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

// An enhancer break costs its TT and markup less its returned Shrapnel; the
// figure says what it leaves out rather than showing a guess.
describe('the enhancer breaks a session kept', () => {
	const broke = (pricing: string, costPed: number | null) => ({
		enhancerName: 'Weapon Damage Enhancer 1',
		weaponName: 'Rifle',
		observedAt: 1000,
		returnedPed: 0.8,
		pricing,
		ttPed: pricing === 'ttUnknown' ? null : 1,
		markupPed: pricing === 'priced' ? 0.5 : null,
		costPed,
	});

	it('says the markup is not set and which breaks have no cost', () => {
		const kept = detail({
			enhancerBreaks: [
				broke('markupNotSet', 0.2),
				broke('ttUnknown', null),
				broke('ttUnknown', null),
			],
		} as Partial<SessionDetailType>);
		kept.summary.costBreakdown.enhancerCost = 0.2;
		render(SessionDetail, { props: { detail: kept } });
		expect(screen.getByText('0.20')).toBeTruthy();
		expect(screen.getByText('; markup not set; 2 breaks unpriced: TT unknown')).toBeTruthy();
	});

	it('says why in place of a figure when no break was priced', () => {
		render(SessionDetail, {
			props: {
				detail: detail({
					enhancerBreaks: [broke('returnAboveTt', null)],
				} as Partial<SessionDetailType>),
			},
		});
		const note = screen.getByText('1 break unpriced: return above TT');
		expect(note.parentElement?.textContent?.trim()).toBe(
			'Enhancers: 1 break unpriced: return above TT',
		);
		expect(screen.queryByText('0.00')).toBeNull();
	});

	it('adds nothing beside a break priced in full', () => {
		const kept = detail({ enhancerBreaks: [broke('priced', 0.7)] } as Partial<SessionDetailType>);
		kept.summary.costBreakdown.enhancerCost = 0.7;
		render(SessionDetail, { props: { detail: kept } });
		expect(screen.getByText('0.70')).toBeTruthy();
		expect(screen.queryByText(/markup not set|unpriced/)).toBeNull();
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
	const exact = (ped: number) => ({ ped, precision: 'exact' as const });
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
		gained: [
			{
				item: { name: 'Animal Oil Residue', place: { kind: 'carried' } },
				role: 'material',
				stacking: 'stack',
				mark: 'arrived',
				units: 1150,
				amount: exact(11.5),
			},
		],
		gainedTotal: exact(11.5),
		gainedPossiblyStrongbox: exact(0),
		events: [],
		totals: {
			ammo: { kind: 'settled', burned: exact(0) },
			weaponDecay: exact(0),
			healingDecay: exact(0),
			armourDecay: exact(0),
			otherEquipmentDecay: exact(0),
			otherConsumption: exact(0),
			unexplainedDepartures: exact(0),
			repairCost: exact(0),
			repairCostDerived: exact(0),
			repairsUncosted: 0,
			gained: exact(11.5),
			gainedPossiblyStrongbox: exact(0),
		},
	} as const;

	it('adds nothing to the detail of a session the account never read', async () => {
		render(SessionDetail, { props: { detail: detail() } });
		await waitFor(() => expect(account.getSessionMeasure).toHaveBeenCalledWith('s1'));
		// The absent answer itself has landed before asserting nothing shows.
		await account.getSessionMeasure.mock.results[0].value;
		await new Promise((settle) => setTimeout(settle, 0));
		expect(screen.queryByText('Your account readings')).toBeNull();
		expect(screen.queryByTestId('account-measure')).toBeNull();
	});

	it('says where the measured costs came from, after the cost summary, with no tracked figure beside them', async () => {
		account.getSessionMeasure.mockResolvedValue(measure);
		const measuredSession = detail();
		measuredSession.summary.costBasis = 'measured';
		measuredSession.summary.costBreakdown.basis.weaponCost = 'measured';
		render(SessionDetail, { props: { detail: measuredSession } });
		const block = await screen.findByTestId('account-measure');
		expect(block.textContent).toContain('Costs from your account');
		expect(within(block).getByText('Complete: the readings cover the whole session.')).toBeTruthy();
		expect(within(block).queryByRole('table')).toBeNull();
		expect(within(block).queryByText('Tracked estimate')).toBeNull();
		// The tracked summary itself is unchanged.
		expect(screen.getByText('Loot TT').closest('div')?.parentElement?.textContent).toContain(
			'12.00',
		);
	});

	it("heads the readings neutrally when the session's costs do not rest on them", async () => {
		account.getSessionMeasure.mockResolvedValue(measure);
		render(SessionDetail, { props: { detail: detail() } });
		const block = await screen.findByTestId('account-measure');
		expect(block.textContent).toContain('Your account readings');
		expect(block.textContent).not.toContain('Costs from your account');
	});

	it("takes loot the readings never saw arrive out of the session through the session's own loot correction", async () => {
		account.getSessionMeasure.mockResolvedValue(measure);
		account.getSessionMinutes.mockResolvedValue(
			minuteSeries({
				steps: [minuteStep(0), minuteStep(1, { lootedNeverArrived: [missingLoot('Shrapnel', 1)] })],
			}),
		);
		vi.mocked(api.deactivateLootItem).mockResolvedValue({
			sessionId: 's1',
			itemName: 'Shrapnel',
			affectedRows: 3,
			totalValueDelta: -0.3,
			sessionTotalReturns: 11.7,
		});
		vi.mocked(api.getSessionDetail).mockResolvedValue(detail());
		render(SessionDetail, { props: { detail: detail() } });
		await fireEvent.click(await screen.findByRole('button', { name: /^Minute by minute/ }));
		await fireEvent.click(
			screen.getByRole('button', { name: "Remove Shrapnel from this session's loot" }),
		);
		await fireEvent.click(screen.getByRole('button', { name: 'Yes' }));
		await waitFor(() => expect(api.deactivateLootItem).toHaveBeenCalledWith('s1', 'Shrapnel'));
		// The session's figures are the backend's own again, and so are its minutes.
		await waitFor(() => expect(api.getSessionDetail).toHaveBeenCalledWith('s1'));
		await waitFor(() => expect(account.getSessionMinutes.mock.calls.length).toBeGreaterThan(1));
	});

	it('reads the minutes again after loot is archived from the loot breakdown', async () => {
		account.getSessionMeasure.mockResolvedValue(measure);
		account.getSessionMinutes.mockResolvedValue(
			minuteSeries({ steps: [minuteStep(0), minuteStep(1)] }),
		);
		vi.mocked(api.deactivateLootItem).mockResolvedValue({
			sessionId: 's1',
			itemName: 'Shrapnel',
			affectedRows: 1,
			totalValueDelta: -1,
			sessionTotalReturns: 11,
		});
		vi.mocked(api.getSessionDetail).mockResolvedValue(detail());
		const looted = detail({ lootBreakdown: [{ name: 'Shrapnel', quantity: 100, ttValue: 1 }] });
		render(SessionDetail, { props: { detail: looted } });
		await screen.findByRole('button', { name: /^Minute by minute/ });
		expect(account.getSessionMinutes).toHaveBeenCalledTimes(1);
		await fireEvent.click(screen.getByRole('button', { name: 'Archive all Shrapnel entries' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Yes' }));
		await waitFor(() => expect(account.getSessionMinutes).toHaveBeenCalledTimes(2));
	});

	it('is not read at all in a build that hides surfaces in development', async () => {
		channel.visible = false;
		render(SessionDetail, { props: { detail: detail() } });
		await waitFor(() => expect(api.getMobDetections).toHaveBeenCalled());
		expect(account.getSessionMeasure).not.toHaveBeenCalled();
	});
});

describe('what each cost rests on', () => {
	type Basis = SessionDetailType['summary']['costBreakdown']['basis'];

	/** A session's detail with its cost split: what each bucket shows, what
	 * the chat log tracked, and what each rests on. */
	function costed(
		costBasis: SessionDetailType['summary']['costBasis'],
		shown: { weaponCost: number; healCost: number; armourCost: number },
		tracked: { weaponCost: number; healCost: number; armourCost: number },
		basis: Partial<Basis>,
	): SessionDetailType {
		const base = detail();
		const breakdown = base.summary.costBreakdown;
		return detail({
			summary: {
				...base.summary,
				costBasis,
				costBreakdown: {
					...breakdown,
					...shown,
					tracked: { ...breakdown.tracked, ...tracked },
					basis: { ...breakdown.basis, ...basis },
				},
			},
		});
	}

	it('says once that a session measured throughout is all from the account, with no mark per bucket', () => {
		render(SessionDetail, {
			props: {
				detail: costed(
					'measured',
					{ weaponCost: 10.6, healCost: 1, armourCost: 0 },
					{ weaponCost: 10.25, healCost: 1.005, armourCost: 0 },
					{ weaponCost: 'measured', healCost: 'measured' },
				),
			},
		});
		const split = screen.getByTestId('cost-breakdown');
		expect(within(split).getByTestId('cost-basis-line').textContent?.trim()).toBe(
			'All from your account',
		);
		expect(within(split).queryByTestId('basis-weaponCost')).toBeNull();
		expect(within(split).queryByTestId('basis-healCost')).toBeNull();
		// The estimate is kept where it differs, and only there.
		expect(within(split).getByTestId('estimate-weaponCost').textContent).toBe(
			'; tracked estimate was 10.25',
		);
		expect(within(split).queryByTestId('estimate-healCost')).toBeNull();
	});

	it('marks each bucket of a session that mixes bases, with its meaning reachable by keyboard', () => {
		render(SessionDetail, {
			props: {
				detail: costed(
					'mixed',
					{ weaponCost: 10.6, healCost: 1, armourCost: 0.75 },
					{ weaponCost: 10.25, healCost: 1, armourCost: 0.5 },
					{ weaponCost: 'measured', healCost: 'tracked', armourCost: 'mixed' },
				),
			},
		});
		const split = screen.getByTestId('cost-breakdown');
		expect(within(split).getByTestId('basis-weaponCost').textContent?.trim()).toBe(
			'from your account',
		);
		expect(within(split).getByTestId('basis-healCost').textContent?.trim()).toBe('tracked');
		expect(within(split).getByTestId('basis-armourCost').textContent?.trim()).toBe(
			'partly from your account',
		);
		expect(within(split).queryByTestId('cost-basis-line')).toBeNull();
		// A tracked bucket has no estimate to set beside itself.
		expect(within(split).queryByTestId('estimate-healCost')).toBeNull();
		expect(within(split).getByTestId('estimate-armourCost').textContent).toBe(
			'; tracked estimate was 0.50',
		);
		const mark = within(split).getByRole('button', { name: 'tracked' });
		const meaning = document.getElementById(mark.getAttribute('aria-describedby') ?? '');
		expect(meaning?.textContent).toContain('Tracked by the app');
	});

	it('shows a session the readings never covered exactly as before', () => {
		render(SessionDetail, {
			props: {
				detail: costed(
					'tracked',
					{ weaponCost: 10, healCost: 1, armourCost: 0 },
					{ weaponCost: 10, healCost: 1, armourCost: 0 },
					{},
				),
			},
		});
		const split = screen.getByTestId('cost-breakdown');
		expect(split.textContent).toMatch(/Weapon:\s*10\.00\s*Healing:\s*1\.00\s*$/);
		expect(within(split).queryByText(/your account/)).toBeNull();
		expect(within(split).queryByText(/tracked/)).toBeNull();
	});

	it('shows the split of a measured session whose only cost is its weapon, but not of a tracked one', () => {
		// The tracked weapon counts every shot fired, those after the last
		// kill too, as the measured one does: the two agree, so no estimate.
		const { unmount } = render(SessionDetail, {
			props: {
				detail: costed(
					'measured',
					{ weaponCost: 10.25, healCost: 0, armourCost: 0 },
					{ weaponCost: 10.25, healCost: 0, armourCost: 0 },
					{ weaponCost: 'measured' },
				),
			},
		});
		expect(screen.getByTestId('cost-breakdown').textContent).toContain('All from your account');
		expect(screen.queryByTestId('estimate-weaponCost')).toBeNull();
		unmount();
		render(SessionDetail, { props: { detail: detail() } });
		expect(screen.queryByTestId('cost-breakdown')).toBeNull();
	});

	it('shows a bucket measured at nothing against a tracked estimate of something', () => {
		render(SessionDetail, {
			props: {
				detail: costed(
					'mixed',
					{ weaponCost: 10, healCost: 0, armourCost: 0 },
					{ weaponCost: 10, healCost: 1.5, armourCost: 0 },
					{ healCost: 'measured' },
				),
			},
		});
		const split = screen.getByTestId('cost-breakdown');
		expect(split.textContent).toMatch(/Healing:\s*0\.00/);
		expect(within(split).getByTestId('estimate-healCost').textContent).toBe(
			'; tracked estimate was 1.50',
		);
	});

	describe('unpriced shots beside the weapon', () => {
		/** The session's detail with two shots no weapon priced. */
		function unpriced(session: SessionDetailType): SessionDetailType {
			return {
				...session,
				weaponAttribution: {
					correctable: true,
					agreed: null,
					evidenced: null,
					evidenceShots: 0,
					unresolved: 0,
					unpriced: 2,
					unpricedCostBound: 0.4,
					effects: [],
					reviews: [],
				} as unknown as SessionDetailType['weaponAttribution'],
			};
		}

		it('qualifies a tracked weapon figure', () => {
			render(SessionDetail, { props: { detail: unpriced(detail()) } });
			const split = screen.getByTestId('cost-breakdown');
			expect(within(split).getByTestId('weapon-unpriced').textContent).toBe(
				'; 2 shots unpriced, up to +0.40 PED',
			);
		});

		it('is dropped beside a measured weapon figure, which holds every shot', () => {
			render(SessionDetail, {
				props: {
					detail: unpriced(
						costed(
							'measured',
							{ weaponCost: 10, healCost: 0, armourCost: 0 },
							{ weaponCost: 10, healCost: 0, armourCost: 0 },
							{ weaponCost: 'measured' },
						),
					),
				},
			});
			const split = screen.getByTestId('cost-breakdown');
			expect(within(split).queryByTestId('weapon-unpriced')).toBeNull();
		});

		it('rides the tracked estimate of a measured weapon, when that estimate shows', () => {
			render(SessionDetail, {
				props: {
					detail: unpriced(
						costed(
							'measured',
							{ weaponCost: 10.6, healCost: 0, armourCost: 0 },
							{ weaponCost: 10.25, healCost: 0, armourCost: 0 },
							{ weaponCost: 'measured' },
						),
					),
				},
			});
			const split = screen.getByTestId('cost-breakdown');
			const estimate = within(split).getByTestId('estimate-weaponCost');
			const warning = within(split).getByTestId('weapon-unpriced');
			expect(estimate.nextElementSibling).toBe(warning);
			expect(split.textContent).toMatch(
				/Weapon:\s*10\.60; tracked estimate was 10\.25; 2 shots unpriced, up to \+0\.40 PED/,
			);
		});
	});
});

/** A session's detail whose cost split rests on the readings, with the
 * given split. */
function measuredSession(
	breakdown: Partial<SessionDetailType['summary']['costBreakdown']>,
	summary: Partial<SessionDetailType['summary']> = {},
): SessionDetailType {
	const base = detail();
	const split = base.summary.costBreakdown;
	return detail({
		summary: {
			...base.summary,
			costBasis: 'measured',
			...summary,
			costBreakdown: {
				...split,
				basis: {
					weaponCost: 'measured',
					healCost: 'measured',
					consumableCost: 'measured',
					enhancerCost: 'measured',
					armourCost: 'measured',
					harvestCost: 'measured',
				},
				reasons: {
					weaponCost: null,
					healCost: null,
					consumableCost: null,
					enhancerCost: null,
					armourCost: null,
					harvestCost: null,
				},
				...breakdown,
			},
		},
	});
}

type CostLine = NonNullable<
	SessionDetailType['summary']['costBreakdown']['measured']
>['buckets'][number]['lines'][number];

function costLine(overrides: Partial<CostLine>): CostLine {
	return {
		kind: 'decay',
		item: 'Item',
		units: null,
		amount: 0.1,
		precision: 'exact',
		markupPercent: null,
		markupNotIncluded: false,
		unpriced: null,
		peggedLedgerEntryId: null,
		peggedLedgerEntryDescription: null,
		...overrides,
	};
}

describe('what the readings could not measure, and the gap they were read across', () => {
	it('gives the reason a bucket was tracked on hover or focus, in a session that has readings', () => {
		const session = measuredSession(
			{
				healCost: 1,
				tracked: { ...detail().summary.costBreakdown.tracked, healCost: 1 },
				basis: {
					...measuredSession({}).summary.costBreakdown.basis,
					healCost: 'tracked',
				},
				reasons: {
					...measuredSession({}).summary.costBreakdown.reasons,
					healCost: 'gap',
				},
			},
			{ costBasis: 'mixed' },
		);
		render(SessionDetail, { props: { detail: session } });
		const split = screen.getByTestId('cost-breakdown');
		const mark = within(split).getByRole('button', { name: 'tracked' });
		const tip = document.getElementById(mark.getAttribute('aria-describedby') ?? '');
		expect(tip?.textContent).toContain('Tracked by the app');
		expect(tip?.textContent).toContain(
			'Two of the readings in this session are too far apart to measure it.',
		);
	});

	it('marks the tracked buckets of a session the readings covered only in part', () => {
		const base = detail();
		const split = base.summary.costBreakdown;
		const session = detail({
			summary: {
				...base.summary,
				costBreakdown: {
					...split,
					reasons: { ...split.reasons, weaponCost: 'partialCoverage' },
				},
			},
		});
		render(SessionDetail, { props: { detail: session } });
		const shown = screen.getByTestId('cost-breakdown');
		expect(within(shown).getByTestId('basis-weaponCost').textContent?.trim()).toBe('tracked');
		const mark = within(shown).getByRole('button', { name: 'tracked' });
		const tip = document.getElementById(mark.getAttribute('aria-describedby') ?? '');
		expect(tip?.textContent).toContain('The readings missed the start or the end of this session.');
	});

	it('says once when the costs were measured across a gap the chat log shows clean', async () => {
		account.getSessionMeasure.mockResolvedValue({
			sessionId: 's1',
			sealed: true,
			coverage: {
				level: 'partial',
				opening: 'read',
				closing: 'unread',
				readingCount: 9,
				gapCount: 0,
			},
			ammo: [],
			decay: [],
			repairs: [],
			otherConsumption: [],
			unexplainedDepartures: [],
			gained: [],
			gainedTotal: { ped: 0, precision: 'exact' },
			gainedPossiblyStrongbox: { ped: 0, precision: 'exact' },
			events: [],
			totals: {},
		});
		render(SessionDetail, {
			props: { detail: measuredSession({ measuredAcrossGapMinutes: 9 }) },
		});
		expect(screen.getByTestId('cost-gap-note').textContent).toBe(
			'Measured across a gap of 9 minutes; the chat log shows no other play in it',
		);
		const coverage = await screen.findByTestId('measure-coverage');
		expect(coverage.textContent).not.toMatch(/missed/);
		expect(screen.getAllByText(/Measured across a gap of 9 minutes/)).toHaveLength(1);
	});

	it('sets no tracked estimate beside measured armour whose hits no recording covers', () => {
		const tracked = { ...detail().summary.costBreakdown.tracked, armourCost: 0.5 };
		const { unmount } = render(SessionDetail, {
			props: {
				detail: measuredSession({ armourCost: 0.75, tracked, armourUnrecorded: true }),
			},
		});
		const split = screen.getByTestId('cost-breakdown');
		expect(split.textContent).toMatch(/Armour:\s*0\.75/);
		expect(within(split).queryByTestId('estimate-armourCost')).toBeNull();
		unmount();
		render(SessionDetail, {
			props: { detail: measuredSession({ armourCost: 0.75, tracked, armourUnrecorded: false }) },
		});
		expect(screen.getByTestId('estimate-armourCost').textContent).toBe(
			'; tracked estimate was 0.50',
		);
	});
});

describe("the session's overhead", () => {
	/** A measured session: Loot TT 12.00, Cycled 10.00, and 0.55 of
	 * overhead, so a Net of 1.45. */
	function withOverhead(
		buckets: NonNullable<SessionDetailType['summary']['costBreakdown']['measured']>['buckets'] = [
			{ bucket: 'weapon', lines: [costLine({ item: 'Rifle', amount: 10 })] },
			{
				bucket: 'travel',
				lines: [
					costLine({
						kind: 'consumed',
						item: 'Mind Essence',
						units: 40,
						amount: 0.4,
						markupNotIncluded: true,
						peggedLedgerEntryId: 'entry-1',
						peggedLedgerEntryDescription: 'Fuel',
					}),
				],
			},
			{ bucket: 'unclassified', lines: [costLine({ item: 'Odd Thing', amount: 0.15 })] },
		],
		overhead = { travel: 0.4, fieldLogistics: 0, unclassified: 0.15, reasons: NO_OVERHEAD_REASONS },
	): SessionDetailType {
		return measuredSession(
			{ overheadCost: 0.55, overhead, measured: { buckets, armourParts: [] } },
			{ net: 1.45 },
		);
	}

	/** The same session once Odd Thing is placed as travel. */
	function placedAsTravel(): SessionDetailType {
		return withOverhead(
			[
				{ bucket: 'weapon', lines: [costLine({ item: 'Rifle', amount: 10 })] },
				{
					bucket: 'travel',
					lines: [
						costLine({ kind: 'consumed', item: 'Mind Essence', units: 40, amount: 0.4 }),
						costLine({ item: 'Odd Thing', amount: 0.15 }),
					],
				},
			],
			{ travel: 0.55, fieldLogistics: 0, unclassified: 0, reasons: NO_OVERHEAD_REASONS },
		);
	}

	it('is absent from a session without any', async () => {
		render(SessionDetail, { props: { detail: measuredSession({}) } });
		expect(screen.queryByTestId('session-overhead')).toBeNull();
		render(SessionDetail, { props: { detail: detail() } });
		expect(screen.queryByTestId('session-overhead')).toBeNull();
	});

	it('lists each category with its amount, opening onto the items behind it', async () => {
		render(SessionDetail, { props: { detail: withOverhead() } });
		const group = screen.getByTestId('session-overhead');
		expect(group.textContent).toMatch(/Overhead\s*0\.55/);
		expect(within(group).queryByTestId('overhead-fieldLogistics')).toBeNull();

		const travel = within(group).getByTestId('overhead-travel');
		const toggle = within(travel).getByRole('button', { name: /Travel/ });
		expect(toggle.textContent).toMatch(/Travel:\s*0\.40/);
		expect(toggle.getAttribute('aria-expanded')).toBe('false');
		expect(travel.textContent).not.toContain('Mind Essence');
		await fireEvent.click(toggle);
		expect(toggle.getAttribute('aria-expanded')).toBe('true');
		expect(travel.textContent).toContain('Mind Essence');
		expect(travel.textContent).toContain('40 units used; markup not included');
		expect(within(travel).getByTestId('cost-line-ledger').textContent?.trim()).toBe(
			'You also recorded this in the ledger: Fuel',
		);

		// What the app could not place is open from the start, offering Place.
		const unclassified = within(group).getByTestId('overhead-unclassified');
		expect(unclassified.textContent).toContain('Odd Thing');
		expect(within(unclassified).getByRole('button', { name: 'Place Odd Thing' })).toBeTruthy();

		// The Net's own arithmetic, from the backend's figures.
		// The Net's parts, listed: never claimed to sum exactly.
		expect(within(group).getByTestId('overhead-arithmetic').textContent).toMatch(
			/Net\s*1\.45:\s*Loot TT\s*12\.00,\s*Cycled\s*10\.00,\s*Overhead\s*0\.55/,
		);
	});

	it('places an item once, then reads the session again with the item in its bucket', async () => {
		let answer!: (value: unknown) => void;
		account.placeCostItem.mockReturnValue(
			new Promise((settle) => {
				answer = settle;
			}),
		);
		vi.mocked(api.getSessionDetail).mockResolvedValue(placedAsTravel());
		render(SessionDetail, { props: { detail: withOverhead() } });
		await waitFor(() => expect(account.getCostPlacements).toHaveBeenCalled());

		const trigger = screen.getByRole('button', { name: 'Place Odd Thing' });
		await fireEvent.click(trigger);
		expect(screen.getByText('Remembered for every session')).toBeTruthy();
		const travel = screen
			.getAllByRole('menuitemradio')
			.find((option) => option.textContent?.trim() === 'Travel');
		if (!travel) throw new Error('no Travel choice');
		await fireEvent.click(travel);

		expect(account.placeCostItem).toHaveBeenCalledWith({ item: 'Odd Thing', category: 'travel' });
		// Pending: the trigger says so and takes no second press.
		await waitFor(() => expect(trigger.textContent).toContain('Placing'));
		expect(trigger.hasAttribute('disabled')).toBe(true);
		expect(api.getSessionDetail).not.toHaveBeenCalled();

		answer({
			placements: {
				awaiting: [],
				placed: [{ item: 'Odd Thing', category: 'travel', placedAt: '2026-11-16T20:00:00Z' }],
			},
			pendingSessions: [],
		});
		await waitFor(() => expect(api.getSessionDetail).toHaveBeenCalledWith('s1'));
		const group = screen.getByTestId('session-overhead');
		await waitFor(() => expect(within(group).queryByTestId('overhead-unclassified')).toBeNull());
		const travelGroup = within(group).getByTestId('overhead-travel');
		expect(travelGroup.textContent).toContain('Odd Thing');
		// Focus returns to the item's own trigger, which now reads its placement.
		await waitFor(() =>
			expect((document.activeElement as HTMLElement | null)?.dataset.placeItem).toBe('Odd Thing'),
		);
		expect(document.activeElement?.textContent).toContain('Travel');
		expect(screen.getByTestId('placement-status').textContent?.trim()).toBe('Placed as Travel');
		// The placement's own answer stands: reading the session again does
		// not read the placements again over it.
		expect(account.getCostPlacements).toHaveBeenCalledTimes(1);
	});

	it('takes every Place menu out of reach while one placement is saving', async () => {
		account.placeCostItem.mockReturnValue(new Promise(() => {}));
		const session = withOverhead([
			{
				bucket: 'unclassified',
				lines: [
					costLine({ item: 'Odd Thing', amount: 0.1 }),
					costLine({ item: 'Other Thing', amount: 0.05 }),
				],
			},
		]);
		render(SessionDetail, { props: { detail: session } });
		await fireEvent.click(screen.getByRole('button', { name: 'Place Odd Thing' }));
		const weapon = screen
			.getAllByRole('menuitemradio')
			.find((option) => option.textContent?.trim() === 'Weapon');
		if (!weapon) throw new Error('no Weapon choice');
		await fireEvent.click(weapon);
		const saving = screen.getByRole('button', { name: 'Place Odd Thing' });
		await waitFor(() => expect(saving.textContent).toContain('Placing'));
		const other = screen.getByRole('button', { name: 'Place Other Thing' });
		expect(other.hasAttribute('disabled')).toBe(true);
		expect(other.textContent).not.toContain('Placing');
	});

	it('returns focus to the cost section when the item leaves the overhead, and says where it went', async () => {
		account.placeCostItem.mockResolvedValue({
			placements: {
				awaiting: [],
				placed: [{ item: 'Odd Thing', category: 'weapon', placedAt: '2026-11-16T20:00:00Z' }],
			},
			pendingSessions: [],
		});
		// Placed as a weapon, the item leaves the overhead for Cycled.
		const placed = measuredSession(
			{
				overheadCost: 0,
				measured: {
					buckets: [
						{
							bucket: 'weapon',
							lines: [costLine({ item: 'Rifle', amount: 10 }), costLine({ item: 'Odd Thing' })],
						},
					],
					armourParts: [],
				},
			},
			{ net: 2 },
		);
		vi.mocked(api.getSessionDetail).mockResolvedValue(placed);
		render(SessionDetail, {
			props: {
				detail: withOverhead(
					[{ bucket: 'unclassified', lines: [costLine({ item: 'Odd Thing', amount: 0.15 })] }],
					{ travel: 0, fieldLogistics: 0, unclassified: 0.15, reasons: NO_OVERHEAD_REASONS },
				),
			},
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Place Odd Thing' }));
		const weapon = screen
			.getAllByRole('menuitemradio')
			.find((option) => option.textContent?.trim() === 'Weapon');
		if (!weapon) throw new Error('no Weapon choice');
		await fireEvent.click(weapon);
		await waitFor(() => expect(screen.queryByTestId('session-overhead')).toBeNull());
		await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('cost-section')));
		expect(screen.getByTestId('placement-status').textContent?.trim()).toBe('Placed as Weapon');
	});

	it('says a category that bills nothing has items to place, rather than reading 0.00', () => {
		const session = measuredSession({
			overheadCost: 0,
			overhead: { travel: 0, fieldLogistics: 0, unclassified: 0, reasons: NO_OVERHEAD_REASONS },
			measured: {
				buckets: [
					{
						bucket: 'unclassified',
						lines: [
							costLine({
								kind: 'consumed',
								item: 'Strange Stack',
								units: 40,
								amount: null,
								unpriced: 'notPlaced',
							}),
						],
					},
				],
				armourParts: [],
			},
		});
		render(SessionDetail, { props: { detail: session } });
		const group = screen.getByTestId('session-overhead');
		expect(within(group).getByTestId('overhead-nothing-billed').textContent?.trim()).toBe(
			'Nothing billed; items to place',
		);
		expect(within(group).getByTestId('overhead-unclassified').textContent).toContain(
			'nothing billed; items to place',
		);
		expect(group.textContent).not.toContain('0.00');
		expect(within(group).queryByTestId('overhead-arithmetic')).toBeNull();
	});

	it('says why a category is withheld, on hover or focus, rather than reading 0.00', () => {
		const session = measuredSession({
			overheadCost: 0.4,
			overhead: {
				travel: 0.4,
				fieldLogistics: 0,
				unclassified: 0,
				reasons: { travel: null, fieldLogistics: 'overheadInGap', unclassified: null },
			},
			measured: {
				buckets: [{ bucket: 'travel', lines: [costLine({ item: 'Mind Essence', amount: 0.4 })] }],
				armourParts: [],
			},
		});
		render(SessionDetail, { props: { detail: session } });
		const logistics = screen.getByTestId('overhead-fieldLogistics');
		expect(logistics.textContent).not.toContain('0.00');
		const mark = within(logistics).getByRole('button', { name: 'not billed' });
		const tip = document.getElementById(mark.getAttribute('aria-describedby') ?? '');
		expect(tip?.textContent).toContain('used up in that gap, possibly after the session');
		// The measured category keeps its figure.
		expect(screen.getByTestId('overhead-travel').textContent).toMatch(/Travel:\s*0\.40/);
	});

	it('says why when the placements cannot be read', async () => {
		account.getCostPlacements.mockRejectedValue(new Error('the placements could not be read'));
		render(SessionDetail, { props: { detail: withOverhead() } });
		expect(await screen.findByText('the placements could not be read')).toBeTruthy();
	});

	it('says why when placing fails, and leaves the figures as they were', async () => {
		account.placeCostItem.mockRejectedValue(new Error('That item is not one the readings hold'));
		render(SessionDetail, { props: { detail: withOverhead() } });
		await fireEvent.click(screen.getByRole('button', { name: 'Place Odd Thing' }));
		const notACost = screen
			.getAllByRole('menuitemradio')
			.find((option) => option.textContent?.trim() === 'Not a cost');
		if (!notACost) throw new Error('no Not a cost choice');
		await fireEvent.click(notACost);
		expect(account.placeCostItem).toHaveBeenCalledWith({
			item: 'Odd Thing',
			category: 'notACost',
		});
		expect((await screen.findByRole('alert')).textContent).toContain(
			'That item is not one the readings hold',
		);
		expect(api.getSessionDetail).not.toHaveBeenCalled();
		expect(screen.getByTestId('overhead-unclassified').textContent).toContain('Odd Thing');
	});

	it('shows where a placed item was placed, and clears it on request', async () => {
		account.getCostPlacements.mockResolvedValue({
			awaiting: [],
			placed: [{ item: 'Mind Essence', category: 'travel', placedAt: '2026-11-16T20:00:00Z' }],
		});
		account.clearCostPlacement.mockResolvedValue({
			placements: { awaiting: [], placed: [] },
			pendingSessions: [],
		});
		vi.mocked(api.getSessionDetail).mockResolvedValue(withOverhead());
		render(SessionDetail, { props: { detail: withOverhead() } });
		const travel = screen.getByTestId('overhead-travel');
		await fireEvent.click(within(travel).getByRole('button', { name: /Travel/ }));
		const placed = await within(travel).findByRole('button', {
			name: 'Mind Essence: placed as Travel; change or clear',
		});
		await fireEvent.click(placed);
		const checked = screen
			.getAllByRole('menuitemradio')
			.filter((option) => option.getAttribute('aria-checked') === 'true');
		expect(checked.map((option) => option.textContent?.trim())).toEqual(['Travel']);
		await fireEvent.click(screen.getByRole('menuitem', { name: 'Clear placement' }));
		expect(account.clearCostPlacement).toHaveBeenCalledWith('Mind Essence');
		await waitFor(() => expect(api.getSessionDetail).toHaveBeenCalledWith('s1'));
	});
});

describe('a loot toggle', () => {
	/** The Net the summary shows. */
	function netShown(): string {
		return screen.getByText('Net', { selector: 'span' }).nextElementSibling?.textContent ?? '';
	}

	function withLoot(): SessionDetailType {
		return detail({ lootBreakdown: [{ name: 'Shrapnel', quantity: 100, ttValue: 1 }] });
	}

	async function archiveShrapnel() {
		await fireEvent.click(screen.getByRole('button', { name: 'Archive all Shrapnel entries' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Yes' }));
	}

	/** The session as the backend reads it after the toggle. */
	function afterToggle(net: number): SessionDetailType {
		const base = detail();
		return detail({ summary: { ...base.summary, returns: 11, net, returnRate: 1.1 } });
	}

	it("reads the session again and shows the backend's own Net", async () => {
		vi.mocked(api.deactivateLootItem).mockResolvedValue({
			sessionId: 's1',
			itemName: 'Shrapnel',
			affectedRows: 1,
			totalValueDelta: -1,
			sessionTotalReturns: 11,
		});
		vi.mocked(api.getSessionDetail).mockResolvedValue(afterToggle(0.45));
		render(SessionDetail, { props: { detail: withLoot() } });
		await archiveShrapnel();
		await waitFor(() => expect(api.getSessionDetail).toHaveBeenCalledWith('s1'));
		await waitFor(() => expect(netShown()).toMatch(/\+0\.45/));
	});

	it('works the Net out from the parts, rounded once, only when the session cannot be read', async () => {
		vi.mocked(api.deactivateLootItem).mockResolvedValue({
			sessionId: 's1',
			itemName: 'Shrapnel',
			affectedRows: 1,
			totalValueDelta: -1,
			sessionTotalReturns: 12.005,
		});
		vi.mocked(api.getSessionDetail).mockRejectedValue(new Error('offline'));
		const session = withLoot();
		session.summary.costBreakdown.overheadCost = 0.004;
		render(SessionDetail, { props: { detail: session } });
		await archiveShrapnel();
		// 12.005 less 10 less 0.004, rounded once: 2.00, never 2.01.
		await waitFor(() => expect(netShown()).toMatch(/\+2\.00/));
	});

	it('never lets an older, slower read overwrite a newer one', async () => {
		let answerOlder!: (value: SessionDetailType) => void;
		vi.mocked(api.getSessionDetail)
			.mockReturnValueOnce(
				new Promise((settle) => {
					answerOlder = settle;
				}),
			)
			.mockResolvedValueOnce(afterToggle(0.45));
		vi.mocked(api.deactivateLootItem).mockResolvedValue({
			sessionId: 's1',
			itemName: 'Shrapnel',
			affectedRows: 1,
			totalValueDelta: -1,
			sessionTotalReturns: 11,
		});
		render(SessionDetail, { props: { detail: withLoot() } });
		// An armour recording elsewhere starts a read that answers late.
		await waitFor(() => expect(protectionListeners.length).toBeGreaterThan(0));
		protectionListeners[0]();
		await archiveShrapnel();
		await waitFor(() => expect(netShown()).toMatch(/\+0\.45/));
		answerOlder(afterToggle(-3));
		await new Promise((settle) => setTimeout(settle, 0));
		expect(netShown()).toMatch(/\+0\.45/);
	});
});
