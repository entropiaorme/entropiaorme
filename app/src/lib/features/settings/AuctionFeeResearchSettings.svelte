<script lang="ts">
	import { onMount } from 'svelte';
	import { Button, Divider } from '$lib/components';
	import {
		getAuctionFeeResearchStatus,
		hideSaleCaptureOverlay,
		showSaleCaptureOverlay,
		startAuctionFeeResearch,
		stopAuctionFeeResearch,
		type AuctionFeeResearchStatus,
	} from '$lib/api';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';

	let research: AuctionFeeResearchStatus | null = $state(null);
	let error: string | null = $state(null);

	async function refresh() {
		try {
			research = await getAuctionFeeResearchStatus();
		} catch {
			// Preserve the last-good session through transient polling errors.
		}
	}

	onMount(() => {
		void refresh();
		return useVisiblePoll(refresh, { intervalMs: 1000, immediate: false });
	});

	async function start() {
		error = null;
		try {
			research = await startAuctionFeeResearch();
			await showSaleCaptureOverlay();
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Could not start auction fee capture';
		}
	}

	async function stop() {
		error = null;
		try {
			research = await stopAuctionFeeResearch();
			await hideSaleCaptureOverlay();
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Could not stop auction fee capture';
		}
	}

	async function reopen() {
		error = null;
		try {
			await showSaleCaptureOverlay();
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Could not reopen sale capture';
		}
	}
</script>

<Divider />
<div class="py-5 flex items-start justify-between gap-6">
	<div class="min-w-0">
		<p class="text-sm text-text">Auction fee research</p>
		<p class="text-xs text-text-tertiary mt-0.5">
			Capture quoted fees without creating a listing. Change the sell queue's values and press Space for each sample.
		</p>
		{#if research?.outputDir}
			<p class="mt-1 break-all font-mono text-[11px] text-text-tertiary">
				{research.sampleCount} samples · {research.outputDir}
			</p>
		{/if}
		{#if error}
			<p class="mt-1 text-xs text-error">{error}</p>
		{/if}
	</div>
	<div class="flex shrink-0 gap-2">
		{#if research?.active}
			<Button variant="ghost" size="sm" onclick={stop}>Finish</Button>
			<Button variant="secondary" size="sm" onclick={reopen}>Reopen</Button>
		{:else}
			<Button variant="secondary" size="sm" onclick={start}>Start capture</Button>
		{/if}
	</div>
</div>
