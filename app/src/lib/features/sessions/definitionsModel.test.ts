import { describe, expect, it, vi } from 'vitest';
import type { SessionDefinition } from '$lib/api';
import { ApiError } from '$lib/api';
import type { Quest, QuestFamily } from '$lib/types';
import { createDefinitionsModel, type DefinitionsModelDeps } from './definitionsModel.svelte';

function definition(overrides: Partial<SessionDefinition> = {}): SessionDefinition {
	return {
		id: '1',
		name: 'ARIS Dailies',
		adHocSegments: false,
		trackProtectionCosts: true,
		isProtected: false,
		isActive: true,
		instanceCount: 0,
		createdAt: 1000,
		updatedAt: null,
		roster: [],
		...overrides,
	};
}

function family(id: string, name: string): QuestFamily {
	return {
		id,
		name,
		planet: 'ARIS',
		cooldownDurationHours: null,
		cooldownAnchor: 'pickup',
		cooldownExpiresAt: null,
		memberCount: 0,
		lastStartedAt: null,
		lastCompletedAt: null,
	};
}

function quest(overrides: Partial<Quest> = {}): Quest {
	return {
		id: '1',
		name: 'Hunt Caboria',
		category: null,
		targetMobs: ['Caboria'],
		planet: 'ARIS',
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
		rewardPolicy: 'none',
		rewardItemNames: [],
		cooldownAnchor: 'completion',
		lastStartedAt: null,
		familyId: null,
		familyName: null,
		familyCooldownDurationHours: null,
		familyCooldownAnchor: null,
		familyCooldownExpiresAt: null,
		rewardUndoAvailable: false,
		...overrides,
	};
}

function makeDeps(overrides: Partial<DefinitionsModelDeps> = {}): DefinitionsModelDeps {
	return {
		listDefinitions: vi.fn(async () => [definition()]),
		createDefinition: vi.fn(async () => definition({ id: '7' })),
		updateDefinition: vi.fn(async () => definition()),
		archiveDefinition: vi.fn(async () => definition({ isActive: false })),
		selectDefinition: vi.fn(async () => ({})),
		refreshTracking: vi.fn(async () => ({})),
		listFamilies: vi.fn(async () => [family('3', 'Daily Hunting 1')]),
		listQuests: vi.fn(async () => []),
		...overrides,
	};
}

describe('createDefinitionsModel', () => {
	it('loads the definition list and surfaces load failures', async () => {
		const deps = makeDeps();
		const model = createDefinitionsModel(deps);
		await model.loadDefinitions();
		expect(model.definitions).toHaveLength(1);
		expect(model.error).toBeNull();

		const failing = createDefinitionsModel(
			makeDeps({
				listDefinitions: vi.fn(async () => {
					throw new Error('boom');
				}),
			}),
		);
		await failing.loadDefinitions();
		expect(failing.error).toBe('boom');
	});

	it('keeps the active catalogue in stable alphabetical order', async () => {
		const model = createDefinitionsModel(
			makeDeps({
				listDefinitions: vi.fn(async () => [
					definition({ id: '3', name: 'Tree Cutting' }),
					definition({ id: '2', name: 'ARIS Dailies' }),
					definition({ id: '4', name: 'Bank Robber Skilling' }),
				]),
			}),
		);
		await model.loadDefinitions();

		expect(model.definitions.map((entry) => entry.name)).toEqual([
			'ARIS Dailies',
			'Bank Robber Skilling',
			'Tree Cutting',
		]);
	});

	it('shapes the selection write as a numeric id and refreshes', async () => {
		const deps = makeDeps();
		const model = createDefinitionsModel(deps);
		await model.select('4');
		expect(deps.selectDefinition).toHaveBeenCalledWith(4);
		expect(deps.refreshTracking).toHaveBeenCalledTimes(1);
	});

	it('drafts a roster with dedupe and removal, ordered by kind then name', () => {
		const model = createDefinitionsModel(makeDeps());
		model.openCreate();
		model.addQuest({ id: '9', name: 'The Ultimate Threat' } as Quest);
		model.addFamily(family('3', 'Daily Hunting 2'));
		model.addQuest({ id: '4', name: 'Aakas Ambush' } as Quest);
		model.addFamily(family('1', 'Daily Hunting 1'));
		model.addFamily(family('1', 'Daily Hunting 1'));
		// Families first, then quests, each alphabetically: nothing is
		// hand-ordered, so the list reads the same however it was built.
		expect(model.roster.map((entry) => entry.displayName)).toEqual([
			'Daily Hunting 1',
			'Daily Hunting 2',
			'Aakas Ambush',
			'The Ultimate Threat',
		]);

		model.removeEntry(1);
		expect(model.roster.map((entry) => entry.displayName)).toEqual([
			'Daily Hunting 1',
			'Aakas Ambush',
			'The Ultimate Threat',
		]);
	});

	it('saves a create, selects the new definition, and closes', async () => {
		const deps = makeDeps();
		const model = createDefinitionsModel(deps);
		model.openCreate();
		model.name = '  General Hunting  ';
		model.addQuest({ id: '9', name: 'The Ultimate Threat' } as Quest);
		expect(await model.save()).toBe(true);
		expect(deps.createDefinition).toHaveBeenCalledWith({
			name: 'General Hunting',
			ad_hoc_segments: false,
			track_protection_costs: true,
			roster: [{ kind: 'quest', ref_id: 9, label: null }],
		});
		expect(deps.selectDefinition).toHaveBeenCalledWith(7);
		expect(model.mode).toBe('closed');
	});

	it('tolerates the fixed-while-active conflict on the post-create selection', async () => {
		const deps = makeDeps({
			selectDefinition: vi.fn(async () => {
				throw new ApiError('conflict', 'fixed for the active session');
			}),
		});
		const model = createDefinitionsModel(deps);
		model.openCreate();
		model.name = 'General Hunting';
		expect(await model.save()).toBe(true);
		expect(model.authoringError).toBeNull();
	});

	it('refuses a blank name client-side', async () => {
		const model = createDefinitionsModel(makeDeps());
		model.openCreate();
		model.name = '   ';
		expect(await model.save()).toBe(false);
		expect(model.authoringError).toBe('A session needs a name');
	});

	it('drops a dead reference from the saved roster and updates in place', async () => {
		const deps = makeDeps();
		const model = createDefinitionsModel(deps);
		model.openEdit(
			definition({
				id: '2',
				roster: [
					{ id: '1', kind: 'quest_family', refId: '3', label: null, displayName: null },
					{ id: '2', kind: 'segment', refId: null, label: 'Grind', displayName: 'Grind' },
				],
			}),
		);
		expect(model.roster[0].missing).toBe(true);
		expect(await model.save()).toBe(true);
		expect(deps.updateDefinition).toHaveBeenCalledWith('2', {
			name: 'ARIS Dailies',
			ad_hoc_segments: false,
			track_protection_costs: true,
			roster: [{ kind: 'segment', ref_id: null, label: 'Grind' }],
		});
	});

	it('defaults new definitions to recording armour hits', () => {
		const model = createDefinitionsModel(makeDeps());
		model.openCreate();

		expect(model.trackProtectionCosts).toBe(true);
	});

	it('persists an armour-cost opt-out on a definition', async () => {
		const deps = makeDeps();
		const model = createDefinitionsModel(deps);
		model.openEdit(definition({ id: '2' }));
		model.trackProtectionCosts = false;

		expect(await model.save()).toBe(true);
		expect(deps.updateDefinition).toHaveBeenCalledWith(
			'2',
			expect.objectContaining({ track_protection_costs: false }),
		);
	});

	it('archives only on the armed second step', async () => {
		const deps = makeDeps();
		const model = createDefinitionsModel(deps);
		model.openEdit(definition({ id: '2' }));
		expect(await model.archiveEditing()).toBe(false);
		expect(deps.archiveDefinition).not.toHaveBeenCalled();
		expect(model.archiveArmed).toBe(true);
		expect(await model.archiveEditing()).toBe(true);
		expect(deps.archiveDefinition).toHaveBeenCalledWith('2');
		expect(model.mode).toBe('closed');
	});

	it('does not offer an archive retry after a post-commit refresh fails', async () => {
		const deps = makeDeps({
			refreshTracking: vi.fn(async () => {
				throw new Error('refresh failed');
			}),
		});
		const model = createDefinitionsModel(deps);
		model.openEdit(definition({ id: '2' }));

		expect(await model.archiveEditing()).toBe(false);
		expect(await model.archiveEditing()).toBe(true);

		expect(deps.archiveDefinition).toHaveBeenCalledTimes(1);
		expect(model.mode).toBe('closed');
		expect(model.authoringError).toBeNull();
		expect(model.error).toBe('refresh failed');
	});

	it('keeps the editor retryable when the archive itself fails', async () => {
		const deps = makeDeps({
			archiveDefinition: vi.fn(async () => {
				throw new Error('archive failed');
			}),
		});
		const model = createDefinitionsModel(deps);
		model.openEdit(definition({ id: '2' }));

		expect(await model.archiveEditing()).toBe(false);
		expect(await model.archiveEditing()).toBe(false);

		expect(model.mode).toBe('edit');
		expect(model.saving).toBe(false);
		expect(model.authoringError).toBe('archive failed');
	});
});

describe('the offered catalogue', () => {
	it('drops the quests a family already stands for, and keeps the rest', async () => {
		const model = createDefinitionsModel(
			makeDeps({
				listFamilies: vi.fn(async () => [family('3', 'ARIS Daily Hunting')]),
				listQuests: vi.fn(async () => [
					quest({ id: '1', name: 'Hunt Caboria', familyId: '3' }),
					quest({ id: '2', name: 'Hunt Molisk', familyId: '3' }),
					quest({ id: '5', name: 'Codex Sweep' }),
				]),
			}),
		);
		model.openCreate();
		await vi.waitFor(() => expect(model.quests).toHaveLength(3));

		expect(model.standaloneQuests.map((q) => q.name)).toEqual(['Codex Sweep']);
	});

	// Nothing else would represent it, so a member outliving its family
	// stays offerable rather than vanishing from the catalogue.
	it('keeps offering a quest whose family is gone', async () => {
		const model = createDefinitionsModel(
			makeDeps({
				listFamilies: vi.fn(async () => []),
				listQuests: vi.fn(async () => [quest({ id: '1', familyId: '3' })]),
			}),
		);
		model.openCreate();
		await vi.waitFor(() => expect(model.quests).toHaveLength(1));

		expect(model.standaloneQuests.map((q) => q.id)).toEqual(['1']);
	});
});

describe('the catalogue', () => {
	async function openOn(planet: string, quests: Quest[], families: QuestFamily[] = []) {
		const model = createDefinitionsModel(
			makeDeps({
				listFamilies: vi.fn(async () => families),
				listQuests: vi.fn(async () => quests),
			}),
		);
		model.openCreate();
		await vi.waitFor(() => expect(model.quests).toHaveLength(quests.length));
		model.catalogPlanet = planet;
		return model;
	}

	it('groups the offered quests by category, alphabetically, uncategorised last', async () => {
		const model = await openOn('ARIS', [
			quest({ id: '1', name: 'Zeta sweep', category: 'Weeklies' }),
			quest({ id: '2', name: 'Loose end' }),
			quest({ id: '3', name: 'Alpha run', category: 'Codex' }),
			quest({ id: '4', name: 'Beta run', category: 'Codex' }),
			quest({ id: '5', name: 'Elsewhere', category: 'Codex', planet: 'Calypso' }),
		]);

		expect(
			model.catalogCategories.map((group) => [group.category, group.quests.map((q) => q.name)]),
		).toEqual([
			['Codex', ['Alpha run', 'Beta run']],
			['Weeklies', ['Zeta sweep']],
			[null, ['Loose end']],
		]);
	});

	it('narrows to the filter, and offers no catalogue before a planet is chosen', async () => {
		const model = await openOn('ARIS', [
			quest({ id: '1', name: 'Alpha run', category: 'Codex' }),
			quest({ id: '2', name: 'Beta run', category: 'Codex' }),
		]);

		model.catalogFilter = 'beta';
		expect(model.catalogCategories.map((group) => group.quests.map((q) => q.name))).toEqual([
			['Beta run'],
		]);

		model.catalogPlanet = null;
		expect(model.catalogCategories).toEqual([]);
	});

	it('adds a whole category in one go, without duplicating what is already rostered', async () => {
		const model = await openOn('ARIS', [
			quest({ id: '1', name: 'Alpha run', category: 'Codex' }),
			quest({ id: '2', name: 'Beta run', category: 'Codex' }),
		]);
		model.addQuest(quest({ id: '1', name: 'Alpha run', category: 'Codex' }));

		model.addQuests(model.catalogCategories[0].quests);

		expect(model.roster.map((entry) => entry.refId)).toEqual(['1', '2']);
		expect(model.hasRosterRef('quest', '2')).toBe(true);
		expect(model.hasRosterRef('quest_family', '2')).toBe(false);
	});
});
