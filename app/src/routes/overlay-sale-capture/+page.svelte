<!--
	The sale-window capture button, reachable while the game holds the screen.

	It is the main window's button in another place, and deliberately nothing
	more: no fields, no editing, no result to read. What it says is only
	whether to try again before tabbing away, because a read that failed
	because the window drifted off the corner is fixed in a second while the
	game is still in front, and discovering it later costs exactly the trip
	this window exists to save. The figures themselves are reviewed in the
	form, which is where they can be corrected and where they are committed.
-->
<script lang="ts">
	import {
		ApiError,
		captureAuctionFeeResearchSample,
		captureSaleFromOverlay,
		getAuctionFeeResearchOverlayStatus,
		hideSaleCaptureOverlay,
		type AuctionFeeOverlayStatus,
	} from '$lib/api';
	import Button from '$lib/components/Button.svelte';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import { createWindowSizeSync } from '$lib/windows/windowSize';
	import { onMount } from 'svelte';

	let overlayRoot: HTMLDivElement | null = $state(null);
	const windowSizeSync = createWindowSizeSync(() => overlayRoot);

	// The window is sized to whatever the panel measures, so the result line
	// appearing does not leave it clipped.
	onMount(() => windowSizeSync.connect());

	let busy = $state(false);
	let message = $state('');
	let failed = $state(false);
	let research = $state<AuctionFeeOverlayStatus | null>(null);

	async function refreshResearchStatus() {
		try {
			research = await getAuctionFeeResearchOverlayStatus();
			if (research.active) {
				failed = research.failed;
				message = research.message ?? 'Ready. Change the values, then press Space.';
			}
		} catch (cause) {
			// Developer mode being off means this is the normal Inventory
			// capture surface. Transient IPC failures retain the last-good
			// authority so a research capture cannot be misrouted as a sale.
			if (cause instanceof ApiError && cause.kind === 'notFound') research = null;
		}
	}

	onMount(() => useVisiblePoll(refreshResearchStatus, { intervalMs: 400 }));

	async function capture() {
		if (busy) return;
		busy = true;
		message = '';
		try {
			if (research?.active) {
				research = await captureAuctionFeeResearchSample();
				failed = research.failed;
				message = research.message ?? 'Sample saved.';
			} else {
				const reply = await captureSaleFromOverlay();
				failed = reply.failed;
				message = reply.message;
			}
		} catch (cause) {
			failed = true;
			message = cause instanceof Error ? cause.message : 'Could not read the sale window';
		} finally {
			busy = false;
		}
	}
</script>

<div
	bind:this={overlayRoot}
	class="bg-surface-raised/95 text-text-primary border-border-bright flex w-[240px] flex-col gap-2 rounded-md border p-3 backdrop-blur"
	data-overlay-surface
>
	<div class="flex items-center gap-2">
		<div data-tauri-drag-region class="text-text-tertiary flex-1 cursor-move text-xs">
			{research?.active ? 'Auction fee study' : 'Capture listing'}
		</div>
		<button
			type="button"
			class="text-text-tertiary hover:text-text-primary text-xs leading-none"
			onclick={() => hideSaleCaptureOverlay()}
			aria-label="Close"
		>
			✕
		</button>
	</div>

	<Button variant="secondary" size="sm" onclick={capture} disabled={busy || research?.busy}>
		{busy || research?.busy
			? 'Reading the window'
			: research?.active
				? 'Capture sample'
				: 'Capture from game'}
	</Button>

	{#if research?.active}
		<div class="flex items-center justify-between text-xs text-text-tertiary">
			<span><kbd class="font-mono text-text-secondary">Space</kbd> captures</span>
			<span>{research.sampleCount} saved</span>
		</div>
	{/if}

	{#if message !== ''}
		<p class="text-xs {failed ? 'text-status-danger' : 'text-text-tertiary'}">{message}</p>
	{/if}
</div>
