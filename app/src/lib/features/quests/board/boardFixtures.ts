/**
 * Test fixtures for the quest board: a two-planet catalogue in the shipped
 * shape and factories for the rows it is laid over.
 */

import type { Quest, QuestCatalogue, QuestFamily, UnresolvedQuestReward } from '$lib/types';

export const NOW = Date.parse('2026-10-17T12:00:00Z');
export const HOUR = 3_600_000;

export function quest(overrides: Partial<Quest> = {}): Quest {
	return {
		id: '1',
		name: 'A quest',
		category: null,
		targetMobs: [],
		planet: 'Cyrene',
		waypoint: null,
		cooldownDurationHours: 21,
		cooldownExpiresAt: null,
		reward: null,
		rewardIsSkill: false,
		rewardDescription: '',
		notes: '',
		chainName: null,
		chainPosition: null,
		chainTotal: null,
		startedAt: null,
		signalLootItem: null,
		completionTrigger: 'mission_log',
		rewardPolicy: 'named_items',
		rewardItemNames: [],
		cooldownAnchor: 'completion',
		lastStartedAt: null,
		familyId: null,
		familyName: null,
		familyCooldownDurationHours: null,
		familyCooldownAnchor: null,
		familyCooldownExpiresAt: null,
		rewardUndoAvailable: false,
		curated: false,
		catalogueKey: null,
		lastAbandonedAt: null,
		lastCompletedAt: null,
		...overrides,
	};
}

export function family(overrides: Partial<QuestFamily> = {}): QuestFamily {
	return {
		id: '1',
		name: 'ARIS - Daily Hunting 1',
		planet: 'ARIS',
		cooldownDurationHours: 20,
		cooldownAnchor: 'pickup',
		cooldownExpiresAt: null,
		memberCount: 2,
		lastStartedAt: null,
		lastCompletedAt: null,
		curated: true,
		catalogueKey: 'aris/hunting-1',
		...overrides,
	};
}

export function unresolved(overrides: Partial<UnresolvedQuestReward> = {}): UnresolvedQuestReward {
	return {
		completionId: 1,
		questId: '10',
		questName: 'ARIS - Daily Hunting 1: Weak Mortirex',
		completedAt: NOW / 1000 - 3600,
		policy: 'named_items',
		reason: 'One or more expected reward items were missing',
		loot: [],
		isolated: true,
		expectedItems: ['Hyperion Daily Voucher'],
		candidates: [],
		declarable: true,
		...overrides,
	};
}

export const CATALOGUE: QuestCatalogue = {
	planets: [
		{
			key: 'aris',
			name: 'ARIS',
			categories: [
				{
					key: 'aris/daily-missions',
					name: 'Daily missions',
					entries: [
						{
							kind: 'family',
							key: 'aris/hunting-1',
							label: 'Hunting 1',
							detail: null,
							rewardLabel: 'Hyperion voucher',
							memberKeys: ['aris/hunting-1/weak-mortirex', 'aris/hunting-1/defective-drones'],
						},
						{
							kind: 'family',
							key: 'aris/samples',
							label: 'Samples',
							detail: null,
							rewardLabel: 'Hyperion voucher',
							memberKeys: [],
						},
					],
				},
				{
					key: 'aris/bosses',
					name: 'Bosses',
					entries: [
						{
							kind: 'quest',
							key: 'aris/verdathos',
							label: 'Verdathos',
							detail: 'Creature boss',
							rewardLabel: 'Hyperion voucher',
							memberKeys: [],
						},
					],
				},
			],
		},
		{
			key: 'cyrene',
			name: 'Cyrene',
			categories: [
				{
					key: 'cyrene/mission-tokens',
					name: 'Mission Tokens',
					entries: [
						{
							kind: 'quest',
							key: 'cyrene/getting-slimed',
							label: 'Getting Slimed!',
							detail: null,
							rewardLabel: '2 tokens',
							memberKeys: [],
						},
						{
							kind: 'quest',
							key: 'cyrene/lord-pyscidon',
							label: 'Lord Pyscidon',
							detail: null,
							rewardLabel: '1 token',
							memberKeys: [],
						},
					],
				},
			],
		},
	],
};

/** The curated rows the fixture catalogue is laid over, all at rest. */
export function curatedRows(): { quests: Quest[]; families: QuestFamily[] } {
	return {
		families: [
			family(),
			family({
				id: '2',
				name: 'ARIS - Daily Samples',
				catalogueKey: 'aris/samples',
				memberCount: 0,
			}),
		],
		quests: [
			quest({
				id: '10',
				name: 'ARIS - Daily Hunting 1: Weak Mortirex',
				planet: 'ARIS',
				familyId: '1',
				cooldownDurationHours: null,
				curated: true,
				catalogueKey: 'aris/hunting-1/weak-mortirex',
				waypoint: '/wp [Aris, 38733, 22335, 165, Weak Mortirex Meekling]',
			}),
			quest({
				id: '11',
				name: 'ARIS - Daily Hunting 1: Defective Drones',
				planet: 'ARIS',
				familyId: '1',
				cooldownDurationHours: null,
				curated: true,
				catalogueKey: 'aris/hunting-1/defective-drones',
			}),
			quest({
				id: '20',
				name: 'ARIS - Creature Boss 1: Verdathos',
				planet: 'ARIS',
				completionTrigger: 'signal_item',
				signalLootItem: 'Hyperion Daily Voucher',
				cooldownDurationHours: 20,
				curated: true,
				catalogueKey: 'aris/verdathos',
				waypoint: '/wp [Aris, 36565, 20566, 120, Verdathos]',
			}),
			quest({
				id: '30',
				name: 'Getting Slimed!',
				planet: 'Cyrene',
				curated: true,
				catalogueKey: 'cyrene/getting-slimed',
				waypoint: '/wp [Planet Cyrene, 138377, 77671, 121, Captain Wrigley]',
			}),
			quest({
				id: '31',
				name: 'Lord Pyscidon',
				planet: 'Cyrene',
				curated: true,
				catalogueKey: 'cyrene/lord-pyscidon',
				waypoint: '/wp [Planet Cyrene, 132430, 75849, 104, Lord Pyscidon]',
			}),
		],
	};
}
