/**
 * A persistent set of names the player has hidden from a ranking: the
 * shared machinery behind "hide this, it is not something I can pursue".
 * The set is a UI preference, read once on first use and saved on every
 * change, so it holds across app restarts. A ranking keeps its hidden
 * entries reachable by ordering them last (`orderByVisibility`) with a
 * restore control, rather than dropping them.
 */

import { getPreference, setPreference } from '$lib/preferences';

/** The ranking with hidden entries moved to the end, each part keeping
 * its ranked order. */
export function orderByVisibility<T>(items: readonly T[], isHidden: (item: T) => boolean): T[] {
	return [...items.filter((item) => !isHidden(item)), ...items.filter(isHidden)];
}

export function createHiddenSet(preferenceKey: string) {
	let hidden = $state<string[]>([]);
	let loaded: Promise<void> | null = null;

	/** Read the saved set, once; later calls share the first read. A
	 * malformed saved value reads as empty. */
	function load(): Promise<void> {
		loaded ??= getPreference<unknown>(preferenceKey, []).then((saved) => {
			if (Array.isArray(saved)) hidden = saved.filter((name) => typeof name === 'string');
		});
		return loaded;
	}

	function has(name: string): boolean {
		return hidden.includes(name);
	}

	function hide(name: string) {
		if (hidden.includes(name)) return;
		hidden = [...hidden, name];
		void setPreference(preferenceKey, hidden);
	}

	function restore(name: string) {
		if (!hidden.includes(name)) return;
		hidden = hidden.filter((entry) => entry !== name);
		void setPreference(preferenceKey, hidden);
	}

	return {
		get names(): readonly string[] {
			return hidden;
		},
		load,
		has,
		hide,
		restore,
	};
}

export type HiddenSet = ReturnType<typeof createHiddenSet>;
