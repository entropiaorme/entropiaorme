/**
 * The quest board's view model: which planet the board shows, the rewards
 * still to settle, and the verbs a row offers. The board's shape itself is
 * the pure `buildBoard`; this module owns state, effects and writes.
 *
 * The quest and family rows come from the dashboard's shared quests model
 * (which already polls them); the board adds the catalogue, read once, and
 * the unresolved rewards, re-read whenever a completion moves.
 */

import type { ActivityOptionsResult, SessionDefinition, SessionDefinitionInput } from '$lib/api';
import type {
	Quest,
	QuestCatalogue,
	QuestFamily,
	QuestRewardReviewInput,
	UnresolvedQuestReward,
} from '$lib/types';
import { describeError } from '$lib/view/errorState';
import { type BoardPlanet, type BoardRow, buildBoard, unplacedRewards } from './questBoard';

export interface QuestBoardDeps {
	quests(): Quest[];
	families(): QuestFamily[];
	activityOptions(): ActivityOptionsResult | null;
	/** The session a start would run as (or the one running). */
	definition(): SessionDefinition | null;
	getCatalogue(): Promise<QuestCatalogue>;
	getUnresolved(): Promise<UnresolvedQuestReward[]>;
	startQuest(id: string): Promise<unknown>;
	completeQuest(id: string): Promise<unknown>;
	abandonQuest(id: string): Promise<unknown>;
	cancelQuest(id: string, undoReward: boolean): Promise<unknown>;
	reviewReward(input: QuestRewardReviewInput): Promise<void>;
	updateDefinition(id: string, input: SessionDefinitionInput): Promise<unknown>;
	/** Re-read the quest rows and the Activities picture after a write. */
	refresh(): Promise<void>;
	/** Re-read the definitions after one changed. */
	refreshDefinitions(): Promise<void>;
	getPreference<T>(key: string, fallback: T): Promise<T>;
	setPreference<T>(key: string, value: T): Promise<void>;
}

/** The busy key for the rewards no row carries. */
export const EARLIER_KEY = 'earlier-rewards';

/** The persisted planet the board last showed. */
export const BOARD_PLANET_PREFERENCE = 'quest_board_planet';

/** The definition as an update payload, with its quest planet replaced. */
function withQuestPlanet(
	definition: SessionDefinition,
	planet: string | null,
): SessionDefinitionInput {
	return {
		name: definition.name,
		ad_hoc_segments: definition.adHocSegments,
		track_protection_costs: definition.trackProtectionCosts,
		quest_planet: planet,
		roster: definition.roster
			.filter((entry) => entry.displayName !== null)
			.map((entry) => ({
				kind: entry.kind,
				ref_id: entry.refId === null ? null : Number(entry.refId),
				label: entry.label,
			})),
	};
}

export function createQuestBoardModel(deps: QuestBoardDeps) {
	let catalogue = $state<QuestCatalogue | null>(null);
	let unresolved = $state<UnresolvedQuestReward[]>([]);
	let chosenPlanet = $state<string | null>(null);
	let now = $state(Date.now());
	/** The row whose reward panel is open. */
	let settling = $state<string | null>(null);
	/** The row a write is in flight for, so its controls hold still. */
	let busy = $state<string | null>(null);
	let error = $state<string | null>(null);

	const recording = $derived(
		new Set(
			(deps.activityOptions()?.active ?? [])
				.filter((activity) => activity.kind === 'quest' && activity.questId != null)
				.map((activity) => String(activity.questId)),
		),
	);
	const waiting = $derived(
		new Set(
			(deps.activityOptions()?.options ?? [])
				.filter((option) => option.handInWaiting && option.questId != null)
				.map((option) => String(option.questId)),
		),
	);

	const planets = $derived<BoardPlanet[]>(
		buildBoard({
			catalogue,
			quests: deps.quests(),
			families: deps.families(),
			unresolved,
			recording,
			waiting,
			now,
		}),
	);

	/** Rewards to settle that no row carries (their quest was deleted). */
	const earlierRewards = $derived(unplacedRewards(planets, unresolved));

	/** The planet the selected session offers in the overlay. */
	const offeredPlanet = $derived(deps.definition()?.questPlanet ?? null);

	/** The planet on screen: the player's choice, else the one the session
	 * offers, else the first the board carries. */
	const planet = $derived.by(() => {
		const names = planets.map((entry) => entry.name);
		if (chosenPlanet && names.includes(chosenPlanet)) return chosenPlanet;
		if (offeredPlanet && names.includes(offeredPlanet)) return offeredPlanet;
		return names[0] ?? null;
	});
	const current = $derived(planets.find((entry) => entry.name === planet) ?? null);

	/** A signature of every recorded completion and abandon: when it moves,
	 * the rewards to settle may have too. */
	const completionSignature = $derived(
		deps
			.quests()
			.map((quest) => `${quest.id}:${quest.lastCompletedAt ?? ''}`)
			.join('|'),
	);

	async function loadCatalogue() {
		try {
			catalogue = await deps.getCatalogue();
		} catch (e) {
			error = describeError(e, 'Failed to load the quest catalogue');
		}
	}

	function rowByKey(key: string): BoardRow | null {
		for (const entry of planets) {
			for (const section of entry.sections) {
				const row = section.rows.find((candidate) => candidate.key === key);
				if (row) return row;
			}
		}
		return null;
	}

	async function loadUnresolved() {
		try {
			unresolved = await deps.getUnresolved();
		} catch {
			// Deliberate swallow: the panel keeps the last good list and the
			// next completion re-reads it.
			return;
		}
		// A panel with nothing left to settle closes itself.
		if (settling !== null && (rowByKey(settling)?.unresolved.length ?? 0) === 0) {
			settling = null;
		}
	}

	async function init() {
		chosenPlanet = await deps.getPreference<string | null>(BOARD_PLANET_PREFERENCE, null);
		await Promise.all([loadCatalogue(), loadUnresolved()]);
	}

	function selectPlanet(name: string) {
		chosenPlanet = name;
		settling = null;
		void deps.setPreference(BOARD_PLANET_PREFERENCE, name);
	}

	/** Run one write for a row, holding its controls and reporting failure
	 * on the board rather than swallowing it. */
	async function act(row: BoardRow, verb: () => Promise<unknown>, failure: string) {
		busy = row.key;
		error = null;
		try {
			await verb();
			await deps.refresh();
		} catch (e) {
			error = describeError(e, failure);
		} finally {
			busy = null;
		}
	}

	function start(row: BoardRow) {
		const quest = row.quest;
		if (!quest) return;
		return act(row, () => deps.startQuest(quest.id), 'Failed to start the quest');
	}

	function complete(row: BoardRow) {
		const quest = row.quest;
		if (!quest) return;
		return act(row, () => deps.completeQuest(quest.id), 'Failed to complete the quest');
	}

	function abandon(row: BoardRow) {
		const quest = row.quest;
		if (!quest) return;
		return act(row, () => deps.abandonQuest(quest.id), 'Failed to abandon the quest');
	}

	function resetCooldown(row: BoardRow) {
		const quest = row.quest;
		if (!quest) return;
		return act(row, () => deps.cancelQuest(quest.id, false), 'Failed to reset the cooldown');
	}

	function undoReward(row: BoardRow) {
		const quest = row.quest;
		if (!quest) return;
		return act(row, () => deps.cancelQuest(quest.id, true), 'Failed to undo the reward');
	}

	function toggleSettle(row: BoardRow) {
		settling = settling === row.key ? null : row.key;
	}

	async function settle(row: BoardRow, input: QuestRewardReviewInput) {
		await act(row, () => deps.reviewReward(input), 'Failed to settle the reward');
		await loadUnresolved();
	}

	/** Settle one of the rewards no row carries. */
	async function settleEarlier(input: QuestRewardReviewInput) {
		busy = EARLIER_KEY;
		error = null;
		try {
			await deps.reviewReward(input);
		} catch (e) {
			error = describeError(e, 'Failed to settle the reward');
		} finally {
			busy = null;
		}
		await loadUnresolved();
	}

	/** Make the selected session offer this planet's quests in the overlay. */
	async function offerInOverlay(name: string) {
		const definition = deps.definition();
		if (!definition) return;
		error = null;
		try {
			await deps.updateDefinition(definition.id, withQuestPlanet(definition, name));
			await deps.refreshDefinitions();
			await deps.refresh();
		} catch (e) {
			error = describeError(e, 'Failed to update the session');
		}
	}

	return {
		get planets() {
			return planets;
		},
		get planet() {
			return planet;
		},
		get current() {
			return current;
		},
		get offeredPlanet() {
			return offeredPlanet;
		},
		get earlierRewards() {
			return earlierRewards;
		},
		get definitionName() {
			return deps.definition()?.name ?? null;
		},
		get catalogueLoaded() {
			return catalogue !== null;
		},
		get now() {
			return now;
		},
		set now(value: number) {
			now = value;
		},
		get settling() {
			return settling;
		},
		get busy() {
			return busy;
		},
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},
		get completionSignature() {
			return completionSignature;
		},
		init,
		loadUnresolved,
		selectPlanet,
		start,
		complete,
		abandon,
		resetCooldown,
		undoReward,
		toggleSettle,
		settle,
		settleEarlier,
		offerInOverlay,
	};
}

export type QuestBoardModel = ReturnType<typeof createQuestBoardModel>;
