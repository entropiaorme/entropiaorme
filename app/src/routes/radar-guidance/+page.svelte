<script lang="ts">
	import { onMount } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import {
		getNavigationSnapshot,
		getRadarGuidance,
		type NavigationRun,
		type RadarGuidance,
	} from '$lib/api';
	import { getPreference } from '$lib/preferences';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import {
		DOT_RADIUS,
		guidanceMarks,
		positionIsCurrent,
		RADAR_GUIDANCE_CHANGED_EVENT,
	} from '$lib/features/maps/radarGuidance';

	// The window is laid over the radar by the shell, which also takes it off
	// the screen whenever guidance has no placement; this only draws.
	let run = $state<NavigationRun | null>(null);
	let receivedAt = $state(0);
	let now = $state(Date.now());
	let pollIntervalS = $state(1);
	let guidance = $state<RadarGuidance | null>(null);
	const placed = $derived(guidance?.kind === 'placed' ? guidance : null);
	// Steering stops when the position is too old to steer from.
	const current = $derived(
		run != null && positionIsCurrent(run.positionAgeS, receivedAt, now, pollIntervalS),
	);
	const marks = $derived(
		placed && current && run?.bearingDegrees != null
			? guidanceMarks(
					run.bearingDegrees,
					run.distanceToActive,
					placed.scale?.edgeRangeM ?? null,
					placed.labelStrip,
				)
			: null,
	);

	async function hydrateRun() {
		try {
			const next = await getNavigationSnapshot();
			run = next;
			receivedAt = Date.now();
		} catch {
			// The next route change re-reads it.
		}
	}

	async function hydrateGuidance() {
		try {
			guidance = await getRadarGuidance();
		} catch {
			// The next guidance change re-reads it.
		}
	}

	onMount(() => {
		const unlisten: Array<() => void> = [];
		let mounted = true;
		const keep = (stop: () => void) => (mounted ? unlisten.push(stop) : stop());
		void hydrateRun();
		void hydrateGuidance();
		void getPreference('navUpdateIntervalSec', 1).then((seconds) => (pollIntervalS = seconds));
		void listen('navigation:updated', () => void hydrateRun()).then(keep);
		void listen(RADAR_GUIDANCE_CHANGED_EVENT, () => void hydrateGuidance()).then(keep);
		// The position ages while no new one arrives.
		const stopClock = useVisiblePoll(
			() => {
				now = Date.now();
			},
			{ intervalMs: 1000, immediate: false },
		);
		return () => {
			mounted = false;
			stopClock();
			for (const stop of unlisten) stop();
		};
	});
</script>

{#if run?.status === 'active' && marks}
	<svg viewBox="0 0 100 100" class="block h-full w-full overflow-visible" aria-hidden="true">
		<defs>
			<filter id="glow"><feGaussianBlur stdDeviation="1.2" result="blur"/><feMerge><feMergeNode in="blur"/><feMergeNode in="SourceGraphic"/></feMerge></filter>
		</defs>
		{#each marks.segments as segment, index (index)}
			<line x1={segment.from.x} y1={segment.from.y} x2={segment.to.x} y2={segment.to.y} stroke="rgba(125,211,252,.9)" stroke-width="2" stroke-linecap="round" filter="url(#glow)" />
		{/each}
		{#if marks.dot}
			<circle cx={marks.dot.x} cy={marks.dot.y} r={DOT_RADIUS} fill="rgba(125,211,252,.95)" filter="url(#glow)" />
		{/if}
	</svg>
{/if}

<style>
	:global(html), :global(body), :global(.overlay-root) { width: 100% !important; height: 100% !important; pointer-events: none !important; }
</style>
