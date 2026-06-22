<script lang="ts">
	import { manualSkillScanCapturePng } from '$lib/api';
	import ScanGalleryPreview from './ScanGalleryPreview.svelte';

	let {
		captured,
		expected,
		dimmed = false,
	}: {
		captured: number;
		expected: number;
		dimmed?: boolean;
	} = $props();

	let previewPage = $state<number | null>(null);

	// Each captured page's preview is fetched once over the `capture_png` command
	// and held as a base64 data URL (the route returns raw bytes off the
	// in-process router, not a loopback URL). Keyed by page so a re-render reuses
	// the loaded URL; the effect loads any page not yet resolved as `captured`
	// grows.
	let srcs = $state<Record<number, string>>({});

	$effect(() => {
		for (let page = 1; page <= captured; page++) {
			if (srcs[page] === undefined) {
				void manualSkillScanCapturePng(page)
					.then((url) => {
						srcs = { ...srcs, [page]: url };
					})
					.catch(() => {
						// A preview that fails to load leaves its cell blank rather than
						// breaking the gallery; the capture itself is unaffected.
					});
			}
		}
	});

	let previewSrc = $derived(previewPage === null ? undefined : srcs[previewPage]);
</script>

<div class="grid grid-cols-3 gap-2 sm:grid-cols-4 md:grid-cols-6" class:opacity-60={dimmed}>
	{#each Array.from({ length: expected }, (_, i) => i + 1) as page (page)}
		{#if page <= captured}
			<button
				type="button"
				class="group relative aspect-[5/6] overflow-hidden rounded border border-border bg-surface transition-colors hover:border-accent cursor-pointer"
				onclick={() => (previewPage = page)}
				aria-label="Preview page {page}"
			>
				{#if srcs[page]}
					<img
						src={srcs[page]}
						alt="Captured page {page}"
						class="h-full w-full object-cover"
					/>
				{/if}
				<span class="absolute left-1 top-1 rounded bg-black/60 px-1.5 py-0.5 text-[10px] tabular-nums text-text">
					{page}
				</span>
			</button>
		{:else}
			<div class="flex aspect-[5/6] items-center justify-center rounded border border-dashed border-border bg-surface/40 text-xs text-text-tertiary tabular-nums">
				{page}
			</div>
		{/if}
	{/each}
</div>

{#if previewPage !== null && previewSrc !== undefined}
	<ScanGalleryPreview src={previewSrc} page={previewPage} onClose={() => (previewPage = null)} />
{/if}
