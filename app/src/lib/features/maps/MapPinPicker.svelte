<script lang="ts">
	import SearchHighlight from '$lib/components/SearchHighlight.svelte';
	import { onDestroy } from 'svelte';
	import PickerInput from '$lib/components/PickerInput.svelte';
	import type { MapPin } from '$lib/api';
	import { createTypeahead } from '$lib/view/typeahead.svelte';
	import { formatGamePoint } from './coords';
	import { filterMapPins } from './mapTools';
	import { pinGlyph } from './pinIcons';

	let { pins, onselect }: { pins: MapPin[]; onselect: (pin: MapPin) => void } = $props();

	const picker = createTypeahead<MapPin>({
		search: async (query) => filterMapPins(pins, query),
		debounceMs: 0,
		minLength: 1,
		labelOf: (pin) => pin.name,
	});

	const model = {
		get query() {
			return picker.query;
		},
		set query(value: string) {
			picker.query = value;
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
			return picker.settledQuery;
		},
		select(pin: MapPin) {
			picker.select(pin);
			onselect(pin);
			picker.clear();
		},
		clear() {
			picker.clear();
		},
	};

	$effect(() => {
		void pins;
		picker.clear();
	});

	onDestroy(() => picker.destroy());
</script>

<div class="space-y-1">
	<label for="map-pin-search" class="block text-[11px] text-text-secondary">Search pins</label>
	<PickerInput
		id="map-pin-search"
		placeholder="Name or notes"
		{model}
		class="relative"
		dropdownClass="absolute left-0 right-0 z-30 shadow-lg"
	>
		{#snippet result({ item, query })}
			<span class="flex min-w-0 items-center gap-2">
				<span aria-hidden="true">{pinGlyph(item.icon)}</span>
				<span class="truncate"><SearchHighlight text={item.name} {query} /></span>
			</span>
			<span class="ml-3 shrink-0 text-xs tabular-nums text-text-tertiary">
				{formatGamePoint({ lon: item.lon, lat: item.lat })}
			</span>
		{/snippet}
		{#snippet selection({ item })}<span>{item.name}</span>{/snippet}
	</PickerInput>
</div>
