import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

// The `./preferences` seam is mocked so the suite never touches Tauri or
// localStorage: getPreference feeds the persisted-shape fixtures into
// initActivityArchive, and setPreference is a spy asserting persistence
// behaviour (call count / payload). Both are reset per test.
const getPreference = vi.fn();
const setPreference = vi.fn();

vi.mock('./preferences', () => ({
	getPreference: (...args: unknown[]) => getPreference(...args),
	setPreference: (...args: unknown[]) => setPreference(...args),
}));

// The store lives at module scope (writable(EMPTY)), so each test re-imports a
// fresh module graph via resetModules + dynamic import to stay order-independent.
type ArchiveModule = typeof import('./activityArchive');

async function freshModule(): Promise<ArchiveModule> {
	vi.resetModules();
	return import('./activityArchive');
}

beforeEach(() => {
	getPreference.mockReset();
	setPreference.mockReset();
	setPreference.mockResolvedValue(undefined);
});

afterEach(() => {
	vi.clearAllMocks();
});

describe('sanitise (via initActivityArchive)', () => {
	it('coerces null from the store into the empty shape', async () => {
		getPreference.mockResolvedValue(null);
		const mod = await freshModule();
		await mod.initActivityArchive();
		expect(get(mod.activityArchive)).toEqual({ mobs: [], tags: [], weapons: [] });
	});

	it('coerces undefined from the store into the empty shape', async () => {
		getPreference.mockResolvedValue(undefined);
		const mod = await freshModule();
		await mod.initActivityArchive();
		expect(get(mod.activityArchive)).toEqual({ mobs: [], tags: [], weapons: [] });
	});

	it('replaces a non-array bucket with an empty array', async () => {
		getPreference.mockResolvedValue({
			mobs: 'not-an-array',
			tags: 42,
			weapons: { foo: 'bar' },
		});
		const mod = await freshModule();
		await mod.initActivityArchive();
		expect(get(mod.activityArchive)).toEqual({ mobs: [], tags: [], weapons: [] });
	});

	it('filters out non-string members from a bucket', async () => {
		getPreference.mockResolvedValue({
			mobs: ['atrox', 1, null, undefined, { x: 1 }, 'molisk', true],
			tags: [],
			weapons: [],
		});
		const mod = await freshModule();
		await mod.initActivityArchive();
		expect(get(mod.activityArchive).mobs).toEqual(['atrox', 'molisk']);
	});

	it('dedupes members via Set, preserving first-occurrence order', async () => {
		getPreference.mockResolvedValue({
			mobs: ['atrox', 'molisk', 'atrox', 'daikiba', 'molisk'],
			tags: [],
			weapons: [],
		});
		const mod = await freshModule();
		await mod.initActivityArchive();
		expect(get(mod.activityArchive).mobs).toEqual(['atrox', 'molisk', 'daikiba']);
	});

	it('passes the storage KEY and EMPTY default through to getPreference', async () => {
		getPreference.mockResolvedValue(null);
		const mod = await freshModule();
		await mod.initActivityArchive();
		expect(getPreference).toHaveBeenCalledTimes(1);
		expect(getPreference).toHaveBeenCalledWith('activityArchive', {
			mobs: [],
			tags: [],
			weapons: [],
		});
	});
});

describe('archive', () => {
	it('maps each ArchiveKind to its bucket and persists once', async () => {
		getPreference.mockResolvedValue(null);
		const mod = await freshModule();
		await mod.initActivityArchive();

		await mod.archive('mob', 'atrox');
		await mod.archive('tag', 'event:beacon');
		await mod.archive('weapon', 'ArMatrix LR-69');

		expect(get(mod.activityArchive)).toEqual({
			mobs: ['atrox'],
			tags: ['event:beacon'],
			weapons: ['ArMatrix LR-69'],
		});
		expect(setPreference).toHaveBeenCalledTimes(3);
	});

	it('persists the new state object under the storage KEY', async () => {
		getPreference.mockResolvedValue(null);
		const mod = await freshModule();
		await mod.initActivityArchive();

		await mod.archive('mob', 'atrox');

		expect(setPreference).toHaveBeenCalledTimes(1);
		expect(setPreference).toHaveBeenCalledWith('activityArchive', {
			mobs: ['atrox'],
			tags: [],
			weapons: [],
		});
	});

	it('appends to the end of an existing bucket', async () => {
		getPreference.mockResolvedValue({ mobs: ['atrox'], tags: [], weapons: [] });
		const mod = await freshModule();
		await mod.initActivityArchive();

		await mod.archive('mob', 'molisk');

		expect(get(mod.activityArchive).mobs).toEqual(['atrox', 'molisk']);
	});

	it('short-circuits on an already-present name: no state change, no persist', async () => {
		getPreference.mockResolvedValue({ mobs: ['atrox'], tags: [], weapons: [] });
		const mod = await freshModule();
		await mod.initActivityArchive();
		const before = get(mod.activityArchive);

		await mod.archive('mob', 'atrox');

		// Reference identity is unchanged because set() was never called.
		expect(get(mod.activityArchive)).toBe(before);
		expect(setPreference).not.toHaveBeenCalled();
	});
});

describe('unarchive', () => {
	it('removes a present name, persists once, and keeps siblings', async () => {
		getPreference.mockResolvedValue({
			mobs: ['atrox', 'molisk', 'daikiba'],
			tags: [],
			weapons: [],
		});
		const mod = await freshModule();
		await mod.initActivityArchive();

		await mod.unarchive('mob', 'molisk');

		expect(get(mod.activityArchive).mobs).toEqual(['atrox', 'daikiba']);
		expect(setPreference).toHaveBeenCalledTimes(1);
		expect(setPreference).toHaveBeenCalledWith('activityArchive', {
			mobs: ['atrox', 'daikiba'],
			tags: [],
			weapons: [],
		});
	});

	it('short-circuits on an absent name: no state change, no persist', async () => {
		getPreference.mockResolvedValue({ mobs: ['atrox'], tags: [], weapons: [] });
		const mod = await freshModule();
		await mod.initActivityArchive();
		const before = get(mod.activityArchive);

		await mod.unarchive('mob', 'nonexistent');

		expect(get(mod.activityArchive)).toBe(before);
		expect(setPreference).not.toHaveBeenCalled();
	});

	it('routes removal to the kind-specific bucket', async () => {
		getPreference.mockResolvedValue({
			mobs: ['shared'],
			tags: ['shared'],
			weapons: ['shared'],
		});
		const mod = await freshModule();
		await mod.initActivityArchive();

		await mod.unarchive('tag', 'shared');

		expect(get(mod.activityArchive)).toEqual({
			mobs: ['shared'],
			tags: [],
			weapons: ['shared'],
		});
	});
});

describe('immutability', () => {
	it('archive produces a new state object reference', async () => {
		getPreference.mockResolvedValue(null);
		const mod = await freshModule();
		await mod.initActivityArchive();
		const before = get(mod.activityArchive);

		await mod.archive('mob', 'atrox');

		const after = get(mod.activityArchive);
		expect(after).not.toBe(before);
		// The original snapshot is not mutated in place.
		expect(before.mobs).toEqual([]);
	});

	it('archive produces a new bucket array reference (no in-place push)', async () => {
		getPreference.mockResolvedValue({ mobs: ['atrox'], tags: [], weapons: [] });
		const mod = await freshModule();
		await mod.initActivityArchive();
		const before = get(mod.activityArchive);
		const beforeMobs = before.mobs;

		await mod.archive('mob', 'molisk');

		expect(get(mod.activityArchive).mobs).not.toBe(beforeMobs);
		expect(beforeMobs).toEqual(['atrox']);
	});

	it('unarchive produces a new state object reference', async () => {
		getPreference.mockResolvedValue({ mobs: ['atrox'], tags: [], weapons: [] });
		const mod = await freshModule();
		await mod.initActivityArchive();
		const before = get(mod.activityArchive);

		await mod.unarchive('mob', 'atrox');

		const after = get(mod.activityArchive);
		expect(after).not.toBe(before);
		expect(before.mobs).toEqual(['atrox']);
	});
});

describe('isArchived', () => {
	it('reports membership for the matching bucket', async () => {
		const mod = await freshModule();
		const state = { mobs: ['atrox'], tags: ['event:beacon'], weapons: ['LR-69'] };
		expect(mod.isArchived(state, 'mob', 'atrox')).toBe(true);
		expect(mod.isArchived(state, 'tag', 'event:beacon')).toBe(true);
		expect(mod.isArchived(state, 'weapon', 'LR-69')).toBe(true);
	});

	it('returns false for a name absent from the queried bucket', async () => {
		const mod = await freshModule();
		const state = { mobs: ['atrox'], tags: [], weapons: [] };
		expect(mod.isArchived(state, 'mob', 'molisk')).toBe(false);
	});

	it('isolates buckets: a mob name is not archived under tag', async () => {
		const mod = await freshModule();
		const state = { mobs: ['atrox'], tags: [], weapons: [] };
		expect(mod.isArchived(state, 'mob', 'atrox')).toBe(true);
		expect(mod.isArchived(state, 'tag', 'atrox')).toBe(false);
		expect(mod.isArchived(state, 'weapon', 'atrox')).toBe(false);
	});
});
