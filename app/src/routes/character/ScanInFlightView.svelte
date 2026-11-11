<script lang="ts">
	import type { SkillScanStatus } from '$lib/api';
	import { createSkillScanModel } from '$lib/features/skillScan/skillScanModel.svelte';
	import { hydrate } from '$lib/stores/scanStore.svelte';
	import ScanReviewPane from './ScanReviewPane.svelte';

	let {
		status,
		onComplete,
	}: {
		status: SkillScanStatus;
		onComplete: () => void;
	} = $props();

	const scan = createSkillScanModel({ status: () => status, refresh: hydrate });
	let view = $derived(scan.view);
</script>

<div class="space-y-4">
	<div class="flex items-baseline justify-between gap-4">
		<div class="flex items-baseline gap-3">
			<span class="text-sm text-text">Skills scan</span>
			{#if view.kind === 'collecting'}
				<span class="text-xs text-text-secondary tabular-nums">{view.progress}</span>
			{/if}
		</div>
		{#if view.kind === 'collecting'}
			<button
				type="button"
				class="rounded bg-surface px-2.5 py-1 text-xs text-text-secondary transition-colors hover:bg-surface-hover hover:text-text disabled:opacity-50 cursor-pointer"
				onclick={scan.cancel}
				disabled={scan.busy}
			>
				Cancel
			</button>
		{/if}
	</div>

	{#if scan.error}
		<p class="text-xs text-warning">{scan.error}</p>
	{/if}

	{#if view.kind === 'collecting'}
		<div class="h-1 overflow-hidden rounded-full bg-surface">
			<div
				class="h-full bg-accent/60 transition-[width] duration-150 ease-out"
				style="width: {Math.min(100, Math.max(0, view.fraction * 100))}%"
			></div>
		</div>
		{#if view.missing || view.hint}
			<div class="space-y-1 text-center text-xs">
				{#if view.missing}
					<p class="text-text-secondary tabular-nums">{view.missing}</p>
				{/if}
				{#if view.hint}
					<p class="text-warning">{view.hint}</p>
				{/if}
			</div>
		{/if}
	{:else if view.kind === 'review' || view.kind === 'saving'}
		<ScanReviewPane {onComplete} saving={view.kind === 'saving'} />
	{/if}
</div>
