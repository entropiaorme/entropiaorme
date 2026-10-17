/**
 * The quest board: what the dashboard's Quests widget shows, derived from the
 * curated catalogue, the quest and family rows, the rewards still to settle,
 * and which quests the overlay is recording.
 *
 * Pure: every instant comes in as `now` (epoch milliseconds), so the
 * derivation is directly testable and the caller owns the ticking.
 *
 * The board is shaped by the catalogue, not by the data: a planet's
 * categories and their entries appear in the order the catalogue authors
 * them, and a family is one row (its slot) whatever variant the day serves.
 * The player's own quests follow the curated ones on their planet, grouped
 * by their own categories.
 */

import type {
	Quest,
	QuestCatalogue,
	QuestCatalogueEntry,
	QuestCompletionTrigger,
	QuestFamily,
	UnresolvedQuestReward,
} from '$lib/types';

/** Where a row stands right now. */
export type BoardRowState =
	/** Nothing gates it: pick it up (a mission) or start it (a run). */
	| 'ready'
	/** In the mission log, or a run is open on it. */
	| 'running'
	/** Running, and the overlay is recording a stretch of it. */
	| 'recording'
	/** A hand-in run waiting for its reward clump. */
	| 'waiting'
	/** Done (or given up) and held until the gate lifts. */
	| 'cooling';

export interface BoardRow {
	/** Stable identity across refreshes. */
	key: string;
	/** A slot is a family: one row whatever variant the day serves. */
	kind: 'slot' | 'quest';
	label: string;
	/** A short qualifier beside the label ("Robot boss"). */
	detail: string | null;
	/** A slot's variant: the one in play, or the one last done today. */
	variant: string | null;
	/** Who hands the mission out, when the waypoint names them. */
	giver: string | null;
	/** The reward, unless the section states it once for every row. */
	rewardLabel: string | null;
	waypoint: string | null;
	state: BoardRowState;
	/** A cooling row was given up rather than finished. */
	abandoned: boolean;
	/** When a cooling row's gate lifts (epoch ms). */
	backAt: number | null;
	/** The quest the row's actions act on: the variant in play or last
	 * done, or the quest itself. Null for a slot nobody has played yet. */
	quest: Quest | null;
	trigger: QuestCompletionTrigger | null;
	/** One of the player's own quests (editable), not a curated one. */
	custom: boolean;
	/** Completions of this row's quests whose reward is still to settle,
	 * newest first. */
	unresolved: UnresolvedQuestReward[];
}

export interface BoardSection {
	key: string;
	name: string;
	/** The reward every row in the section shares, stated once. */
	rewardNote: string | null;
	/** How many rows are done for now (cooling). */
	done: number;
	rows: BoardRow[];
	/** The player's own quests rather than a curated category. */
	custom: boolean;
}

export interface BoardPlanet {
	name: string;
	/** The catalogue carries this planet. */
	curated: boolean;
	sections: BoardSection[];
}

export interface BoardInput {
	catalogue: QuestCatalogue | null;
	quests: Quest[];
	families: QuestFamily[];
	unresolved: UnresolvedQuestReward[];
	/** Quest ids the overlay is recording a stretch of. */
	recording: ReadonlySet<string>;
	/** Quest ids whose hand-in is waiting for the next reward clump. */
	waiting: ReadonlySet<string>;
	now: number;
}

const CUSTOM_SECTION = 'Your quests';

/** The part of a "Family: Variant" mission name after its family. */
export function variantOf(name: string): string {
	const idx = name.lastIndexOf(':');
	return idx < 0 ? name : name.slice(idx + 1).trim();
}

/** The label a waypoint carries (`/wp [Planet, x, y, z, Label]`), the NPC or
 * spot it points at; null when the waypoint has none. */
export function waypointLabel(waypoint: string | null): string | null {
	if (!waypoint) return null;
	const match = /\[([^\]]*)\]/.exec(waypoint);
	if (!match) return null;
	const parts = match[1].split(',');
	if (parts.length < 5) return null;
	const label = parts.slice(4).join(',').trim();
	return label || null;
}

function epochMs(iso: string | null): number | null {
	return iso ? new Date(iso).getTime() : null;
}

function later(a: number | null, b: number | null): number | null {
	if (a == null) return b;
	if (b == null) return a;
	return Math.max(a, b);
}

/** Whether the latest thing to happen to a quest was giving it up. */
function lastWasAbandon(quest: Quest): boolean {
	if (quest.lastAbandonedAt == null) return false;
	return quest.lastCompletedAt == null || quest.lastAbandonedAt > quest.lastCompletedAt;
}

/** The latest instant a quest was touched: started, finished, or given up. */
function lastTouched(quest: Quest): number {
	return Math.max(quest.lastStartedAt ?? 0, quest.lastCompletedAt ?? 0, quest.lastAbandonedAt ?? 0);
}

function runningState(quest: Quest, input: BoardInput): BoardRowState {
	if (input.waiting.has(quest.id)) return 'waiting';
	return input.recording.has(quest.id) ? 'recording' : 'running';
}

function unresolvedFor(ids: Set<string>, input: BoardInput): UnresolvedQuestReward[] {
	return input.unresolved
		.filter((reward) => ids.has(reward.questId))
		.sort((a, b) => b.completedAt - a.completedAt);
}

/** One standalone quest as a row. */
function questRow(
	quest: Quest,
	input: BoardInput,
	shape: {
		key: string;
		label: string;
		detail: string | null;
		rewardLabel: string | null;
		custom: boolean;
	},
): BoardRow {
	let state: BoardRowState = 'ready';
	let backAt: number | null = null;
	if (quest.startedAt != null) {
		state = runningState(quest, input);
	} else {
		const expiry = later(epochMs(quest.cooldownExpiresAt), epochMs(quest.familyCooldownExpiresAt));
		if (expiry != null && expiry > input.now) {
			state = 'cooling';
			backAt = expiry;
		}
	}
	const pickupLabel = waypointLabel(quest.waypoint);
	const giver =
		quest.completionTrigger === 'mission_log' && pickupLabel && pickupLabel !== shape.label
			? pickupLabel
			: null;
	return {
		key: shape.key,
		kind: 'quest',
		label: shape.label,
		detail: shape.detail,
		variant: null,
		giver,
		rewardLabel: shape.rewardLabel,
		waypoint: quest.waypoint,
		state,
		abandoned: state === 'cooling' && lastWasAbandon(quest),
		backAt,
		quest,
		trigger: quest.completionTrigger,
		custom: shape.custom,
		unresolved: unresolvedFor(new Set([quest.id]), input),
	};
}

/** One family as its slot row. */
function slotRow(
	family: QuestFamily,
	members: Quest[],
	input: BoardInput,
	shape: { key: string; label: string; rewardLabel: string | null; custom: boolean },
): BoardRow {
	const serving = members.find((member) => member.startedAt != null) ?? null;
	const touched = [...members].sort((a, b) => lastTouched(b) - lastTouched(a))[0] ?? null;
	const expiry = epochMs(family.cooldownExpiresAt);
	let state: BoardRowState = 'ready';
	let backAt: number | null = null;
	let quest: Quest | null = null;
	if (serving) {
		state = runningState(serving, input);
		quest = serving;
	} else if (expiry != null && expiry > input.now) {
		state = 'cooling';
		backAt = expiry;
		quest = touched;
	}
	return {
		key: shape.key,
		kind: 'slot',
		label: shape.label,
		detail: null,
		variant: quest ? variantOf(quest.name) : null,
		giver: null,
		rewardLabel: shape.rewardLabel,
		waypoint: serving?.waypoint ?? null,
		state,
		abandoned: state === 'cooling' && quest != null && lastWasAbandon(quest),
		backAt,
		quest,
		trigger: quest?.completionTrigger ?? 'mission_log',
		custom: shape.custom,
		unresolved: unresolvedFor(new Set(members.map((member) => member.id)), input),
	};
}

/** State a reward once when every row of a section shares it. */
function finishSection(key: string, name: string, rows: BoardRow[], custom: boolean): BoardSection {
	const rewards = new Set(rows.map((row) => row.rewardLabel));
	const shared = rows.length > 1 && rewards.size === 1 ? (rows[0]?.rewardLabel ?? null) : null;
	return {
		key,
		name,
		rewardNote: shared,
		done: rows.filter((row) => row.state === 'cooling').length,
		rows: shared ? rows.map((row) => ({ ...row, rewardLabel: null })) : rows,
		custom,
	};
}

function curatedRow(
	entry: QuestCatalogueEntry,
	input: BoardInput,
	familiesByKey: Map<string, QuestFamily>,
	questsByKey: Map<string, Quest>,
): BoardRow | null {
	if (entry.kind === 'family') {
		const family = familiesByKey.get(entry.key);
		if (!family) return null;
		const members = input.quests.filter((quest) => quest.familyId === family.id);
		return slotRow(family, members, input, {
			key: entry.key,
			label: entry.label,
			rewardLabel: entry.rewardLabel,
			custom: false,
		});
	}
	const quest = questsByKey.get(entry.key);
	if (!quest) return null;
	return questRow(quest, input, {
		key: entry.key,
		label: entry.label,
		detail: entry.detail,
		rewardLabel: entry.rewardLabel,
		custom: false,
	});
}

/** The player's own quests on a planet, grouped by their own categories.
 * Their families (if any) stay one slot row each. */
function customSections(
	planet: string,
	input: BoardInput,
	curatedFamilyIds: Set<string>,
): BoardSection[] {
	const own = input.quests.filter(
		(quest) =>
			quest.planet === planet &&
			!quest.curated &&
			(quest.familyId == null || !curatedFamilyIds.has(quest.familyId)),
	);
	const byCategory = new Map<string, Quest[]>();
	for (const quest of own) {
		const category = quest.category?.trim() || CUSTOM_SECTION;
		byCategory.set(category, [...(byCategory.get(category) ?? []), quest]);
	}
	const familiesById = new Map(input.families.map((family) => [family.id, family]));
	return [...byCategory.entries()]
		.sort(([a], [b]) => a.localeCompare(b, undefined, { sensitivity: 'base' }))
		.map(([category, quests]) => {
			const rows: BoardRow[] = [];
			const seenFamilies = new Set<string>();
			for (const quest of quests) {
				const family = quest.familyId ? familiesById.get(quest.familyId) : undefined;
				if (family) {
					if (seenFamilies.has(family.id)) continue;
					seenFamilies.add(family.id);
					rows.push(
						slotRow(
							family,
							input.quests.filter((member) => member.familyId === family.id),
							input,
							{ key: `family:${family.id}`, label: family.name, rewardLabel: null, custom: true },
						),
					);
				} else {
					rows.push(
						questRow(quest, input, {
							key: `quest:${quest.id}`,
							label: quest.name,
							detail: null,
							rewardLabel: quest.rewardDescription || null,
							custom: true,
						}),
					);
				}
			}
			rows.sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }));
			return finishSection(`custom:${planet}:${category}`, category, rows, true);
		});
}

/** Every planet the board can show: the catalogue's, in its order, then any
 * planet carrying only the player's own quests, alphabetically. */
export function buildBoard(input: BoardInput): BoardPlanet[] {
	const familiesByKey = new Map(
		input.families
			.filter((family) => family.catalogueKey)
			.map((family) => [family.catalogueKey as string, family]),
	);
	const questsByKey = new Map(
		input.quests
			.filter((quest) => quest.catalogueKey)
			.map((quest) => [quest.catalogueKey as string, quest]),
	);
	const curatedFamilyIds = new Set([...familiesByKey.values()].map((family) => family.id));

	const planets: BoardPlanet[] = [];
	for (const planet of input.catalogue?.planets ?? []) {
		const sections = planet.categories
			.map((category) => {
				const rows = category.entries
					.map((entry) => curatedRow(entry, input, familiesByKey, questsByKey))
					.filter((row): row is BoardRow => row !== null);
				return finishSection(category.key, category.name, rows, false);
			})
			.filter((section) => section.rows.length > 0);
		sections.push(...customSections(planet.name, input, curatedFamilyIds));
		if (sections.length > 0) planets.push({ name: planet.name, curated: true, sections });
	}
	const curatedNames = new Set(planets.map((planet) => planet.name));
	const ownPlanets = [
		...new Set(input.quests.filter((quest) => !quest.curated).map((quest) => quest.planet)),
	]
		.filter((name) => !curatedNames.has(name))
		.sort((a, b) => a.localeCompare(b));
	for (const name of ownPlanets) {
		const sections = customSections(name, input, curatedFamilyIds);
		if (sections.length > 0) planets.push({ name, curated: false, sections });
	}
	return planets;
}

/** When a gate lifts, as a player plans around it: a time today, "tomorrow"
 * with a time, or a weekday with a time within the week. */
export function formatBackAt(backAt: number, now: number): string {
	const at = new Date(backAt);
	const time = at.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	const startOfDay = (instant: Date) =>
		new Date(instant.getFullYear(), instant.getMonth(), instant.getDate()).getTime();
	const days = Math.round((startOfDay(at) - startOfDay(new Date(now))) / 86_400_000);
	if (days <= 0) return time;
	if (days === 1) return `tomorrow ${time}`;
	if (days < 7) return `${at.toLocaleDateString([], { weekday: 'short' })} ${time}`;
	return at.toLocaleDateString([], { day: 'numeric', month: 'short' });
}
