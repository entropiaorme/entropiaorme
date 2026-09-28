<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import StatDisplay from '$lib/components/StatDisplay.svelte';
	import { formatPed } from '$lib/utils/format';
	import { targetLabel } from './codexRankingTarget';
	import SkillingBar from './SkillingBar.svelte';
	import SkillingSection from './SkillingSection.svelte';
	import { formatGoal, formatLevel, groupByReason } from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';

	// The cheapest skill path: where skill PES (codex rewards, chips) moves
	// the target furthest. A profession gets the least-PES allocation to the
	// goal; HP gets every contributing skill ranked by PES per HP.
	let { hub }: { hub: SkillingModel } = $props();

	const isHp = $derived(hub.target.kind === 'hp');
	const isFamily = $derived(hub.target.kind === 'family');
	const name = $derived(targetLabel(hub.target));
	const path = $derived(hub.path);
	const allocated = $derived(path?.allocations.filter((alloc) => alloc.levelsToGain > 0) ?? []);
	const unallocated = $derived(path?.allocations.filter((alloc) => alloc.levelsToGain === 0) ?? []);
	const topCost = $derived(Math.max(0, ...allocated.map((alloc) => alloc.pedCost)));
	const hpSkills = $derived(hub.hpPath?.skills ?? []);
	const cheapestHp = $derived(hpSkills[0]?.pedPerHp ?? 0);
	const SHOWN_HP_SKILLS = 12;
	let showAllHp = $state(false);

	const rankTone = (i: number) => (i === 0 ? 'text-success' : i < 3 ? 'text-accent' : 'text-text');
</script>

{#snippet attributeList(items: { name: string; figure: string }[], note: string)}
	<p class="mt-4 text-xs leading-relaxed text-text-tertiary">
		<span class="text-text-secondary">If an attribute is offered as a reward:</span>
		{#each items as item, i (item.name)}
			{i > 0 ? ', ' : ' '}{item.name} <span class="tabular-nums text-text-secondary">{item.figure}</span>
		{/each}. {note}
	</p>
{/snippet}

<SkillingSection
	id="skilling-path"
	title={isHp ? 'Cheapest skills for HP' : 'Cheapest skill path'}
	description={isHp
		? 'Every skill that adds HP, ranked by the PES it takes to add one HP from your current level: where codex rewards and chips go furthest.'
		: `The least skill PES that reaches the goal, spread across ${isFamily ? 'the profession' : name}'s skills where each moves it most: where codex rewards and chips go furthest.`}
	busy={hub.pathLoading}
>
	{#if isFamily}
		<p class="py-4 text-sm text-text-tertiary">
			A skill path follows one profession at a time. Pick one of the family's professions above.
		</p>
	{:else if hub.pathError}
		<ErrorNotice message={hub.pathError} />
	{:else if hub.pathLoading}
		<div class="space-y-3">
			<Skeleton class="h-6 w-48" />
			{#each { length: 5 } as _}
				<Skeleton class="h-5 w-full" />
			{/each}
		</div>
	{:else if isHp}
		{#if hpSkills.length === 0}
			<p class="py-4 text-sm text-text-tertiary">No skill data yet. Scan your skills to rank them.</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-border/60">
							<th class="w-8 py-2 pr-2 text-left eyebrow">#</th>
							<th class="py-2 pr-3 text-left eyebrow">Skill</th>
							<th class="px-3 py-2 text-right eyebrow">Level</th>
							<th class="px-3 py-2 text-right eyebrow">Levels per HP</th>
							<th class="w-40 px-3 py-2 text-left eyebrow">Relative cost</th>
							<th class="py-2 pl-3 text-right eyebrow">PES per HP</th>
						</tr>
					</thead>
					<tbody>
						{#each showAllHp ? hpSkills : hpSkills.slice(0, SHOWN_HP_SKILLS) as skill, i (skill.name)}
							<tr class="border-b border-border/30 transition-colors hover:bg-surface-hover/40">
								<td class="py-2.5 pr-2 text-xs tabular-nums text-text-tertiary">{i + 1}</td>
								<td class="py-2.5 pr-3 {skill.currentLevel > 0 ? 'text-text' : 'text-text-tertiary'}">{skill.name}</td>
								<td class="px-3 py-2.5 text-right tabular-nums text-text-secondary">
									{#if skill.currentLevel > 0}
										{formatLevel(skill.currentLevel)}
									{:else}
										<span class="text-xs text-text-tertiary">not unlocked</span>
									{/if}
								</td>
								<td class="px-3 py-2.5 text-right tabular-nums text-text-secondary">
									{skill.levelsPerHp.toLocaleString('en-GB')}
								</td>
								<td class="px-3 py-2.5">
									<SkillingBar
										fraction={skill.pedPerHp > 0 ? cheapestHp / skill.pedPerHp : 0}
										tone={i < 3 ? 'accent' : 'muted'}
									/>
								</td>
								<td class="py-2.5 pl-3 text-right font-medium tabular-nums {rankTone(i)}">
									{formatPed(skill.pedPerHp)}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
			{#if hpSkills.length > SHOWN_HP_SKILLS}
				<button type="button" class="linklet mt-3" onclick={() => (showAllHp = !showAllHp)}>
					{showAllHp ? 'Show the cheapest only' : `Show all ${hpSkills.length} skills`}
				</button>
			{/if}
			{#if hub.hpPath && hub.hpPath.attributes.length > 0}
				{@render attributeList(
					hub.hpPath.attributes.map((attr) => ({ name: attr.name, figure: `${attr.levelsPerHp} lvl/HP` })),
					'Fewer levels per HP is better.',
				)}
			{/if}
		{/if}
	{:else if !hub.goalActive}
		<p class="py-4 text-sm text-text-tertiary">Set a goal above your current level to plan a path.</p>
	{:else if path}
		<div class="grid grid-cols-2 gap-x-8 gap-y-6 lg:grid-cols-4">
			<StatDisplay label="Skill PES" value={formatPed(path.totalPed)} unit="PES" comparison="to {formatGoal(hub.target, hub.goal ?? 0)}" />
			<StatDisplay
				label="Levels gained"
				value={`+${path.professionLevelsGained.toFixed(2)}`}
				comparison="{path.currentLevel.toFixed(2)} to {path.endLevel.toFixed(2)}"
			/>
			<StatDisplay label="Skills involved" value={allocated.length} comparison="ranked by PES below" />
		</div>

		{#if allocated.length > 0}
			<div class="mt-7 overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-border/60">
							<th class="w-8 py-2 pr-2 text-left eyebrow">#</th>
							<th class="py-2 pr-3 text-left eyebrow">Skill</th>
							<th class="px-3 py-2 text-right eyebrow">Weight</th>
							<th class="px-3 py-2 text-right eyebrow">Level</th>
							<th class="w-40 px-3 py-2 text-left eyebrow">Share of the PES</th>
							<th class="py-2 pl-3 text-right eyebrow">PES</th>
						</tr>
					</thead>
					<tbody>
						{#each allocated as alloc, i (alloc.name)}
							<tr class="border-b border-border/30 transition-colors hover:bg-surface-hover/40">
								<td class="py-2.5 pr-2 text-xs tabular-nums text-text-tertiary">{i + 1}</td>
								<td class="py-2.5 pr-3 text-text">{alloc.name}</td>
								<td class="px-3 py-2.5 text-right tabular-nums text-text-secondary">{alloc.weight}</td>
								<td class="whitespace-nowrap px-3 py-2.5 text-right tabular-nums text-text-secondary">
									{formatLevel(alloc.currentLevel)}
									<span class="mx-1 text-text-tertiary">&rarr;</span>
									<span class="text-text">{formatLevel(alloc.newLevel)}</span>
								</td>
								<td class="px-3 py-2.5">
									<div class="flex items-center gap-2.5">
										<SkillingBar fraction={topCost > 0 ? alloc.pedCost / topCost : 0} class="flex-1" />
										<span class="w-11 text-right text-xs tabular-nums text-text-tertiary">
											{path.totalPed > 0 ? ((alloc.pedCost / path.totalPed) * 100).toFixed(1) : '0.0'}%
										</span>
									</div>
								</td>
								<td class="py-2.5 pl-3 text-right font-medium tabular-nums {rankTone(i)}">{formatPed(alloc.pedCost)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		{#if unallocated.length > 0 || path.excluded.length > 0}
			<p class="mt-4 text-xs leading-relaxed text-text-tertiary">
				<span class="text-text-secondary">Left out,</span>
				{groupByReason([
					...unallocated.map((alloc) => ({ name: alloc.name, reason: 'not needed for this goal' })),
					...path.excluded.map((skill) => ({ name: skill.name, reason: skill.reason })),
				])}.
			</p>
		{/if}

		{#if path.attributes.length > 0}
			{@render attributeList(
				path.attributes.map((attr) => ({ name: attr.name, figure: `×${attr.contributionFactor}` })),
				'Higher contribution is better.',
			)}
		{/if}
	{/if}
</SkillingSection>
