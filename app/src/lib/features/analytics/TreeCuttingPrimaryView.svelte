<script lang="ts">
	import InfoTip from '$lib/components/InfoTip.svelte';
	import type { HarvestYieldTier } from '$lib/types/analytics';
	import type { TableModel } from '$lib/view/tableModel.svelte';
	import ActivityLootComposition from './ActivityLootComposition.svelte';
	import TreeCuttingActivityPicker from './TreeCuttingActivityPicker.svelte';
	import TreeCuttingStats from './TreeCuttingStats.svelte';
	import type { TreeCuttingOverall, TreeCuttingSection } from './treeCuttingModel.svelte';

	let {
		overall,
		table,
		selected,
		totalCount,
		onselect,
	}: {
		overall: TreeCuttingOverall;
		table: TableModel<TreeCuttingSection>;
		selected: TreeCuttingSection | null;
		totalCount: number;
		onselect: (tier: HarvestYieldTier | null) => void;
	} = $props();

	const line = $derived(selected ?? overall);
</script>

{#snippet scopeControl()}
	<TreeCuttingActivityPicker {table} {selected} {overall} {totalCount} {onselect} />
{/snippet}

<section class="relative" data-testid="tree-cutting-primary-surface">
	{#if selected?.yieldTier === 'unknown'}
		<div class="min-w-0">
			{@render scopeControl()}
			<div class="mt-5 flex min-h-28 items-center justify-center border-t border-border/50 pt-5">
				<div class="flex items-center gap-1.5 text-sm text-text-secondary">
					<span>
						{selected.swings} {selected.swings === 1 ? 'swing is' : 'swings are'} unclassified and cannot be assigned to a board activity.
					</span>
					<InfoTip label="Why swings can be unclassified" width="w-80">
						<p class="text-xs font-semibold leading-relaxed text-text">Why swings can be unclassified</p>
						<p class="mt-1 text-xs leading-relaxed text-text-secondary">
							A swing is unclassified when no board output identifies its activity. This can happen on a failed or shavings-only swing without nearby board evidence, when neighbouring evidence conflicts, or when a board name is not recognised.
						</p>
						<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
							Its recorded cost and loot still count in Overall. It cannot support a board-tier comparison.
						</p>
					</InfoTip>
				</div>
			</div>
		</div>
	{:else}
		<div class="min-w-0">
			<span class="eyebrow block text-text-tertiary">Board activity</span>
			<div class="mt-0.5">{@render scopeControl()}</div>
		</div>

		<div class="mt-6">
			<TreeCuttingStats
				cycled={line.cycled}
				returns={line.returns}
				lootRate={line.lootRate}
				muProjectedReturns={line.muProjectedReturns}
				muRate={line.muRate}
				realisedReturns={line.realisedReturns}
				realisedRate={line.realisedRate}
				overhead={line.overhead}
				unassignedOverhead={'unassignedOverhead' in line ? line.unassignedOverhead : 0}
			/>
		</div>

		{#if selected}
			<ActivityLootComposition
				items={selected.items}
				marketAvailable={selected.muProjectedReturns !== null}
				emptyLabel="No loot recorded for this board activity yet."
				disclosure="activity"
			/>
		{/if}
	{/if}
</section>
