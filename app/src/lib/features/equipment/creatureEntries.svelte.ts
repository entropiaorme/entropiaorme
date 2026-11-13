/**
 * The mob list behind Equipment's Mobs tab: the mobs kept for the
 * name reader beside the bundled list, and the entry that adds or edits one.
 *
 * A mob is typed as its plate prints it. A name ending in a number
 * (`Survival Zombie 04`, `Kal-Rahm The Cannibal S04`) is kept at every
 * number, as the bundled list's mobs graded by number are. While the
 * player types, the entry is checked against what the reader already knows,
 * so a mob it reads already, or one it could not tell from another, is
 * said before anything is saved.
 */

import {
	type CreatureEntry,
	type CreatureEntryCheck,
	checkCreatureEntry,
	getCreatureEntries,
	removeCreatureEntry,
	saveCreatureEntry,
} from '$lib/api';
import { describeError } from '$lib/view/errorState';

/** How long typing rests before the entry is checked. */
const CHECK_DELAY_MS = 250;

export interface CreatureEntryDeps {
	list: () => Promise<CreatureEntry[]>;
	check: (text: string, replacing: string | null) => Promise<CreatureEntryCheck>;
	save: (
		text: string,
		species: string | null,
		replacing: string | null,
	) => Promise<CreatureEntry[]>;
	remove: (name: string) => Promise<CreatureEntry[]>;
}

const LIVE: CreatureEntryDeps = {
	list: getCreatureEntries,
	check: checkCreatureEntry,
	save: saveCreatureEntry,
	remove: removeCreatureEntry,
};

/** A kept mob as its plate prints it: the name, then its maturity. */
export function plateText(entry: Pick<CreatureEntry, 'name' | 'maturity'>): string {
	return entry.maturity ? `${entry.name} ${entry.maturity}` : entry.name;
}

export function createCreatureEntries(deps: CreatureEntryDeps = LIVE) {
	let entries = $state<CreatureEntry[]>([]);
	let loaded = $state(false);
	let error = $state<string | null>(null);
	let busy = $state(false);
	/** The kept mob being edited, by name; `null` while adding. */
	let editing = $state<string | null>(null);
	let text = $state('');
	let species = $state('');
	let check = $state<CreatureEntryCheck | null>(null);
	let checking = $state(false);
	let timer: ReturnType<typeof setTimeout> | null = null;
	let asked = 0;

	async function load() {
		try {
			entries = await deps.list();
			error = null;
		} catch (e) {
			error = describeError(e, 'Failed to load kept mobs');
		} finally {
			loaded = true;
		}
	}

	function clearTimer() {
		if (timer !== null) {
			clearTimeout(timer);
			timer = null;
		}
	}

	/** Check what is typed once typing rests; the latest answer wins. */
	function scheduleCheck() {
		clearTimer();
		const typed = text.trim();
		if (!typed) {
			check = null;
			checking = false;
			return;
		}
		checking = true;
		timer = setTimeout(() => {
			timer = null;
			const ask = ++asked;
			deps
				.check(typed, editing)
				.then((answer) => {
					if (ask === asked) check = answer;
				})
				.catch((e) => {
					if (ask === asked) check = { kind: 'refused', reason: describeError(e) };
				})
				.finally(() => {
					if (ask === asked) checking = false;
				});
		}, CHECK_DELAY_MS);
	}

	function reset() {
		clearTimer();
		asked++;
		editing = null;
		text = '';
		species = '';
		check = null;
		checking = false;
	}

	async function write(action: () => Promise<CreatureEntry[]>, failure: string) {
		busy = true;
		try {
			entries = await action();
			error = null;
			reset();
		} catch (e) {
			error = describeError(e, failure);
		} finally {
			busy = false;
		}
	}

	return {
		get entries() {
			return entries;
		},
		get loaded() {
			return loaded;
		},
		get error() {
			return error;
		},
		dismissError() {
			error = null;
		},
		get busy() {
			return busy;
		},
		get editing() {
			return editing;
		},
		get text() {
			return text;
		},
		set text(value: string) {
			text = value;
			scheduleCheck();
		},
		get species() {
			return species;
		},
		set species(value: string) {
			species = value;
		},
		get check() {
			return check;
		},
		get checking() {
			return checking;
		},
		/** The entry can be kept as it stands. */
		get ready() {
			return !busy && !checking && check?.kind === 'ready';
		},
		load,
		edit(entry: CreatureEntry) {
			reset();
			editing = entry.name;
			species = entry.species ?? '';
			text = plateText(entry);
			scheduleCheck();
		},
		cancel: reset,
		save() {
			if (check?.kind !== 'ready') return Promise.resolve();
			const typed = text.trim();
			const kept = species.trim() || null;
			const replacing = editing;
			return write(() => deps.save(typed, kept, replacing), 'Failed to keep the mob');
		},
		remove(entry: CreatureEntry) {
			return write(() => deps.remove(entry.name), 'Failed to remove the mob');
		},
		destroy() {
			clearTimer();
		},
	};
}

export type CreatureEntriesModel = ReturnType<typeof createCreatureEntries>;
