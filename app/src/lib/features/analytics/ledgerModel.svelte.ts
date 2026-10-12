/**
 * Ledger-tab view model: the keyset-paged entry list, the entry and preset
 * forms with tag suggestions, and the net-impact summaries. Presentation
 * lives in the tab component; it composes over this state.
 *
 * Paging is two-layered by design: the server side stays keyset (an opaque
 * cursor grows the loaded window on demand as the pager steps past it),
 * while the client-side pager over the loaded window is the shared table
 * model; the server's whole-table count gives the pager its true bounds.
 */

import {
	addLedgerEntry,
	addLedgerPreset,
	deleteLedgerEntry,
	deleteLedgerPreset,
	getLedgerEntries,
	getLedgerPresets,
	getLedgerSummary,
} from '$lib/api';
import type { LedgerEntry, LedgerEntryType, LedgerPreset } from '$lib/types/analytics';
import { describeError } from '$lib/view/errorState';
import { filterByQuery, isSearchQuery } from '$lib/view/search';
import { createTableModel } from '$lib/view/tableModel.svelte';
import { ANALYTICS_RANGES, type AnalyticsRange, analyticsPeriod } from './analyticsRange';

export const PAGE_SIZE = 5;

export const netRanges = ANALYTICS_RANGES;
export type NetRange = AnalyticsRange;

export const tagLabels: Record<string, string> = {
	equipment: 'Equipment',
	repair: 'Repair',
	other: 'Other',
	item_sale: 'Auction Sales',
	quest_reward: 'Quest Reward',
	codex: 'Codex',
	inventory_sale: 'Mayhem',
};

export function createLedgerModel() {
	let netRange = $state<NetRange>('All Time');

	// The whole-ledger per-tag summary for the selected range, served by
	// the backend independently of the paginated entry list: the loaded
	// page window is a viewing slice, never the aggregate's source.
	let summaryGains = $state<Record<string, number>>({});
	let summaryLosses = $state<Record<string, number>>({});

	async function loadSummary() {
		try {
			const summary = await getLedgerSummary(analyticsPeriod(netRange));
			summaryGains = summary.gains;
			summaryLosses = summary.losses;
		} catch (e) {
			error = describeError(e, 'Failed to load ledger summary');
		}
	}

	let entries = $state<LedgerEntry[]>([]);
	// The whole-ledger row count from the server, so the pager reports
	// true bounds rather than the loaded window's size.
	let total = $state(0);
	let presets = $state<LedgerPreset[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	// Keyset pagination: the cursor for the next server page (null once the
	// whole ledger is loaded), and whether a "load more" fetch is in flight.
	let nextCursor = $state<string | null>(null);
	let loadingMore = $state(false);

	// Entry form state
	let entryType = $state<LedgerEntryType>('expense');
	let entryAmount = $state(0);
	let entryDescription = $state('');
	let entryTag = $state('');
	let tagInputFocused = $state(false);

	// Preset form state
	let presetName = $state('');
	let presetType = $state<LedgerEntryType>('expense');
	let presetAmount = $state(0);
	let presetDescription = $state('');
	let presetTag = $state('');
	let presetTagInputFocused = $state(false);

	let showAddModal = $state(false);
	let showPresetForm = $state(false);
	let showLedgerSources = $state(false);

	function buildTagSuggestions(query: string, type: LedgerEntryType): string[] {
		if (!isSearchQuery(query)) return [];
		// The tag already typed in full is not a suggestion.
		const typed = query.trim().toLowerCase();

		const tagCounts = new Map<string, number>();
		for (const entry of entries) {
			if (entry.type !== type) continue;
			const tag = entry.tag.trim();
			if (!tag || tag.toLowerCase() === typed) continue;
			tagCounts.set(tag, (tagCounts.get(tag) ?? 0) + 1);
		}

		return filterByQuery(Array.from(tagCounts.entries()), query, ([tag]) => tag)
			.sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
			.slice(0, 6)
			.map(([tag]) => tag);
	}

	const ledgerTagSuggestions = $derived(
		tagInputFocused ? buildTagSuggestions(entryTag, entryType) : [],
	);

	const presetTagSuggestions = $derived(
		presetTagInputFocused ? buildTagSuggestions(presetTag, presetType) : [],
	);

	// Client-side pager over the loaded entry window; server order preserved.
	const table = createTableModel<LedgerEntry>({
		rows: () => entries,
		pageSize: PAGE_SIZE,
	});

	async function loadAll() {
		loading = true;
		error = null;
		try {
			const [entryPage, presetRows] = await Promise.all([
				getLedgerEntries(),
				getLedgerPresets(),
				loadSummary(),
			]);
			entries = entryPage.items;
			nextCursor = entryPage.nextCursor;
			total = entryPage.total;
			presets = presetRows;
		} catch (e) {
			error = describeError(e, 'Failed to load ledger');
		} finally {
			loading = false;
		}
	}

	// Fetch the next keyset page and append it, growing the client paginator's
	// range. Older entries stay reachable without loading the whole table up
	// front.
	async function loadMoreEntries() {
		if (!nextCursor || loadingMore) return;
		error = null;
		loadingMore = true;
		try {
			const page = await getLedgerEntries(nextCursor);
			entries = [...entries, ...page.items];
			nextCursor = page.nextCursor;
			total = page.total;
		} catch (e) {
			error = describeError(e, 'Failed to load more entries');
		} finally {
			loadingMore = false;
		}
	}

	// Pager bounds from the server total: the client pages the loaded
	// window, and stepping past it fetches the next keyset page on demand.
	const totalPages = $derived(Math.max(1, Math.ceil(total / PAGE_SIZE)));

	async function nextPage() {
		const nextStart = (table.page + 1) * PAGE_SIZE;
		if (nextStart >= total) return;
		if (nextStart >= entries.length && nextCursor) await loadMoreEntries();
		if (nextStart < entries.length) table.page++;
	}

	function prevPage() {
		if (table.page > 0) table.page--;
	}

	async function addEntry() {
		const description = entryDescription.trim();
		const tag = entryTag.trim();
		if (!description || !tag || entryAmount <= 0) return;
		error = null;
		try {
			const newEntry = await addLedgerEntry({
				date: new Date().toISOString(),
				type: entryType,
				description,
				amount: entryAmount,
				tag,
			});
			entries = [newEntry, ...entries];
			total += 1;
			entryDescription = '';
			entryAmount = 0;
			entryTag = '';
			table.page = 0;
			showAddModal = false;
			void loadSummary();
		} catch (e) {
			error = describeError(e, 'Failed to add entry');
		}
	}

	function applyTagSuggestion(tag: string) {
		entryTag = tag;
		tagInputFocused = false;
	}

	function applyPresetTagSuggestion(tag: string) {
		presetTag = tag;
		presetTagInputFocused = false;
	}

	async function deleteEntry(id: string) {
		error = null;
		try {
			await deleteLedgerEntry(id);
			entries = entries.filter((e) => e.id !== id);
			total = Math.max(0, total - 1);
			void loadSummary();
		} catch (e) {
			error = describeError(e, 'Failed to delete entry');
		}
	}

	async function savePreset() {
		const name = presetName.trim();
		const description = presetDescription.trim();
		const tag = presetTag.trim();
		if (!name || !description || !tag || presetAmount <= 0) return;
		error = null;
		try {
			const newPreset = await addLedgerPreset({
				name,
				type: presetType,
				description,
				amount: presetAmount,
				tag,
			});
			presets = [...presets, newPreset];
			presetName = '';
			presetAmount = 0;
			presetDescription = '';
			presetTag = '';
			showPresetForm = false;
		} catch (e) {
			error = describeError(e, 'Failed to save preset');
		}
	}

	async function removePreset(id: string) {
		error = null;
		try {
			await deleteLedgerPreset(id);
			presets = presets.filter((p) => p.id !== id);
		} catch (e) {
			error = describeError(e, 'Failed to delete preset');
		}
	}

	async function applyPreset(preset: LedgerPreset) {
		error = null;
		try {
			const newEntry = await addLedgerEntry({
				date: new Date().toISOString(),
				type: preset.type,
				description: preset.description,
				amount: preset.amount,
				tag: preset.tag,
			});
			entries = [newEntry, ...entries];
			total += 1;
			table.page = 0;
			showAddModal = false;
			void loadSummary();
		} catch (e) {
			error = describeError(e, 'Failed to add entry');
		}
	}

	// Computed summaries: the netRange-scoped server aggregate.
	const expenseTags = $derived(
		Object.entries(summaryLosses).map(([tag, total]) => ({ tag, total })),
	);

	const markupTags = $derived(Object.entries(summaryGains).map(([tag, total]) => ({ tag, total })));

	const totalExpenses = $derived(expenseTags.reduce((sum, { total }) => sum + total, 0));

	const totalMarkup = $derived(markupTags.reduce((sum, { total }) => sum + total, 0));

	const netLedger = $derived(totalMarkup - totalExpenses);

	return {
		table,

		get netRange() {
			return netRange;
		},
		set netRange(value: NetRange) {
			netRange = value;
			void loadSummary();
		},
		get entries() {
			return entries;
		},
		get presets() {
			return presets;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},
		get nextCursor() {
			return nextCursor;
		},
		get loadingMore() {
			return loadingMore;
		},
		get total() {
			return total;
		},
		get totalPages() {
			return totalPages;
		},

		// Entry form
		get entryType() {
			return entryType;
		},
		set entryType(value: LedgerEntryType) {
			entryType = value;
		},
		get entryAmount() {
			return entryAmount;
		},
		set entryAmount(value: number) {
			entryAmount = value;
		},
		get entryDescription() {
			return entryDescription;
		},
		set entryDescription(value: string) {
			entryDescription = value;
		},
		get entryTag() {
			return entryTag;
		},
		set entryTag(value: string) {
			entryTag = value;
		},
		get tagInputFocused() {
			return tagInputFocused;
		},
		set tagInputFocused(value: boolean) {
			tagInputFocused = value;
		},

		// Preset form
		get presetName() {
			return presetName;
		},
		set presetName(value: string) {
			presetName = value;
		},
		get presetType() {
			return presetType;
		},
		set presetType(value: LedgerEntryType) {
			presetType = value;
		},
		get presetAmount() {
			return presetAmount;
		},
		set presetAmount(value: number) {
			presetAmount = value;
		},
		get presetDescription() {
			return presetDescription;
		},
		set presetDescription(value: string) {
			presetDescription = value;
		},
		get presetTag() {
			return presetTag;
		},
		set presetTag(value: string) {
			presetTag = value;
		},
		get presetTagInputFocused() {
			return presetTagInputFocused;
		},
		set presetTagInputFocused(value: boolean) {
			presetTagInputFocused = value;
		},

		get showAddModal() {
			return showAddModal;
		},
		set showAddModal(value: boolean) {
			showAddModal = value;
		},
		get showPresetForm() {
			return showPresetForm;
		},
		set showPresetForm(value: boolean) {
			showPresetForm = value;
		},
		get showLedgerSources() {
			return showLedgerSources;
		},
		set showLedgerSources(value: boolean) {
			showLedgerSources = value;
		},

		// Computed
		get ledgerTagSuggestions() {
			return ledgerTagSuggestions;
		},
		get presetTagSuggestions() {
			return presetTagSuggestions;
		},
		get expenseTags() {
			return expenseTags;
		},
		get markupTags() {
			return markupTags;
		},
		get totalExpenses() {
			return totalExpenses;
		},
		get totalMarkup() {
			return totalMarkup;
		},
		get netLedger() {
			return netLedger;
		},

		loadAll,
		loadMoreEntries,
		nextPage,
		prevPage,
		addEntry,
		applyTagSuggestion,
		applyPresetTagSuggestion,
		deleteEntry,
		savePreset,
		removePreset,
		applyPreset,
	};
}

export type LedgerModel = ReturnType<typeof createLedgerModel>;
