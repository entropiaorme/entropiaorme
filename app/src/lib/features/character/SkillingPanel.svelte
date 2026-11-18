<script lang="ts">
	import type { Snippet } from 'svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';

	// One view of the skilling hub. It opens with its answer in a sentence
	// (the lead, with what it rests on behind a tip) and the controls that
	// change it beside, over the evidence. The switcher names the view, so
	// the panel carries no heading of its own.
	let {
		id,
		label,
		lead,
		info,
		actions,
		children,
		busy = false,
	}: {
		id: string;
		label: string;
		lead?: Snippet;
		info?: Snippet;
		actions?: Snippet;
		children: Snippet;
		busy?: boolean;
	} = $props();
</script>

<section {id} aria-label={label} aria-busy={busy}>
	{#if lead || info || actions}
		<header class="mb-6 flex min-w-0 flex-wrap items-center justify-between gap-x-6 gap-y-3">
			<div class="flex min-w-0 items-center gap-1.5 text-sm text-text-secondary">
				{#if lead}
					<p class="min-w-0">{@render lead()}</p>
				{/if}
				{#if info}
					<InfoTip label="About {label}" width="w-80" align="left">
						{@render info()}
					</InfoTip>
				{/if}
			</div>
			{#if actions}
				<div class="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2">{@render actions()}</div>
			{/if}
		</header>
	{/if}
	{@render children()}
</section>
