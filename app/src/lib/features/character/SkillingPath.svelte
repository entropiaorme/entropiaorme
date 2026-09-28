<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import { formatPed } from '$lib/utils/format';
	import { targetLabel } from './codexRankingTarget';
	import SkillingBar from './SkillingBar.svelte';
	import SkillingSection from './SkillingSection.svelte';
	import { formatGoal, formatLevel, groupByReason } from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';

	// The Optimiser: where skill PES (codex rewards, chips) moves the target
	// furthest. A profession gets the least-PES allocation to the goal; HP
	// gets every contributing skill ranked by PES per HP.
	let { hub }: { hub: SkillingModel } = $props();

	const isHp = $derived(hub.target.kind === 'hp');
	const isFamily = $derived(hub.target.kind === 'family');
	const name = $derived(targetLabel(hub.target));
	const path = $derived(hub.path);
	const allocated = $derived(path?.allocations.filter((alloc) => alloc.levelsToGain > 0) ?? []);
	const leftOut = $derived([
		...(path?.allocations
			.filter((alloc) => alloc.levelsToGain === 0)
			.map((alloc) => ({ name: alloc.name, reason: 'not needed for this goal' })) ?? []),
		...(path?.excluded.map((skill) => ({ name: skill.name, reason: skill.reason })) ?? []),
	]);
	const topCost = $derived(Math.max(0, ...allocated.map((alloc) => alloc.pedCost)));
	const hpSkills = $derived(hub.hpPath?.skills ?? []);
	const cheapestHp = $derived(hpSkills[0]?.pedPerHp ?? 0);
	const SHOWN_HP_SKILLS = 12;
	let showAllHp = $state(false);

	const rankTone = (i: number) => (i === 0 ? 'text-success' : i < 3 ? 'text-accent' : 'text-text');
</script>

{#snippet attributeTip(items: { name: string; figure: string }[], better: string)}
	<InfoTip label="Attribute rewards" width="w-72" align="left">
		{#snippet trigger()}
			<span class="linklet">Attribute rewards</span>
		{/snippet}
		<p class="text-xs font-semibold leading-relaxed text-text">If an attribute is offered</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			{items.map((item) => `${item.name} ${item.figure}`).join(', ')}. {better}
		</p>
	</InfoTip>
{/snippet}

<SkillingSection id="skilling-path" title="Optimiser" busy={hub.pathLoading}>
	{#snippet info()}
		<p class="text-xs font-semibold leading-relaxed text-text">Where skill PES goes furthest</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			{#if isHp}
				Every skill that adds HP, ranked by the PES it takes to add one HP from your current level.
			{:else}
				The least skill PES that reaches the goal, spread across {isFamily ? 'the profession' : name}'s
				skills where each moves it most.
			{/if}
			For codex rewards and chips.
		</p>
	{/snippet}

	{#if isFamily}
		<p class="text-sm text-text-tertiary">Pick one profession.</p>
	{:else if hub.pathError}
		<ErrorNotice message={hub.pathError} />
	{:else if hub.pathLoading}
		<div class="space-y-3">
			{#each { length: 5 } as _}
				<Skeleton class="h-5 w-full" />
			{/each}
		</div>
	{:else if isHp}
		{#if hpSkills.length === 0}
			<p class="text-sm text-text-tertiary">Scan your skills first.</p>
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
								<td class="py-2.5 pr-3 {skill.currentLevel > 0 ? 'text-text' : 'text-text-tertiary'}">
									{skill.name}
								</td>
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
			<div class="mt-3 flex flex-wrap items-center gap-x-5 gap-y-1">
				{#if hpSkills.length > SHOWN_HP_SKILLS}
					<button type="button" class="linklet" onclick={() => (showAllHp = !showAllHp)}>
						{showAllHp ? 'Show fewer' : `Show all ${hpSkills.length}`}
					</button>
				{/if}
				{#if hub.hpPath && hub.hpPath.attributes.length > 0}
					{@render attributeTip(
						hub.hpPath.attributes.map((attr) => ({ name: attr.name, figure: `${attr.levelsPerHp} lvl/HP` })),
						'Fewer levels per HP is better.',
					)}
				{/if}
			</div>
		{/if}
	{:else if !hub.goalActive}
		<p class="text-sm text-text-tertiary">Set a goal above your current level.</p>
	{:else if path}
		{#if allocated.length > 0}
			<div class="overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-border/60">
							<th class="w-8 py-2 pr-2 text-left eyebrow">#</th>
							<th class="py-2 pr-3 text-left eyebrow">Skill</th>
							<th class="px-3 py-2 text-right eyebrow">Weight</th>
							<th class="px-3 py-2 text-right eyebrow">Level</th>
							<th class="w-40 px-3 py-2 text-left eyebrow">Share of PES</th>
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
								<td class="py-2.5 pl-3 text-right font-medium tabular-nums {rankTone(i)}">
									{formatPed(alloc.pedCost)}
								</td>
							</tr>
						{/each}
					</tbody>
					<tfoot>
						<tr>
							<td></td>
							<td class="pt-3 pr-3 text-xs text-text-secondary" colspan="4">
								Total to {formatGoal(hub.target, hub.goal ?? 0)}
							</td>
							<td class="pt-3 pl-3 text-right font-semibold tabular-nums text-text">
								{formatPed(path.totalPed)}
							</td>
						</tr>
					</tfoot>
				</table>
			</div>
		{:else}
			<p class="text-sm text-text-tertiary">Already there.</p>
		{/if}

		{#if leftOut.length > 0 || path.attributes.length > 0}
			<div class="mt-3 flex flex-wrap items-center gap-x-5 gap-y-1">
				{#if leftOut.length > 0}
					<InfoTip label="Skills left out" width="w-80" align="left">
						{#snippet trigger()}
							<span class="linklet">{leftOut.length} left out</span>
						{/snippet}
						<p class="text-xs leading-relaxed text-text-secondary">{groupByReason(leftOut)}.</p>
					</InfoTip>
				{/if}
				{#if path.attributes.length > 0}
					{@render attributeTip(
						path.attributes.map((attr) => ({ name: attr.name, figure: `×${attr.contributionFactor}` })),
						'Higher contribution is better.',
					)}
				{/if}
			</div>
		{/if}
	{/if}
</SkillingSection>
