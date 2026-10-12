import { createSearchIndex, type SearchText } from './search';

export type SortDir = 'asc' | 'desc';
export type SortKey<T> = Extract<keyof T, string>;

export interface TableModelOptions<T> {
	/** Reactive rows source; called inside deriveds so state-backed sources re-track. */
	rows: () => T[];
	pageSize: number;
	/**
	 * The text a row is searched by (one string or several fields), matched
	 * through the shared search (`$lib/view/search`). Omit to make `search`
	 * a no-op.
	 */
	searchText?: (row: T) => SearchText;
	/**
	 * Category value per row; rows yielding null carry no category. Omit to make
	 * `category` a no-op and `categories` empty.
	 */
	categoryOf?: (row: T) => string | null;
	/**
	 * Per-key comparators in ascending base order; the active direction flips the
	 * result. Keys without one fall back to the default (null/undefined last in
	 * both directions, numbers numeric, everything else localeCompare).
	 */
	comparators?: Partial<Record<SortKey<T>, (a: T, b: T) => number>>;
	initialSort?: { key: SortKey<T>; dir: SortDir };
	/** Direction to use when a key becomes active for the first time.
	 * Text normally reads ascending, while magnitude comparisons often
	 * benefit from showing the largest values first. */
	defaultSortDirs?: Partial<Record<SortKey<T>, SortDir>>;
}

export interface TableModel<T> {
	search: string;
	category: string | null;
	page: number;
	readonly sortKey: SortKey<T> | undefined;
	readonly sortDir: SortDir;
	readonly categories: string[];
	readonly filtered: T[];
	readonly totalPages: number;
	readonly pageRows: T[];
	readonly rangeLabel: string;
	setSort(key: SortKey<T>): void;
	reset(): void;
}

function defaultCompare(aVal: unknown, bVal: unknown, dir: 1 | -1): number {
	if (aVal == null && bVal == null) return 0;
	if (aVal == null) return 1; // nulls last in both directions
	if (bVal == null) return -1;
	if (typeof aVal === 'number' && typeof bVal === 'number') return dir * (aVal - bVal);
	return dir * String(aVal).localeCompare(String(bVal));
}

/**
 * Reactive filter/sort/paginate view model for client-side tables: category and
 * text filters, per-column sort with direction toggling, and fixed-size paging
 * with clamping. Filter changes reset the page; a shrinking row set clamps the
 * page into range without losing the user's position unnecessarily.
 */
export function createTableModel<T>(options: TableModelOptions<T>): TableModel<T> {
	const { rows, pageSize, searchText, categoryOf, comparators, defaultSortDirs } = options;

	let search = $state('');
	let category = $state<string | null>(null);
	let rawPage = $state(0);
	let sortKey = $state<SortKey<T> | undefined>(options.initialSort?.key);
	let sortDir = $state<SortDir>(options.initialSort?.dir ?? 'asc');

	const categories = $derived.by(() => {
		if (!categoryOf) return [];
		const unique = new Set<string>();
		for (const row of rows()) {
			const value = categoryOf(row);
			if (value !== null) unique.add(value);
		}
		return [...unique].sort();
	});

	// Rebuilt only when the rows change, so each keystroke reuses the
	// folded row text.
	const searchIndex = $derived(searchText ? createSearchIndex(rows(), searchText) : null);

	const filtered = $derived.by(() => {
		let result = searchIndex ? searchIndex.filter(search) : rows();
		if (categoryOf && category !== null) {
			const wanted = category;
			result = result.filter((row) => categoryOf(row) === wanted);
		}
		if (sortKey !== undefined) {
			const key = sortKey;
			const dir = sortDir === 'asc' ? 1 : -1;
			const custom = comparators?.[key];
			result = [...result].sort((a, b) => {
				if (custom) return dir * custom(a, b);
				return defaultCompare(a[key], b[key], dir);
			});
		}
		return result;
	});

	const totalPages = $derived(Math.max(1, Math.ceil(filtered.length / pageSize)));
	// Clamped view over the raw page so a shrinking filtered set can never leave
	// the model pointing past the last page.
	const page = $derived(Math.max(0, Math.min(rawPage, totalPages - 1)));
	const pageRows = $derived(filtered.slice(page * pageSize, (page + 1) * pageSize));
	const rangeLabel = $derived.by(() => {
		const total = filtered.length;
		if (total === 0) return '0-0 of 0';
		const start = page * pageSize + 1;
		const end = Math.min((page + 1) * pageSize, total);
		return `${start}-${end} of ${total}`;
	});

	return {
		get search() {
			return search;
		},
		set search(value: string) {
			if (value === search) return;
			search = value;
			rawPage = 0;
		},
		get category() {
			return category;
		},
		set category(value: string | null) {
			if (value === category) return;
			category = value;
			rawPage = 0;
		},
		get page() {
			return page;
		},
		set page(value: number) {
			rawPage = Math.max(0, value);
		},
		get sortKey() {
			return sortKey;
		},
		get sortDir() {
			return sortDir;
		},
		get categories() {
			return categories;
		},
		get filtered() {
			return filtered;
		},
		get totalPages() {
			return totalPages;
		},
		get pageRows() {
			return pageRows;
		},
		get rangeLabel() {
			return rangeLabel;
		},
		setSort(key: SortKey<T>) {
			if (sortKey === key) {
				sortDir = sortDir === 'asc' ? 'desc' : 'asc';
			} else {
				sortKey = key;
				sortDir = defaultSortDirs?.[key] ?? 'asc';
			}
			rawPage = 0;
		},
		reset() {
			search = '';
			category = null;
			rawPage = 0;
			sortKey = options.initialSort?.key;
			sortDir = options.initialSort?.dir ?? 'asc';
		},
	};
}
