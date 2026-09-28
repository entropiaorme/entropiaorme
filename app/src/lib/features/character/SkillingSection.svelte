<script lang="ts">
	import type { Snippet } from 'svelte';

	// One facet of the skilling hub: a typographic header (title, one line
	// of what it answers, an optional control on the right) over its body.
	// Sections separate by a hairline and whitespace, never an enclosure.
	let {
		id,
		title,
		description,
		actions,
		children,
		busy = false,
	}: {
		id: string;
		title: string;
		description: string;
		actions?: Snippet;
		children: Snippet;
		busy?: boolean;
	} = $props();
</script>

<section {id} class="scroll-mt-6 border-t border-border/50 pt-7" aria-labelledby="{id}-title" aria-busy={busy}>
	<header class="mb-5 flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
		<div class="min-w-0">
			<h2 id="{id}-title" class="text-base font-semibold tracking-tight text-text">{title}</h2>
			<p class="mt-1 max-w-2xl text-xs leading-relaxed text-text-tertiary">{description}</p>
		</div>
		{#if actions}
			<div class="min-w-0 shrink-0">{@render actions()}</div>
		{/if}
	</header>
	{@render children()}
</section>
