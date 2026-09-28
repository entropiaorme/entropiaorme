<script lang="ts">
	import type { Snippet } from 'svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';

	// One facet of the skilling hub: its title, an optional explanation
	// behind a hover tip, and its control beside the title, over the body.
	// Sections separate by a hairline and whitespace, never an enclosure.
	let {
		id,
		title,
		info,
		actions,
		children,
		busy = false,
	}: {
		id: string;
		title: string;
		info?: Snippet;
		actions?: Snippet;
		children: Snippet;
		busy?: boolean;
	} = $props();
</script>

<section {id} class="scroll-mt-6 border-t border-border/50 pt-6" aria-labelledby="{id}-title" aria-busy={busy}>
	<header class="mb-5 flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2">
		<div class="flex items-center gap-1.5">
			<h2 id="{id}-title" class="text-base font-semibold tracking-tight text-text">{title}</h2>
			{#if info}
				<InfoTip label="About {title}" width="w-80" align="left">
					{@render info()}
				</InfoTip>
			{/if}
		</div>
		{#if actions}
			<div class="flex min-w-0 items-center gap-3">{@render actions()}</div>
		{/if}
	</header>
	{@render children()}
</section>
