<script lang="ts">
	// The one marker beside the hotbar's tool display while reading the tool
	// in hand off the screen is turned on but the screen is not being read.
	// The hotbar's display and its cues stand as they always do; expanding
	// the marker says why, in the shared expanding shell.
	import HoverFacet from '$lib/components/overlay/HoverFacet.svelte';
	import type { HoverPanel } from '$lib/components/overlay/hoverPanel.svelte';
	import { SCREEN_NOT_READ_MARKER } from './heldTool';

	let { line, panel }: { line: string; panel: HoverPanel } = $props();
</script>

{#snippet shows()}
	<span
		class="text-[9px] leading-tight uppercase tracking-wider whitespace-nowrap text-amber-300/90"
		data-testid="screen-unread-marker"
	>
		{SCREEN_NOT_READ_MARKER}
	</span>
{/snippet}

<HoverFacet
	{panel}
	label="Reading the tool in hand"
	panelId="screen-unread-panel"
	toggleTitle={line}
	toggleTestId="screen-unread"
	panelTestId="screen-unread-panel"
	testId="screen-unread-facet"
	class="flex items-center shrink-0"
>
	{#snippet control(toggle)}
		{@render toggle(shows)}
	{/snippet}
	{#snippet content()}
		<span class="facet-label mb-2 block">Tool</span>
		<p class="text-[11px] leading-snug text-white/70" data-testid="screen-unread-line">{line}</p>
	{/snippet}
</HoverFacet>
