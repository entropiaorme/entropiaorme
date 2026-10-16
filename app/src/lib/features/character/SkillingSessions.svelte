<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import LooterBasisSelect from '$lib/components/LooterBasisSelect.svelte';
	import Select from '$lib/components/Select.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import { looterLevelsFrom, setLooterBasis } from '$lib/looterBasis.svelte';
	import type { ProfessionLevel } from '$lib/types/analytics';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';
	import { targetLabel } from './codexRankingTarget';
	import SkillingBar from './SkillingBar.svelte';
	import SkillingPanel from './SkillingPanel.svelte';
	import SkillingPremium from './SkillingPremium.svelte';
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
	import { sessionRows } from './sessionsView';

	// Your named sessions carried to the goal as you recorded them: their
	// own pace, loot return, markup, and the whole spread of skills they
	// train, not only the ones the goal counts. Narrowed to one weapon, each
	// is only that weapon's part of it, so a session played with several
	// does not blend them. Each loss is shown twice: expected from its
	// weapons, which the premium compares, and as recorded.
	let { hub, professions }: { hub: SkillingModel; professions: ProfessionLevel[] } = $props();
	const looterLevels = $derived(looterLevelsFrom(professions));

	const name = $derived(targetLabel(hub.target));
	const isHp = $derived(hub.target.kind === 'hp');
	const source = $derived(hub.selectedSource);
	const rows = $derived(sessionRows(hub.sources));
	const skills = $derived(
		(source?.skills ?? [])
			.filter((skill) => skill.levelGain > 0)
			.sort((a, b) => (b.pesShare ?? 0) - (a.pesShare ?? 0)),
	);
	const topShare = $derived(Math.max(0, ...skills.map((skill) => skill.pesShare ?? 0)));
	// A gain below the shown precision reads as nothing toward the goal.
	const minorGain = $derived(isHp ? 0.005 : 0.0005);
	const SHOWN_SKILLS = 12;
	let showAll = $state(false);

	const COL_FIGURE = 'px-3 py-2.5 text-right tabular-nums';
</script>

<SkillingPanel id="skilling-sessions" label="Sessions" busy={hub.forecastLoading}>
	{#snippet actions()}
		{#if hub.goalActive && hub.sessionWeapons.length > 0}
			<label class="flex items-center gap-2">
				<span class="eyebrow">Weapon</span>
				<Select
					class="w-64"
					aria-label="Weapon to narrow the sessions to"
					value={hub.sessionWeapon ?? ''}
					onchange={(event) => hub.setSessionWeapon(event.currentTarget.value || null)}
				>
					<option value="">All weapons</option>
					{#each hub.sessionWeapons as weapon (weapon.name)}
						<option value={weapon.name}>{weapon.name}</option>
					{/each}
					{#if hub.sessionWeapon && !hub.sessionWeapons.some((weapon) => weapon.name === hub.sessionWeapon)}
						<option value={hub.sessionWeapon}>{hub.sessionWeapon}</option>
					{/if}
				</Select>
			</label>
		{/if}
	{/snippet}

	{#if !hub.goalActive}
		<p class="text-sm text-text-tertiary">Set a goal above where you stand.</p>
	{:else if hub.forecastError}
		<ErrorNotice message={hub.forecastError} />
	{:else if hub.forecastLoading && !hub.forecast}
		<div class="space-y-3">
			{#each { length: 4 } as _}
				<Skeleton class="h-5 w-full" />
			{/each}
		</div>
	{:else if hub.forecast && hub.sources.length === 0}
		<p class="text-sm text-text-tertiary">
			{hub.sessionWeapon ? `No named session used ${hub.sessionWeapon}.` : 'No named sessions recorded yet.'}
		</p>
	{:else}
		<div class="overflow-x-auto">
			<table class="w-full text-sm">
				<thead>
					<tr class="border-b border-border/60">
						<th class="py-2 pr-3 text-left eyebrow">Session</th>
						<th class="px-3 py-2 text-right eyebrow">Cycled</th>
						<th class="px-3 py-2 text-right eyebrow">Expected loss</th>
						<th class="px-3 py-2 text-right eyebrow">Recorded loss</th>
						<th class="px-3 py-2 text-right eyebrow">After markup</th>
						<th class="px-3 py-2 text-right eyebrow">Time</th>
						<th class="px-3 py-2 text-right eyebrow">Kills</th>
						<th class="py-2 pl-3 text-right eyebrow">Over cheapest</th>
					</tr>
				</thead>
				<tbody>
					{#each rows as { source: row, premium, premiumPerHour } (row.definitionId)}
						{@const current = row.definitionId === source?.definitionId}
						<tr
							class="cursor-pointer border-b border-border/30 transition-colors hover:bg-surface-hover/40
								{current ? 'bg-accent/[0.06]' : ''}"
							aria-selected={current}
							onclick={() => hub.selectSource(row.definitionId)}
						>
							<td class="py-2.5 pr-3 {current ? 'text-accent' : 'text-text'}">
								{row.name}
								{#if row.status === 'ready' && row.warnings.length > 0}
									<span class="ml-1.5 text-xs text-warning">thin sample</span>
								{/if}
							</td>
							{#if row.status !== 'ready'}
								<td colspan="7" class="py-2.5 pl-3 text-right text-xs text-text-tertiary">
									{statusMessage(row.status, row.name, name)}
								</td>
							{:else}
								<td class="{COL_FIGURE} text-text-secondary">{formatRoutePed(row.cycledPed)}</td>
								<td class="{COL_FIGURE} font-medium text-text">
									{row.expected !== null ? formatRoutePed(row.expected.ttCost) : NO_DATA}
								</td>
								<td class="{COL_FIGURE} text-text-secondary">{formatRoutePed(row.ttCost)}</td>
								<td class="{COL_FIGURE} text-text-secondary">
									{row.netCost !== null ? formatRoutePed(row.netCost) : NO_DATA}
								</td>
								<td class="{COL_FIGURE} text-text-secondary">{formatHours(row.hours)}</td>
								<td class="{COL_FIGURE} text-text-secondary">
									{row.kills !== null ? formatKills(row.kills) : NO_DATA}
								</td>
								<td class="whitespace-nowrap py-2.5 pl-3 text-right tabular-nums">
									<SkillingPremium premium={premium} perHour={premiumPerHour} />
								</td>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		{#if source && source.status === 'ready'}
			<div class="mt-8 border-t border-border/40 pt-6" aria-label="{source.name} in detail">
				<p class="mb-2 text-sm font-medium text-text">{source.name}</p>

				<div class="flex flex-wrap items-center gap-x-1.5 gap-y-1 text-xs tabular-nums text-text-tertiary">
					<span>
						From {source.sample.sessions}
						{source.sample.sessions === 1 ? 'session' : 'sessions'}, {formatHours(source.sample.hours)},
						{formatPed(source.sample.cycledPed)} PED cycled
					</span>
					<span aria-hidden="true">·</span>
					<span class="text-text-secondary">
						{formatPercent(source.sample.lootTt / source.sample.cycledPed)} loot return
					</span>
					<span aria-hidden="true">·</span>
					{#if source.expected !== null}
						<span class="inline-flex items-center gap-1 text-text-secondary">
							<span>
								{formatPercent(source.expected.weaponReturn)} expected on
								{source.expected.coverage < 0.995 ? `${Math.round(source.expected.coverage * 100)}% of` : ''}
								weapon spend at
								<LooterBasisSelect
									levels={looterLevels}
									onchange={(basis) => {
										void setLooterBasis(basis);
										hub.reloadForecast();
									}}
								/>
							</span>
							<InfoTip label="About the expected loss" width="w-80">
								<p class="text-xs font-semibold leading-relaxed text-text">Community Model v1</p>
								<p class="mt-1 text-xs leading-relaxed text-text-secondary">
									The loot return community research expects from these weapons' Efficiency at the looter
									level chosen, a planning estimate rather than game truth. Spend it does not cover, such
									as armour and healing, counts as lost.
								</p>
								<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
									A recorded run usually trails it until it meets the rare large loot events the long-run
									average includes, so sessions are compared on their expected loss.
								</p>
							</InfoTip>
						</span>
					{:else}
						<span>no expected return modelled</span>
					{/if}
					<span aria-hidden="true">·</span>
					<span class="text-text-secondary">
						{#if source.sample.markupLift !== null}
							{formatLift(source.sample.markupLift)} markup realised from sales
						{:else}
							no loot sales recorded
						{/if}
					</span>
					{#if source.warnings.length > 0}
						<span aria-hidden="true">·</span>
						<span class="text-warning">
							rough: {source.warnings.map((warning) => warningText(warning, source.sample)).join('; ')}
						</span>
					{/if}
				</div>

				{#if skills.length > 0}
					<div class="mt-7 overflow-x-auto">
						<table class="w-full text-sm">
							<thead>
								<tr class="border-b border-border/60">
									<th class="py-2 pr-3 text-left eyebrow">Skill</th>
									<th class="w-48 px-3 py-2 text-left eyebrow">Share of PES</th>
									<th class="px-3 py-2 text-right eyebrow">Level</th>
									<th class="py-2 pl-3 text-right eyebrow">Toward {isHp ? 'HP' : name}</th>
								</tr>
							</thead>
							<tbody>
								{#each showAll ? skills : skills.slice(0, SHOWN_SKILLS) as skill (skill.name)}
									{@const counts = skill.movesTarget && skill.targetGain >= minorGain}
									<tr class="border-b border-border/30 transition-colors hover:bg-surface-hover/40">
										<td class="py-2.5 pr-3 {counts ? 'text-text' : 'text-text-secondary'}">{skill.name}</td>
										<td class="px-3 py-2.5">
											{#if skill.pesShare !== null}
												<div class="flex items-center gap-2.5">
													<SkillingBar
														fraction={topShare > 0 ? skill.pesShare / topShare : 0}
														tone={counts ? 'accent' : 'muted'}
														class="flex-1"
													/>
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
										<td class="whitespace-nowrap py-2.5 pl-3 text-right tabular-nums">
											{#if counts}
												<span class="font-medium text-accent">{formatTargetGain(hub.target, skill.targetGain)}</span>
											{:else}
												<span class="text-text-tertiary">{NO_DATA}</span>
											{/if}
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
					{#if skills.length > SHOWN_SKILLS}
						<button type="button" class="linklet mt-3" onclick={() => (showAll = !showAll)}>
							{showAll ? 'Show fewer' : `Show all ${skills.length} skills`}
						</button>
					{/if}
				{/if}
			</div>
		{/if}
	{/if}
</SkillingPanel>
