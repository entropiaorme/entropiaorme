import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LedgerEntry, LedgerPreset } from '$lib/types/analytics';
import { createLedgerModel, PAGE_SIZE } from './ledgerModel.svelte';

vi.mock('$lib/api', () => ({
	getLedgerEntries: vi.fn(),
	getLedgerSummary: vi.fn(),
	addLedgerEntry: vi.fn(),
	deleteLedgerEntry: vi.fn(),
	getLedgerPresets: vi.fn(),
	getAllSessionDefinitions: vi.fn(),
	addLedgerPreset: vi.fn(),
	deleteLedgerPreset: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function entry(overrides: Partial<LedgerEntry> = {}): LedgerEntry {
	return {
		id: 'e1',
		date: '2026-07-01T10:00:00.000Z',
		type: 'expense',
		description: 'L weapon purchase',
		amount: 45,
		tag: 'equipment',
		sessionName: null,
		...overrides,
	};
}

function preset(overrides: Partial<LedgerPreset> = {}): LedgerPreset {
	return {
		id: 'p1',
		name: 'L weapon',
		type: 'expense',
		description: 'Weapon restock',
		amount: 45,
		tag: 'equipment',
		...overrides,
	};
}

function seedLoad(
	entries: LedgerEntry[] = [entry()],
	nextCursor: string | null = null,
	total: number = entries.length,
) {
	mocked.getLedgerEntries.mockResolvedValue({ items: entries, nextCursor, total });
	mocked.getLedgerPresets.mockResolvedValue([preset()]);
	mocked.getAllSessionDefinitions.mockResolvedValue([]);
}

beforeEach(() => {
	vi.clearAllMocks();
	// A benign default summary; the summary-focused tests override it.
	mocked.getLedgerSummary.mockResolvedValue({ gains: {}, losses: {} });
});

describe('loadAll', () => {
	it('loads the first keyset page and the presets', async () => {
		seedLoad([entry()], 'cursor-1');
		const model = createLedgerModel();
		await model.loadAll();

		expect(model.entries).toHaveLength(1);
		expect(model.presets).toHaveLength(1);
		expect(model.nextCursor).toBe('cursor-1');
		expect(model.loading).toBe(false);
		expect(model.error).toBeNull();
	});

	it('surfaces a load failure', async () => {
		mocked.getLedgerEntries.mockRejectedValue(new Error('backend unreachable'));
		mocked.getLedgerPresets.mockResolvedValue([]);
		const model = createLedgerModel();
		await model.loadAll();
		expect(model.error).toBe('backend unreachable');
		expect(model.loading).toBe(false);
	});
});

describe('loadMoreEntries', () => {
	it('appends the next keyset page and advances the cursor', async () => {
		seedLoad([entry({ id: 'e1' })], 'cursor-1');
		const model = createLedgerModel();
		await model.loadAll();

		mocked.getLedgerEntries.mockResolvedValue({
			items: [entry({ id: 'e2' })],
			nextCursor: null,
			total: 2,
		});
		await model.loadMoreEntries();

		expect(mocked.getLedgerEntries).toHaveBeenLastCalledWith('cursor-1');
		expect(model.entries.map((e) => e.id)).toEqual(['e1', 'e2']);
		expect(model.nextCursor).toBeNull();
	});

	it('is a no-op when the cursor is exhausted', async () => {
		seedLoad([entry()], null);
		const model = createLedgerModel();
		await model.loadAll();
		mocked.getLedgerEntries.mockClear();

		await model.loadMoreEntries();
		expect(mocked.getLedgerEntries).not.toHaveBeenCalled();
	});

	it('surfaces a load-more failure and keeps the loaded window', async () => {
		seedLoad([entry()], 'cursor-1');
		const model = createLedgerModel();
		await model.loadAll();

		mocked.getLedgerEntries.mockRejectedValue(new Error('timed out'));
		await model.loadMoreEntries();
		expect(model.error).toBe('timed out');
		expect(model.entries).toHaveLength(1);
		expect(model.nextCursor).toBe('cursor-1');
	});
});

describe('paging', () => {
	it('pages five entries per page and resets to the first page on add', async () => {
		seedLoad(
			Array.from({ length: 12 }, (_, i) => entry({ id: `e${i}` })),
			null,
		);
		const model = createLedgerModel();
		await model.loadAll();

		expect(PAGE_SIZE).toBe(5);
		expect(model.table.totalPages).toBe(3);
		model.table.page = 2;
		expect(model.table.pageRows.map((e) => e.id)).toEqual(['e10', 'e11']);

		mocked.addLedgerEntry.mockResolvedValue(entry({ id: 'new' }));
		model.entryDescription = 'Fresh entry';
		model.entryTag = 'other';
		model.entryAmount = 3;
		await model.addEntry();

		expect(model.table.page).toBe(0);
		expect(model.entries[0].id).toBe('new');
	});
});

describe('on-demand paging', () => {
	it('reports totals from the server and fetches on a step past the loaded window', async () => {
		seedLoad(
			Array.from({ length: 5 }, (_, i) => entry({ id: `e${i}` })),
			'cursor-1',
			12,
		);
		const model = createLedgerModel();
		await model.loadAll();
		expect(model.total).toBe(12);
		expect(model.totalPages).toBe(3);

		mocked.getLedgerEntries.mockResolvedValueOnce({
			items: Array.from({ length: 7 }, (_, i) => entry({ id: `e${5 + i}` })),
			nextCursor: null,
			total: 12,
		});
		await model.nextPage();
		expect(mocked.getLedgerEntries).toHaveBeenLastCalledWith('cursor-1');
		expect(model.table.page).toBe(1);
		expect(model.table.pageRows.map((e) => e.id)).toEqual(
			Array.from({ length: 5 }, (_, i) => `e${5 + i}`),
		);

		await model.nextPage();
		expect(model.table.page).toBe(2);
		await model.nextPage();
		expect(model.table.page).toBe(2);
	});
});

describe('addEntry', () => {
	it('rejects an incomplete form without calling the API', async () => {
		seedLoad();
		const model = createLedgerModel();
		await model.loadAll();

		model.entryDescription = ' ';
		model.entryTag = 'other';
		model.entryAmount = 3;
		await model.addEntry();
		expect(mocked.addLedgerEntry).not.toHaveBeenCalled();
	});

	it('pegs the entry to the chosen session, then clears the choice', async () => {
		seedLoad();
		mocked.addLedgerEntry.mockResolvedValue(entry({ id: 'new', sessionName: 'Mayhem' }));
		const model = createLedgerModel();
		await model.loadAll();

		model.entryDescription = 'Fuel';
		model.entryTag = 'Operating';
		model.entryAmount = 8;
		model.entrySessionId = '17';
		await model.addEntry();

		expect(mocked.addLedgerEntry).toHaveBeenCalledWith(
			expect.objectContaining({ sessionDefinitionId: 17 }),
		);
		expect(model.entrySessionId).toBe('');
	});

	it('clears the form and closes the modal on success', async () => {
		seedLoad();
		mocked.addLedgerEntry.mockResolvedValue(entry({ id: 'new' }));
		const model = createLedgerModel();
		await model.loadAll();

		model.showAddModal = true;
		model.entryDescription = 'Fresh entry';
		model.entryTag = 'other';
		model.entryAmount = 3;
		await model.addEntry();

		expect(model.entryDescription).toBe('');
		expect(model.entryTag).toBe('');
		expect(model.entryAmount).toBe(0);
		expect(model.showAddModal).toBe(false);
	});

	it('surfaces a failure and clears a stale error on entry', async () => {
		seedLoad();
		mocked.addLedgerEntry.mockRejectedValueOnce(new Error('rejected'));
		const model = createLedgerModel();
		await model.loadAll();

		model.entryDescription = 'Fresh entry';
		model.entryTag = 'other';
		model.entryAmount = 3;
		await model.addEntry();
		expect(model.error).toBe('rejected');

		mocked.addLedgerEntry.mockResolvedValue(entry({ id: 'new' }));
		await model.addEntry();
		expect(model.error).toBeNull();
	});
});

describe('presets', () => {
	it('applies a preset as a new entry and resets the pager', async () => {
		seedLoad(
			Array.from({ length: 6 }, (_, i) => entry({ id: `e${i}` })),
			null,
		);
		mocked.addLedgerEntry.mockResolvedValue(entry({ id: 'from-preset' }));
		const model = createLedgerModel();
		await model.loadAll();
		model.table.page = 1;

		await model.applyPreset(preset());
		expect(model.entries[0].id).toBe('from-preset');
		expect(model.table.page).toBe(0);
		expect(model.showAddModal).toBe(false);
	});

	it('saves and removes presets', async () => {
		seedLoad();
		mocked.addLedgerPreset.mockResolvedValue(preset({ id: 'p2', name: 'Repairs' }));
		mocked.deleteLedgerPreset.mockResolvedValue(undefined);
		const model = createLedgerModel();
		await model.loadAll();

		model.presetName = 'Repairs';
		model.presetDescription = 'Armour repairs';
		model.presetTag = 'repair';
		model.presetAmount = 5;
		await model.savePreset();
		expect(model.presets.map((p) => p.id)).toEqual(['p1', 'p2']);
		expect(model.presetName).toBe('');

		await model.removePreset('p1');
		expect(model.presets.map((p) => p.id)).toEqual(['p2']);
	});
});

describe('tag suggestions', () => {
	it('suggests same-type tags by frequency, excluding exact matches, only while focused', async () => {
		seedLoad(
			[
				entry({ id: 'e1', tag: 'equipment' }),
				entry({ id: 'e2', tag: 'equipment' }),
				entry({ id: 'e3', tag: 'enhancers' }),
				entry({ id: 'e4', type: 'markup', tag: 'estate' }),
			],
			null,
		);
		const model = createLedgerModel();
		await model.loadAll();

		model.entryTag = 'e';
		expect(model.ledgerTagSuggestions).toEqual([]);

		model.tagInputFocused = true;
		expect(model.ledgerTagSuggestions).toEqual(['equipment', 'enhancers']);

		model.entryTag = 'equipment';
		expect(model.ledgerTagSuggestions).toEqual([]);

		model.applyTagSuggestion('enhancers');
		expect(model.entryTag).toBe('enhancers');
		expect(model.tagInputFocused).toBe(false);
	});
});

describe('net-range summaries', () => {
	it('derives the tag cards and the net from the server summary, not the loaded page', async () => {
		// One loaded page entry only: the aggregates must NOT be a fold over
		// the page window, so they reflect the whole-ledger server summary.
		seedLoad([entry({ id: 'e1', type: 'expense', amount: 40 })], 'cursor-1');
		mocked.getLedgerSummary.mockResolvedValue({
			gains: { item_sale: 100, quest_reward: 2.5 },
			losses: { equipment: 500, repair: 50 },
		});
		const model = createLedgerModel();
		await model.loadAll();

		expect(mocked.getLedgerSummary).toHaveBeenCalledWith('all');
		expect(model.totalExpenses).toBe(550);
		expect(model.totalMarkup).toBe(102.5);
		expect(model.netLedger).toBe(-447.5);
		expect(model.expenseTags).toEqual([
			{ tag: 'equipment', total: 500 },
			{ tag: 'repair', total: 50 },
		]);
		expect(model.markupTags).toEqual([
			{ tag: 'item_sale', total: 100 },
			{ tag: 'quest_reward', total: 2.5 },
		]);
	});

	it('reloads the summary for the selected range', async () => {
		seedLoad();
		const model = createLedgerModel();
		await model.loadAll();

		mocked.getLedgerSummary.mockResolvedValue({ gains: {}, losses: { equipment: 40 } });
		model.netRange = '30d';
		await vi.waitFor(() => expect(model.totalExpenses).toBe(40));
		expect(mocked.getLedgerSummary).toHaveBeenLastCalledWith('30d');
		expect(model.netLedger).toBe(-40);
	});

	it('refreshes the summary after an add and a delete', async () => {
		seedLoad();
		mocked.addLedgerEntry.mockResolvedValue(entry({ id: 'new' }));
		mocked.deleteLedgerEntry.mockResolvedValue(undefined);
		const model = createLedgerModel();
		await model.loadAll();
		expect(mocked.getLedgerSummary).toHaveBeenCalledTimes(1);

		model.entryDescription = 'Fresh entry';
		model.entryTag = 'other';
		model.entryAmount = 3;
		await model.addEntry();
		expect(mocked.getLedgerSummary).toHaveBeenCalledTimes(2);

		await model.deleteEntry('new');
		expect(mocked.getLedgerSummary).toHaveBeenCalledTimes(3);
	});
});
