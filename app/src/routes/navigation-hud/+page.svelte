<script lang="ts">
	import { onMount } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import Toggle from '$lib/components/Toggle.svelte';
	import { createWindowSizeSync } from '$lib/windows/windowSize';
	import { formatGamePoint } from '$lib/features/maps/coords';
	import { pinGlyph } from '$lib/features/maps/pinIcons';
	import { CARTOGRAPHY_OVERLAY_CHANGED_EVENT } from '$lib/features/maps/cartographyOverlay.svelte';
	import { createNavigationHudController } from '$lib/features/maps/navigationHudController.svelte';
	import {
		ROUTE_AREA_SELECTION_CANCELLED_EVENT,
		ROUTE_AREA_SELECTION_RESULT_EVENT,
	} from '$lib/features/maps/routeAreaSelection';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';

	let root: HTMLDivElement;
	const c = createNavigationHudController();
	const sizeSync = createWindowSizeSync(() => root);

	onMount(() => {
		let unlisten: (() => void) | undefined;
		let unlistenContext: (() => void) | undefined;
		let unlistenAreaResult: (() => void) | undefined;
		let unlistenAreaCancelled: (() => void) | undefined;
		void c.hydrate();
		void c.loadPrefs();
		void listen('navigation:updated', c.hydrate).then((stop) => (unlisten = stop));
		void listen(CARTOGRAPHY_OVERLAY_CHANGED_EVENT, (event) => c.applyContext(event.payload)).then(
			(stop) => (unlistenContext = stop),
		);
		void listen(ROUTE_AREA_SELECTION_RESULT_EVENT, (event) => c.applyRouteAreaSelection(event.payload)).then(
			(stop) => (unlistenAreaResult = stop),
		);
		void listen(ROUTE_AREA_SELECTION_CANCELLED_EVENT, (event) => c.applyRouteAreaSelectionCancelled(event.payload)).then(
			(stop) => (unlistenAreaCancelled = stop),
		);
		sizeSync.schedule();
		const observer = new ResizeObserver(() => sizeSync.schedule());
		observer.observe(root);
		return () => {
			unlisten?.();
			unlistenContext?.();
			unlistenAreaResult?.();
			unlistenAreaCancelled?.();
			observer.disconnect();
			sizeSync.cancel();
			c.dispose();
		};
	});

	// The position poll runs the observe-only path while a route is live. It
	// routes through the sanctioned visibility-gated helper (the single home for
	// timer loops), and the effect attaches it to the HUD's lifecycle.
	$effect(() => {
		if (c.run?.status !== 'active') return;
		const period = Math.max(1, c.updateIntervalSec) * 1000;
		return useVisiblePoll(() => c.pollPosition(), { intervalMs: period, immediate: false });
	});

	function drag(event: PointerEvent) {
		if (event.button !== 0 || !(event.target instanceof Element) || event.target.closest('button'))
			return;
		void getCurrentWindow().startDragging();
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div bind:this={root} class="p-2" onpointerdown={drag}>
	<div class="glass-panel w-72 rounded-xl p-3 text-white shadow-xl">
		<div class="flex items-start justify-between gap-3 border-b border-white/10 pb-2">
			<div class="min-w-0">
				<p class="text-[9px] font-bold uppercase tracking-wider text-white/35">{c.run ? 'Route guidance' : 'Plan route'}</p>
				<p class="mt-1 truncate text-[11px] text-white/65">{c.run ? `${c.run.planet} · ${c.run.mapViewName ?? 'Default'}${c.run.visitOnKey ? ' · F marks visited' : ''}` : c.planet ? `${c.planet} · new route` : 'No planet selected'}</p>
			</div>
			<div class="flex items-center gap-2">
				{#if c.run && c.active}
					<span class="rounded bg-white/10 px-1.5 py-0.5 text-[10px] font-semibold tabular-nums text-white/70">{c.cutCount}/{c.totalStops}</span>
				{/if}
				{#if c.run}
					<button class="release-btn" aria-label="End route and close" onclick={c.endRoute}>×</button>
				{:else}
					<button class="release-btn" aria-label="Close" onclick={c.closeOverlay}>×</button>
				{/if}
			</div>
		</div>
		{#if c.run && c.active}
			<div class="py-3">
				{#if c.outOfOrder}
					<div class="flex min-h-[2.75rem] items-center">
						<p class="text-[11px] text-amber-300">Tree visited out of order, recomputing path…</p>
					</div>
				{:else}
					<div class="flex items-center gap-2">
						<span class="text-xl">{pinGlyph(c.active.icon)}</span>
						<div class="min-w-0 flex-1">
							<p class="truncate text-sm font-semibold">{c.active.name}</p>
							<p class="text-[11px] tabular-nums text-white/55">{formatGamePoint(c.active)}</p>
						</div>
						{#if c.badge === 'cut'}
							<span class="rounded bg-emerald-400/20 px-1.5 py-0.5 text-[10px] font-semibold text-emerald-300">Tree Cut</span>
						{:else if c.badge === 'visited'}
							<span class="rounded bg-amber-400/20 px-1.5 py-0.5 text-[10px] font-semibold text-amber-300">Visited</span>
						{:else}
							<span class="text-[10px] tabular-nums text-sky-300">{c.run.distanceToActive?.toFixed(1) ?? '-'} m</span>
						{/if}
					</div>
				{/if}
			</div>
			{#if c.pendingHarvest}
				<div class="mb-2 rounded-md border border-emerald-300/30 bg-emerald-300/10 p-2">
					<p class="text-[10px] text-emerald-200">Harvest {c.pendingHarvest.distance.toFixed(1)} m from {c.pendingHarvest.name}. Cutting this tree?</p>
					<div class="mt-1.5 grid grid-cols-2 gap-1.5">
						<button class="hud-btn primary" disabled={c.busy} onclick={() => c.resolveHarvest(true)}>Yes, mark cut</button>
						<button class="hud-btn" disabled={c.busy} onclick={() => c.resolveHarvest(false)}>Not this one</button>
					</div>
				</div>
			{/if}
			{#if c.pendingVisit}
				<div class="mb-2 rounded-md border border-orange-300/30 bg-orange-300/10 p-2">
					<p class="text-[10px] text-orange-200">{c.pendingVisit.distance.toFixed(1)} m from {c.pendingVisit.name}. Mark it visited anyway?</p>
					<div class="mt-1.5 grid grid-cols-2 gap-1.5">
						<button class="hud-btn primary" disabled={c.busy} onclick={() => c.markVisited(true)}>Visit anyway</button>
						<button class="hud-btn" disabled={c.busy} onclick={c.clearPendingVisit}>Cancel</button>
					</div>
				</div>
			{/if}
			<div class="grid grid-cols-3 gap-1.5">
				<button class="hud-btn" disabled={c.busy} onclick={() => c.markVisited(false)}>Visited</button>
				<button class="hud-btn" disabled={c.busy} onclick={c.skip}>Skip</button>
				<button class="hud-btn" disabled={c.busy} onclick={c.undo}>Undo</button>
			</div>
		{:else if c.run?.status === 'completed'}
			<div class="py-4 text-center">
				<p class="text-sm font-semibold text-emerald-300">Route complete</p>
				<p class="mt-1 text-[10px] text-white/45">{c.run.stops.length} stops resolved</p>
			</div>
			<div class="grid grid-cols-2 gap-1.5">
				<button class="hud-btn" disabled={c.busy} onclick={c.undo}>Undo last</button>
				<button class="hud-btn primary" disabled={c.busy} onclick={c.endRoute}>Done</button>
			</div>
		{:else if c.planet}
			<div class="space-y-2.5 py-3">
				<div>
					<p class="text-[9px] uppercase tracking-wider text-white/35">Starting position</p>
					<p class="mt-0.5 text-[11px] tabular-nums text-white/70">{c.start ? formatGamePoint(c.start) : 'Not captured'}</p>
				</div>
				<button class="hud-btn primary w-full" disabled={c.busy} onclick={c.captureStart}>
					{c.start ? 'Capture again' : 'Capture current position'}
				</button>
				<div>
					<span class="mb-0.5 block text-[9px] uppercase tracking-wider text-white/35">Route trees</span>
					{#if c.selectedTreeCount == null}
						<p class="mb-1.5 text-[11px] text-white/70">All eligible trees</p>
						<button class="hud-btn w-full" disabled={c.busy} onclick={c.chooseRouteArea}>Select map area</button>
					{:else}
						<p class="mb-1.5 text-[11px] text-sky-300">{c.selectedTreeCount} {c.selectedTreeCount === 1 ? 'tree' : 'trees'} selected</p>
						<div class="grid grid-cols-2 gap-1.5">
							<button class="hud-btn" disabled={c.busy} onclick={c.chooseRouteArea}>Edit area</button>
							<button class="hud-btn" disabled={c.busy} onclick={c.useAllTrees}>Use all trees</button>
						</div>
					{/if}
				</div>
				<div class="flex items-center justify-between gap-2">
					<div class="min-w-0">
						<p class="text-[11px] text-white/70">Visit on F press</p>
						<p class="text-[10px] text-white/40">{c.visitOnKey ? 'Pressing F records the tree you are heading to' : 'Cuts are detected from your harvests'}</p>
					</div>
					<Toggle checked={c.visitOnKey} onchange={c.setVisitOnKey} label="Visit on F press" />
				</div>
				<label class="flex items-center gap-2">
					<span class="text-[10px] text-white/55">Update position every</span>
					<input class="hud-field w-14" type="number" min="1" max="60" bind:value={c.updateIntervalSec} onchange={c.persistInterval} aria-label="Position update interval in seconds" />
					<span class="text-[10px] text-white/55">s</span>
				</label>
				<button class="hud-btn primary w-full" disabled={!c.canStart} onclick={c.beginRoute}>Start route</button>
			</div>
		{:else}
			<p class="py-4 text-xs text-white/55">Open a planet map in the app to plan a route.</p>
		{/if}
		<div class="h-4 pt-1"><p class="truncate text-[9px] text-orange-300/85" role="status">{c.feedback ?? ''}</p></div>
	</div>
</div>

<style>
	.glass-panel { background: rgba(10,14,23,.88); backdrop-filter: blur(16px) saturate(150%); border: 1px solid rgba(255,255,255,.08); }
	.release-btn { color: rgba(255,255,255,.4); font-size: 18px; line-height: 1; cursor: pointer; }
	.hud-btn { border: 1px solid rgba(255,255,255,.1); background: rgba(255,255,255,.05); border-radius: 5px; padding: 5px 6px; color: rgba(255,255,255,.7); font-size: 10px; cursor: pointer; }
	.hud-btn:hover { border-color: rgba(56,189,248,.35); color: rgb(125 211 252); }
	.hud-btn.primary { background: rgba(56,189,248,.16); color: rgb(125 211 252); }
	.hud-btn:disabled { opacity: .4; cursor: default; }
	.hud-field { width: 100%; border: 1px solid rgba(255,255,255,.12); background: rgba(255,255,255,.05); border-radius: 5px; padding: 4px 7px; color: rgba(255,255,255,.85); font-size: 11px; }
	.hud-field:focus { outline: none; border-color: rgba(56,189,248,.5); }
</style>
