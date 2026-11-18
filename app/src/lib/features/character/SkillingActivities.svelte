<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import { targetLabel } from './codexRankingTarget';
	import RecommenderChart from './RecommenderChart.svelte';
	import SkillingBar from './SkillingBar.svelte';
	import SkillingPanel from './SkillingPanel.svelte';
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
	const shown = $derived(
		activities.candidates.filter((candidate) => !activities.isHidden(candidate.activity)),
	);
	// The quickest activity other than the target's own, and whether the
	// target's own beats it.
	const top = $derived(shown.find((candidate) => !activities.isDirect(candidate.activity)) ?? null);
	const directLeads = $derived(shown[0] !== undefined && activities.isDirect(shown[0].activity));
</script>

<SkillingPanel id="skilling-activities" label="Activities" busy={activities.loading}>
	{#snippet lead()}
		{#if top && !activities.loading}
			{#if directLeads && activities.direct}
				Other than skilling {activities.direct.activity} directly,
			{/if}
			<span class="font-medium text-text">{top.activity}</span> trains {targetLabel(hub.target)} quickest{#if top.pesToPlusOne !== null}:
				{formatPes(top.pesToPlusOne)} PES of skilling {isHp ? 'an HP' : 'a level'}{/if}.
		{/if}
	{/snippet}

	{#snippet info()}
		<p class="text-xs font-semibold leading-relaxed text-text">A model, per PES of skilling</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			Every activity ranked by the skilling PES it takes to add one {isHp ? 'HP' : 'level'}, from the
			profession weights and your current skill levels. How much PES an hour of each activity yields
			is not modelled here: Routes prices that per weapon and mob.
		</p>
		{#if activities.direct}
			<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
				{activities.direct.activity} itself is ranked among the others, marked direct, so you can see
				how close each comes; the chart draws it as a faded line beside the one you pick.
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
					{@const hidden = activities.isHidden(candidate.activity)}
					{@const isDirect = activities.isDirect(candidate.activity)}
					<li
						class="group relative flex items-center rounded-md transition-colors duration-[var(--duration-fast)]
							{active ? 'bg-accent/[0.07]' : 'hover:bg-surface-hover/60'}
							{hidden && i === activities.visibleCount && i > 0 ? 'mt-2 border-t border-border/40 pt-2' : ''}"
					>
						{#if active}
							<span class="absolute inset-y-1.5 left-0 w-0.5 rounded-full bg-accent" aria-hidden="true"></span>
						{/if}
						<button
							type="button"
							class="flex min-w-0 flex-1 cursor-pointer items-baseline gap-3 py-2 pl-3 pr-2 text-left text-sm
								{hidden ? 'text-text-tertiary' : active ? 'text-text' : 'text-text-secondary group-hover:text-text'}"
							aria-pressed={active}
							onclick={() => activities.select(candidate.activity)}
						>
							<span
								class="w-5 shrink-0 text-right text-xs tabular-nums
									{hidden ? 'text-transparent' : i === 0 ? 'text-success' : i < 3 ? 'text-accent' : 'text-text-tertiary'}"
								aria-hidden={hidden}
							>
								{hidden ? '' : i + 1}
							</span>
							<span class="min-w-0 flex-1 truncate">
								{candidate.activity}
								{#if isDirect}
									<span class="ml-1.5 text-[0.625rem] font-medium uppercase tracking-wide text-text-tertiary">
										Direct
									</span>
								{/if}
							</span>
							<span
								class="shrink-0 whitespace-nowrap text-xs tabular-nums
									{active && !hidden ? 'text-accent' : 'text-text-tertiary'}
									{hidden ? 'opacity-60' : isDirect ? '' : 'group-hover:hidden group-focus-within:hidden'}"
							>
								{#if candidate.pesToPlusOne !== null}
									{formatPes(candidate.pesToPlusOne)} PES
								{:else}
									+{candidate.gainAtCap.toFixed(2)} at cap
								{/if}
							</span>
						</button>
						{#if hidden}
							<button
								type="button"
								class="linklet shrink-0 px-2"
								aria-label="Restore {candidate.activity} to the ranking"
								onclick={() => activities.restore(candidate.activity)}
							>
								Restore
							</button>
						{:else if !isDirect}
							<button
								type="button"
								class="linklet hidden shrink-0 px-2 group-hover:block group-focus-within:block"
								aria-label="Hide {candidate.activity} from every ranking"
								title="Hide it from every ranking"
								onclick={() => activities.hide(candidate.activity)}
							>
								Hide
							</button>
						{/if}
					</li>
				{/each}
			</ol>

			<div class="min-w-0 space-y-5">
				{#if selected}
					<RecommenderChart
						{selected}
						direct={activities.isDirect(selected.activity) ? null : activities.direct}
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
</SkillingPanel>
