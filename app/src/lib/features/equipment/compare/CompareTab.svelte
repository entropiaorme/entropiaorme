<script lang="ts">
	import { ErrorNotice } from '$lib/components';
	import ComparisonAssumptionsTip from './ComparisonAssumptionsTip.svelte';
	import ComparisonTable from './ComparisonTable.svelte';
	import LossChart from './LossChart.svelte';
	import { type Comparator, sessionComparator } from './compareModel.svelte';
	import { buildChart, type ChartModel } from './comparisonView';

	/** The comparator to drive; the app session's own unless a test supplies one. */
	let { comparator = sessionComparator() }: { comparator?: Comparator } = $props();

	// Recompare shortly after the setups settle, so typing a markup does not
	// send a request per keystroke.
	const COMPARE_DELAY_MS = 250;
	$effect(() => {
		const key = comparator.requestKey;
		if (key === null) return;
		const timer = setTimeout(() => void comparator.compare(), COMPARE_DELAY_MS);
		return () => clearTimeout(timer);
	});

	const comparison = $derived(comparator.comparison);
	const ready = $derived(comparator.requestKey !== null);
	const chart = $derived.by(() => {
		if (!ready || !comparison) return null;
		const slots = comparator.computedIds.map((id) => comparator.draft(id)?.slot ?? null);
		return buildChart(comparison, slots);
	});
	// The chart's frame stands from the first paint, so adding a setup fills
	// it in place rather than growing the page.
	const EMPTY_CHART: ChartModel = { series: [], markers: [], hpMax: 1000, lossMax: 0 };
</script>

<!-- Chart series colours: the categorical palette, stepped per theme and
     validated on each surface; a setup keeps its colour while listed. -->
<div class="compare-root space-y-6">
	<ErrorNotice message={comparator.error} onDismiss={() => (comparator.error = null)} />

	<section
		class="transition-opacity {ready && !comparator.current ? 'opacity-60' : ''}"
		aria-labelledby="compare-chart-heading"
		aria-busy={ready && !comparator.current}
	>
		<div class="mb-3 flex h-5 items-center gap-2">
			<h3 id="compare-chart-heading" class="eyebrow">Loss per kill by mob HP</h3>
			{#if comparison}
				<ComparisonAssumptionsTip assumptions={comparison.assumptions} />
			{/if}
		</div>
		<LossChart
			chart={chart ?? EMPTY_CHART}
			emptyMessage={ready ? undefined : 'Add a weapon to plot its loss per kill'}
		/>
	</section>

	<ComparisonTable {comparator} />
</div>

<style>
	.compare-root {
		--compare-series-1: #3987e5;
		--compare-series-2: #d95926;
		--compare-series-3: #199e70;
		--compare-series-4: #c98500;
		--compare-series-5: #d55181;
		--compare-series-6: #008300;
		--compare-series-7: #9085e9;
		--compare-series-8: #e66767;
	}
	:global(:root[data-theme='light']) .compare-root {
		--compare-series-1: #2a78d6;
		--compare-series-2: #eb6834;
		--compare-series-3: #1baf7a;
		--compare-series-4: #eda100;
		--compare-series-5: #e87ba4;
		--compare-series-6: #008300;
		--compare-series-7: #4a3aa7;
		--compare-series-8: #e34948;
	}
</style>
