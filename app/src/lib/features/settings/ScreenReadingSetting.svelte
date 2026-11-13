<script lang="ts">
	// A setting that reads the game screen while a session runs (creature
	// detection, reading the held tool): its toggle, its line of copy, and a
	// note when, turned on, capture cannot run. Rendered inside a block gated
	// on the in-development channel.
	import type { ScreenCapture } from '$lib/api';
	import { Toggle } from '$lib/components';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import { captureNote, type ScreenReading } from './screenReadingSetting';

	let {
		reading,
		enabled,
		capture,
		disabled,
		saved,
		onchange,
	}: {
		reading: ScreenReading;
		enabled: boolean;
		capture: ScreenCapture;
		disabled: boolean;
		saved: boolean;
		onchange: (checked: boolean) => void;
	} = $props();

	const note = $derived(enabled ? captureNote(capture) : null);
</script>

<div class="py-5 flex items-start justify-between gap-6">
	<div>
		<p class="flex items-center gap-2 text-sm text-text">
			{reading.title}
			<InDevelopmentMark id={reading.mark} align="left" />
		</p>
		<p class="text-xs text-text-tertiary mt-0.5">{reading.description}</p>
		{#if note}
			<p class="text-xs text-warning mt-1">{note}</p>
		{/if}
		{#if saved}
			<p class="text-xs text-success mt-1">Saved</p>
		{/if}
	</div>
	<Toggle checked={enabled} {disabled} {onchange} label={reading.title} />
</div>
