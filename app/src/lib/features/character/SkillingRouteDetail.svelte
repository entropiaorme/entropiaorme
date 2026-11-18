<script lang="ts">
	import type { SkillingRouteForecast } from '$lib/api/commands.gen';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import Input from '$lib/components/Input.svelte';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';
	import { targetLabel } from './codexRankingTarget';
	import {
		hpPastTheKill,
		MAX_LOOT_MARKUP,
		MEANINGFUL_OVERKILL,
		MIN_HUNTING_SHARE,
		MIN_LOOT_MARKUP,
		readRoutePercent,
		type RouteRow,
		routeStatusLabel,
		type StoredRoute,
	} from './routesView';
	import SkillingBar from './SkillingBar.svelte';
	import {
		formatKills,
		formatLevel,
		formatRoutePed,
		formatRoutePercent,
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
	const breakEven = $derived(row.breakEven);

	// The route's own planning figures: typed, then committed on Enter or on
	// leaving the field. Blank clears one; an unreadable entry snaps back.
	type PlanningField = 'lootMarkupPercent' | 'huntingSharePercent';
	const BOUNDS: Record<PlanningField, [number, number]> = {
		lootMarkupPercent: [MIN_LOOT_MARKUP, MAX_LOOT_MARKUP],
		huntingSharePercent: [MIN_HUNTING_SHARE, 100],
	};
	function commit(field: PlanningField, input: HTMLInputElement) {
		const [min, max] = BOUNDS[field];
		const read = readRoutePercent(input.value, min, max);
		const stored = row.stored[field] ?? null;
		// All of the clock reads as no share at all.
		const next = field === 'huntingSharePercent' && read === 100 ? null : read;
		if (next === undefined || next === stored) {
			input.value = stored === null ? '' : String(stored);
			return;
		}
		const updated: StoredRoute = { ...row.stored, [field]: next };
		hub.updateRoute(row.index, updated);
	}
	function onKey(field: PlanningField, event: KeyboardEvent & { currentTarget: HTMLInputElement }) {
		if (event.key === 'Enter') commit(field, event.currentTarget);
	}
</script>

<div class="mt-8 border-t border-border/40 pt-6" aria-label="{row.stored.weaponName} on {row.stored.display} in detail">
	<p class="mb-2 text-sm font-medium text-text">
		{row.stored.weaponName} on {row.stored.display}
	</p>

	<!-- What the player expects of this route beyond the model: its loot's markup and its downtime. -->
	<div class="mb-4 flex flex-wrap items-center gap-x-5 gap-y-2 text-xs text-text-tertiary">
		<label class="flex items-center gap-2">
			<span>Loot sells at</span>
			<Input
				class="w-20"
				align="right"
				type="number"
				min={MIN_LOOT_MARKUP}
				max={MAX_LOOT_MARKUP}
				step="0.1"
				inputmode="decimal"
				placeholder="100"
				aria-label="Expected loot markup percent"
				value={row.stored.lootMarkupPercent ?? ''}
				onblur={(event) => commit('lootMarkupPercent', event.currentTarget)}
				onkeydown={(event) => onKey('lootMarkupPercent', event)}
			/>
			<span>% of TT</span>
		</label>
		<label class="flex items-center gap-2">
			<span>Hunting</span>
			<Input
				class="w-16"
				align="right"
				type="number"
				min={MIN_HUNTING_SHARE}
				max="100"
				step="1"
				inputmode="numeric"
				placeholder="100"
				aria-label="Hunting share percent"
				value={row.stored.huntingSharePercent ?? ''}
				onblur={(event) => commit('huntingSharePercent', event.currentTarget)}
				onkeydown={(event) => onKey('huntingSharePercent', event)}
			/>
			<span>% of the time</span>
		</label>
		<InfoTip label="About the expected markup and hunting share" width="w-80" align="left">
			<p class="text-xs leading-relaxed text-text-secondary">
				The markup you expect this mob's loot to sell at nets off the TT lost, so a mob that is no
				skilling target by its HP can still be the cheapest way to the goal. Leave it blank to price the
				route at TT.
			</p>
			<p class="mt-2 text-xs leading-relaxed text-text-secondary">
				The hunting share is for downtime every run carries, such as an instance's timer: 10 minutes of
				hunting in every 13 is 77%. Your recorded pace leaves breaks out, so it never counts them.
			</p>
			<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
				Both are your planning assumptions. Markup is only realised when the loot sells, after auction
				fees.
			</p>
		</InfoTip>
	</div>

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
					expected under community model v1 at the looter basis above, at TT; armour and healing are
					left out, as most of their cost returns in loot.
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

		<!-- The markup that would make this route the cheapest. -->
		{#if breakEven}
			<p class="mt-3 max-w-2xl text-xs leading-relaxed text-text-secondary">
				{#if breakEven.beyond}
					Even loot at {formatRoutePercent(breakEven.percent)} of TT would leave this route dearer than
					{breakEven.against.weaponName} on {breakEven.against.display}.
				{:else}
					With loot selling at {formatRoutePercent(breakEven.percent)} of TT, this route would cost as
					much as {breakEven.against.weaponName} on {breakEven.against.display}.
				{/if}
			</p>
		{/if}

		<!-- What the weapon's fit to the mob costs, read by the route's rank:
			 headroom on the cheapest, the deciding factor or a part of the gap on the rest. -->
		{#if overkill && overkill.kind !== 'no_cost' && overkill.share >= MEANINGFUL_OVERKILL}
			<p class="mt-3 max-w-2xl text-xs leading-relaxed text-text-secondary">
				{#if overkill.kind === 'headroom'}
					A lighter weapon on {row.stored.display} could save up to {formatRoutePed(overkill.ped)} PED:
					with every hit's damage used, this route would lose {formatRoutePed(overkill.fittedPed)} PED.
				{:else if overkill.kind === 'covers_premium'}
					Overkill costs more than this route's {formatRoutePed(overkill.premium)} PED premium, so a
					better-fitted weapon on {row.stored.display} could make it the cheapest: with every hit's damage
					used it would lose {formatRoutePed(overkill.fittedPed)} PED, against the cheapest route's
					{formatRoutePed(overkill.cheapestPed)}.
				{:else if overkill.kind === 'part_of_premium'}
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
