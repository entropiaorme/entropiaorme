<script lang="ts">
	import type { SkillingRouteForecast } from '$lib/api/commands.gen';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';
	import { targetLabel } from './codexRankingTarget';
	import { hpPastTheKill, MEANINGFUL_OVERKILL, type RouteRow, routeStatusLabel } from './routesView';
	import SkillingBar from './SkillingBar.svelte';
	import {
		formatKills,
		formatLevel,
		formatRoutePed,
		formatTargetGain,
	} from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';

	// The chosen route in full: what reaching the goal along it takes, what
	// each kill yields and costs, and which skills it trains on the way.
	let { hub, row, route }: { hub: SkillingModel; row: RouteRow; route: SkillingRouteForecast } =
		$props();

	const name = $derived(targetLabel(hub.target));
	const isHp = $derived(hub.target.kind === 'hp');
	const isFamily = $derived(hub.target.kind === 'family');
	// A gain below the shown precision reads as "+0.000": those, and the
	// skills that do not count toward the target at all, sit behind a tip
	// rather than as rows that look like nothing.
	const minorGain = $derived(isHp ? 0.005 : 0.0005);
	const onTarget = $derived(
		route.skills.filter((skill) => skill.movesTarget && skill.targetGain >= minorGain),
	);
	const others = $derived(
		route.skills.filter((skill) => !skill.movesTarget || skill.targetGain < minorGain),
	);
	const topShare = $derived(Math.max(0, ...onTarget.map((skill) => skill.pesShare ?? 0)));
	const overkill = $derived(row.overkill);
	const pastTheKill = $derived(
		route.mobHp !== null && route.overkillShare > 0
			? hpPastTheKill(route.mobHp, route.overkillShare)
			: null,
	);
	const firing = $derived(
		route.secondsPerKill !== null
			? route.secondsPerKill - (route.pace?.betweenKillSeconds ?? 1)
			: null,
	);
</script>

<div class="mt-8 border-t border-border/40 pt-6" aria-label="{row.stored.weaponName} on {row.stored.display} in detail">
	<p class="mb-2 text-sm font-medium text-text">
		{row.stored.weaponName} on {row.stored.display}
	</p>

	{#if route.status !== 'ready'}
		<p class="text-sm text-text-tertiary">{routeStatusLabel(route.status)}.</p>
	{:else}
		<!-- What every kill does: the figures the table's totals are made of. -->
		<div class="flex flex-wrap items-center gap-x-1.5 gap-y-1 text-xs tabular-nums text-text-tertiary">
			<span>Each kill:</span>
			<span class="text-text-secondary">{formatPed(route.skillPerKillPec)} PEC of skill</span>
			<span aria-hidden="true">·</span>
			<span class="text-text-secondary">{formatPed(route.cycledPerKillPec)} PEC cycled</span>
			<span aria-hidden="true">·</span>
			<span class="text-text-secondary">{formatPed(route.lossPerKillPec)} PEC lost</span>
			{#if pastTheKill !== null}
				<span aria-hidden="true">·</span>
				<span class="text-text-secondary">
					{pastTheKill.toFixed(pastTheKill < 0.1 ? 2 : 1)} HP dealt past the kill
				</span>
			{/if}
			{#if route.secondsPerKill !== null && firing !== null}
				<span aria-hidden="true">·</span>
				<span class="text-text-secondary">
					{route.secondsPerKill.toFixed(1)} s ({firing.toFixed(1)} s firing, {(
						route.pace?.betweenKillSeconds ?? 1
					).toFixed(1)} s to the next mob)
				</span>
			{/if}
			<InfoTip label="About each kill" width="w-80" align="left">
				<p class="text-xs leading-relaxed text-text-secondary">
					A kill yields skill by the mob alone: a bonus for its level ({route.mobLevel ?? NO_DATA}) and
					skill for each HP removed ({route.mobHp ?? NO_DATA}), whatever the weapon. The loss is
					expected under community model v1 at the looter basis above; markup is not included.
				</p>
				<p class="mt-2 text-xs leading-relaxed text-text-secondary">
					{#if route.pace}
						The time to the next mob is your recorded pace on {row.stored.display}, from
						{formatKills(route.pace.kills)} kills.
					{:else}
						The time to the next mob is a 1 s allowance until you have recorded kills of
						{row.stored.display}.
					{/if}
				</p>
			</InfoTip>
		</div>

		<!-- What the weapon's fit to the mob costs, read by the route's rank:
			 headroom on the cheapest, the deciding factor or a part of the gap on the rest. -->
		{#if overkill && overkill.share >= MEANINGFUL_OVERKILL}
			<p class="mt-3 max-w-2xl text-xs leading-relaxed text-text-secondary">
				{#if overkill.kind === 'headroom'}
					A lighter weapon on {row.stored.display} could save up to {formatRoutePed(overkill.ped)} PED:
					with every hit's damage used, this route would lose {formatRoutePed(overkill.fittedPed)} PED.
				{:else if overkill.kind === 'covers_premium'}
					Overkill costs more than this route's {formatRoutePed(overkill.premium)} PED premium, so a
					better-fitted weapon on {row.stored.display} could make it the cheapest: with every hit's damage
					used it would lose {formatRoutePed(overkill.fittedPed)} PED, against the cheapest route's
					{formatRoutePed(overkill.cheapestPed)}.
				{:else}
					Overkill accounts for {formatRoutePed(overkill.ped)} PED of this route's
					{formatRoutePed(overkill.premium)} PED premium; the rest is down to the mob and the weapon's own
					economy.
				{/if}
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
								<td class="py-2.5 pr-3 text-text">{skill.name}</td>
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
