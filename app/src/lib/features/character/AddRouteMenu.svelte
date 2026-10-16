<script lang="ts">
	import type { EquipmentSearchResult, ManualMobSuggestion } from '$lib/api';
	import { getManualMobSuggestions, searchEquipmentItems } from '$lib/api';
	import Button from '$lib/components/Button.svelte';
	import Menu from '$lib/components/Menu.svelte';
	import PickerInput from '$lib/components/PickerInput.svelte';
	import SearchHighlight from '$lib/components/SearchHighlight.svelte';
	import { createTypeahead } from '$lib/view/typeahead.svelte';
	import type { StoredRoute } from './routesView';

	// Plan a route to price: a catalogue weapon on a mob maturity. Nothing
	// needs to have been played; the route is priced from the catalogue.
	let { onadd }: { onadd: (route: StoredRoute) => void } = $props();

	const weapon = createTypeahead<EquipmentSearchResult>({
		search: (query) => searchEquipmentItems(query, 'weapon'),
		labelOf: (item) => item.name,
	});
	const mob = createTypeahead<ManualMobSuggestion>({
		search: (query) => getManualMobSuggestions(query),
		labelOf: (item) => item.display,
	});

	$effect(() => () => {
		weapon.destroy();
		mob.destroy();
	});

	function add(close: () => void) {
		const picked = weapon.selected;
		const target = mob.selected;
		if (!picked?.catalogId || !target) return;
		onadd({
			weaponId: picked.catalogId,
			weaponName: picked.name,
			species: target.species,
			maturity: target.maturity,
			display: target.display,
		});
		weapon.clear();
		mob.clear();
		close();
	}
</script>

<Menu
	ariaLabel="Plan a route"
	overlay
	align="right"
	initialFocus="first-input"
	panelClass="w-[min(26rem,calc(100vw-1rem))] p-3"
>
	{#snippet trigger({ open, toggle, keydown })}
		<button
			type="button"
			class="linklet"
			aria-haspopup="menu"
			aria-expanded={open}
			onclick={toggle}
			onkeydown={keydown}
		>
			Add a route
		</button>
	{/snippet}

	{#snippet children({ close })}
		<div class="space-y-3">
			<label class="block space-y-1.5">
				<span class="eyebrow">Weapon</span>
				<PickerInput id="skilling-route-weapon" model={weapon} placeholder="Search weapons">
					{#snippet result({ item, query })}
						<span class="truncate text-text"><SearchHighlight text={item.name} {query} /></span>
					{/snippet}
					{#snippet selection({ item })}
						<span class="flex-1 truncate text-text">{item.name}</span>
					{/snippet}
				</PickerInput>
			</label>
			<label class="block space-y-1.5">
				<span class="eyebrow">Mob</span>
				<PickerInput id="skilling-route-mob" model={mob} placeholder="Search mobs, e.g. puny carabok">
					{#snippet result({ item, query })}
						<span class="truncate text-text"><SearchHighlight text={item.display} {query} /></span>
					{/snippet}
					{#snippet selection({ item })}
						<span class="flex-1 truncate text-text">{item.display}</span>
					{/snippet}
				</PickerInput>
			</label>
			<div class="flex justify-end pt-1">
				<Button size="sm" disabled={!weapon.selected?.catalogId || !mob.selected} onclick={() => add(close)}>
					Add route
				</Button>
			</div>
		</div>
	{/snippet}
</Menu>
