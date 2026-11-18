<script lang="ts">
	/**
	 * The item box on a conversion line: suggestions from what the player
	 * holds, and free text for anything else, since a result is often an item
	 * the app has never seen.
	 */
	import { onDestroy, untrack } from 'svelte';
	import PickerInput from '$lib/components/PickerInput.svelte';
	import SearchHighlight from '$lib/components/SearchHighlight.svelte';
	import { rankByQuery } from '$lib/view/search';
	import { createTypeahead } from '$lib/view/typeahead.svelte';
	import type { ConvertibleItem } from './conversionDraft';

	let {
		id,
		value,
		items,
		placeholder = 'Item',
		onchange,
		class: className = '',
	}: {
		id: string;
		value: string;
		items: ConvertibleItem[];
		placeholder?: string;
		/** A typed name carries no asset; a chosen suggestion may. */
		onchange: (name: string, assetId: string | null) => void;
		class?: string;
	} = $props();

	const picker = createTypeahead<ConvertibleItem>({
		search: async (query) => rankByQuery(items, query, (row) => row.name, 12),
		debounceMs: 0,
		minLength: 1,
		labelOf: (row) => row.name,
	});

	// A draft can be filled from outside (a repeat of an earlier conversion),
	// and that must not open the suggestions over the form.
	$effect(() => {
		const next = value;
		untrack(() => {
			if (next !== picker.query) picker.fill(next);
		});
	});

	const model = {
		get query() {
			return picker.query;
		},
		set query(text: string) {
			picker.query = text;
			onchange(text, null);
		},
		get results() {
			return picker.results;
		},
		get selected() {
			return picker.selected;
		},
		get loading() {
			return picker.loading;
		},
		get error() {
			return picker.error;
		},
		get settledQuery() {
			return null;
		},
		select(row: ConvertibleItem) {
			picker.select(row);
			onchange(row.name, row.assetId);
		},
		clear() {
			picker.clear();
			onchange('', null);
		},
	};

	onDestroy(() => picker.destroy());
</script>

<PickerInput
	{id}
	{placeholder}
	{model}
	class="relative {className}"
	dropdownClass="absolute left-0 right-0 z-30 shadow-lg"
>
	{#snippet result({ item, query })}
		<span class="truncate"><SearchHighlight text={item.name} {query} /></span>
		{#if item.assetId}
			<span class="ml-3 shrink-0 text-xs text-text-tertiary">Asset</span>
		{/if}
	{/snippet}
	{#snippet selection({ item })}
		<span class="truncate">{item.name}</span>
	{/snippet}
</PickerInput>
