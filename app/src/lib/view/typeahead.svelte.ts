export interface TypeaheadOptions<T> {
	search: (query: string) => Promise<T[]>;
	/** Debounce between the last keystroke and the search firing. Default 200ms. */
	debounceMs?: number;
	/** Minimum trimmed query length before a search fires. Default 2. */
	minLength?: number;
	/** Display label for an item; `select` snaps the query onto it when provided. */
	labelOf?: (item: T) => string;
}

export interface Typeahead<T> {
	query: string;
	readonly results: T[];
	readonly selected: T | null;
	readonly loading: boolean;
	readonly error: string | null;
	/**
	 * The trimmed query the current results (or error) answer, once its
	 * search settles; null while nothing has been searched. A presenter
	 * compares it with the live query to tell "no matches" from "not yet".
	 */
	readonly settledQuery: string | null;
	select(item: T): void;
	/**
	 * Put text in the box without asking for suggestions: sets the query,
	 * cancels any pending search and blanks the results. For text that did
	 * not come from typing (a screen read, a restored draft), where a
	 * dropdown would flash open over the form and close again unbidden.
	 */
	fill(value: string): void;
	clear(): void;
	/**
	 * Re-run the search for the current query, dropping any in-flight
	 * response first. For when the search's behaviour changed out from under
	 * an unchanged query (e.g. the endpoint it dispatches to switched), so a
	 * response computed under the old behaviour must not land.
	 */
	refresh(): void;
	/**
	 * Cancel pending and in-flight work and blank the results, loading and
	 * error state, keeping the query (and any selection). For suspending the
	 * picker while its input is hidden without losing the typed text.
	 */
	cancel(): void;
	destroy(): void;
}

/**
 * Debounced async picker view model. No search fires while an item is selected
 * or the trimmed query is under `minLength`; responses are dropped as stale
 * unless the query they answered still matches (and no selection, refresh,
 * cancel, or teardown intervened), so out-of-order resolutions can never
 * overwrite fresher state.
 */
export function createTypeahead<T>(options: TypeaheadOptions<T>): Typeahead<T> {
	const { search, debounceMs = 200, minLength = 2, labelOf } = options;

	let query = $state('');
	let results = $state<T[]>([]);
	let selected = $state<T | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let settledQuery = $state<string | null>(null);

	let timer: ReturnType<typeof setTimeout> | null = null;
	let destroyed = false;
	// Bumped by refresh()/cancel() to invalidate in-flight responses whose
	// query still matches (the query guard alone cannot catch those).
	let epoch = 0;

	function cancelPending(): void {
		if (timer !== null) {
			clearTimeout(timer);
			timer = null;
		}
	}

	function isCurrent(searched: string, searchedEpoch: number): boolean {
		return !destroyed && selected === null && epoch === searchedEpoch && query.trim() === searched;
	}

	async function run(searched: string, searchedEpoch: number): Promise<void> {
		try {
			const items = await search(searched);
			if (isCurrent(searched, searchedEpoch)) {
				results = items;
				error = null;
				settledQuery = searched;
			}
		} catch (e) {
			if (isCurrent(searched, searchedEpoch)) {
				results = [];
				error = e instanceof Error ? e.message : String(e);
				settledQuery = searched;
			}
		} finally {
			if (isCurrent(searched, searchedEpoch)) loading = false;
		}
	}

	function schedule(): void {
		cancelPending();
		const trimmed = query.trim();
		if (selected !== null || trimmed.length < minLength) {
			results = [];
			loading = false;
			error = null;
			settledQuery = null;
			return;
		}
		timer = setTimeout(() => {
			timer = null;
			loading = true;
			void run(trimmed, epoch);
		}, debounceMs);
	}

	return {
		get query() {
			return query;
		},
		set query(value: string) {
			if (value === query) return;
			query = value;
			schedule();
		},
		get results() {
			return results;
		},
		get selected() {
			return selected;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		get settledQuery() {
			return settledQuery;
		},
		select(item: T) {
			cancelPending();
			selected = item;
			if (labelOf) query = labelOf(item);
			results = [];
			loading = false;
			error = null;
			settledQuery = null;
		},
		fill(value: string) {
			cancelPending();
			selected = null;
			query = value;
			results = [];
			loading = false;
			error = null;
			settledQuery = null;
		},
		clear() {
			cancelPending();
			selected = null;
			query = '';
			results = [];
			loading = false;
			error = null;
			settledQuery = null;
		},
		refresh() {
			epoch += 1;
			schedule();
		},
		cancel() {
			epoch += 1;
			cancelPending();
			results = [];
			loading = false;
			error = null;
			settledQuery = null;
		},
		destroy() {
			destroyed = true;
			cancelPending();
		},
	};
}
