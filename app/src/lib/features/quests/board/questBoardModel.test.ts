import { describe, expect, it, vi } from 'vitest';
import type { ActivityOptionsResult, SessionDefinition } from '$lib/api';
import { reactiveBox } from '$lib/view/__fixtures__/reactiveBox.svelte';
import { CATALOGUE, curatedRows, unresolved } from './boardFixtures';
import {
	BOARD_PLANET_PREFERENCE,
	createQuestBoardModel,
	type QuestBoardDeps,
} from './questBoardModel.svelte';

function definition(overrides: Partial<SessionDefinition> = {}): SessionDefinition {
	return {
		id: '5',
		name: 'ARIS Dailies',
		adHocSegments: false,
		trackProtectionCosts: true,
		questPlanet: null,
		isProtected: false,
		isActive: true,
		instanceCount: 3,
		createdAt: 0,
		updatedAt: null,
		roster: [
			{ id: '1', kind: 'segment', refId: null, label: 'Travel', displayName: 'Travel' },
			{ id: '2', kind: 'quest', refId: '20', label: null, displayName: 'Verdathos' },
			{ id: '3', kind: 'quest', refId: '99', label: null, displayName: null },
		],
		...overrides,
	};
}

function rig(options: { preference?: string | null; definition?: SessionDefinition | null } = {}) {
	const rows = curatedRows();
	const quests = reactiveBox(rows.quests);
	const activity = reactiveBox<ActivityOptionsResult | null>(null);
	const selected = reactiveBox<SessionDefinition | null>(options.definition ?? null);
	const preferences = new Map<string, unknown>([
		[BOARD_PLANET_PREFERENCE, options.preference ?? null],
	]);
	const deps: QuestBoardDeps = {
		quests: () => quests.value,
		families: () => rows.families,
		activityOptions: () => activity.value,
		definition: () => selected.value,
		getCatalogue: vi.fn(async () => CATALOGUE),
		getUnresolved: vi.fn(async () => [unresolved({ questId: '30' })]),
		startQuest: vi.fn(async () => undefined),
		completeQuest: vi.fn(async () => undefined),
		abandonQuest: vi.fn(async () => undefined),
		cancelQuest: vi.fn(async () => undefined),
		reviewReward: vi.fn(async () => undefined),
		updateDefinition: vi.fn(async () => undefined),
		refresh: vi.fn(async () => undefined),
		refreshDefinitions: vi.fn(async () => undefined),
		getPreference: vi.fn(
			async <T>(key: string, fallback: T) => (preferences.get(key) as T) ?? fallback,
		),
		setPreference: vi.fn(async (key: string, value: unknown) => {
			preferences.set(key, value);
		}),
	};
	const model = createQuestBoardModel(deps);
	return { model, deps, quests, activity, selected, preferences };
}

describe('which planet the board shows', () => {
	it('opens on the planet the session offers, else the first', async () => {
		const offered = rig({ definition: definition({ questPlanet: 'Cyrene' }) });
		await offered.model.init();
		expect(offered.model.planet).toBe('Cyrene');
		expect(offered.model.offeredPlanet).toBe('Cyrene');

		const plain = rig();
		await plain.model.init();
		expect(plain.model.planet).toBe('ARIS');
	});

	it("keeps the player's own choice, and remembers it", async () => {
		const { model, deps, preferences } = rig({
			preference: 'Cyrene',
			definition: definition({ questPlanet: 'ARIS' }),
		});
		await model.init();
		expect(model.planet).toBe('Cyrene');
		model.selectPlanet('ARIS');
		expect(model.planet).toBe('ARIS');
		expect(deps.setPreference).toHaveBeenCalledWith(BOARD_PLANET_PREFERENCE, 'ARIS');
		expect(preferences.get(BOARD_PLANET_PREFERENCE)).toBe('ARIS');
	});

	it('falls back when the remembered planet has nothing to show', async () => {
		const { model } = rig({ preference: 'Monria' });
		await model.init();
		expect(model.planet).toBe('ARIS');
	});
});

describe('the rows', () => {
	it('carry the rewards to settle and the overlay recording state', async () => {
		const { model, quests, activity } = rig();
		await model.init();
		expect(model.catalogueLoaded).toBe(true);
		model.selectPlanet('Cyrene');
		expect(model.current?.sections[0].rows[0].unresolved).toHaveLength(1);

		quests.value = quests.value.map((quest) =>
			quest.id === '30' ? { ...quest, startedAt: 1 } : quest,
		);
		activity.value = {
			definitionId: null,
			definitionName: null,
			visible: true,
			adHocSegments: false,
			readyCount: 0,
			options: [],
			active: [
				{
					key: 'quest:30',
					kind: 'quest',
					name: 'Getting Slimed!',
					questId: 30,
					manualHandIn: false,
					handInWaiting: false,
				},
			],
		};
		expect(model.current?.sections[0].rows[0].state).toBe('recording');
	});
});

describe('the verbs', () => {
	it('act on the row quest, then refresh', async () => {
		const { model, deps } = rig();
		await model.init();
		const boss = model.current?.sections[1].rows[0];
		if (!boss) throw new Error('no boss row');
		await model.start(boss);
		expect(deps.startQuest).toHaveBeenCalledWith('20');
		await model.abandon(boss);
		expect(deps.abandonQuest).toHaveBeenCalledWith('20');
		await model.resetCooldown(boss);
		expect(deps.cancelQuest).toHaveBeenCalledWith('20', false);
		await model.undoReward(boss);
		expect(deps.cancelQuest).toHaveBeenCalledWith('20', true);
		expect(deps.refresh).toHaveBeenCalledTimes(4);
		expect(model.busy).toBeNull();
	});

	it('report a refused write on the board', async () => {
		const { model, deps } = rig();
		await model.init();
		vi.mocked(deps.abandonQuest).mockRejectedValueOnce(
			new Error('Only a quest in progress can be abandoned'),
		);
		const boss = model.current?.sections[1].rows[0];
		if (!boss) throw new Error('no boss row');
		await model.abandon(boss);
		expect(model.error).toContain('Only a quest in progress can be abandoned');
	});

	it('settle a reward and re-read what is left to settle', async () => {
		const { model, deps } = rig();
		await model.init();
		model.selectPlanet('Cyrene');
		const slimed = model.current?.sections[0].rows[0];
		if (!slimed) throw new Error('no row');
		model.toggleSettle(slimed);
		expect(model.settling).toBe('cyrene/getting-slimed');
		vi.mocked(deps.getUnresolved).mockResolvedValueOnce([]);
		await model.settle(slimed, { completionId: 1, declarePaid: true });
		expect(deps.reviewReward).toHaveBeenCalledWith({ completionId: 1, declarePaid: true });
		expect(model.current?.sections[0].rows[0].unresolved).toEqual([]);
		expect(model.settling).toBeNull();
	});

	it('make the session offer a planet, keeping its roster but its dead entries', async () => {
		const { model, deps } = rig({ definition: definition() });
		await model.init();
		await model.offerInOverlay('ARIS');
		expect(deps.updateDefinition).toHaveBeenCalledWith('5', {
			name: 'ARIS Dailies',
			ad_hoc_segments: false,
			track_protection_costs: true,
			quest_planet: 'ARIS',
			roster: [
				{ kind: 'segment', ref_id: null, label: 'Travel' },
				{ kind: 'quest', ref_id: 20, label: null },
			],
		});
		expect(deps.refreshDefinitions).toHaveBeenCalled();
	});
});
