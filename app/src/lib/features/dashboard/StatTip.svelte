<script lang="ts">
	import type { TrackingSnapshot } from '$lib/api';
	import type { StatId, StatRender } from '$lib/statsRegistry';
	import LooterBasisSelect from '$lib/components/LooterBasisSelect.svelte';
	import { liveMarkupSettings, setLiveLooterBasis } from '$lib/liveMarkupSettings.svelte';
	import CycledTip from './CycledTip.svelte';
	import ExpectedMuTip from './ExpectedMuTip.svelte';
	import type { LiveValuationView } from './liveLootValuation.svelte';
	import MarkupStatTip from './MarkupStatTip.svelte';

	/**
	 * What a dashboard stat tile carries beside its label, when its figure
	 * has one: the tip that explains it, and for the expected figure the
	 * looter basis it is read at. The one place that maps a stat to them, so
	 * the tile itself stays a plain label and value. They explain the
	 * session in play, so the lifetime flip carries none.
	 */
	let {
		id,
		status,
		render,
		valuation = null,
	}: {
		id: StatId;
		status: TrackingSnapshot | null;
		render: StatRender;
		valuation?: LiveValuationView | null;
	} = $props();

	const active = $derived(status?.status === 'active' ? status : null);
	// A markup tip explains a figure; while the figure reads empty there is
	// nothing to explain.
	const valued = $derived(
		active && valuation && valuation.sessionId === active.session_id && valuation.lootTt > 0
			? valuation
			: null,
	);
	const cycle = $derived(active?.expectedCycle ?? null);
	const levels = $derived(
		cycle
			? {
					animal: cycle.bases.animal.looterLevel,
					mutant: cycle.bases.mutant.looterLevel,
					robot: cycle.bases.robot.looterLevel,
				}
			: null,
	);
	const shown = $derived(
		active !== null &&
			(id === 'cycled' ||
				((id === 'loot_mu' || id === 'mu_rate') && valued !== null) ||
				(id === 'expected_mu_rate' && levels !== null)),
	);
</script>

{#if active && shown}
	<!-- A press on the tip must not start the tile's drag-reorder. -->
	<span class="inline-flex" onpointerdown={(event) => event.stopPropagation()} role="presentation">
		{#if id === 'cycled'}
			<CycledTip
				lines={active.cycledBreakdown ?? []}
				totalPed={active.cost ?? 0}
				incomplete={render.incomplete}
			/>
		{:else if valued && id === 'loot_mu'}
			<MarkupStatTip valuation={valued} />
		{:else if valued && id === 'mu_rate'}
			<MarkupStatTip valuation={valued} cycledPed={active.cost ?? 0} />
		{:else if id === 'expected_mu_rate' && levels && cycle}
			<LooterBasisSelect
				compact
				{levels}
				basis={liveMarkupSettings.looterBasis}
				onchange={(next) => void setLiveLooterBasis(next)}
			/>
			{#if valued?.lootMuFactor != null}
				<ExpectedMuTip
					{cycle}
					basis={liveMarkupSettings.looterBasis}
					lootMuFactor={valued.lootMuFactor}
					cycledPed={active.cost ?? 0}
				/>
			{/if}
		{/if}
	</span>
{/if}
