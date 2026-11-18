import { createSearchIndex, isSearchQuery, type SearchIndex, type SearchText } from './search';

/**
 * A search over a static catalogue the backend serves whole: loaded on the
 * first query, kept for the window's lifetime, and matched locally through
 * the shared search, best first. A failed load is not kept, so the next
 * query tries again.
 */
export function createCatalogueSearch<T>(
	load: () => Promise<T[]>,
	text: (item: T) => SearchText,
	limit: number,
): (query: string) => Promise<T[]> {
	let index: Promise<SearchIndex<T>> | null = null;
	return async (query) => {
		if (!isSearchQuery(query)) return [];
		index ??= load()
			.then((items) => createSearchIndex(items, text))
			.catch((error: unknown) => {
				index = null;
				throw error;
			});
		return (await index).rank(query, limit);
	};
}
