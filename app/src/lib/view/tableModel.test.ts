import { describe, expect, it } from 'vitest';
import { reactiveBox } from './__fixtures__/reactiveBox.svelte';
import { createTableModel } from './tableModel.svelte';

interface Row {
	name: string;
	category: string;
	level: number | null;
}

const row = (name: string, category: string, level: number | null): Row => ({
	name,
	category,
	level,
});

// Seven rows across three categories, with two null levels for the nulls-last
// sort contract. Base order is deliberately not sorted by any column.
const baseRows: Row[] = [
	row('Rifle', 'Ranged', 30),
	row('Sword', 'Melee', 10),
	row('axe', 'Melee', null),
	row('Pistol', 'Ranged', 20),
	row('Wand', 'Magic', null),
	row('Bow', 'Ranged', 10),
	row('Club', 'Melee', 5),
];

function makeModel(rows: Row[] = baseRows, pageSize = 3) {
	return createTableModel<Row>({
		rows: () => rows,
		pageSize,
		searchText: (r) => [r.name],
		categoryOf: (r) => r.category,
	});
}

const names = (rows: Row[]): string[] => rows.map((r) => r.name);

describe('filtering', () => {
	it('passes all rows through in source order when no filter or sort is active', () => {
		const model = makeModel();
		expect(names(model.filtered)).toEqual(names(baseRows));
	});

	it('matches through the shared search: any word order, across fields', () => {
		const model = createTableModel<Row>({
			rows: () => baseRows,
			pageSize: 10,
			searchText: (r) => [r.name, r.category],
		});
		model.search = 'melee club';
		expect(names(model.filtered)).toEqual(['Club']);
		model.search = 'ranged -bow';
		expect(names(model.filtered)).toEqual(['Rifle', 'Pistol']);
	});

	it('matches the search query case-insensitively as a substring of any search field', () => {
		const model = makeModel();
		model.search = 'AX';
		expect(names(model.filtered)).toEqual(['axe']);
		model.search = 'w';
		expect(names(model.filtered)).toEqual(['Sword', 'Wand', 'Bow']);
	});

	it('checks every field returned by searchText, not just the first', () => {
		const model = createTableModel<Row>({
			rows: () => baseRows,
			pageSize: 10,
			searchText: (r) => [r.name, r.category],
		});
		model.search = 'magic';
		expect(names(model.filtered)).toEqual(['Wand']);
	});

	it('ignores search entirely when no searchText option is given', () => {
		const model = createTableModel<Row>({ rows: () => baseRows, pageSize: 10 });
		model.search = 'zzz no match';
		expect(model.filtered).toHaveLength(baseRows.length);
	});

	it('filters by category and stacks with search', () => {
		const model = makeModel();
		model.category = 'Melee';
		expect(names(model.filtered)).toEqual(['Sword', 'axe', 'Club']);
		model.search = 'c';
		expect(names(model.filtered)).toEqual(['Club']);
	});

	it('ignores category entirely when no categoryOf option is given', () => {
		const model = createTableModel<Row>({
			rows: () => baseRows,
			pageSize: 10,
			searchText: (r) => [r.name],
		});
		model.category = 'Melee';
		expect(model.filtered).toHaveLength(baseRows.length);
	});
});

describe('categories', () => {
	it('derives sorted unique categories from the full row set', () => {
		const model = makeModel();
		expect(model.categories).toEqual(['Magic', 'Melee', 'Ranged']);
	});

	it('stays derived from all rows while a filter is active', () => {
		const model = makeModel();
		model.category = 'Melee';
		model.search = 'club';
		expect(model.categories).toEqual(['Magic', 'Melee', 'Ranged']);
	});

	it('excludes rows whose categoryOf yields null', () => {
		const model = createTableModel<Row>({
			rows: () => baseRows,
			pageSize: 10,
			categoryOf: (r) => (r.category === 'Magic' ? null : r.category),
		});
		expect(model.categories).toEqual(['Melee', 'Ranged']);
	});

	it('is empty when no categoryOf option is given', () => {
		const model = createTableModel<Row>({ rows: () => baseRows, pageSize: 10 });
		expect(model.categories).toEqual([]);
	});
});

describe('sorting', () => {
	it('starts unsorted unless an initialSort is given', () => {
		const model = makeModel();
		expect(model.sortKey).toBeUndefined();
		expect(names(model.filtered)).toEqual(names(baseRows));
	});

	it('applies an initialSort from options', () => {
		const model = createTableModel<Row>({
			rows: () => baseRows,
			pageSize: 10,
			initialSort: { key: 'level', dir: 'desc' },
		});
		expect(model.sortKey).toBe('level');
		expect(model.sortDir).toBe('desc');
		expect(names(model.filtered)).toEqual([
			'Rifle',
			'Pistol',
			'Sword',
			'Bow',
			'Club',
			'axe',
			'Wand',
		]);
	});

	it('setSort on a new key sorts ascending', () => {
		const model = makeModel();
		model.setSort('level');
		expect(model.sortDir).toBe('asc');
		expect(names(model.filtered)).toEqual([
			'Club',
			'Sword',
			'Bow',
			'Pistol',
			'Rifle',
			'axe',
			'Wand',
		]);
	});

	it('supports a per-key first-sort direction', () => {
		const model = createTableModel<Row>({
			rows: () => baseRows,
			pageSize: 10,
			defaultSortDirs: { level: 'desc' },
		});
		model.setSort('level');
		expect(model.sortDir).toBe('desc');
		expect(names(model.filtered).slice(0, 3)).toEqual(['Rifle', 'Pistol', 'Sword']);

		model.setSort('name');
		expect(model.sortDir).toBe('asc');
	});

	it('setSort on the same key toggles the direction each time', () => {
		const model = makeModel();
		model.setSort('level');
		model.setSort('level');
		expect(model.sortDir).toBe('desc');
		model.setSort('level');
		expect(model.sortDir).toBe('asc');
	});

	it('sorts null levels last in both directions', () => {
		const model = makeModel();
		model.setSort('level');
		expect(names(model.filtered).slice(-2)).toEqual(['axe', 'Wand']);
		model.setSort('level');
		expect(names(model.filtered).slice(-2)).toEqual(['axe', 'Wand']);
	});

	it('sorts strings with localeCompare, so case does not split the ordering', () => {
		const model = makeModel();
		model.setSort('name');
		expect(names(model.filtered)).toEqual([
			'axe',
			'Bow',
			'Club',
			'Pistol',
			'Rifle',
			'Sword',
			'Wand',
		]);
	});

	it('uses a per-key comparator when provided and flips it with the direction', () => {
		const model = createTableModel<Row>({
			rows: () => baseRows,
			pageSize: 10,
			comparators: { name: (a, b) => a.name.length - b.name.length },
		});
		model.setSort('name');
		expect(model.filtered[0].name).toBe('axe');
		expect(model.filtered.at(-1)?.name).toBe('Pistol');
		model.setSort('name');
		expect(model.filtered[0].name).toBe('Pistol');
	});

	it('does not mutate the source rows array when sorting', () => {
		const rows = [...baseRows];
		const model = makeModel(rows);
		model.setSort('name');
		void model.filtered;
		expect(names(rows)).toEqual(names(baseRows));
	});
});

describe('pagination', () => {
	it('slices pageRows by page and reports totalPages', () => {
		const model = makeModel();
		expect(model.totalPages).toBe(3);
		expect(names(model.pageRows)).toEqual(['Rifle', 'Sword', 'axe']);
		model.page = 2;
		expect(names(model.pageRows)).toEqual(['Club']);
	});

	it('reports at least one page for an empty row set', () => {
		const model = makeModel([]);
		expect(model.totalPages).toBe(1);
		expect(model.pageRows).toEqual([]);
	});

	it('clamps a page set beyond the last page down into range', () => {
		const model = makeModel();
		model.page = 99;
		expect(model.page).toBe(2);
		expect(names(model.pageRows)).toEqual(['Club']);
	});

	it('clamps a negative page to zero', () => {
		const model = makeModel();
		model.page = -3;
		expect(model.page).toBe(0);
	});

	it('formats rangeLabel as "X-Y of Z" with a partial final page', () => {
		const model = makeModel();
		expect(model.rangeLabel).toBe('1-3 of 7');
		model.page = 2;
		expect(model.rangeLabel).toBe('7-7 of 7');
	});

	it('formats rangeLabel as "0-0 of 0" when nothing matches', () => {
		const model = makeModel();
		model.search = 'no such row';
		expect(model.rangeLabel).toBe('0-0 of 0');
	});
});

describe('page reset on state changes', () => {
	it('resets the page when the search changes', () => {
		const model = makeModel();
		model.page = 2;
		model.search = 'o';
		expect(model.page).toBe(0);
	});

	it('resets the page when the category changes', () => {
		const model = makeModel();
		model.page = 2;
		model.category = 'Ranged';
		expect(model.page).toBe(0);
	});

	it('resets the page on setSort, including a direction toggle', () => {
		const model = makeModel();
		model.setSort('name');
		model.page = 2;
		model.setSort('name');
		expect(model.page).toBe(0);
	});

	it('does not reset the page when search or category is re-assigned unchanged', () => {
		const model = makeModel();
		model.page = 1;
		model.search = '';
		model.category = null;
		expect(model.page).toBe(1);
	});
});

describe('reactive rows source', () => {
	it('re-derives filtered, categories and paging when the source changes', () => {
		const box = reactiveBox<Row[]>(baseRows);
		const model = createTableModel<Row>({
			rows: () => box.value,
			pageSize: 3,
			searchText: (r) => [r.name],
			categoryOf: (r) => r.category,
		});
		expect(model.totalPages).toBe(3);

		box.value = [row('Staff', 'Magic', 1)];
		expect(names(model.filtered)).toEqual(['Staff']);
		expect(model.categories).toEqual(['Magic']);
		expect(model.totalPages).toBe(1);
	});

	it('clamps the page when the source shrinks, without a manual reset', () => {
		const box = reactiveBox<Row[]>(baseRows);
		const model = createTableModel<Row>({
			rows: () => box.value,
			pageSize: 3,
			searchText: (r) => [r.name],
		});
		model.page = 2;
		box.value = baseRows.slice(0, 2);
		expect(model.page).toBe(0);
		expect(names(model.pageRows)).toEqual(['Rifle', 'Sword']);
	});
});

describe('reset', () => {
	it('restores search, category, page and the initial sort', () => {
		const model = createTableModel<Row>({
			rows: () => baseRows,
			pageSize: 3,
			searchText: (r) => [r.name],
			categoryOf: (r) => r.category,
			initialSort: { key: 'level', dir: 'desc' },
		});
		model.search = 'o';
		model.category = 'Ranged';
		model.setSort('name');
		model.page = 1;

		model.reset();
		expect(model.search).toBe('');
		expect(model.category).toBeNull();
		expect(model.page).toBe(0);
		expect(model.sortKey).toBe('level');
		expect(model.sortDir).toBe('desc');
	});

	it('clears the sort entirely when no initialSort was configured', () => {
		const model = makeModel();
		model.setSort('name');
		model.reset();
		expect(model.sortKey).toBeUndefined();
		expect(names(model.filtered)).toEqual(names(baseRows));
	});
});
