<script lang="ts">
	import InfoTip from '$lib/components/InfoTip.svelte';
	import StatDisplay from '$lib/components/StatDisplay.svelte';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';
	import { targetLabel } from './codexRankingTarget';
	import type { RouteRow } from './routesView';
	import { routeStatusLabel } from './routesView';
	import SkillingBar from './SkillingBar.svelte';
	import {
		formatHours,
		formatKills,
		formatLevel,
		formatLift,
		formatRoutePed,
		formatTargetGain,
		statusMessage,
		warningText,
	} from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';

	// The chosen route in full: what reaching the goal along it takes, and
	// which skills it trains on the way. A planned route shows its kills and
	// what each kill yields and costs; a named session shows its recorded
	// loot return and realised markup.
	let { hub, row }: { hub: SkillingModel; row: RouteRow } = $props();

	const name = $derived(targetLabel(hub.target));
	const isHp = $derived(hub.target.kind === 'hp');
	const isFamily = $derived(hub.target.kind === 'family');
	const skills = $derived(row.kind === 'planned' ? row.route.skills : row.source.skills);
	// A gain below the shown precision reads as "+0.000": those, and the
	// skills that do not count toward the target at all, sit behind a tip
	// rather than as rows that look like nothing.
	const minorGain = $derived(isHp ? 0.005 : 0.0005);
	const onTarget = $derived(
		skills.filter((skill) => skill.movesTarget && skill.targetGain >= minorGain),
	);
	const others = $derived(
		skills.filter((skill) => !skill.movesTarget || skill.targetGain < minorGain),
	);
	const topShare = $derived(Math.max(0, ...onTarget.map((skill) => skill.pesShare ?? 0)));
</script>

<div class="mt-8 border-t border-border/40 pt-6" aria-label="{row.name} in detail">
	<p class="mb-5 text-sm font-semibold tracking-tight text-text">{row.name}</p>

	{#if row.status !== 'ready'}
		<p class="text-sm text-text-tertiary">
			{row.kind === 'recorded'
				? statusMessage(row.source.status, row.source.name, name)
				: `${routeStatusLabel(row.status)}.`}
		</p>
	{:else}
		<div class="grid grid-cols-2 gap-x-8 gap-y-6 lg:grid-cols-4">
			<StatDisplay label="PED lost" value={formatRoutePed(row.pedLost)} unit="PED">
				{#snippet labelSuffix()}
					<InfoTip label="About PED lost">
						{#if row.kind === 'planned'}
							<p class="text-xs font-semibold leading-relaxed text-text">Expected, not recorded</p>
							<p class="mt-1 text-xs leading-relaxed text-text-secondary">
								{formatPed(row.route.lossPerKillPec)} PEC lost per kill of {formatPed(
									row.route.cycledPerKillPec,
								)} PEC cycled, under the community model at the looter basis below. Markup is not
								included.
							</p>
						{:else}
							<p class="text-xs font-semibold leading-relaxed text-text">Cycled less loot TT</p>
							<p class="mt-1 text-xs leading-relaxed text-text-secondary">
								{formatPed(row.source.lootTt)} PED of loot TT back, at this session's recorded
								{formatPercent(row.source.sample.lootTt / row.source.sample.cycledPed)} loot return.
								Markup is not included.
							</p>
						{/if}
					</InfoTip>
				{/snippet}
			</StatDisplay>
			<StatDisplay label="Time" value={row.hours !== null ? formatHours(row.hours) : NO_DATA}>
				{#snippet labelSuffix()}
					{#if row.kind === 'planned'}
						<InfoTip label="About the time">
							<p class="text-xs leading-relaxed text-text-secondary">
								{row.route.secondsPerKill === null
									? "The weapon's attack rate is unknown."
									: row.route.measured
										? `${row.route.secondsPerKill.toFixed(1)} s per kill, at your recorded pace.`
										: `${row.route.secondsPerKill.toFixed(1)} s per kill: firing at the weapon's attack rate plus a second to reach the next mob.`}
							</p>
						</InfoTip>
					{/if}
				{/snippet}
			</StatDisplay>
			<StatDisplay label="Cycled" value={formatRoutePed(row.cycledPed)} unit="PED" />
			{#if row.kind === 'planned'}
				<StatDisplay label="Kills" value={formatKills(row.route.kills)}>
					{#snippet labelSuffix()}
						<InfoTip label="About the kills">
							<p class="text-xs font-semibold leading-relaxed text-text">
								{formatPed(row.route.skillPerKillPec)} PEC of skill per kill
							</p>
							{#if row.route.measured}
								<p class="mt-1 text-xs leading-relaxed text-text-secondary">
									Measured from {formatKills(row.route.measured.kills)} of your kills over {row.route
										.measured.sessions}
									{row.route.measured.sessions === 1 ? 'session' : 'sessions'}: their skill, cycling, and
									pace, restated for the pill chosen. The loss rate is still the model's.
								</p>
							{:else}
								<p class="mt-1 text-xs leading-relaxed text-text-secondary">
									A kill yields skill by the mob alone: a bonus for its level ({row.route.mobLevel ??
										NO_DATA}) and skill for each HP removed ({row.route.mobHp ?? NO_DATA}), whatever
									the weapon. The weapon sets what the kill costs and which skills receive it.
								</p>
							{/if}
						</InfoTip>
					{/snippet}
				</StatDisplay>
			{:else}
				{@const source = row.source}
				<StatDisplay
					label="After markup"
					value={source.netCost !== null ? formatRoutePed(source.netCost) : NO_DATA}
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
									Record a sale of {source.name}'s loot and its realised markup is applied here.
									Nothing is estimated from market prices.
								</p>
							{/if}
						</InfoTip>
					{/snippet}
				</StatDisplay>
			{/if}
		</div>

		{#if row.kind === 'recorded' && row.source.warnings.length > 0}
			<p class="mt-4 text-xs text-warning">
				Thin sample: {row.source.warnings
					.map((warning) => warningText(warning, row.source.sample))
					.join('; ')}.
			</p>
		{/if}

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
</div>
