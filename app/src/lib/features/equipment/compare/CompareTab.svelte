<script lang="ts">
	import { ErrorNotice } from '$lib/components';
	import BreakpointSlider from './BreakpointSlider.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import ComparisonAssumptionsTip from './ComparisonAssumptionsTip.svelte';
	import ComparisonTable from './ComparisonTable.svelte';
	import LossChart from './LossChart.svelte';
	import { type Comparator, sessionComparator } from './compareModel.svelte';
	import { buildChart, type ChartModel, formatCompact, planFigures, setupLabels } from './comparisonView';

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
	// What the kill count costs in PED with the reference setup, for anyone
	// who thinks of a year's hunting as spend.
	const spendHint = $derived.by(() => {
		const id = comparator.referenceDraftId ?? comparator.computedIds[0] ?? null;
		const setup = id === null ? null : comparator.figuresFor(id);
		if (!setup || !comparison) return null;
		const figures = planFigures(setup, comparator.killCount, comparator.pinnedHp);
		if (!figures) return null;
		return { ped: figures.cycledPed, label: setupLabels(comparison)[setup.index] ?? setup.name };
	});

	const ready = $derived(comparator.requestKey !== null);
	const chart = $derived.by(() => {
		if (!ready || !comparison) return null;
		const slots = comparator.computedIds.map((id) => comparator.draft(id)?.slot ?? null);
		return buildChart(comparison, slots);
	});
	// The chart's frame stands from the first paint, so adding a setup fills
	// it in place rather than growing the page.
	const EMPTY_CHART = $derived<ChartModel>({
		series: [],
		markers: [],
		hpMax: comparator.hpMax,
		lossMax: 0,
	});
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
			pinnedHp={comparator.pinnedHp}
			onPin={(hp) => (comparator.pinnedHp = hp)}
		/>
	</section>

	<section aria-label="Setups">
		<!-- What the table's answers are for: a year's kills, the pinned mob HP, and the looter level. -->
		<div class="mb-4 grid grid-cols-1 gap-x-10 gap-y-3 sm:grid-cols-3">
			<BreakpointSlider
				label="Mobs killed"
				unit="mobs"
				value={comparator.killCount}
				max={comparator.killMax}
				breakpoint={10_000}
				onValue={(value) => (comparator.killCount = value)}
				onMax={(max) => (comparator.killMax = max)}
			>
				{#snippet note()}
					{#if spendHint}≈ {formatCompact(spendHint.ped)} PED cycled with {spendHint.label}{/if}
				{/snippet}
			</BreakpointSlider>
			<BreakpointSlider
				label="Mob HP"
				unit="HP"
				value={comparator.pinnedHp}
				max={comparator.hpMax}
				breakpoint={500}
				significant={3}
				onValue={(value) => (comparator.pinnedHp = value)}
				onMax={(max) => (comparator.hpMax = max)}
			/>
			{#if comparator.ownLooterLevel !== null}
				{@const own = comparator.ownLooterLevel}
				<BreakpointSlider
					label="Looter level"
					unit=""
					value={comparator.looterLevel ?? Math.min(100, own)}
					max={100}
					step={0.1}
					decimals={1}
					fixedMax
					marker={{ value: Math.min(100, own), label: 'yours' }}
					onValue={(value) =>
						(comparator.looterLevel =
							Math.abs(value - Math.min(100, own)) < 0.05 ? null : value)}
				>
					{#snippet info()}
						<InfoTip label="About the looter level" width="w-80">
							<p class="text-xs leading-relaxed text-text-secondary">
								The three-looter mean the return model runs at; it starts at yours, marked on the
								track. Levels go past 100 in game, but the community model assumes looter skill
								stops counting at 100, so the slider stops there too.
							</p>
							<p class="mt-1.5 text-xs leading-relaxed text-text-secondary">
								It changes what each extra PED cycled costs, so it matters most between setups that
								cycle different amounts per kill; two setups that cycle the same barely move apart.
							</p>
						</InfoTip>
					{/snippet}
				</BreakpointSlider>
			{/if}
		</div>
		<ComparisonTable {comparator} />
	</section>
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
