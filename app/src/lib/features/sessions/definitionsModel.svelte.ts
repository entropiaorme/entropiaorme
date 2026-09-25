/**
 * Session-definitions view model: the definition list, the selection
 * write (which definition the next session starts as an instance of),
 * and the full-screen authoring environment's state (form, roster
 * draft, the morph lifecycle). Deps-injected so the dashboard island
 * and the tests compose it the same way.
 *
 * A definition's roster is authored data replaced wholesale on save,
 * so the draft here is a plain array
 * the editor mutates freely; nothing persists until Save. Segment
 * entries are never drafted here: they arrive by being named in the
 * overlay while playing, and the editor only prunes them.
 *
 * The draft is kept in display order (by kind, then alphabetically),
 * which is also the order it saves in and the order the overlay offers
 * it. Nothing is hand-ordered: a roster is a set of things this session
 * is for, and a predictable A-Z beats remembering how it was typed up.
 */

import type { SessionDefinition, SessionDefinitionInput, SessionRosterEntryKind } from '$lib/api';
import {
	ApiError,
	archiveSessionDefinition,
	createSessionDefinition,
	getQuestFamilies,
	getQuests,
	getSessionDefinitions,
	selectDefinition,
	updateSessionDefinition,
} from '$lib/api';
import { hydrate } from '$lib/stores/trackingStore.svelte';
import type { Quest, QuestFamily } from '$lib/types';
import { describeError } from '$lib/view/errorState';
import { sortDefinitions } from './definitionCatalogue';

/** One roster row as the editor drafts it (ids stringified for the UI;
 * `missing` marks a stored reference whose target has been deleted). */
export interface RosterDraftEntry {
	/** Identity for the list, stable across reordering: without it the
	 * moved rows are destroyed and recreated, and the button the user is
	 * pressing goes with them. */
	key: number;
	kind: SessionRosterEntryKind;
	refId: string | null;
	label: string | null;
	displayName: string;
	missing: boolean;
}

/** Roster kinds in reading order, most general first; entries sort by
 * this and then by name. */
const KIND_ORDER: Record<SessionRosterEntryKind, number> = {
	quest_family: 0,
	quest: 1,
	segment: 2,
};

function byKindThenName(a: RosterDraftEntry, b: RosterDraftEntry): number {
	return (
		KIND_ORDER[a.kind] - KIND_ORDER[b.kind] ||
		a.displayName.localeCompare(b.displayName, undefined, { sensitivity: 'base' })
	);
}

/** A catalogue group: one quest category, or the uncategorised tail
 * (`category: null`). */
export interface QuestCategoryGroup {
	category: string | null;
	quests: Quest[];
}

export interface DefinitionsModelDeps {
	listDefinitions(): Promise<SessionDefinition[]>;
	createDefinition(data: SessionDefinitionInput): Promise<SessionDefinition>;
	updateDefinition(id: string, data: SessionDefinitionInput): Promise<SessionDefinition>;
	archiveDefinition(id: string): Promise<SessionDefinition>;
	/** The tracking-family selection verb. */
	selectDefinition(id: number): Promise<unknown>;
	/** Re-read the tracking snapshot after a selection write. */
	refreshTracking(): Promise<unknown>;
	/** The roster's offer sources (loaded when the authoring opens). */
	listFamilies(): Promise<QuestFamily[]>;
	listQuests(): Promise<Quest[]>;
}

export type AuthoringMode = 'closed' | 'create' | 'edit';

export function createDefinitionsModel(deps: DefinitionsModelDeps) {
	let definitions = $state<SessionDefinition[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let selecting = $state(false);

	// ── The authoring environment ──
	let mode = $state<AuthoringMode>('closed');
	let editingId = $state<string | null>(null);
	/** The definition being edited cannot be archived (see the wire's
	 * `isProtected`): tracking always needs one to run under. */
	let editingProtected = $state(false);
	let name = $state('');
	let adHocSegments = $state(false);
	let trackProtectionCosts = $state(true);
	let roster = $state<RosterDraftEntry[]>([]);
	let saving = $state(false);
	let authoringError = $state<string | null>(null);
	let archiveArmed = $state(false);

	// Roster-entry identity, monotonic within one editing session; the
	// roster is replaced wholesale on save, so these never reach the
	// database.
	let nextRosterKey = 0;
	const rosterKey = () => nextRosterKey++;

	// The roster's offer sources, loaded when the authoring opens so the
	// editor never presents a stale catalogue.
	let families = $state<QuestFamily[]>([]);
	let quests = $state<Quest[]>([]);
	let sourcesLoading = $state(false);

	// A family entry matches whichever variant the day serves, so its own
	// members are never offered beside it: rostering one variant would
	// silently miss the rest, and nobody wants the variant without the
	// family. A member whose family has gone stays offerable, because
	// nothing else would represent it.
	const familyIds = $derived(new Set(families.map((family) => family.id)));
	const standaloneQuests = $derived(
		quests.filter((quest) => quest.familyId === null || !familyIds.has(quest.familyId)),
	);

	// ── The catalogue: what the editor offers, and how it is narrowed ──
	// Reached through its planet (how the Quests page scopes the same
	// content), then read as a list grouped by the quests' own categories.
	let catalogPlanet = $state<string | null>(null);
	let catalogFilter = $state('');

	/** The planets with something to offer, so a choice can never lead to
	 * an empty catalogue. */
	const catalogPlanets = $derived(
		[
			...new Set([
				...families.map((family) => family.planet),
				...standaloneQuests.map((quest) => quest.planet),
			]),
		].sort(),
	);

	function matchesFilter(value: string): boolean {
		return value.toLowerCase().includes(catalogFilter.trim().toLowerCase());
	}

	const catalogFamilies = $derived(
		catalogPlanet === null
			? []
			: families
					.filter((family) => family.planet === catalogPlanet && matchesFilter(family.name))
					.toSorted((a, b) => a.name.localeCompare(b.name)),
	);

	/** The offered quests grouped by their own category, alphabetically,
	 * with the uncategorised ones last (they are just quests, and get no
	 * heading of their own). */
	const catalogCategories = $derived.by((): QuestCategoryGroup[] => {
		if (catalogPlanet === null) return [];
		const grouped = new Map<string | null, Quest[]>();
		for (const quest of standaloneQuests) {
			if (quest.planet !== catalogPlanet || !matchesFilter(quest.name)) continue;
			const key = quest.category?.trim() ? quest.category : null;
			const bucket = grouped.get(key);
			if (bucket) bucket.push(quest);
			else grouped.set(key, [quest]);
		}
		const byName = (a: Quest, b: Quest) => a.name.localeCompare(b.name);
		const named = [...grouped.entries()]
			.filter((entry): entry is [string, Quest[]] => entry[0] !== null)
			.sort((a, b) => a[0].localeCompare(b[0]))
			.map(([category, group]) => ({ category, quests: [...group].sort(byName) }));
		const loose = grouped.get(null) ?? [];
		return loose.length > 0
			? [...named, { category: null, quests: [...loose].sort(byName) }]
			: named;
	});

	/** Whether the draft roster already references this target. */
	function hasRosterRef(kind: SessionRosterEntryKind, refId: string): boolean {
		return roster.some((entry) => entry.kind === kind && entry.refId === refId);
	}

	async function loadDefinitions() {
		loading = true;
		try {
			definitions = sortDefinitions(await deps.listDefinitions());
			error = null;
		} catch (e) {
			error = describeError(e, 'Failed to load sessions');
		} finally {
			loading = false;
		}
	}

	/** Select the definition the next session starts under; the backend
	 * writes the name facet with it in the same motion. A session always
	 * runs under one, so there is nothing to withdraw to. */
	async function select(id: string) {
		selecting = true;
		try {
			await deps.selectDefinition(Number(id));
			error = null;
			await deps.refreshTracking();
		} catch (e) {
			error = describeError(e, 'Failed to select the session');
		} finally {
			selecting = false;
		}
	}

	async function loadSources() {
		sourcesLoading = true;
		try {
			[families, quests] = await Promise.all([deps.listFamilies(), deps.listQuests()]);
		} catch (e) {
			authoringError = describeError(e, 'Failed to load roster options');
		} finally {
			sourcesLoading = false;
		}
	}

	function openCreate() {
		mode = 'create';
		editingId = null;
		editingProtected = false;
		name = '';
		adHocSegments = false;
		trackProtectionCosts = true;
		roster = [];
		authoringError = null;
		archiveArmed = false;
		catalogPlanet = null;
		catalogFilter = '';
		void loadSources();
	}

	function openEdit(definition: SessionDefinition) {
		mode = 'edit';
		editingId = definition.id;
		editingProtected = definition.isProtected;
		name = definition.name;
		adHocSegments = definition.adHocSegments;
		trackProtectionCosts = definition.trackProtectionCosts;
		roster = definition.roster
			.map((entry) => ({
				key: rosterKey(),
				kind: entry.kind,
				refId: entry.refId,
				label: entry.label,
				displayName: entry.displayName ?? entry.label ?? '',
				missing: entry.displayName === null,
			}))
			.sort(byKindThenName);
		authoringError = null;
		archiveArmed = false;
		catalogPlanet = null;
		catalogFilter = '';
		void loadSources();
	}

	function close() {
		mode = 'closed';
		editingId = null;
		archiveArmed = false;
	}

	// ── Roster drafting ──

	function hasReference(kind: SessionRosterEntryKind, refId: string) {
		return roster.some((entry) => entry.kind === kind && entry.refId === refId);
	}

	function addFamily(family: QuestFamily) {
		if (hasReference('quest_family', family.id)) return;
		const entry: RosterDraftEntry = {
			key: rosterKey(),
			kind: 'quest_family',
			refId: family.id,
			label: null,
			displayName: family.name,
			missing: false,
		};
		roster = [...roster, entry].sort(byKindThenName);
	}

	function addQuest(quest: Quest) {
		if (hasReference('quest', quest.id)) return;
		const entry: RosterDraftEntry = {
			key: rosterKey(),
			kind: 'quest',
			refId: quest.id,
			label: null,
			displayName: quest.name,
			missing: false,
		};
		roster = [...roster, entry].sort(byKindThenName);
	}

	/** Add every quest in a category in one go; the per-quest guard makes
	 * the ones already rostered no-ops. */
	function addQuests(toAdd: Quest[]) {
		for (const quest of toAdd) addQuest(quest);
	}

	function removeEntry(index: number) {
		roster = roster.filter((_, i) => i !== index);
	}

	// ── Persistence ──

	function toInput(): SessionDefinitionInput {
		return {
			name: name.trim(),
			ad_hoc_segments: adHocSegments,
			track_protection_costs: trackProtectionCosts,
			// A dead reference is dropped on save: keeping it would fail
			// the server's active-target validation, and the editor showed
			// the hole explicitly before this point.
			roster: roster
				.filter((entry) => !entry.missing)
				.map((entry) => ({
					kind: entry.kind,
					ref_id: entry.refId === null ? null : Number(entry.refId),
					label: entry.label,
				})),
		};
	}

	/** Persist the draft. On a create, the new definition becomes the
	 * selection in the same motion (the environment contracts back with
	 * it selected); with a session running the selection is fixed, so a
	 * 409 there leaves the save intact and quiet. */
	async function save(): Promise<boolean> {
		if (!name.trim()) {
			authoringError = 'A session needs a name';
			return false;
		}
		saving = true;
		authoringError = null;
		try {
			if (editingId !== null) {
				await deps.updateDefinition(editingId, toInput());
			} else {
				const created = await deps.createDefinition(toInput());
				try {
					await deps.selectDefinition(Number(created.id));
				} catch (e) {
					if (!(e instanceof ApiError && e.kind === 'conflict')) throw e;
				}
			}
			await loadDefinitions();
			await deps.refreshTracking();
			close();
			return true;
		} catch (e) {
			authoringError = describeError(e, 'Failed to save the session');
			return false;
		} finally {
			saving = false;
		}
	}

	/** Archive the definition being edited (two-step: arm, then confirm).
	 * Its roster and instances stay intact; it simply leaves active play. */
	async function archiveEditing(): Promise<boolean> {
		if (editingId === null) return false;
		if (!archiveArmed) {
			archiveArmed = true;
			return false;
		}
		saving = true;
		authoringError = null;
		try {
			await deps.archiveDefinition(editingId);
		} catch (e) {
			authoringError = describeError(e, 'Failed to archive the session');
			saving = false;
			return false;
		}

		// The archive has committed. Leave the editor immediately so a
		// secondary refresh failure cannot offer a destructive retry.
		close();
		await loadDefinitions();
		try {
			await deps.refreshTracking();
		} catch (e) {
			error = describeError(e, 'Failed to refresh sessions');
		}
		saving = false;
		return true;
	}

	return {
		get definitions() {
			return definitions;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},
		get selecting() {
			return selecting;
		},
		get mode() {
			return mode;
		},
		get editingId() {
			return editingId;
		},
		get editingProtected() {
			return editingProtected;
		},
		get name() {
			return name;
		},
		set name(value: string) {
			name = value;
		},
		get adHocSegments() {
			return adHocSegments;
		},
		set adHocSegments(value: boolean) {
			adHocSegments = value;
		},
		get trackProtectionCosts() {
			return trackProtectionCosts;
		},
		set trackProtectionCosts(value: boolean) {
			trackProtectionCosts = value;
		},
		get roster() {
			return roster;
		},
		get saving() {
			return saving;
		},
		get authoringError() {
			return authoringError;
		},
		set authoringError(value: string | null) {
			authoringError = value;
		},
		get archiveArmed() {
			return archiveArmed;
		},
		set archiveArmed(value: boolean) {
			archiveArmed = value;
		},
		get families() {
			return families;
		},
		get quests() {
			return quests;
		},
		/** The quests to offer alongside the families (see above). */
		get standaloneQuests() {
			return standaloneQuests;
		},
		get catalogPlanet() {
			return catalogPlanet;
		},
		set catalogPlanet(value: string | null) {
			catalogPlanet = value;
		},
		get catalogFilter() {
			return catalogFilter;
		},
		set catalogFilter(value: string) {
			catalogFilter = value;
		},
		get catalogPlanets() {
			return catalogPlanets;
		},
		get catalogFamilies() {
			return catalogFamilies;
		},
		get catalogCategories() {
			return catalogCategories;
		},
		get sourcesLoading() {
			return sourcesLoading;
		},

		loadDefinitions,
		select,
		openCreate,
		openEdit,
		close,
		hasRosterRef,
		addFamily,
		addQuest,
		addQuests,
		removeEntry,
		save,
		archiveEditing,
	};
}

export type DefinitionsModel = ReturnType<typeof createDefinitionsModel>;

/** The model wired to the live API and the tracking store, with the
 * definition list loading immediately: the app's composition (the
 * deps-injected factory above is the testable seam). */
export function createLiveDefinitionsModel(): DefinitionsModel {
	const model = createDefinitionsModel({
		listDefinitions: getSessionDefinitions,
		createDefinition: createSessionDefinition,
		updateDefinition: updateSessionDefinition,
		archiveDefinition: archiveSessionDefinition,
		selectDefinition: (id) => selectDefinition(id),
		refreshTracking: () => hydrate(),
		listFamilies: getQuestFamilies,
		listQuests: getQuests,
	});
	void model.loadDefinitions();
	return model;
}
