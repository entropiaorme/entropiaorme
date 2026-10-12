<script lang="ts">
	import DataTable from '$lib/components/DataTable.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import ExpectedReturnInfoTip from '$lib/components/ExpectedReturnInfoTip.svelte';
	import LooterBasisSelect from '$lib/components/LooterBasisSelect.svelte';
	import { createBreakEvenModel } from '$lib/features/market/breakEvenModel.svelte';
	import { NO_DATA } from '$lib/utils/format';

	const model = createBreakEvenModel();

	$effect(() => {
		void model.loadData();
	});

	const columns = $derived([
		{ key: 'name', label: 'Loadout' },
		{ key: 'weightedEfficiencyPct', label: 'TT-weighted Efficiency', align: 'right' as const },
		{ key: 'expectedTtReturnPct', label: 'Model TT', align: 'right' as const },
		{ key: 'breakEvenLootMarkupPct', label: 'Break-even loot MU', align: 'right' as const },
	]);
</script>

{#snippet expectedReturnTip()}
	<ExpectedReturnInfoTip />
{/snippet}

{#if model.error}
	<ErrorNotice message={model.error} />
{:else if !model.loading && model.weapons.length === 0}
		<div class="py-10 text-center space-y-2">
			<p class="text-sm text-text-secondary">No weapons in your equipment library yet.</p>
			<p class="text-sm text-text-tertiary">
				Add the weapons you hunt with on the Equipment page to see their break-even markup.
			</p>
		</div>
{:else}
	<section>
		<div class="mb-5 max-w-3xl">
			<div class="flex items-center gap-1.5">
				<h2 class="text-base font-semibold text-text">Expected Return</h2>
				{@render expectedReturnTip()}
			</div>
			<p class="mt-1 text-sm leading-relaxed text-text-secondary">
				Long-run loadout economics under Community Model v1. Weapon and amplifier efficiencies
				are weighted by their own TT streams; limited-item premium raises cost without raising loot.
			</p>
			{#if model.looters.length > 0}
				<div class="mt-2 text-xs text-text-tertiary">
					Read at <LooterBasisSelect levels={model.looterLevels} />
				</div>
			{/if}
		</div>
		<DataTable
			{columns}
			rows={model.weapons}
			emptyMessage={model.loading ? 'Loading break-even data' : 'No weapons'}
		>
			{#snippet cell({ row, column })}
				{#if column.key === 'name'}
					<div class="min-w-0">
						<p class="truncate text-text">{row.name}</p>
						{#if row.amplifierName}<p class="truncate text-xs text-text-tertiary">+ {row.amplifierName}</p>{/if}
					</div>
				{:else if column.key === 'weightedEfficiencyPct'}
					<span class="tabular-nums text-text-secondary">{row.weightedEfficiencyPct !== null ? `${row.weightedEfficiencyPct.toFixed(1)}%` : NO_DATA}</span>
				{:else if column.key === 'expectedTtReturnPct'}
					<span class="tabular-nums text-text">{row.expectedTtReturnPct !== null ? `${row.expectedTtReturnPct.toFixed(1)}%` : NO_DATA}</span>
				{:else if column.key === 'breakEvenLootMarkupPct'}
					<span class="tabular-nums text-text">{row.breakEvenLootMarkupPct !== null ? `${row.breakEvenLootMarkupPct.toFixed(1)}%` : NO_DATA}</span>
				{/if}
			{/snippet}
		</DataTable>
	</section>
{/if}
