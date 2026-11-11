<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import Menu from '$lib/components/Menu.svelte';
	import type { MapPin } from '$lib/api';
	import MapPinPicker from './MapPinPicker.svelte';

	let {
		pins,
		ontoggleoverlay,
		onconfigure,
		oncalibrate,
		onselectpin,
		onroute,
		onselectpins,
		onradarcalibrate,
		disabled = false,
	}: {
		pins: MapPin[];
		ontoggleoverlay: () => void;
		onconfigure: () => void;
		oncalibrate: () => void;
		onselectpin: (pin: MapPin) => void;
		onroute: () => void;
		onselectpins: () => void;
		onradarcalibrate: () => void;
		disabled?: boolean;
	} = $props();
	let searchOpen = $state(false);

	const setupItems = $derived([
		{ label: 'Configure pin overlay', onSelect: onconfigure },
		// Escape hatches for a setup where the radar cannot be found: both
		// are read only when the automatic find fails.
		{ label: 'Set the coordinate region by hand', onSelect: oncalibrate },
		{ label: 'Set the radar by hand', onSelect: onradarcalibrate },
		{
			label: searchOpen ? 'Hide pin search' : 'Search pins',
			onSelect: () => (searchOpen = !searchOpen),
		},
	]);
</script>

<div class="relative flex shrink-0 items-center gap-1">
	<Button size="sm" {disabled} onclick={ontoggleoverlay}>Pin overlay</Button>
	<Button size="sm" variant="secondary" {disabled} onclick={onroute}>Route</Button>
	<Button size="sm" variant="secondary" {disabled} onclick={onselectpins}>Select pins</Button>
	<Menu ariaLabel="Map setup" items={setupItems}>
		{#snippet trigger({ open, toggle })}
			<Button size="sm" variant="ghost" {disabled} aria-haspopup="menu" aria-expanded={open} onclick={toggle}>Setup</Button>
		{/snippet}
	</Menu>

	{#if searchOpen}
		<div class="absolute right-0 top-full z-30 mt-2 w-72 rounded-md border border-border bg-surface-raised p-2 shadow-lg">
			<MapPinPicker {pins} onselect={onselectpin} />
		</div>
	{/if}
</div>
