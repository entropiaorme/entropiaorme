/**
 * The overlay mob control's typed entry: the catalogue typeahead behind its
 * input, the suggestion menu it drives, and what a pick or a clear does.
 *
 * The menu is the overlay's shared popup window, which the owner presents
 * (`menu`); this module decides when it shows and what it holds. A pick goes
 * to the mob source (`MobSourceModel.choose`): the typed mob at once, or,
 * while steered, a creature waiting in the panel to be pinned or kept as the
 * backstop. Only a catalogue creature can be entered, so Enter takes the top
 * match rather than inventing a name.
 *
 * Call it while a component initialises: it runs effects that follow the
 * input and the search, and tears them down with the component.
 */

import { untrack } from 'svelte';
import { ApiError, type ManualMobSuggestion } from '$lib/api';
import { createTypeahead } from '$lib/view/typeahead.svelte';
import { computeMenuWidth, type OverlayMenuState } from '$lib/windows/overlayMenu';
import type { MobSourceModel } from './mobSourceModel.svelte';

/** How long the menu stays after the input blurs, so a pointer landing on a
 * suggestion (which moves focus into the menu window) is a pick, not a
 * dismissal. */
const BLUR_CLOSE_MS = 120;

export interface MobEntryDeps {
	source: MobSourceModel;
	/** The strip is in a state that takes a typed creature. */
	available: () => boolean;
	search: (query: string) => Promise<ManualMobSuggestion[]>;
	menu: {
		show: (anchor: HTMLElement, state: OverlayMenuState) => Promise<void>;
		hide: () => Promise<void>;
		/** The shared menu is showing this entry's suggestions. */
		isOpen: () => boolean;
		/** The menu window holds focus (a pick in progress). */
		isFocused: () => Promise<boolean>;
	};
}

export function createMobEntry(deps: MobEntryDeps) {
	let query = $state('');
	let input = $state<HTMLInputElement | null>(null);
	let focused = $state(false);
	let error = $state<string | null>(null);
	let selecting = $state(false);
	let closeTimer: ReturnType<typeof setTimeout> | undefined;

	// Search failures read in the overlay's established wording.
	const typeahead = createTypeahead<ManualMobSuggestion>({
		search: async (text) => {
			try {
				return await deps.search(text);
			} catch (e) {
				throw new Error(e instanceof ApiError ? e.message : 'Mob lookup failed');
			}
		},
		debounceMs: 120,
		minLength: 1,
	});
	const shown = $derived(deps.available() && deps.source.inputShown);

	function clearCloseTimer() {
		if (closeTimer === undefined) return;
		clearTimeout(closeTimer);
		closeTimer = undefined;
	}

	function menuState(anchorWidth: number): OverlayMenuState | null {
		const trimmed = query.trim();
		const results = typeahead.results;
		if (!(typeahead.loading || error || results.length > 0 || trimmed)) return null;
		const labels = typeahead.loading
			? ['Searching...']
			: error
				? [error]
				: results.length > 0
					? results.map((option) => option.display)
					: ['No matches'];
		return {
			kind: 'mob',
			width: computeMenuWidth(anchorWidth, labels, 28),
			query: trimmed,
			loading: typeahead.loading,
			error,
			mobSuggestions: results,
		};
	}

	async function openMenu() {
		if (!input) return;
		const state = menuState(input.getBoundingClientRect().width);
		if (state) await deps.menu.show(input, state);
	}

	async function closeMenu() {
		clearCloseTimer();
		if (deps.menu.isOpen()) await deps.menu.hide();
	}

	/** Forget what was typed once it has been used. */
	async function reset() {
		query = '';
		typeahead.cancel();
		await closeMenu();
	}

	async function select(option: ManualMobSuggestion) {
		clearCloseTimer();
		selecting = true;
		error = null;
		if (await deps.source.choose(option)) {
			await reset();
		} else {
			error = deps.source.error;
			deps.source.dismissError();
		}
		selecting = false;
	}

	// Drive the search from the input. Hiding the input or emptying the
	// query suspends the search and closes the menu, keeping the text.
	$effect(() => {
		if (!shown || !query.trim()) {
			if (shown) typeahead.query = query;
			typeahead.cancel();
			void closeMenu();
			return;
		}
		typeahead.query = query;
		typeahead.refresh();
	});

	// Present the search lifecycle in the menu: mirror the settled error and
	// re-sync the menu at each transition while the input is focused or the
	// menu already open. Only the lifecycle is tracked, so a bare focus
	// change cannot re-open a menu with nothing new to show.
	$effect(() => {
		void typeahead.loading;
		void typeahead.results;
		error = typeahead.error;
		untrack(() => {
			if (!shown || !query.trim()) return;
			if (focused || deps.menu.isOpen()) void openMenu();
		});
	});

	$effect(() => () => {
		clearCloseTimer();
		typeahead.destroy();
	});

	return {
		get query() {
			return query;
		},
		set query(value: string) {
			query = value;
		},
		get input() {
			return input;
		},
		set input(element: HTMLInputElement | null) {
			input = element;
		},
		/** The input is shown at all. */
		get shown() {
			return shown;
		},
		get selecting() {
			return selecting;
		},
		get releasing() {
			return deps.source.busy === 'clear';
		},
		clearCloseTimer,
		select,

		focus() {
			clearCloseTimer();
			focused = true;
			if (query.trim() && (typeahead.results.length > 0 || typeahead.loading || error)) {
				void openMenu();
			}
		},
		blur() {
			focused = false;
			clearCloseTimer();
			closeTimer = setTimeout(() => {
				closeTimer = undefined;
				void (async () => {
					if (await deps.menu.isFocused()) return;
					await closeMenu();
				})();
			}, BLUR_CLOSE_MS);
		},
		async keydown(event: KeyboardEvent) {
			if (event.key === 'Escape') {
				await closeMenu();
				return;
			}
			if (event.key !== 'Enter') return;
			const top = typeahead.results[0];
			if (top) {
				event.preventDefault();
				await select(top);
			}
		},
		/** Clear the typed mob (the strip's own clear). */
		async release() {
			if (!(await deps.source.clearTyped())) return;
			await reset();
			error = null;
		},
	};
}

export type MobEntry = ReturnType<typeof createMobEntry>;
