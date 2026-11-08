import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	AnalyticsHuntingActivity,
	ExpectedHuntingEconomics,
	HuntingActivityComparison,
	HuntingDefinitionComparison,
	HuntingSpeciesComparison,
	MarketHarvestItem,
} from '$lib/api/commands.gen';
import { createHuntingModel } from './huntingModel.svelte';

vi.mock('$lib/api', () => ({
	getAnalyticsHuntingActivity: vi.fn(),
	getMarketHuntMarkups: vi.fn(),
	getHuntingRealisedMarkup: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function expectedEconomics(over: Partial<ExpectedHuntingEconomics> = {}): ExpectedHuntingEconomics {
	return {
		modelVersion: 'community_v1',
		looterSource: 'three_looter_mean',
		looterLevel: 50,
		expectedLootTt: 940,
		modelledRawTt: 1000,
		eligibleOffensiveCost: 1000,
		offensiveTtRecovery: 0.94,
		expectedTtRate: 0.94,
		effectiveEfficiency: { status: 'within_model_range', efficiencyPct: 64.29 },
		breakEvenLootMarkup: 1 / 0.94,
		coverage: 1,
		incomplete: false,
		missingBasisPhases: 0,
		...over,
	};
}

function required<T>(value: T | null | undefined, label: string): T {
	if (value == null) throw new Error(`Expected ${label}`);
	return value;
}

function definition(over: Partial<HuntingDefinitionComparison> = {}): HuntingDefinitionComparison {
	return {
		definitionId: 1,
		name: 'ARIS Dailies',
		isArchived: false,
		cycled: 1200,
		returns: 1080,
		lootRate: 0.9,
		expected: null,
		lootItems: [{ itemName: 'Animal Muscle Oil', quantity: 400, valuePed: 120 }],
		activities: [],
		...over,
	};
}

function species(over: Partial<HuntingSpeciesComparison> = {}): HuntingSpeciesComparison {
	return {
		mobSpecies: 'Atrox',
		cycled: 900,
		returns: 810,
		lootRate: 0.9,
		expected: null,
		lootItems: [{ itemName: 'Animal Muscle Oil', quantity: 400, valuePed: 120 }],
		...over,
	};
}

function sessionActivity(over: Partial<HuntingActivityComparison> = {}): HuntingActivityComparison {
	return {
		kind: 'quest',
		label: 'Daily Hunting 1',
		cycled: 100,
		returns: 90,
		lootRate: 0.9,
		expected: null,
		confirmedRewardPed: 15,
		realisedRewardMarkup: 0,
		rewardItems: [{ itemName: 'Animal Muscle Oil', quantity: 50, valuePed: 15 }],
		rewardedReturns: 105,
		rewardedRate: 1.05,
		rewardStatus: 'fixed_liquid',
		lootItems: [{ itemName: 'Animal Muscle Oil', quantity: 40, valuePed: 12 }],
		variants: [],
		...over,
	};
}

function activity(over: Partial<AnalyticsHuntingActivity> = {}): AnalyticsHuntingActivity {
	return {
		overall: { cycled: 1500, returns: 1350, lootRate: 0.9, expected: null },
		definitions: [
			definition(),
			definition({ definitionId: null, name: 'Unassigned', cycled: 300, returns: 270 }),
		],
		species: [species(), species({ mobSpecies: '', cycled: 600, returns: 540, lootItems: [] })],
		...over,
	};
}

function obs(
	name: string,
	markupPct: number | null,
	horizon: string | null,
	salesPed: number | null,
	recommendedPacketTt: number | null = null,
): MarketHarvestItem {
	const readings = ['day', 'week', 'month', 'year'].map((h) => ({
		horizon: h,
		markupPct: h === horizon ? markupPct : null,
		salesPed: h === horizon ? (salesPed ?? 0) : 0,
	}));
	return {
		itemName: name,
		markupPct,
		unitPricePed: null,
		marketObservedAt: null,
		manual: null,
		preferred: markupPct === null ? null : 'market',
		nanocubeMarked: false,
		horizon,
		salesPed,
		recommendedPacketTt,
		readings,
	};
}

beforeEach(() => {
	vi.clearAllMocks();
	mocked.getAnalyticsHuntingActivity.mockResolvedValue(activity());
	mocked.getMarketHuntMarkups.mockResolvedValue({
		nanocubeMarkupPct: 100.6,
		items: [obs('Animal Muscle Oil', 130, 'week', 5000)],
	});
	mocked.getHuntingRealisedMarkup.mockResolvedValue({
		species: [{ mobSpecies: 'Atrox', netMarkup: 12 }],
		definitions: [{ definitionId: 1, netMarkup: 18 }],
		derived: [],
	});
});

describe('createHuntingModel', () => {
	it('projects the Tree Cutting economics for each session definition', async () => {
		const model = createHuntingModel();
		await model.loadData();

		const session = required(
			model.sessionSections.find((section) => section.definitionId === 1),
			'ARIS session',
		);
		expect(session.key).toBe('definition:1');
		expect(required(session.muProjectedReturns, 'MU projection')).toBeCloseTo(156, 5);
		expect(session.realisedMarkup).toBe(18);
		expect(session.realisedReturns).toBe(1098);
		expect(session.realisedRate).toBeCloseTo(1098 / 1200, 5);
		expect(session.items.map((item) => item.name)).toEqual(['Animal Muscle Oil']);

		const unassigned = required(
			model.sessionSections.find((section) => section.isUnassigned),
			'unassigned bucket',
		);
		expect(unassigned.key).toBe('unassigned');
		expect(unassigned.realisedMarkup).toBe(0);
	});

	it('keeps 100%-anchored loot MU separate and composes it with expected return', async () => {
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				overall: {
					cycled: 1500,
					returns: 1350,
					lootRate: 0.9,
					expected: expectedEconomics(),
				},
				definitions: [definition({ expected: expectedEconomics() })],
			}),
		);
		const model = createHuntingModel();
		await model.loadData();

		const session = required(model.sessionSections[0], 'modelled session');
		expect(session.lootMarkupFactor).toBeCloseTo(1.3, 6);
		expect(session.expectedTtRate).toBe(0.94);
		expect(session.expectedMarketRate).toBeCloseTo(1.222, 6);

		const overall = required(model.overall, 'modelled overall');
		expect(overall.lootMarkupFactor).toBeCloseTo(1.3, 6);
		expect(overall.expectedMarketRate).toBeCloseTo(1.222, 6);
	});

	it('projects expected economics at activity grain without folding quest rewards into loot MU', async () => {
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				definitions: [
					definition({
						activities: [sessionActivity({ expected: expectedEconomics() })],
					}),
				],
			}),
		);
		const model = createHuntingModel();
		await model.loadData();

		const row = required(model.sessionSections[0].activities[0], 'modelled activity');
		expect(row.lootMarkupFactor).toBeCloseTo(1.3, 6);
		expect(row.expectedTtRate).toBe(0.94);
		expect(row.expectedMarketRate).toBeCloseTo(1.222, 6);
		expect(row.rewardMuPed).toBeCloseTo(19.5, 6);
	});

	it('adds confirmed rewards once to activity, session, and Overall realised outcomes', async () => {
		const variant = sessionActivity();
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				definitions: [
					definition({
						activities: [sessionActivity({ kind: 'quest_family', variants: [variant] })],
					}),
				],
			}),
		);
		const model = createHuntingModel();
		await model.loadData();
		model.selectSession('definition:1');

		const row = required(model.selectedSession?.activities[0], 'rewarded activity');
		expect(row.rewardedRate).toBeCloseTo(1.05, 5);
		expect(row.rewardMuPed).toBeCloseTo(19.5, 5);
		expect(required(row.muProjectedReturns, 'activity MU projection')).toBeCloseTo(15.6, 5);
		const session = required(model.selectedSession, 'rewarded session');
		expect(session.confirmedRewardPed).toBe(15);
		expect(session.realisedReturns).toBe(1113);
		expect(session.realisedRate).toBeCloseTo(1113 / 1200, 5);
		const overall = required(model.overall, 'rewarded Overall');
		expect(overall.realisedReturns).toBe(1377);
		expect(overall.realisedRate).toBeCloseTo(1377 / 1500, 5);
	});

	it('leaves a recorded reward item at TT when it has no usable market data', async () => {
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				definitions: [
					definition({
						activities: [
							sessionActivity({
								rewardItems: [{ itemName: 'Mission Token', quantity: 1, valuePed: 15 }],
							}),
						],
					}),
				],
			}),
		);
		const model = createHuntingModel();
		await model.loadData();

		expect(model.sessionSections[0].activities[0].rewardMuPed).toBe(15);
	});

	it('values a Universal Ammo reward at face value rather than dropping it', async () => {
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				definitions: [
					definition({
						activities: [
							sessionActivity({
								rewardItems: [{ itemName: 'Universal Ammo', quantity: 40000, valuePed: 4 }],
							}),
						],
					}),
				],
			}),
		);
		const model = createHuntingModel();
		await model.loadData();
		expect(model.sessionSections[0].activities[0].rewardMuPed).toBe(4);
	});

	it('projects the stock item and keeps ammo at face value in a mixed reward', async () => {
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				definitions: [
					definition({
						activities: [
							sessionActivity({
								rewardItems: [
									{ itemName: 'Universal Ammo', quantity: 40000, valuePed: 4 },
									{ itemName: 'Mission Token', quantity: 1, valuePed: 1 },
								],
							}),
						],
					}),
				],
			}),
		);
		const model = createHuntingModel();
		await model.loadData();
		expect(model.sessionSections[0].activities[0].rewardMuPed).toBe(5);
	});

	it('aggregates a session reward from the activities that partition it', async () => {
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				definitions: [
					definition({
						cycled: 200,
						activities: [
							sessionActivity({
								rewardItems: [{ itemName: 'Animal Muscle Oil', quantity: 50, valuePed: 15 }],
								confirmedRewardPed: 15,
								rewardStatus: 'item',
								// A family variant restates its parent's reward and must
								// never be summed alongside it.
								variants: [
									sessionActivity({
										label: 'Variant',
										rewardItems: [{ itemName: 'Animal Muscle Oil', quantity: 50, valuePed: 15 }],
										confirmedRewardPed: 15,
										rewardStatus: 'item',
									}),
								],
							}),
							sessionActivity({
								label: 'Skill daily',
								rewardItems: [],
								confirmedRewardPed: 0,
								rewardStatus: 'skill',
							}),
						],
					}),
				],
			}),
		);
		const model = createHuntingModel();
		await model.loadData();

		const session = model.sessionSections[0];
		expect(session.reward.rewardTtPed).toBe(15);
		// 15 TT at the 130% week markup, the variant excluded.
		expect(session.reward.rewardMuPed).toBeCloseTo(19.5, 10);
		expect(session.reward.treatments.sort()).toEqual(['item', 'skill']);
		expect(session.rewardMuRate).toBeCloseTo(19.5 / 200, 10);
	});

	it('adds the reward to the long-run rate without disturbing the loot-only rate', async () => {
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				definitions: [
					definition({
						cycled: 100,
						lootItems: [{ itemName: 'Animal Muscle Oil', quantity: 40, valuePed: 100 }],
						expected: expectedEconomics(),
						activities: [
							sessionActivity({
								rewardItems: [{ itemName: 'Animal Muscle Oil', quantity: 10, valuePed: 10 }],
								confirmedRewardPed: 10,
								rewardStatus: 'item',
							}),
						],
					}),
				],
			}),
		);
		const model = createHuntingModel();
		await model.loadData();

		const session = model.sessionSections[0];
		const lootOnly = required(session.expectedMarketRate, 'expectedMarketRate');
		expect(lootOnly).toBeCloseTo(0.94 * 1.3, 10);
		expect(session.rewardMuRate).toBeCloseTo(13 / 100, 10);
		expect(session.expectedTotalRate).toBeCloseTo(lootOnly + 13 / 100, 10);
	});

	it('leaves a reward-free scope without a reward term at all', async () => {
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				definitions: [
					definition({
						expected: expectedEconomics(),
						activities: [
							sessionActivity({
								rewardItems: [],
								confirmedRewardPed: 0,
								rewardStatus: 'none',
							}),
						],
					}),
				],
			}),
		);
		const model = createHuntingModel();
		await model.loadData();

		const session = model.sessionSections[0];
		expect(session.reward.treatments).toEqual([]);
		expect(session.reward.rewardMuPed).toBeNull();
		expect(session.rewardMuRate).toBeNull();
		// With no reward, the long-run rate is exactly the loot-only rate.
		expect(session.expectedTotalRate).toBe(session.expectedMarketRate);
	});

	it('values zero-TT reward items from an absolute PED-per-unit quote', async () => {
		mocked.getAnalyticsHuntingActivity.mockResolvedValue(
			activity({
				definitions: [
					definition({
						activities: [
							sessionActivity({
								rewardItems: [{ itemName: 'Hyperion Daily Voucher', quantity: 20, valuePed: 0 }],
							}),
						],
					}),
				],
			}),
		);
		mocked.getMarketHuntMarkups.mockResolvedValue({
			nanocubeMarkupPct: 100.6,
			items: [{ ...obs('Hyperion Daily Voucher', null, null, null), unitPricePed: 2 }],
		});
		const model = createHuntingModel();
		await model.loadData();
		expect(model.sessionSections[0].activities[0].rewardMuPed).toBe(40);
	});

	it('merges market opportunity and realised markup into target rows', async () => {
		const model = createHuntingModel();
		await model.loadData();

		const atrox = required(
			model.targetSections.find((section) => section.mobSpecies === 'Atrox'),
			'Atrox target',
		);
		expect(required(atrox.muProjectedReturns, 'MU projection')).toBeCloseTo(156, 5);
		expect(atrox.realisedMarkup).toBeCloseTo(12, 5);
		expect(atrox.realisedReturns).toBeCloseTo(822, 5);
		expect(atrox.realisedRate).toBeCloseTo(822 / 900, 5);
		expect(model.targetSections.at(-1)?.label).toBe('Unclassified');
	});

	it('reconciles Overall market figures with the target rows', async () => {
		const model = createHuntingModel();
		await model.loadData();

		const overall = required(model.overall, 'overall');
		const projected = model.targetSections.reduce(
			(sum, section) => sum + (section.muProjectedReturns ?? 0),
			0,
		);
		expect(required(overall.muProjectedReturns, 'MU aggregate')).toBeCloseTo(projected, 5);
		expect(overall.realisedMarkup).toBeCloseTo(12, 5);
		expect(overall.realisedRate).toBeCloseTo((1350 + 12) / 1500, 5);
	});

	it('uses Overall by default and returns there for a stale session selection', async () => {
		const model = createHuntingModel();
		await model.loadData();

		expect(model.selectedSession).toBeNull();
		model.selectSession('definition:1');
		expect(required(model.selectedSession, 'selected session').definitionId).toBe(1);
		model.selectSession('definition:99');
		expect(model.selectedSession).toBeNull();
		model.selectSession(null);
		expect(model.selectedSession).toBeNull();
	});

	it('keeps the economic spine available when optional market context fails', async () => {
		mocked.getMarketHuntMarkups.mockRejectedValue(new Error('offline'));
		mocked.getHuntingRealisedMarkup.mockRejectedValue(new Error('offline'));
		const model = createHuntingModel();
		await model.loadData();
		expect(model.error).toBeNull();
		const atrox = required(
			model.targetSections.find((section) => section.mobSpecies === 'Atrox'),
			'Atrox target',
		);
		expect(atrox.muProjectedReturns).toBeNull();
		expect(atrox.realisedMarkup).toBe(0);

		mocked.getAnalyticsHuntingActivity.mockRejectedValue(new Error('backend unreachable'));
		const broken = createHuntingModel();
		await broken.loadData();
		expect(broken.error).toContain('backend unreachable');
	});
});
