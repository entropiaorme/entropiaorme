import { get } from 'svelte/store';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Mock the preferences seam so getPreference/setPreference are observable and
// do not touch Tauri/localStorage. getPreference is keyed by its first arg so
// initNews's parallel Promise.all reads resolve per-key.
vi.mock('./preferences', () => ({
	getPreference: vi.fn(),
	setPreference: vi.fn(),
}));

import { getPreference, setPreference } from './preferences';
import {
	initNews,
	markNewsAsRead,
	markNewsOptInSeen,
	NEWS_PREFERENCE_KEYS,
	newsCache,
	newsHasUnread,
	newsLastViewedAt,
	newsOptIn,
	newsOptInSeen,
	persistNewsCache,
	purgeNewsCache,
	setNewsOptIn,
	type NewsCache,
	type NewsEntry,
} from './news';

const getPreferenceMock = vi.mocked(getPreference);
const setPreferenceMock = vi.mocked(setPreference);

function makeEntry(overrides: Partial<NewsEntry> = {}): NewsEntry {
	return {
		slug: 'sample-slug',
		title: 'Sample title',
		date: '2026-01-01',
		category: 'article',
		body: 'Sample body.',
		...overrides,
	};
}

function makeCache(items: NewsEntry[], fetchedAt = '2026-05-01T00:00:00Z'): NewsCache {
	return { items, fetchedAt };
}

// Configure getPreference to answer per key. Any key not present resolves to
// the defaultValue passed by the caller, mirroring the real signature.
function stubPreferences(values: Record<string, unknown>): void {
	getPreferenceMock.mockImplementation(async (key: string, defaultValue: unknown) => {
		return key in values ? values[key] : defaultValue;
	});
}

beforeEach(() => {
	// Reset module-level store state so tests are order-independent.
	newsOptIn.set(false);
	newsOptInSeen.set(false);
	newsCache.set(null);
	newsLastViewedAt.set(null);
	getPreferenceMock.mockReset();
	setPreferenceMock.mockReset();
	setPreferenceMock.mockResolvedValue(undefined);
});

afterEach(() => {
	vi.clearAllMocks();
});

describe('initNews', () => {
	it('discards a malformed cache value (empty object) and purges the stale shape', async () => {
		stubPreferences({
			[NEWS_PREFERENCE_KEYS.optIn]: true,
			[NEWS_PREFERENCE_KEYS.optInSeen]: true,
			[NEWS_PREFERENCE_KEYS.cache]: {},
			[NEWS_PREFERENCE_KEYS.lastViewedAt]: '2026-02-02',
		});

		await initNews();

		expect(get(newsCache)).toBeNull();
		// Stale shape is purged with an explicit null write to the cache key.
		expect(setPreferenceMock).toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.cache, null);
		// opt-in / seen / lastViewed are loaded into their stores.
		expect(get(newsOptIn)).toBe(true);
		expect(get(newsOptInSeen)).toBe(true);
		expect(get(newsLastViewedAt)).toBe('2026-02-02');
	});

	it('discards a non-cache object (missing fetchedAt) and purges it', async () => {
		stubPreferences({
			[NEWS_PREFERENCE_KEYS.cache]: { items: [makeEntry()] },
		});

		await initNews();

		expect(get(newsCache)).toBeNull();
		expect(setPreferenceMock).toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.cache, null);
	});

	it('loads a valid NewsCache and does NOT write a purge for the cache key', async () => {
		const cache = makeCache([makeEntry()]);
		stubPreferences({
			[NEWS_PREFERENCE_KEYS.cache]: cache,
		});

		await initNews();

		expect(get(newsCache)).toEqual(cache);
		// No purge write to the cache key when the stored shape is valid.
		expect(setPreferenceMock).not.toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.cache, null);
	});

	it('leaves the cache null with no purge write when the stored cache is null', async () => {
		stubPreferences({
			[NEWS_PREFERENCE_KEYS.cache]: null,
		});

		await initNews();

		expect(get(newsCache)).toBeNull();
		// null is the absence of a cache, not a stale shape: no purge write.
		expect(setPreferenceMock).not.toHaveBeenCalled();
	});

	it('loads opt-in / seen / lastViewed defaults when preferences are unset', async () => {
		stubPreferences({});

		await initNews();

		expect(get(newsOptIn)).toBe(false);
		expect(get(newsOptInSeen)).toBe(false);
		expect(get(newsLastViewedAt)).toBeNull();
		expect(get(newsCache)).toBeNull();
	});
});

describe('setNewsOptIn', () => {
	it('opting out sets the flag, persists it, and purges the cache', async () => {
		newsCache.set(makeCache([makeEntry()]));

		await setNewsOptIn(false);

		expect(get(newsOptIn)).toBe(false);
		expect(setPreferenceMock).toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.optIn, false);
		// Opt-out purges the cache (store cleared + null persisted).
		expect(get(newsCache)).toBeNull();
		expect(setPreferenceMock).toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.cache, null);
	});

	it('opting in sets the flag, persists it, and does NOT purge the cache', async () => {
		const cache = makeCache([makeEntry()]);
		newsCache.set(cache);

		await setNewsOptIn(true);

		expect(get(newsOptIn)).toBe(true);
		expect(setPreferenceMock).toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.optIn, true);
		// No purge on opt-in: cache untouched and no null write to the cache key.
		expect(get(newsCache)).toEqual(cache);
		expect(setPreferenceMock).not.toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.cache, null);
	});
});

describe('markNewsOptInSeen', () => {
	it('sets the seen flag and persists it', async () => {
		await markNewsOptInSeen();

		expect(get(newsOptInSeen)).toBe(true);
		expect(setPreferenceMock).toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.optInSeen, true);
	});
});

describe('purgeNewsCache', () => {
	it('clears the store and persists a null cache', async () => {
		newsCache.set(makeCache([makeEntry()]));

		await purgeNewsCache();

		expect(get(newsCache)).toBeNull();
		expect(setPreferenceMock).toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.cache, null);
	});
});

describe('persistNewsCache', () => {
	it('sets the store and persists the given cache', async () => {
		const cache = makeCache([makeEntry()]);

		await persistNewsCache(cache);

		expect(get(newsCache)).toEqual(cache);
		expect(setPreferenceMock).toHaveBeenCalledWith(NEWS_PREFERENCE_KEYS.cache, cache);
	});
});

describe('markNewsAsRead', () => {
	it('advances lastViewed to the newest item date and persists it', async () => {
		newsCache.set(
			makeCache([
				makeEntry({ slug: 'a', date: '2026-01-01' }),
				makeEntry({ slug: 'c', date: '2026-03-15' }),
				makeEntry({ slug: 'b', date: '2026-02-10' }),
			]),
		);

		await markNewsAsRead();

		expect(get(newsLastViewedAt)).toBe('2026-03-15');
		expect(setPreferenceMock).toHaveBeenCalledWith(
			NEWS_PREFERENCE_KEYS.lastViewedAt,
			'2026-03-15',
		);
	});

	it('is a no-op when current cursor already equals the newest date', async () => {
		newsLastViewedAt.set('2026-03-15');
		newsCache.set(
			makeCache([
				makeEntry({ slug: 'a', date: '2026-01-01' }),
				makeEntry({ slug: 'c', date: '2026-03-15' }),
			]),
		);

		await markNewsAsRead();

		// current >= newest: never moves backward, no store change, no write.
		expect(get(newsLastViewedAt)).toBe('2026-03-15');
		expect(setPreferenceMock).not.toHaveBeenCalled();
	});

	it('is a no-op when current cursor is already past the newest date', async () => {
		newsLastViewedAt.set('2026-12-31');
		newsCache.set(makeCache([makeEntry({ date: '2026-03-15' })]));

		await markNewsAsRead();

		expect(get(newsLastViewedAt)).toBe('2026-12-31');
		expect(setPreferenceMock).not.toHaveBeenCalled();
	});

	it('is a no-op when the cache is empty', async () => {
		newsCache.set(makeCache([]));

		await markNewsAsRead();

		expect(get(newsLastViewedAt)).toBeNull();
		expect(setPreferenceMock).not.toHaveBeenCalled();
	});

	it('is a no-op when the cache is null', async () => {
		newsCache.set(null);

		await markNewsAsRead();

		expect(get(newsLastViewedAt)).toBeNull();
		expect(setPreferenceMock).not.toHaveBeenCalled();
	});

	it('advances from a null cursor to the newest date', async () => {
		newsLastViewedAt.set(null);
		newsCache.set(makeCache([makeEntry({ date: '2026-04-01' })]));

		await markNewsAsRead();

		expect(get(newsLastViewedAt)).toBe('2026-04-01');
		expect(setPreferenceMock).toHaveBeenCalledWith(
			NEWS_PREFERENCE_KEYS.lastViewedAt,
			'2026-04-01',
		);
	});

	it('advances to the true max from an intermediate cursor regardless of item order', async () => {
		// Cursor sits strictly between two unordered item dates: the reduce must
		// pick the global max, not the first item greater than the cursor.
		newsLastViewedAt.set('2026-02-10');
		newsCache.set(
			makeCache([
				makeEntry({ slug: 'c', date: '2026-03-15' }),
				makeEntry({ slug: 'a', date: '2026-01-01' }),
			]),
		);

		await markNewsAsRead();

		expect(get(newsLastViewedAt)).toBe('2026-03-15');
		expect(setPreferenceMock).toHaveBeenCalledWith(
			NEWS_PREFERENCE_KEYS.lastViewedAt,
			'2026-03-15',
		);
	});

	it('is a no-op when a non-empty cache yields a falsy newest (empty dates)', async () => {
		// Degenerate guard: reduce seeds '' and uses a strict '>' predicate, so
		// all-empty dates leave newest '' and the `if (!newest) return` fires
		// even though items.length > 0.
		newsLastViewedAt.set(null);
		newsCache.set(makeCache([makeEntry({ date: '' }), makeEntry({ date: '' })]));

		await markNewsAsRead();

		expect(get(newsLastViewedAt)).toBeNull();
		expect(setPreferenceMock).not.toHaveBeenCalled();
	});
});

describe('newsHasUnread (derived)', () => {
	it('is false when the cache is null', () => {
		newsCache.set(null);
		newsLastViewedAt.set(null);
		expect(get(newsHasUnread)).toBe(false);
	});

	it('is false when the cache has no items', () => {
		newsCache.set(makeCache([]));
		newsLastViewedAt.set(null);
		expect(get(newsHasUnread)).toBe(false);
	});

	it('is true when there are items and lastViewed is null', () => {
		newsCache.set(makeCache([makeEntry({ date: '2026-01-01' })]));
		newsLastViewedAt.set(null);
		expect(get(newsHasUnread)).toBe(true);
	});

	it('is false when lastViewed equals the newest item date', () => {
		newsCache.set(
			makeCache([
				makeEntry({ slug: 'a', date: '2026-01-01' }),
				makeEntry({ slug: 'b', date: '2026-03-15' }),
			]),
		);
		newsLastViewedAt.set('2026-03-15');
		// Strict '>' compare: equality is NOT unread.
		expect(get(newsHasUnread)).toBe(false);
	});

	it('is true when an item date is lexicographically greater than lastViewed', () => {
		newsCache.set(
			makeCache([
				makeEntry({ slug: 'a', date: '2026-01-01' }),
				makeEntry({ slug: 'b', date: '2026-03-16' }),
			]),
		);
		newsLastViewedAt.set('2026-03-15');
		expect(get(newsHasUnread)).toBe(true);
	});

	it('is false when every item date is older than lastViewed', () => {
		newsCache.set(makeCache([makeEntry({ date: '2026-02-01' })]));
		newsLastViewedAt.set('2026-03-15');
		expect(get(newsHasUnread)).toBe(false);
	});

	it('is false when a non-empty cache yields a falsy newest (empty dates)', () => {
		// Mirrors markNewsAsRead's `!newest` guard: all-empty dates reduce to ''
		// so the derived returns false even with items present and a null cursor.
		newsCache.set(makeCache([makeEntry({ date: '' }), makeEntry({ date: '' })]));
		newsLastViewedAt.set(null);
		expect(get(newsHasUnread)).toBe(false);
	});

	it('reacts to cache and cursor changes after subscription', () => {
		const seen: boolean[] = [];
		const unsubscribe = newsHasUnread.subscribe((v) => seen.push(v));

		newsCache.set(makeCache([makeEntry({ date: '2026-05-01' })]));
		newsLastViewedAt.set('2026-05-01');
		unsubscribe();

		// Initial false -> true (items, no cursor) -> false (cursor caught up).
		expect(seen).toEqual([false, true, false]);
	});
});
