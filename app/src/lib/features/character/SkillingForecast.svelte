<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import StatDisplay from '$lib/components/StatDisplay.svelte';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';
	import { targetLabel } from './codexRankingTarget';
	import SkillingBar from './SkillingBar.svelte';
	import SkillingSection from './SkillingSection.svelte';
	import SkillingSourcePicker from './SkillingSourcePicker.svelte';
	import {
		formatHours,
		formatLevel,
		formatLift,
		formatTargetGain,
		statusMessage,
		warningText,
	} from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';

	// The Session Forecast: the play it takes to reach the goal in one of your named
	// sessions, at the pace, loot return, and skill mix it recorded, with its
	// own realised markup.
	let { hub }: { hub: SkillingModel } = $props();

	const source = $derived(hub.selectedSource);
	const name = $derived(targetLabel(hub.target));
	const isHp = $derived(hub.target.kind === 'hp');
	const isFamily = $derived(hub.target.kind === 'family');
	// A gain below the shown precision reads as "+0.000": those, and the
	// skills that do not count toward the target at all, sit behind a tip
	// rather than as rows that look like nothing.
	const minorGain = $derived(isHp ? 0.005 : 0.0005);
	const onTarget = $derived(
		source?.skills.filter((skill) => skill.movesTarget && skill.targetGain >= minorGain) ?? [],
	);
	const others = $derived(
		source?.skills.filter((skill) => !skill.movesTarget || skill.targetGain < minorGain) ?? [],
	);
	const topShare = $derived(Math.max(0, ...onTarget.map((skill) => skill.pesShare ?? 0)));
</script>

<SkillingSection id="skilling-forecast" title="Session Forecast" busy={hub.forecastLoading}>
	{#snippet info()}
		<p class="text-xs font-semibold leading-relaxed text-text">From your own play</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			The play it takes to reach the goal in one of your named sessions, at the pace, loot return,
			and skill mix it has recorded, with its own realised markup.
		</p>
		{#if source}
			<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
				{source.name}: {source.sample.sessions}
				{source.sample.sessions === 1 ? 'session' : 'sessions'}, {formatHours(source.sample.hours)},
				{formatPed(source.sample.cycledPed)} PED cycled, {formatPed(source.sample.pes)} PES earned.
			</p>
		{/if}
	{/snippet}

	{#snippet actions()}
		{#if hub.goalActive && hub.sources.length > 0}
			<SkillingSourcePicker sources={hub.sources} selected={source} onselect={hub.selectSource} />
		{/if}
		{#if source?.status === 'ready' && source.warnings.length > 0}
			<InfoTip label="Why this is rough" width="w-72">
				{#snippet trigger()}
					<span class="text-xs font-medium text-warning">Thin sample</span>
				{/snippet}
				<p class="text-xs leading-relaxed text-text-secondary">
					{source.warnings.map((warning) => warningText(warning, source.sample)).join('; ')}.
				</p>
			</InfoTip>
		{/if}
	{/snippet}

	{#if !hub.goalActive}
		<p class="text-sm text-text-tertiary">Set a goal above where you stand.</p>
	{:else if hub.forecastError}
		<ErrorNotice message={hub.forecastError} />
	{:else if hub.forecastLoading}
		<div class="grid grid-cols-2 gap-x-8 gap-y-6 lg:grid-cols-4">
			{#each { length: 4 } as _}
				<div class="space-y-2">
					<Skeleton class="h-3 w-20" />
					<Skeleton class="h-6 w-28" />
				</div>
			{/each}
		</div>
	{:else if hub.forecast && hub.sources.length === 0}
		<p class="text-sm text-text-tertiary">No named sessions recorded yet.</p>
	{:else if source && source.status !== 'ready'}
		<p class="text-sm text-text-tertiary">{statusMessage(source.status, source.name, name)}</p>
	{:else if source}
		<div class="grid grid-cols-2 gap-x-8 gap-y-6 lg:grid-cols-4">
			<StatDisplay label="Cycled" value={formatPed(source.cycledPed)} unit="PED" />
			<StatDisplay label="Time" value={formatHours(source.hours)} />
			<StatDisplay label="TT cost" value={formatPed(source.ttCost)} unit="PED">
				{#snippet labelSuffix()}
					<InfoTip label="About TT cost">
						<p class="text-xs font-semibold leading-relaxed text-text">Cycled less loot TT</p>
						<p class="mt-1 text-xs leading-relaxed text-text-secondary">
							{formatPed(source.lootTt)} PED of loot TT back, at this session's recorded
							{formatPercent(source.sample.lootTt / source.sample.cycledPed)} loot return. Markup is
							not included.
						</p>
					</InfoTip>
				{/snippet}
			</StatDisplay>
			<StatDisplay
				label="After markup"
				value={source.netCost !== null ? formatPed(source.netCost) : NO_DATA}
				unit={source.netCost !== null ? 'PED' : ''}
				valueClass={source.netCost === null ? 'text-text-tertiary' : ''}
			>
				{#snippet labelSuffix()}
					<InfoTip label="About the markup">
						{#if source.sample.realisedMarkup !== null}
							<p class="text-xs font-semibold leading-relaxed text-text">
								{formatLift(source.sample.markupLift ?? 0)} realised on loot
							</p>
							<p class="mt-1 text-xs leading-relaxed text-text-secondary">
								{formatPed(source.sample.realisedMarkup)} PED net from recorded sales of {source.name}'s
								loot, over {formatPed(source.sample.lootTt)} PED of loot TT. Unsold stock adds nothing
								until it sells.
							</p>
						{:else}
							<p class="text-xs font-semibold leading-relaxed text-text">No sales yet</p>
							<p class="mt-1 text-xs leading-relaxed text-text-secondary">
								Record a sale of {source.name}'s loot and its realised markup is applied here. Nothing
								is estimated from market prices.
							</p>
						{/if}
					</InfoTip>
				{/snippet}
			</StatDisplay>
		</div>

		{#if onTarget.length > 0}
			<div class="mt-7 overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-border/60">
							<th class="py-2 pr-3 text-left eyebrow">Skill</th>
							<th class="w-48 px-3 py-2 text-left eyebrow">Share of PES</th>
							<th class="px-3 py-2 text-right eyebrow">Level</th>
							<th class="py-2 pl-3 text-right eyebrow">{isHp ? 'HP' : isFamily ? 'Combined' : 'Profession'}</th>
						</tr>
					</thead>
					<tbody>
						{#each onTarget as skill (skill.name)}
							<tr class="border-b border-border/30 transition-colors hover:bg-surface-hover/40">
								<td class="py-2.5 pr-3 text-text">
									{skill.name}
									{#if skill.isAttribute}
										<span class="ml-1.5 text-[0.625rem] font-medium uppercase tracking-wide text-text-tertiary">
											Attribute
										</span>
									{/if}
								</td>
								<td class="px-3 py-2.5">
									{#if skill.pesShare !== null}
										<div class="flex items-center gap-2.5">
											<SkillingBar fraction={topShare > 0 ? skill.pesShare / topShare : 0} class="flex-1" />
											<span class="w-12 text-right text-xs tabular-nums text-text-secondary">
												{formatPercent(skill.pesShare)}
											</span>
										</div>
									{:else}
										<span class="text-xs text-text-tertiary">{NO_DATA}</span>
									{/if}
								</td>
								<td class="whitespace-nowrap px-3 py-2.5 text-right tabular-nums text-text-secondary">
									{formatLevel(skill.currentLevel)}
									<span class="mx-1 text-text-tertiary">&rarr;</span>
									<span class="text-text">{formatLevel(skill.endLevel)}</span>
								</td>
								<td class="whitespace-nowrap py-2.5 pl-3 text-right font-medium tabular-nums text-accent">
									{formatTargetGain(hub.target, skill.targetGain)}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		{#if others.length > 0}
			<div class="mt-3">
				<InfoTip label="Other skills trained" width="w-80" align="left">
					{#snippet trigger()}
						<span class="linklet">+{others.length} other skills trained</span>
					{/snippet}
					<p class="text-xs font-semibold leading-relaxed text-text">
						{isHp ? 'Little or no HP' : `Little or nothing toward ${name}`}
					</p>
					<p class="mt-1 text-xs leading-relaxed text-text-secondary">
						{others.map((skill) => `${skill.name} +${skill.levelGain.toFixed(2)}`).join(', ')}
					</p>
				</InfoTip>
			</div>
		{/if}
	{/if}
</SkillingSection>
