<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import MarkupConfidenceControl from '$lib/features/analytics/MarkupConfidenceControl.svelte';
	import TreeCuttingPrimaryView from '$lib/features/analytics/TreeCuttingPrimaryView.svelte';
	import { ANALYTICS_RANGES } from '$lib/features/analytics/analyticsRange';
	import { createTreeCuttingModel } from '$lib/features/analytics/treeCuttingModel.svelte';

	const model = createTreeCuttingModel();

	$effect(() => {
		void model.loadData(model.period);
	});
</script>

{#if model.loading}
	<p class="text-sm text-text-secondary">Loading tree cutting data...</p>
{:else if model.error && !model.overall}
	<ErrorNotice message={model.error} />
{:else if model.overall}
	<div class="space-y-5" data-testid="analytics-treecutting-area">
		<ErrorNotice message={model.error} />

		<div class="flex flex-wrap items-center justify-between gap-3 pb-2">
			<SegmentedControl
				options={ANALYTICS_RANGES.map((range) => ({ id: range, label: range }))}
				active={model.activeRange}
				onchange={(id) => (model.activeRange = id)}
			/>

			<MarkupConfidenceControl
				active={model.confidenceMode}
				onchange={(id) => (model.confidenceMode = id)}
			/>
		</div>

		<TreeCuttingPrimaryView
			overall={model.overall}
			table={model.activityTable}
			selected={model.selectedSection}
			totalCount={model.sections.length}
			onselect={(tier) => model.selectSection(tier)}
		/>
	</div>
{:else}
	<p
		class="py-10 text-center text-sm text-text-tertiary"
		data-testid="analytics-treecutting-area"
	>
		No tree cutting data yet. Harvest trees during a tracked session to compare board outputs.
	</p>
{/if}
