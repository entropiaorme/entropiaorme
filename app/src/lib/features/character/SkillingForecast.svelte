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
		formatGoal,
		formatHours,
		formatLevel,
		formatLift,
		formatTargetGain,
		statusMessage,
		warningText,
	} from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';

	// What your sessions say: the play it takes to reach the goal in one of
	// your named sessions, at the pace, loot return, and skill mix that
	// session recorded, with its own realised markup.
	let { hub }: { hub: SkillingModel } = $props();

	const source = $derived(hub.selectedSource);
	const name = $derived(targetLabel(hub.target));
	const isHp = $derived(hub.target.kind === 'hp');
	const isFamily = $derived(hub.target.kind === 'family');
	// A gain below the shown precision reads as "+0.000": fold those into a
	// line of their own rather than rows that look like nothing.
	const minorGain = $derived(isHp ? 0.005 : 0.0005);
	const onTarget = $derived(
		source?.skills.filter((skill) => skill.movesTarget && skill.targetGain >= minorGain) ?? [],
	);
	const barely = $derived(
		source?.skills.filter((skill) => skill.movesTarget && skill.targetGain < minorGain) ?? [],
	);
	const offTarget = $derived(source?.skills.filter((skill) => !skill.movesTarget) ?? []);
	const topShare = $derived(Math.max(0, ...onTarget.map((skill) => skill.pesShare ?? 0)));
	const sessionsEquivalent = $derived(
		source && source.sample.sessions > 0 && source.sample.cycledPed > 0
			? source.cycledPed / (source.sample.cycledPed / source.sample.sessions)
			: null,
	);
	const SHOWN_OFF_TARGET = 6;
</script>

<SkillingSection
	id="skilling-forecast"
	title="What your sessions say"
	description="The play it takes to reach the goal in one of your named sessions, at the pace, loot return, and skill mix it has recorded."
	busy={hub.forecastLoading}
>
	{#snippet actions()}
		{#if hub.sources.length > 0}
			<SkillingSourcePicker sources={hub.sources} selected={source} onselect={hub.selectSource} />
		{/if}
	{/snippet}

	{#if isFamily}
		<p class="py-4 text-sm text-text-tertiary">
			A forecast follows one profession at a time. Pick one of the family's professions above.
		</p>
	{:else if !hub.goalActive}
		<p class="py-4 text-sm text-text-tertiary">
			Set a goal above your current {isHp ? 'HP' : 'level'} to forecast it.
		</p>
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
		<p class="py-4 text-sm text-text-tertiary">
			No named sessions recorded yet. Track play under a session definition and it will forecast
			from here.
		</p>
	{:else if source && source.status !== 'ready'}
		<p class="py-4 text-sm text-text-tertiary">
			{statusMessage(source.status, source.name, name)}
			{#if hub.sources.some((candidate) => candidate.status === 'ready')}
				Choose another session above.
			{/if}
		</p>
	{:else if source}
		<div class="grid grid-cols-2 gap-x-8 gap-y-6 lg:grid-cols-4">
			<StatDisplay
				label="Cycled"
				value={formatPed(source.cycledPed)}
				unit="PED"
				comparison={sessionsEquivalent !== null
					? `about ${sessionsEquivalent < 10 ? sessionsEquivalent.toFixed(1) : Math.round(sessionsEquivalent)} sessions like yours`
					: undefined}
			/>
			<StatDisplay label="Time" value={formatHours(source.hours)} comparison="at your recorded pace" />
			<StatDisplay
				label="TT cost"
				value={formatPed(source.ttCost)}
				unit="PED"
				comparison={source.sample.cycledPed > 0
					? `${formatPercent(source.sample.lootTt / source.sample.cycledPed)} loot TT return`
					: undefined}
			>
				{#snippet labelSuffix()}
					<InfoTip label="About TT cost">
						<span class="block text-xs leading-relaxed text-text-secondary">
							What the forecast cycles, less the loot TT it returns at this session's recorded loot
							rate ({formatPed(source.lootTt)} PED). Markup is not included.
						</span>
					</InfoTip>
				{/snippet}
			</StatDisplay>
			<StatDisplay
				label="After markup"
				value={source.netCost !== null ? formatPed(source.netCost) : NO_DATA}
				unit={source.netCost !== null ? 'PED' : ''}
				valueClass={source.netCost === null ? 'text-text-tertiary' : ''}
				comparison={source.sample.markupLift !== null
					? `${formatLift(source.sample.markupLift)} realised on loot`
					: 'no sales from it yet'}
			>
				{#snippet labelSuffix()}
					<InfoTip label="About the markup">
						<span class="block space-y-1.5 text-xs leading-relaxed text-text-secondary">
							{#if source.sample.realisedMarkup !== null}
								<span class="block">
									The TT cost less the markup {source.name}'s loot has actually realised: {formatPed(
										source.sample.realisedMarkup,
									)} PED net from recorded sales, over {formatPed(source.sample.lootTt)} PED of loot
									TT ({formatLift(source.sample.markupLift ?? 0)}).
								</span>
								<span class="block text-text-tertiary">
									Unsold stock adds nothing until it sells, so this rises as you sell.
								</span>
							{:else}
								<span class="block">
									Record a sale of {source.name}'s loot and its realised markup is applied here.
									Nothing is estimated from market prices.
								</span>
							{/if}
						</span>
					</InfoTip>
				{/snippet}
			</StatDisplay>
		</div>

		<p class="mt-5 flex flex-wrap gap-x-1.5 gap-y-1 text-xs text-text-tertiary">
			<span>
				From {source.sample.sessions}
				{source.sample.sessions === 1 ? 'session' : 'sessions'}, {formatHours(source.sample.hours)},
				<span class="tabular-nums">{formatPed(source.sample.cycledPed)}</span> PED cycled,
				<span class="tabular-nums">{formatPed(source.sample.pes)}</span> PES earned.
			</span>
			{#if source.warnings.length > 0}
				<span class="text-warning">
					Treat it as rough: {source.warnings.map((warning) => warningText(warning, source.sample)).join('; ')}.
				</span>
			{/if}
		</p>

		{#if onTarget.length > 0}
			<div class="mt-7 overflow-x-auto">
				<table class="w-full text-sm">
					<caption class="sr-only">Skills this session moves toward {formatGoal(hub.target, hub.goal ?? 0)}</caption>
					<thead>
						<tr class="border-b border-border/60">
							<th class="py-2 pr-3 text-left eyebrow">Skill</th>
							<th class="w-48 px-3 py-2 text-left eyebrow">Share of its PES</th>
							<th class="px-3 py-2 text-right eyebrow">Level</th>
							<th class="py-2 pl-3 text-right eyebrow">{isHp ? 'HP' : 'Profession'}</th>
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
										<span class="text-xs text-text-tertiary">no PES</span>
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

		{#if barely.length > 0}
			<p class="mt-3 text-xs leading-relaxed text-text-tertiary">
				<span class="text-text-secondary">Barely moves it</span>
				{barely.map((skill) => skill.name).join(', ')}.
			</p>
		{/if}

		{#if offTarget.length > 0}
			<p class="mt-3 text-xs leading-relaxed text-text-tertiary">
				<span class="text-text-secondary">Also trains</span>
				{offTarget
					.slice(0, SHOWN_OFF_TARGET)
					.map((skill) => `${skill.name} +${skill.levelGain.toFixed(2)}`)
					.join(', ')}{offTarget.length > SHOWN_OFF_TARGET
					? `, and ${offTarget.length - SHOWN_OFF_TARGET} more`
					: ''}, which {isHp ? 'add no HP' : `do not count toward ${name}`}.
			</p>
		{/if}
	{/if}
</SkillingSection>
