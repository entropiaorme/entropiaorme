/**
 * The labels behind Equipment's Mobs tab: what a mob is shown as where the
 * game's own name for it is not what the player wants to read.
 *
 * A label changes only what is shown. The mob stays the one its kills were
 * recorded as, so kills made before and after a label are one mob's. A mob
 * graded by number is labelled as its plate prints it without being asked
 * (`04` where the bundled list has `4`); those are listed too, and the
 * player's own label replaces one.
 */

import { clearMobLabel, getMobLabels, type MobLabel, setMobLabel } from '$lib/api';
import { describeError } from '$lib/view/errorState';

export interface MobLabelDeps {
	list: () => Promise<MobLabel[]>;
	set: (mob: string, label: string) => Promise<MobLabel[]>;
	clear: (species: string, maturity: string) => Promise<MobLabel[]>;
}

const LIVE: MobLabelDeps = {
	list: getMobLabels,
	set: setMobLabel,
	clear: clearMobLabel,
};

/** A stable key for a labelled mob. */
export function labelKey(label: Pick<MobLabel, 'species' | 'maturity'>): string {
	return `${label.species}\u0000${label.maturity}`;
}

export function createMobLabels(deps: MobLabelDeps = LIVE) {
	let labels = $state<MobLabel[]>([]);
	let loaded = $state(false);
	let error = $state<string | null>(null);
	let busy = $state(false);
	let mob = $state('');
	let label = $state('');

	async function load() {
		try {
			labels = await deps.list();
			error = null;
		} catch (e) {
			error = describeError(e, 'Failed to load mob labels');
		} finally {
			loaded = true;
		}
	}

	async function write(action: () => Promise<MobLabel[]>, failure: string) {
		busy = true;
		try {
			labels = await action();
			error = null;
			mob = '';
			label = '';
		} catch (e) {
			error = describeError(e, failure);
		} finally {
			busy = false;
		}
	}

	return {
		get labels() {
			return labels;
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
		get mob() {
			return mob;
		},
		set mob(value: string) {
			mob = value;
		},
		get label() {
			return label;
		},
		set label(value: string) {
			label = value;
		},
		/** Both halves are typed: the label can be saved. */
		get ready() {
			return !busy && mob.trim() !== '' && label.trim() !== '';
		},
		load,
		/** Put a labelled mob back in the entry, to change its label. */
		edit(held: MobLabel) {
			mob = held.printed;
			label = held.label;
		},
		save() {
			if (mob.trim() === '' || label.trim() === '') return Promise.resolve();
			const typed = mob.trim();
			const shown = label.trim();
			return write(() => deps.set(typed, shown), 'Failed to label the mob');
		},
		remove(held: MobLabel) {
			return write(() => deps.clear(held.species, held.maturity), 'Failed to remove the label');
		},
	};
}

export type MobLabelsModel = ReturnType<typeof createMobLabels>;
