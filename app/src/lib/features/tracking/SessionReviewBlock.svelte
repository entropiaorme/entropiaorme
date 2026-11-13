<script lang="ts">
	// The shell an after-session review block in the session detail shares:
	// a divider, the heading with its in-development mark, the error and
	// the latest notice, then the block's rows, each on its own hairline.
	// The block decides whether it is shown at all.
	import type { Snippet } from 'svelte';
	import { Divider, ErrorNotice } from '$lib/components';
	import { InDevelopmentMark } from '$lib/inDevelopment';

	let {
		id,
		heading,
		mark,
		error,
		onDismissError,
		notice,
		children,
	}: {
		/** Stable id: names the heading and the block for tests. */
		id: string;
		heading: string;
		/** The registered in-development surface the block belongs to. */
		mark: string;
		error: string | null;
		onDismissError: () => void;
		notice: string | null;
		children: Snippet;
	} = $props();
</script>

<Divider />
<section aria-labelledby="{id}-heading" data-testid={id}>
	<div class="mb-3 flex flex-wrap items-baseline gap-2">
		<h3 id="{id}-heading" class="eyebrow">{heading}</h3>
		<InDevelopmentMark id={mark} align="left" />
	</div>

	<ErrorNotice message={error} onDismiss={onDismissError} class="mb-3" />
	{#if notice}
		<p class="mb-3 text-xs text-text-secondary" role="status">{notice}</p>
	{/if}

	<div class="divide-y divide-border/40">
		{@render children()}
	</div>
</section>
