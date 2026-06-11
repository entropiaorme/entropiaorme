import { get } from 'svelte/store';
import { type NewsCache, type NewsEntry, type NewsFeed, newsOptIn, persistNewsCache } from './news';

// This file is the sole site of outbound non-loopback HTTP in the app.
// The CSP `connect-src` entry in frontend/src-tauri/entropia-orme/tauri.conf.json gates
// the host allowlist at the webview boundary; this constant declares it.
const NEWS_SOURCE_BASE = 'https://entropiaorme.com';
const FEED_URL = `${NEWS_SOURCE_BASE}/news.json`;
const REQUEST_TIMEOUT_MS = 10_000;

async function httpsFetch(url: string): Promise<Response> {
	if (!url.startsWith('https://')) {
		throw new Error(`refusing non-HTTPS URL: ${url}`);
	}
	const ctl = new AbortController();
	const timer = setTimeout(() => ctl.abort(), REQUEST_TIMEOUT_MS);
	try {
		const res = await fetch(url, {
			credentials: 'omit',
			cache: 'no-store',
			signal: ctl.signal,
		});
		if (!res.ok) {
			throw new Error(`HTTP ${res.status} for ${url}`);
		}
		return res;
	} finally {
		clearTimeout(timer);
	}
}

function isEntry(value: unknown): value is NewsEntry {
	if (!value || typeof value !== 'object') return false;
	const e = value as Partial<NewsEntry>;
	if (typeof e.slug !== 'string' || !e.slug) return false;
	if (typeof e.title !== 'string') return false;
	if (typeof e.date !== 'string' || !e.date) return false;
	if (e.category !== 'article' && e.category !== 'changelog') return false;
	if (typeof e.body !== 'string') return false;
	if (e.dek !== undefined && typeof e.dek !== 'string') return false;
	if (e.eyebrow !== undefined && typeof e.eyebrow !== 'string') return false;
	if (e.hero !== undefined && typeof e.hero !== 'string') return false;
	if (e.link !== undefined && typeof e.link !== 'string') return false;
	if (
		e.pin_slot !== undefined &&
		e.pin_slot !== 'community' &&
		e.pin_slot !== 'release' &&
		e.pin_slot !== 'foundations'
	)
		return false;
	if (e.pin_blurb !== undefined && typeof e.pin_blurb !== 'string') return false;
	if (e.pin_icon !== undefined && typeof e.pin_icon !== 'string') return false;
	if (e.pin_cta !== undefined && typeof e.pin_cta !== 'string') return false;
	return true;
}

function isFeed(value: unknown): value is NewsFeed {
	if (!value || typeof value !== 'object') return false;
	const v = value as { items?: unknown };
	return Array.isArray(v.items) && v.items.every(isEntry);
}

export async function fetchNews(): Promise<NewsCache> {
	const res = await httpsFetch(FEED_URL);
	const raw: unknown = await res.json();
	if (!isFeed(raw)) {
		throw new Error('feed schema rejected');
	}
	return {
		items: raw.items,
		fetchedAt: new Date().toISOString(),
	};
}

export type RefreshResult = { ok: true } | { ok: false; reason: string };

export async function refreshNews(): Promise<RefreshResult> {
	if (!get(newsOptIn)) {
		return { ok: false, reason: 'opt-in is off' };
	}
	try {
		const cache = await fetchNews();
		await persistNewsCache(cache);
		return { ok: true };
	} catch (err) {
		return {
			ok: false,
			reason: err instanceof Error ? err.message : String(err),
		};
	}
}

export async function maybeRefreshOnMount(): Promise<void> {
	if (!get(newsOptIn)) return;
	await refreshNews();
}
