<script lang="ts">
	/**
	 * The hotbar preset switch, mirroring the game's: three keys, the one in
	 * force lit. Flipping it here tells the app which preset's slots a hotbar
	 * key press now means.
	 */
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import type { HotbarPreset } from '$lib/types/settings';
	import { HOTBAR_PRESETS } from './carriedWeapons';

	let {
		active,
		onchange,
	}: {
		active: HotbarPreset;
		onchange: (preset: HotbarPreset) => void;
	} = $props();

	const options = HOTBAR_PRESETS.map((preset) => ({
		id: String(preset),
		label: String(preset),
		ariaLabel: `Hotbar preset ${preset}`,
		title: `Hotbar preset ${preset}`,
	}));

	function select(id: string) {
		const preset = HOTBAR_PRESETS.find((candidate) => String(candidate) === id);
		if (preset !== undefined) onchange(preset);
	}
</script>

<SegmentedControl
	{options}
	active={String(active)}
	onchange={select}
	variant="keys"
	label="Hotbar preset"
/>
