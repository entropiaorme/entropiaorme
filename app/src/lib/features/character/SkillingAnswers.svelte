<script lang="ts">
	import type { Snippet } from 'svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import { formatPed } from '$lib/utils/format';
	import { targetLabel } from './codexRankingTarget';
	import { formatGoal, formatHours, formatPes } from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';

	// The hub's three answers at a glance, one per facet; each jumps to the
	// section that explains it.
	let { hub }: { hub: SkillingModel } = $props();

	const isHp = $derived(hub.target.kind === 'hp');
	const isFamily = $derived(hub.target.kind === 'family');
	const unit = $derived(isHp ? 'HP' : 'level');
	const topActivity = $derived(hub.activities.candidates[0] ?? null);
	const source = $derived(hub.selectedSource);
	const goalText = $derived(hub.goal !== null ? formatGoal(hub.target, hub.goal) : '');
	const pathSkills = $derived(hub.path?.allocations.filter((alloc) => alloc.levelsToGain > 0).length ?? 0);
	// The cheapest skill you can actually train: a locked skill ranks cheap
	// (level zero) but takes no PES until it is unlocked.
	const cheapestHp = $derived(
		hub.hpPath?.skills.find((skill) => skill.currentLevel > 0) ?? hub.hpPath?.skills[0] ?? null,
	);

	function jump(id: string) {
		document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
	}
</script>

{#snippet answer(label: string, section: string, body: Snippet)}
	<button
		type="button"
		class="group flex min-w-0 flex-col gap-1.5 py-4 text-left sm:px-6 sm:first:pl-0
			focus:outline-none focus-visible:[box-shadow:var(--shadow-glow)] rounded-sm cursor-pointer"
		onclick={() => jump(section)}
	>
		<span class="eyebrow transition-colors group-hover:text-text-secondary">{label}</span>
		{@render body()}
	</button>
{/snippet}

{#snippet muted(text: string)}
	<span class="text-sm text-text-tertiary">{text}</span>
{/snippet}

{#snippet loading()}
	<Skeleton class="h-6 w-32" />
	<Skeleton class="h-3 w-40" />
{/snippet}

{#snippet activityBody()}
	{#if hub.activities.loading}
		{@render loading()}
	{:else if topActivity}
		<span class="truncate text-xl font-semibold leading-tight tracking-tight text-text group-hover:text-accent transition-colors">
			{topActivity.activity}
		</span>
		<span class="text-xs tabular-nums text-text-tertiary">
			{#if topActivity.pesToPlusOne !== null}
				+1 {unit} per {formatPes(topActivity.pesToPlusOne)} PES of skilling
			{:else}
				+{topActivity.gainAtCap.toFixed(2)} {isHp ? 'HP' : 'levels'} per 1,000 PES
			{/if}
		</span>
	{:else if hub.activities.result}
		{@render muted('No activity trains it from your current skills')}
	{:else}
		{@render muted('Unavailable')}
	{/if}
{/snippet}

{#snippet sessionBody()}
	{#if isFamily}
		{@render muted('Pick one profession to forecast')}
	{:else if !hub.goalActive}
		{@render muted('Set a goal above where you stand')}
	{:else if hub.forecastLoading}
		{@render loading()}
	{:else if source?.status === 'ready'}
		<span class="text-xl font-semibold leading-tight tabular-nums tracking-tight text-text">
			{formatPed(source.netCost ?? source.ttCost)}
			<span class="text-xs font-medium uppercase tracking-wider text-text-tertiary">
				PED {source.netCost !== null ? 'after markup' : 'TT cost'}
			</span>
		</span>
		<span class="truncate text-xs text-text-tertiary">
			{formatHours(source.hours)} of {source.name} to {goalText}
		</span>
	{:else if hub.forecast && hub.sources.length === 0}
		{@render muted('No named sessions recorded yet')}
	{:else if hub.forecast}
		{@render muted(`None of your sessions trains ${targetLabel(hub.target)}`)}
	{:else}
		{@render muted('Unavailable')}
	{/if}
{/snippet}

{#snippet pathBody()}
	{#if isFamily}
		{@render muted('Pick one profession for a skill path')}
	{:else if hub.pathLoading}
		{@render loading()}
	{:else if isHp && cheapestHp}
		<span class="text-xl font-semibold leading-tight tabular-nums tracking-tight text-text">
			{formatPed(cheapestHp.pedPerHp)}
			<span class="text-xs font-medium uppercase tracking-wider text-text-tertiary">PES per HP</span>
		</span>
		<span class="truncate text-xs text-text-tertiary">cheapest through {cheapestHp.name}</span>
	{:else if isHp}
		{@render muted('Scan your skills to rank them')}
	{:else if !hub.goalActive}
		{@render muted('Set a goal above where you stand')}
	{:else if hub.path && hub.path.professionLevelsGained > 0}
		<span class="text-xl font-semibold leading-tight tabular-nums tracking-tight text-text">
			{formatPed(hub.path.totalPed)}
			<span class="text-xs font-medium uppercase tracking-wider text-text-tertiary">PES</span>
		</span>
		<span class="truncate text-xs text-text-tertiary">
			across {pathSkills} {pathSkills === 1 ? 'skill' : 'skills'} to {goalText}
		</span>
	{:else}
		{@render muted('Unavailable')}
	{/if}
{/snippet}

<div
	class="grid grid-cols-1 divide-y divide-border/50 border-t border-border/50 sm:grid-cols-3 sm:divide-x sm:divide-y-0"
	aria-label="Answers at a glance"
	role="group"
>
	{@render answer('Best place to train', 'skilling-activities', activityBody)}
	{@render answer('From your sessions', 'skilling-forecast', sessionBody)}
	{@render answer(isHp ? 'Cheapest HP skill' : 'Cheapest skill path', 'skilling-path', pathBody)}
</div>

