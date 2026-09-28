import { describe, expect, it } from 'vitest';
import {
	filterRows,
	type PickerRow,
	PROFESSION_FAMILIES,
	rowToTarget,
	targetLabel,
	targetProfessions,
} from './codexRankingTarget';

const NAMES = ['Animal Looter', 'Dodger', 'Evader', 'Ranger (Hit)', 'Sniper (Hit)'];

describe('targetProfessions', () => {
	it('resolves a single profession and a family, and is empty otherwise', () => {
		expect(targetProfessions({ kind: 'none' })).toEqual([]);
		expect(targetProfessions({ kind: 'hp' })).toEqual([]);
		expect(targetProfessions({ kind: 'profession', name: 'Evader' })).toEqual(['Evader']);
		expect(targetProfessions({ kind: 'family', key: 'defensive' })).toEqual(['Dodger', 'Evader']);
		expect(targetProfessions({ kind: 'family', key: 'looter' })).toEqual([
			'Animal Looter',
			'Mutant Looter',
			'Robot Looter',
		]);
	});

	it('answers an unknown family key with no professions', () => {
		expect(targetProfessions({ kind: 'family', key: 'nope' })).toEqual([]);
	});
});

describe('targetLabel', () => {
	it('labels every target kind', () => {
		expect(targetLabel({ kind: 'none' })).toBe('No profession');
		expect(targetLabel({ kind: 'hp' })).toBe('HP gain');
		expect(targetLabel({ kind: 'profession', name: 'Evader' })).toBe('Evader');
		expect(targetLabel({ kind: 'family', key: 'looter' })).toBe('Looter professions');
	});
});

describe('filterRows', () => {
	const kindsOf = (rows: PickerRow[]) => rows.map((row) => row.kind);
	const labelOf = (row: PickerRow) => (row.kind === 'profession' ? row.name : row.label);

	it('lists everything for an empty query: fixed targets, families, favourites, the rest', () => {
		const rows = filterRows(NAMES, ['Evader'], '');
		expect(kindsOf(rows).slice(0, 2)).toEqual(['none', 'hp']);
		expect(rows.filter((row) => row.kind === 'family')).toHaveLength(PROFESSION_FAMILIES.length);
		const professions = rows.filter((row) => row.kind === 'profession');
		expect(professions[0]).toEqual({ kind: 'profession', name: 'Evader', favourite: true });
		expect(professions).toHaveLength(NAMES.length);
	});

	it('filters case-insensitively by substring', () => {
		const rows = filterRows(NAMES, [], 'hit');
		expect(rows.map(labelOf)).toEqual(['Ranger (Hit)', 'Sniper (Hit)']);
	});

	it('matches a family on its label and on its member names', () => {
		const byLabel = filterRows(NAMES, [], 'defensive');
		expect(byLabel.map(labelOf)).toEqual(['Defensive professions']);
		const byMember = filterRows(NAMES, [], 'dodger');
		expect(byMember.map(labelOf)).toEqual(['Defensive professions', 'Dodger']);
	});

	it('keeps the fixed targets reachable by name', () => {
		expect(filterRows(NAMES, [], 'hp gain').map(labelOf)).toEqual(['HP gain']);
		expect(filterRows(NAMES, [], 'no profession').map(labelOf)).toEqual(['No profession']);
	});

	it('drops the none row where no target is not a choice', () => {
		const rows = filterRows(NAMES, [], '', { includeNone: false });
		expect(rows.some((row) => row.kind === 'none')).toBe(false);
		expect(rows[0].kind).toBe('hp');
		expect(filterRows(NAMES, [], 'no profession', { includeNone: false })).toEqual([]);
	});
});

describe('rowToTarget', () => {
	it('maps each row kind onto its target', () => {
		expect(rowToTarget({ kind: 'none', label: 'No profession' })).toEqual({ kind: 'none' });
		expect(rowToTarget({ kind: 'hp', label: 'HP gain' })).toEqual({ kind: 'hp' });
		expect(rowToTarget({ kind: 'profession', name: 'Evader', favourite: false })).toEqual({
			kind: 'profession',
			name: 'Evader',
		});
		expect(
			rowToTarget({ kind: 'family', key: 'looter', label: 'Looter professions', professions: [] }),
		).toEqual({ kind: 'family', key: 'looter' });
	});
});
