<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import HuntingPrimaryView from '$lib/features/analytics/HuntingPrimaryView.svelte';
	import MarkupConfidenceControl from '$lib/features/analytics/MarkupConfidenceControl.svelte';
	import { ANALYTICS_RANGES } from '$lib/features/analytics/analyticsRange';
	import { createHuntingModel } from '$lib/features/analytics/huntingModel.svelte';

	const model = createHuntingModel();

	$effect(() => {
		void model.loadData(model.period);
	});
</script>

{#if model.loading}
	<p class="text-sm text-text-secondary">Loading hunting data...</p>
{:else if model.error && !model.overall}
	<ErrorNotice message={model.error} />
{:else if model.overall}
	<div class="space-y-5" data-testid="analytics-hunting-area">
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

		<HuntingPrimaryView
			overall={model.overall}
			table={model.sessionTable}
			selected={model.selectedSession}
			totalCount={model.sessionSections.length}
			onselect={(key) => model.selectSession(key)}
		/>
	</div>
{:else}
	<p
		class="py-10 text-center text-sm text-text-tertiary"
		data-testid="analytics-hunting-area"
	>
		No hunting data yet. Track a hunting session to compare your routines and activities.
	</p>
{/if}
