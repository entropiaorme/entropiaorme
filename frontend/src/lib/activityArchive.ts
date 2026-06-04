import { get, type Writable, writable } from 'svelte/store';
import { getPreference, setPreference } from './preferences';

export type ArchiveKind = 'mob' | 'tag' | 'weapon';

export type ActivityArchiveState = {
	mobs: string[];
	tags: string[];
	weapons: string[];
};

const KEY = 'activityArchive';

const EMPTY: ActivityArchiveState = { mobs: [], tags: [], weapons: [] };

export const activityArchive: Writable<ActivityArchiveState> = writable(EMPTY);

function sanitise(value: unknown): ActivityArchiveState {
	const v = (value ?? {}) as Partial<ActivityArchiveState>;
	const arr = (x: unknown): string[] =>
		Array.isArray(x)
			? Array.from(new Set(x.filter((s): s is string => typeof s === 'string')))
			: [];
	return {
		mobs: arr(v.mobs),
		tags: arr(v.tags),
		weapons: arr(v.weapons),
	};
}

export async function initActivityArchive(): Promise<void> {
	const raw = await getPreference<unknown>(KEY, EMPTY);
	activityArchive.set(sanitise(raw));
}

function bucketKey(kind: ArchiveKind): keyof ActivityArchiveState {
	return kind === 'mob' ? 'mobs' : kind === 'tag' ? 'tags' : 'weapons';
}

export async function archive(kind: ArchiveKind, name: string): Promise<void> {
	const state = { ...get(activityArchive) };
	const bucket = bucketKey(kind);
	if (state[bucket].includes(name)) return;
	state[bucket] = [...state[bucket], name];
	activityArchive.set(state);
	await setPreference(KEY, state);
}

export async function unarchive(kind: ArchiveKind, name: string): Promise<void> {
	const state = { ...get(activityArchive) };
	const bucket = bucketKey(kind);
	if (!state[bucket].includes(name)) return;
	state[bucket] = state[bucket].filter((n) => n !== name);
	activityArchive.set(state);
	await setPreference(KEY, state);
}

export function isArchived(state: ActivityArchiveState, kind: ArchiveKind, name: string): boolean {
	return state[bucketKey(kind)].includes(name);
}
