<script lang="ts">
	import { Card, DataTable, ErrorNotice, SegmentedControl } from '$lib/components';
	import { formatPed } from '$lib/utils/format';
	import { createTableModel } from '$lib/view/tableModel.svelte';
	import {
		computeQuestAnalytics,
		type QuestAnalyticsComputed,
		type RewardMode
	} from './economics';
	import type { QuestsModel } from './questsModel.svelte';

	let { model }: { model: QuestsModel } = $props();

	const computedAnalytics = $derived(
		computeQuestAnalytics(
			model.analyticsData,
			model.analyticsRewardMode,
			model.rewardMarket
		)
	);

	// The default table-model comparator sorts nulls last in both directions;
	// this column historically fell back to String() comparison when either
	// side is null, so that ordering is pinned here.
	function markupCompare(a: QuestAnalyticsComputed, b: QuestAnalyticsComputed): number {
		const aVal = a.rewardMarkupPercent;
		const bVal = b.rewardMarkupPercent;
		if (typeof aVal === 'number' && typeof bVal === 'number') return aVal - bVal;
		return String(aVal).localeCompare(String(bVal));
	}

	// Sort-only adoption: the analytics table renders all rows, so the page
	// size just has to keep everything on page one.
	const table = createTableModel<QuestAnalyticsComputed>({
		rows: () => computedAnalytics,
		pageSize: Number.MAX_SAFE_INTEGER,
		comparators: { rewardMarkupPercent: markupCompare }
	});

	type ColumnDef<T> = {
		key: keyof T & string;
		label: string;
		align?: 'left' | 'right' | 'center';
		sortable?: boolean;
	};

	const analyticsColumns = $derived.by((): ColumnDef<QuestAnalyticsComputed>[] => {
		const columns: ColumnDef<QuestAnalyticsComputed>[] = [
			{ key: 'questName', label: 'Quest', sortable: true },
			{ key: 'recordedCompletions', label: 'Runs', align: 'right', sortable: true },
			{ key: 'totalRecordedRewardTt', label: 'Reward TT', align: 'right', sortable: true }
		];
		if (model.analyticsRewardMode === 'markup') {
			columns.splice(3, 0, {
				key: 'totalRecordedRewardMu',
				label: 'Projected Value',
				align: 'right',
				sortable: true
			});
			columns.splice(4, 0, {
				key: 'totalRealisedRewardMarkup',
				label: 'Realised MU',
				align: 'right',
				sortable: true
			});
		}
		columns.splice(model.analyticsRewardMode === 'markup' ? 5 : 3, 0, {
			key: 'unresolvedCompletions',
			label: 'Unresolved',
			align: 'right',
			sortable: true
		});
		columns.push({
			key: 'displayLiquidReward',
			label: 'Per Run',
			align: 'right',
			sortable: true
		});
		return columns;
	});

</script>

{#if model.analyticsLoading}
	<div class="text-sm text-text-tertiary py-8 text-center">Loading quest analytics...</div>
{:else if model.analyticsError}
	<ErrorNotice message={model.analyticsError} />
{:else if computedAnalytics.length === 0}
	<Card class="p-6">
		<p class="text-sm text-text-tertiary text-center">
			No quest runs recorded yet.
		</p>
	</Card>
{:else}
	<div class="space-y-3">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<div>
				<h3 class="text-sm font-medium text-text-secondary">Quest Rewards</h3>
				<p class="mt-0.5 text-xs text-text-tertiary">
					Cost and net per quest will return once they are costed from the session segments that ran each quest.
				</p>
			</div>
			<SegmentedControl
				options={[
					{ id: 'tt', label: 'Reward TT' },
					{ id: 'markup', label: 'Reward TT + MU' }
				]}
				active={model.analyticsRewardMode}
				onchange={(id) => (model.analyticsRewardMode = id as RewardMode)}
			/>
		</div>
		{#snippet analyticsCell({ column, value, row }: { column: { key: string }; value: unknown; row: QuestAnalyticsComputed })}
			{#if column.key === 'questName'}
				<span class="font-medium">{value}</span>
			{:else if column.key === 'totalRecordedRewardTt'}
				<div class="flex flex-col items-end leading-tight">
					<span class="tabular-nums">{formatPed(Number(value))}</span>
					{#if row.totalRecordedRewardPes > 0}
						<span class="text-[11px] text-accent">+{formatPed(row.totalRecordedRewardPes)} PES</span>
					{/if}
				</div>
			{:else if column.key === 'totalRecordedRewardMu'}
				<span class="tabular-nums text-accent">{formatPed(Number(value))}</span>
			{:else if column.key === 'totalRealisedRewardMarkup'}
				<span class="tabular-nums {Number(value) >= 0 ? 'text-positive' : 'text-negative'}">
					{Number(value) > 0 ? '+' : ''}{formatPed(Number(value))}
				</span>
			{:else if column.key === 'unresolvedCompletions'}
				<span class="tabular-nums {Number(value) > 0 ? 'text-warning' : 'text-text-tertiary'}">{value}</span>
			{:else if column.key === 'displayLiquidReward'}
				<div class="flex flex-col items-end leading-tight">
					<span class="tabular-nums">{formatPed(Number(value))}</span>
					{#if row.avgRewardPes > 0}
						<span class="text-[11px] text-accent">+{formatPed(row.avgRewardPes)} PES</span>
					{/if}
				</div>
			{:else if column.key === 'rewardMarkupPercent'}
				<span class="tabular-nums text-text-secondary">
					{value == null ? '\u2014' : `${Number(value).toFixed(0)}%`}
				</span>
			{:else}
				{value}
			{/if}
		{/snippet}
		<DataTable
			columns={analyticsColumns}
			rows={table.filtered}
			bind:sortKey={() => table.sortKey, (key) => {
				if (key !== undefined && key !== table.sortKey) table.setSort(key);
			}}
			bind:sortDir={() => table.sortDir, (dir) => {
				if (table.sortKey !== undefined && dir !== table.sortDir) table.setSort(table.sortKey);
			}}
			cell={analyticsCell}
			emptyMessage="No quest runs recorded"
		/>
	</div>
{/if}
