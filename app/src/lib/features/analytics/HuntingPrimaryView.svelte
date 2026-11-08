<script lang="ts">
	import type { TableModel } from '$lib/view/tableModel.svelte';
	import HuntingSessionPicker from './HuntingSessionPicker.svelte';
	import HuntingSessions from './HuntingSessions.svelte';
	import TreeCuttingStats from './TreeCuttingStats.svelte';
	import type {
		HuntingOverallLine,
		HuntingSessionSection,
	} from './huntingModel.svelte';

	let {
		overall,
		table,
		selected,
		totalCount,
		onselect,
	}: {
		overall: HuntingOverallLine;
		table: TableModel<HuntingSessionSection>;
		selected: HuntingSessionSection | null;
		totalCount: number;
		onselect: (key: string | null) => void;
	} = $props();

	const line = $derived(selected ?? overall);
</script>

{#snippet scopeControl()}
	<HuntingSessionPicker {table} {selected} {overall} {totalCount} {onselect} />
{/snippet}

<section class="relative" data-testid="hunting-primary-surface">
	{#if selected?.isUnassigned}
		<div class="min-w-0">
			{@render scopeControl()}
			<div class="mt-5">
				<HuntingSessions {selected} />
			</div>
		</div>
	{:else}
		<div class="min-w-0">
			<span class="eyebrow block text-text-tertiary">Session view</span>
			<div class="mt-0.5">{@render scopeControl()}</div>
		</div>

		<div class="mt-6">
			<TreeCuttingStats
				cycled={line.cycled}
				returns={line.returns}
				lootRate={line.lootRate}
				muProjectedReturns={line.muProjectedReturns}
				muRate={line.muRate}
				lootMarkupFactor={line.lootMarkupFactor}
				expectedTtRate={line.expectedTtRate}
				expectedMarketRate={line.expectedMarketRate}
				expectedEconomics={line.expected}
				realisedReturns={line.realisedReturns}
				realisedRate={line.realisedRate}
				realisedDerived={line.realisedDerived}
				reward={line.reward}
				rewardScope={selected ? 'session' : 'overall'}
				rewardMuRate={line.rewardMuRate}
				expectedTotalRate={line.expectedTotalRate}
			/>
		</div>

		{#if selected}
			<div class="mt-5">
				<HuntingSessions {selected} />
			</div>
		{/if}
	{/if}
</section>
