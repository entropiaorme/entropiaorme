<script lang="ts">
	import type { TrackingSnapshot } from '$lib/api';
	import type { StatId, StatRender } from '$lib/statsRegistry';
	import CycledTip from './CycledTip.svelte';

	/**
	 * The explanatory tip a dashboard stat tile carries beside its label,
	 * when its figure has one: the one place that maps a stat to the tip
	 * that explains it, so the tile itself stays a plain label and value.
	 * Tips explain the session in play, so the lifetime flip carries none.
	 */
	let {
		id,
		status,
		render
	}: {
		id: StatId;
		status: TrackingSnapshot | null;
		render: StatRender;
	} = $props();

	/** The stats that carry a tip. */
	const TIPPED: ReadonlySet<StatId> = new Set(['cycled']);

	const active = $derived(status?.status === 'active' ? status : null);
</script>

{#if active && TIPPED.has(id)}
	<!-- A press on the tip must not start the tile's drag-reorder. -->
	<span class="inline-flex" onpointerdown={(event) => event.stopPropagation()} role="presentation">
		{#if id === 'cycled'}
			<CycledTip
				lines={active.cycledBreakdown ?? []}
				totalPed={active.cost ?? 0}
				incomplete={render.incomplete}
			/>
		{/if}
	</span>
{/if}
