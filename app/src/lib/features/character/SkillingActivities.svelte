<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import { targetLabel } from './codexRankingTarget';
	import RecommenderChart from './RecommenderChart.svelte';
	import SkillingBar from './SkillingBar.svelte';
	import SkillingSection from './SkillingSection.svelte';
	import { formatPes } from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';

	// The Activity Recommender: every activity ranked by the skilling PES it
	// takes to add one to the target, with the chosen activity's projection
	// and what drives it.
	let { hub }: { hub: SkillingModel } = $props();
	const activities = $derived(hub.activities);
	const isHp = $derived(hub.target.kind === 'hp');
	const unitLabel = $derived(isHp ? 'HP' : 'levels');
	const selected = $derived(activities.selected);
	const topContribution = $derived(
		Math.max(0, ...(selected?.contributors.map((c) => c.targetGain) ?? [0])),
	);
	const SHOWN_CONTRIBUTORS = 5;
</script>

<SkillingSection id="skilling-activities" title="Activity Recommender" busy={activities.loading}>
	{#snippet info()}
		<p class="text-xs font-semibold leading-relaxed text-text">A model, per PES of skilling</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			Every activity ranked by the skilling PES it takes to add one {isHp ? 'HP' : 'level'}, from the
			profession weights and your current skill levels. How much PES an hour of each activity yields
			is not modelled: the Prospect measures that from your own sessions.
		</p>
		{#if activities.result?.direct}
			<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
				The faded line is {activities.result.direct.activity} itself, for reference; some professions
				have no direct grind path.
			</p>
		{/if}
	{/snippet}

	{#if hub.activitiesError}
		<ErrorNotice message={hub.activitiesError} />
	{:else if activities.loading}
		<div class="grid grid-cols-1 gap-8 lg:grid-cols-[minmax(15rem,1fr)_2fr]">
			<div class="space-y-3">
				{#each { length: 6 } as _}
					<Skeleton class="h-5 w-full" />
				{/each}
			</div>
			<Skeleton class="h-60 w-full" />
		</div>
	{:else if activities.result && activities.candidates.length === 0}
		<p class="py-4 text-sm text-text-tertiary">No activity trains {targetLabel(hub.target)} yet.</p>
	{:else if activities.result}
		<div class="grid grid-cols-1 gap-8 lg:grid-cols-[minmax(15rem,1fr)_2fr]">
			<ol class="max-h-80 overflow-y-auto overscroll-contain pr-1" aria-label="Activities, quickest first">
				{#each activities.candidates as candidate, i (candidate.activity)}
					{@const active = selected?.activity === candidate.activity}
					<li>
						<button
							type="button"
							class="relative flex w-full cursor-pointer items-baseline gap-3 rounded-md py-2 pl-3 pr-2 text-left text-sm
								transition-colors duration-[var(--duration-fast)]
								{active ? 'bg-accent/[0.07] text-text' : 'text-text-secondary hover:bg-surface-hover/60 hover:text-text'}"
							aria-pressed={active}
							onclick={() => activities.select(candidate.activity)}
						>
							{#if active}
								<span class="absolute inset-y-1.5 left-0 w-0.5 rounded-full bg-accent" aria-hidden="true"></span>
							{/if}
							<span
								class="w-5 shrink-0 text-right text-xs tabular-nums
									{i === 0 ? 'text-success' : i < 3 ? 'text-accent' : 'text-text-tertiary'}"
							>
								{i + 1}
							</span>
							<span class="min-w-0 flex-1 truncate">{candidate.activity}</span>
							<span class="shrink-0 whitespace-nowrap text-xs tabular-nums {active ? 'text-accent' : 'text-text-tertiary'}">
								{#if candidate.pesToPlusOne !== null}
									{formatPes(candidate.pesToPlusOne)} PES
								{:else}
									+{candidate.gainAtCap.toFixed(2)} at cap
								{/if}
							</span>
						</button>
					</li>
				{/each}
			</ol>

			<div class="min-w-0 space-y-5">
				{#if selected}
					<RecommenderChart
						{selected}
						direct={activities.result.direct}
						pesCap={activities.result.pesCap}
						sampleStep={activities.result.sampleStep}
						{unitLabel}
					/>
					{#if selected.contributors.length > 0}
						<div>
							<p class="eyebrow mb-2.5">At 1,000 PES</p>
							<ul class="max-w-xl space-y-2">
								{#each selected.contributors.slice(0, SHOWN_CONTRIBUTORS) as contributor (contributor.name)}
									<li class="grid grid-cols-[minmax(0,10rem)_1fr_auto] items-center gap-3 text-xs">
										<span class="truncate text-text-secondary">{contributor.name}</span>
										<SkillingBar fraction={topContribution > 0 ? contributor.targetGain / topContribution : 0} />
										<span class="w-24 text-right tabular-nums text-text">
											+{contributor.targetGain.toFixed(isHp ? 2 : 3)}
											<span class="text-text-tertiary">{isHp ? 'HP' : 'lv'}</span>
										</span>
									</li>
								{/each}
							</ul>
							{#if selected.contributors.length > SHOWN_CONTRIBUTORS}
								{@const more = selected.contributors.length - SHOWN_CONTRIBUTORS}
								<p class="mt-2 text-xs text-text-tertiary">
									+{more} more
								</p>
							{/if}
						</div>
					{/if}
				{/if}
			</div>
		</div>
	{/if}
</SkillingSection>
