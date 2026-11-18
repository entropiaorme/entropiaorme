<script lang="ts">
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import Button from '$lib/components/Button.svelte';
	import { hideScanOverlay } from '$lib/api';
	import { createSkillScanModel } from '$lib/features/skillScan/skillScanModel.svelte';
	import { hydrate, scanStatus, subscribeScan } from '$lib/stores/scanStore.svelte';
	import { createWindowSizeSync } from '$lib/windows/windowSize';

	let overlayRoot: HTMLDivElement | null = $state(null);
	const windowSizeSync = createWindowSizeSync(() => overlayRoot);

	const scan = createSkillScanModel({ status: () => scanStatus.current, refresh: hydrate });
	let view = $derived(scan.view);

	// Attach the listener before the first read, so a change between the read
	// and the listener attaching re-reads rather than being lost. The webview
	// keeps listening while hidden, so the status is current when it shows.
	$effect(() => {
		let unlisten: (() => void) | undefined;
		let disposed = false;
		void subscribeScan().then((fn) => {
			if (disposed) {
				fn();
				return;
			}
			unlisten = fn;
			void hydrate();
		});
		return () => {
			disposed = true;
			unlisten?.();
		};
	});

	$effect(() => {
		if (!overlayRoot) return;
		return windowSizeSync.connect();
	});

	async function handleDrag(e: MouseEvent) {
		const target = e.target as HTMLElement;
		if (target.closest('button, [role="button"], input, select, textarea')) return;
		await getCurrentWindow().startDragging();
	}

	// Closing the overlay also stops a scan that is collecting: the shell
	// ends the watching with the window.
	function handleClose() {
		hideScanOverlay().catch(() => {});
	}
</script>

<!-- Kept: mousedown is the frameless overlay window's drag handle (pointer-only by nature); the controls inside are native buttons. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="flex flex-col items-start overlay-frame w-max" bind:this={overlayRoot} onmousedown={handleDrag}>
	<div class="overlay-strip glass-panel flex flex-col gap-3 rounded-xl px-4 py-3 w-72" data-overlay-surface>
		<div class="flex items-center gap-3">
			<span class="text-[10px] font-bold tracking-wider uppercase text-white/40 shrink-0">Skill scan</span>
			<button type="button" class="ml-auto release-btn" aria-label="Close" onclick={handleClose} title="Close">×</button>
		</div>

		{#if view.kind === 'idle'}
			<p class="text-[11px] leading-snug text-white/60 m-0">
				Start, then page through the Skills window in the game.
			</p>
			<Button variant="primary" size="sm" onclick={scan.start} disabled={scan.busy || !view.canStart}>
				Start
			</Button>
			{#if view.note}
				<p class="warn-line">{view.note}</p>
			{/if}
		{:else if view.kind === 'collecting'}
			<div class="flex items-baseline gap-2">
				<span class="text-sm font-semibold text-white/90 tabular-nums">{view.progress}</span>
			</div>
			{#if view.missing}
				<p class="text-[11px] leading-snug text-white/60 m-0 tabular-nums">{view.missing}</p>
			{/if}
			{#if view.hint}
				<p class="warn-line">{view.hint}</p>
			{/if}
			<Button variant="secondary" size="sm" onclick={scan.cancel} disabled={scan.busy}>Cancel</Button>
		{:else if view.kind === 'review' || view.kind === 'saving'}
			<p class="text-[11px] leading-snug text-white/70 m-0">
				Every page is in. Review the results in the main window.
			</p>
		{/if}

		{#if scan.error}
			<p class="text-[11px] text-white/50 m-0 break-words">{scan.error}</p>
		{/if}
	</div>
</div>

<style>
	.overlay-frame,
	.overlay-strip {
		overflow: visible;
	}

	.glass-panel {
		background: rgba(10, 14, 23, 0.85);
		backdrop-filter: blur(16px) saturate(150%);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}

	.warn-line {
		font-size: 11px;
		line-height: 1.35;
		color: rgba(251, 146, 60, 0.85);
		margin: 0;
	}

	.release-btn {
		width: 18px;
		height: 18px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.15);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.4);
		font-size: 12px;
		line-height: 1;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: all 150ms ease-out;
	}
	.release-btn:hover {
		background: rgba(255, 255, 255, 0.1);
		color: rgba(255, 255, 255, 0.7);
		border-color: rgba(255, 255, 255, 0.25);
	}
</style>
