/**
 * The codex recommendation's ranking target and the picker's row model:
 * a single profession, a curated profession family (ranked by summed
 * weights backend-side), HP gain, or nothing. Favourite professions
 * persist as a UI preference and float to the top of the picker.
 */

import { getPreference, setPreference } from '$lib/preferences';

export type CodexRankingTarget =
	| { kind: 'none' }
	| { kind: 'hp' }
	| { kind: 'profession'; name: string }
	| { kind: 'family'; key: string };

export interface ProfessionFamily {
	key: string;
	label: string;
	professions: string[];
}

/** Curated families worth optimising as one target. */
export const PROFESSION_FAMILIES: ProfessionFamily[] = [
	{
		key: 'looter',
		label: 'Looter professions',
		professions: ['Animal Looter', 'Mutant Looter', 'Robot Looter'],
	},
	{
		key: 'defensive',
		label: 'Defensive professions',
		professions: ['Dodger', 'Evader'],
	},
];

export function familyByKey(key: string): ProfessionFamily | undefined {
	return PROFESSION_FAMILIES.find((family) => family.key === key);
}

/** The professions a target ranks against (empty for none / HP gain). */
export function targetProfessions(target: CodexRankingTarget): string[] {
	switch (target.kind) {
		case 'profession':
			return [target.name];
		case 'family':
			return familyByKey(target.key)?.professions ?? [];
		default:
			return [];
	}
}

export function targetLabel(target: CodexRankingTarget): string {
	switch (target.kind) {
		case 'none':
			return 'No profession';
		case 'hp':
			return 'HP gain';
		case 'profession':
			return target.name;
		case 'family':
			return familyByKey(target.key)?.label ?? target.key;
	}
}

/** One row of the picker's dropdown. */
export type PickerRow =
	| { kind: 'none'; label: string }
	| { kind: 'hp'; label: string }
	| { kind: 'family'; key: string; label: string; professions: string[] }
	| { kind: 'profession'; name: string; favourite: boolean };

export function rowToTarget(row: PickerRow): CodexRankingTarget {
	switch (row.kind) {
		case 'none':
			return { kind: 'none' };
		case 'hp':
			return { kind: 'hp' };
		case 'family':
			return { kind: 'family', key: row.key };
		case 'profession':
			return { kind: 'profession', name: row.name };
	}
}

/**
 * The picker's rows for a query: the fixed targets and families first,
 * then favourite professions, then the rest, all filtered by
 * case-insensitive substring (a family also matches on its members).
 * A surface where "no target" is not a choice drops the none row.
 */
export function filterRows(
	professionNames: string[],
	favourites: string[],
	query: string,
	{ includeNone = true }: { includeNone?: boolean } = {},
): PickerRow[] {
	const q = query.trim().toLowerCase();
	const matches = (label: string) => q === '' || label.toLowerCase().includes(q);

	const rows: PickerRow[] = [];
	if (includeNone && matches('No profession')) rows.push({ kind: 'none', label: 'No profession' });
	if (matches('HP gain')) rows.push({ kind: 'hp', label: 'HP gain' });
	for (const family of PROFESSION_FAMILIES) {
		if (matches(family.label) || family.professions.some((name) => matches(name))) {
			rows.push({ kind: 'family', ...family });
		}
	}
	const favouriteSet = new Set(favourites);
	for (const name of professionNames.filter((name) => favouriteSet.has(name) && matches(name))) {
		rows.push({ kind: 'profession', name, favourite: true });
	}
	for (const name of professionNames.filter((name) => !favouriteSet.has(name) && matches(name))) {
		rows.push({ kind: 'profession', name, favourite: false });
	}
	return rows;
}

const FAVOURITES_KEY = 'codex_favourite_professions';

export async function loadFavouriteProfessions(): Promise<string[]> {
	return getPreference<string[]>(FAVOURITES_KEY, []);
}

export async function saveFavouriteProfessions(favourites: string[]): Promise<void> {
	await setPreference(FAVOURITES_KEY, favourites);
}
