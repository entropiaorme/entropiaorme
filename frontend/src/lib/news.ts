import { derived, get, writable, type Readable, type Writable } from 'svelte/store';
import { getPreference, setPreference } from './preferences';

export type NewsCategory = 'article' | 'changelog';

// Three-slot pinned-cards architecture in /news. Each slot is a named role
// with its own visual register; each slot holds at most one article;
// replacement within a slot is automatic by date. Slot defaults (icon,
// label, CTA copy) live in $lib/news-pins; per-article frontmatter
// overrides slot defaults where needed (pin_blurb, pin_icon, pin_cta).
// Release slot auto-derives from latest `category: changelog` entry when
// no article explicitly claims it.
export type SlotId = 'community' | 'release' | 'foundations';

export type NewsEntry = {
	slug: string;
	title: string;
	date: string;
	category: NewsCategory;
	body: string;
	dek?: string;
	eyebrow?: string;
	hero?: string;
	link?: string;
	pin_slot?: SlotId;
	pin_blurb?: string;
	pin_icon?: string;
	pin_cta?: string;
};

export type NewsFeed = {
	items: NewsEntry[];
};

export type NewsCache = {
	items: NewsEntry[];
	fetchedAt: string;
};

const KEY_OPT_IN = 'news_opt_in';
const KEY_OPT_IN_SEEN = 'news_opt_in_seen';
const KEY_CACHE = 'news_cache';
const KEY_LAST_VIEWED_AT = 'news_last_viewed_at';

export const newsOptIn: Writable<boolean> = writable(false);
export const newsOptInSeen: Writable<boolean> = writable(false);
export const newsCache: Writable<NewsCache | null> = writable(null);
// Cursor for the unread-dot derivation: the highest article date the user
// has acknowledged by visiting /news. Compared lex-greater against article
// dates in the cache; persisted as an ISO string. Article-date rather than
// wall-clock-now, because articles can be stamped to a planned future
// release date (e.g. a launch-day announcement dated tomorrow), and a
// wall-clock cursor would trail those dates forever.
export const newsLastViewedAt: Writable<string | null> = writable(null);

function isCache(value: unknown): value is NewsCache {
	if (!value || typeof value !== 'object') return false;
	const c = value as Partial<NewsCache>;
	if (typeof c.fetchedAt !== 'string') return false;
	if (!Array.isArray(c.items)) return false;
	return true;
}

export async function initNews(): Promise<void> {
	const [optIn, seen, rawCache, lastViewed] = await Promise.all([
		getPreference<boolean>(KEY_OPT_IN, false),
		getPreference<boolean>(KEY_OPT_IN_SEEN, false),
		getPreference<unknown>(KEY_CACHE, null),
		getPreference<string | null>(KEY_LAST_VIEWED_AT, null),
	]);
	newsOptIn.set(optIn);
	newsOptInSeen.set(seen);
	// Discard caches written under any earlier shape; opt-in users will
	// repopulate on the next refresh.
	newsCache.set(isCache(rawCache) ? rawCache : null);
	if (!isCache(rawCache) && rawCache !== null) {
		await setPreference<NewsCache | null>(KEY_CACHE, null);
	}
	newsLastViewedAt.set(lastViewed);
}

export async function setNewsOptIn(value: boolean): Promise<void> {
	newsOptIn.set(value);
	await setPreference(KEY_OPT_IN, value);
	if (!value) {
		await purgeNewsCache();
	}
}

export async function markNewsOptInSeen(): Promise<void> {
	newsOptInSeen.set(true);
	await setPreference(KEY_OPT_IN_SEEN, true);
}

export async function purgeNewsCache(): Promise<void> {
	newsCache.set(null);
	await setPreference<NewsCache | null>(KEY_CACHE, null);
}

export async function persistNewsCache(cache: NewsCache): Promise<void> {
	newsCache.set(cache);
	await setPreference(KEY_CACHE, cache);
}

export async function markNewsAsRead(): Promise<void> {
	const cache = get(newsCache);
	if (!cache || cache.items.length === 0) return;
	const newest = cache.items.reduce((max, e) => (e.date > max ? e.date : max), '');
	if (!newest) return;
	const current = get(newsLastViewedAt);
	if (current && current >= newest) return;
	newsLastViewedAt.set(newest);
	await setPreference(KEY_LAST_VIEWED_AT, newest);
}

// True when the cache contains an entry strictly newer than the last viewed
// timestamp. Acts as a binary unread indicator for the sidebar.
export const newsHasUnread: Readable<boolean> = derived(
	[newsCache, newsLastViewedAt],
	([$cache, $lastViewed]) => {
		if (!$cache || $cache.items.length === 0) return false;
		const newest = $cache.items.reduce((max, e) => (e.date > max ? e.date : max), '');
		if (!newest) return false;
		if (!$lastViewed) return true;
		return newest > $lastViewed;
	},
);

export const NEWS_PREFERENCE_KEYS = {
	optIn: KEY_OPT_IN,
	optInSeen: KEY_OPT_IN_SEEN,
	cache: KEY_CACHE,
	lastViewedAt: KEY_LAST_VIEWED_AT,
} as const;
