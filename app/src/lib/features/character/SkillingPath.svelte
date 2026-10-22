<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import { formatPed } from '$lib/utils/format';
	import { targetLabel } from './codexRankingTarget';
	import { hpAnswer, professionAnswer } from './pathView';
	import SkillingBar from './SkillingBar.svelte';
	import SkillingPanel from './SkillingPanel.svelte';
	import { formatGoal, formatLevel, groupByReason } from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';

	// The Optimiser: where skill PES (codex rewards, chips) moves the target
	// furthest. A profession and HP alike get the least-PES allocation to
	// the goal, spread where each skill moves the target most.
	let { hub }: { hub: SkillingModel } = $props();

	const isHp = $derived(hub.target.kind === 'hp');
	const isFamily = $derived(hub.target.kind === 'family');
	const name = $derived(targetLabel(hub.target));
	const answer = $derived(
		isHp
			? hub.hpPath && hpAnswer(hub.hpPath)
			: hub.path && professionAnswer(hub.path),
	);
	const allocated = $derived(answer?.allocated ?? []);
	const topCost = $derived(Math.max(0, ...allocated.map((row) => row.pedCost)));

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

<SkillingPanel id="skilling-path" label="Optimiser" busy={hub.pathLoading}>
	{#snippet lead()}
		{#if hub.pathLoading || hub.pathError || (answer && !answer.climbable)}
			<!-- The body says what is happening. -->
		{:else if !hub.goalActive}
			Set a goal above where you stand.
		{:else if answer && answer.short}
			<span class="font-medium text-text">{formatPed(answer.totalPed)} PES</span> of codex rewards or
			chips tops every skill out at {formatGoal(hub.target, answer.end)}, short of
			{formatGoal(hub.target, hub.goal ?? 0)}.
		{:else if answer && allocated.length > 0}
			<span class="font-medium text-text">{formatPed(answer.totalPed)} PES</span> of codex rewards or
			chips reaches {formatGoal(hub.target, hub.goal ?? 0)}, over {allocated.length}
			{allocated.length === 1 ? 'skill' : 'skills'}.
		{:else if answer}
			Already there.
		{/if}
	{/snippet}

	{#snippet info()}
		<p class="text-xs font-semibold leading-relaxed text-text">Where skill PES goes furthest</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			{#if isHp}
				The least skill PES that reaches the HP goal, spread across the skills that add HP where
				each adds it cheapest.
			{:else if isFamily}
				The least skill PES that reaches the goal on the family's combined level, spread across its
				professions' skills where each moves the total most.
			{:else}
				The least skill PES that reaches the goal, spread across {name}'s skills where each moves it
				most.
			{/if}
			For codex rewards and chips.
		</p>
	{/snippet}

	{#if hub.pathError}
		<ErrorNotice message={hub.pathError} />
	{:else if hub.pathLoading}
		<div class="space-y-3">
			{#each { length: 5 } as _}
				<Skeleton class="h-5 w-full" />
			{/each}
		</div>
	{:else if answer && !answer.climbable}
		<p class="text-sm text-text-tertiary">Scan your skills first.</p>
	{:else if answer}
		{#if allocated.length > 0}
			<div class="overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-border/60">
							<th class="w-8 py-2 pr-2 text-left eyebrow">#</th>
							<th class="py-2 pr-3 text-left eyebrow">Skill</th>
							<th class="px-3 py-2 text-right eyebrow">{answer.rateLabel}</th>
							<th class="px-3 py-2 text-right eyebrow">Level</th>
							<th class="w-40 px-3 py-2 text-left eyebrow">Share of PES</th>
							<th class="py-2 pl-3 text-right eyebrow">PES</th>
						</tr>
					</thead>
					<tbody>
						{#each allocated as row, i (row.name)}
							<tr class="border-b border-border/30 transition-colors hover:bg-surface-hover/40">
								<td class="py-2.5 pr-2 text-xs tabular-nums text-text-tertiary">{i + 1}</td>
								<td class="py-2.5 pr-3 text-text">{row.name}</td>
								<td class="px-3 py-2.5 text-right tabular-nums text-text-secondary">{row.rate}</td>
								<td class="whitespace-nowrap px-3 py-2.5 text-right tabular-nums text-text-secondary">
									{formatLevel(row.currentLevel)}
									<span class="mx-1 text-text-tertiary">&rarr;</span>
									<span class="text-text">{formatLevel(row.newLevel)}</span>
								</td>
								<td class="px-3 py-2.5">
									<div class="flex items-center gap-2.5">
										<SkillingBar fraction={topCost > 0 ? row.pedCost / topCost : 0} class="flex-1" />
										<span class="w-11 text-right text-xs tabular-nums text-text-tertiary">
											{answer.totalPed > 0 ? ((row.pedCost / answer.totalPed) * 100).toFixed(1) : '0.0'}%
										</span>
									</div>
								</td>
								<td class="py-2.5 pl-3 text-right font-medium tabular-nums {rankTone(i)}">
									{formatPed(row.pedCost)}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		{#if answer.leftOut.length > 0 || answer.attributes.length > 0}
			<div class="mt-3 flex flex-wrap items-center gap-x-5 gap-y-1">
				{#if answer.leftOut.length > 0}
					<InfoTip label="Skills left out" width="w-80" align="left">
						{#snippet trigger()}
							<span class="linklet">{answer.leftOut.length} left out</span>
						{/snippet}
						<p class="text-xs leading-relaxed text-text-secondary">{groupByReason(answer.leftOut)}.</p>
					</InfoTip>
				{/if}
				{#if answer.attributes.length > 0}
					{@render attributeTip(answer.attributes, answer.attributeNote)}
				{/if}
			</div>
		{/if}
	{/if}
</SkillingPanel>
