<script lang="ts">
	import type { SkillBoostLevel } from '$lib/api/commands.gen';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import LooterBasisSelect from '$lib/components/LooterBasisSelect.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import { looterLevelsFrom, setLooterBasis } from '$lib/looterBasis.svelte';
	import { NO_DATA } from '$lib/utils/format';
	import AddRouteMenu from './AddRouteMenu.svelte';
	import { routeStatusLabel } from './routesView';
	import SkillingRouteDetail from './SkillingRouteDetail.svelte';
	import SkillingSection from './SkillingSection.svelte';
	import { formatHours, formatRoutePed } from './skillingFormat';
	import type { SkillingModel } from './skillingModel.svelte';
	import type { ProfessionLevel } from '$lib/types/analytics';

	// Every way to the goal, priced: planned routes (a weapon on a mob,
	// costed from the catalogue before any of it is played) beside your
	// named sessions (costed from what they recorded). The premium over the
	// cheapest is what a more comfortable route costs.
	let { hub, professions }: { hub: SkillingModel; professions: ProfessionLevel[] } = $props();
	const looterLevels = $derived(looterLevelsFrom(professions));
	const anyPlanned = $derived(hub.rows.some((row) => row.kind === 'planned'));

	const PILLS: { id: SkillBoostLevel; label: string; ariaLabel: string }[] = [
		{ id: 'none', label: 'No pill', ariaLabel: 'Price planned routes without a skill pill' },
		{ id: 'fifty', label: '50%', ariaLabel: 'Price planned routes at the 50% skill pill' },
		{ id: 'hundred', label: '100%', ariaLabel: 'Price planned routes at the 100% skill pill' },
	];
</script>

<SkillingSection id="skilling-routes" title="Routes" busy={hub.forecastLoading}>
	{#snippet info()}
		<p class="text-xs font-semibold leading-relaxed text-text">What each way to the goal costs</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			A planned route is a weapon on a mob, priced before you play it: each kill yields skill by the
			mob's level and HP, whatever the weapon, and costs what the weapon spends killing it. Your named
			sessions are priced from what they recorded. "Over cheapest" is the price of choosing a route
			over the cheapest one, in total and per hour of play.
		</p>
	{/snippet}

	{#snippet actions()}
		{#if hub.goalActive}
			<SegmentedControl
				options={PILLS}
				active={hub.skillBoost ?? ''}
				onchange={(id) => hub.setSkillBoost(id as SkillBoostLevel)}
			/>
			<AddRouteMenu onadd={hub.addRoute} />
		{/if}
	{/snippet}

	{#if !hub.goalActive}
		<p class="text-sm text-text-tertiary">Set a goal above where you stand.</p>
	{:else if hub.forecastError}
		<ErrorNotice message={hub.forecastError} />
	{:else if hub.forecastLoading && hub.rows.length === 0}
		<div class="space-y-3">
			{#each { length: 3 } as _}
				<Skeleton class="h-5 w-full" />
			{/each}
		</div>
	{:else if hub.rows.length === 0}
		<p class="text-sm text-text-tertiary">
			Add a route (a weapon on a mob) to price this goal, or record a named session.
		</p>
	{:else}
		<div class="overflow-x-auto">
			<table class="w-full text-sm">
				<thead>
					<tr class="border-b border-border/60">
						<th class="py-2 pr-3 text-left eyebrow">Route</th>
						<th class="px-3 py-2 text-right eyebrow">PED lost</th>
						<th class="px-3 py-2 text-right eyebrow">Time</th>
						<th class="px-3 py-2 text-right eyebrow">Cycled</th>
						<th class="py-2 pl-3 text-right eyebrow">Over cheapest</th>
						<th class="w-6"><span class="sr-only">Remove</span></th>
					</tr>
				</thead>
				<tbody>
					{#each hub.rows as row (row.key)}
						{@const current = row.key === hub.selectedRow?.key}
						{@const ready = row.status === 'ready'}
						<tr
							class="border-b border-border/30 transition-colors hover:bg-surface-hover/40
								{current ? 'bg-accent/[0.06]' : ''}"
						>
							<td class="py-2.5 pr-3">
								<button
									type="button"
									class="flex max-w-full cursor-pointer items-center gap-2 text-left
										{current ? 'text-accent' : ready ? 'text-text' : 'text-text-tertiary'}"
									aria-pressed={current}
									onclick={() => hub.selectRow(row.key)}
								>
									<span class="truncate" title={row.name}>{row.name}</span>
									{#if row.kind === 'recorded'}
										<span class="shrink-0 text-[0.625rem] font-medium uppercase tracking-wide text-text-tertiary">
											Recorded
										</span>
									{:else if row.route.measured}
										<span
											class="shrink-0 text-[0.625rem] font-medium uppercase tracking-wide text-text-tertiary"
											title="Priced from your own kills"
										>
											Measured
										</span>
									{/if}
								</button>
							</td>
							{#if ready}
								<td class="px-3 py-2.5 text-right font-medium tabular-nums text-text">
									{formatRoutePed(row.pedLost)}
								</td>
								<td class="px-3 py-2.5 text-right tabular-nums text-text-secondary">
									{row.hours !== null ? formatHours(row.hours) : NO_DATA}
								</td>
								<td class="px-3 py-2.5 text-right tabular-nums text-text-secondary">
									{formatRoutePed(row.cycledPed)}
								</td>
								<td class="whitespace-nowrap py-2.5 pl-3 text-right tabular-nums">
									{#if row.premium === null}
										<span class="text-text-tertiary">{NO_DATA}</span>
									{:else if row.premium <= 0}
										<span class="text-success">Cheapest</span>
									{:else}
										<span class="text-text">+{formatRoutePed(row.premium)}</span>
										{#if row.premiumPerHour !== null}
											<span class="ml-1 text-xs text-text-tertiary">
												{formatRoutePed(row.premiumPerHour)}/h
											</span>
										{/if}
									{/if}
								</td>
							{:else}
								<td colspan="4" class="py-2.5 pl-3 text-right text-xs text-text-tertiary">
									{routeStatusLabel(row.status)}
								</td>
							{/if}
							<td class="py-2.5 pl-2 text-right">
								{#if row.kind === 'planned'}
									<button
										type="button"
										class="cursor-pointer text-text-tertiary hover:text-text"
										aria-label="Remove {row.name}"
										title="Remove"
										onclick={() => hub.removeRoute(row.index)}
									>
										<svg width="8" height="8" viewBox="0 0 10 10" aria-hidden="true">
											<path d="M1 1 L9 9 M9 1 L1 9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
										</svg>
									</button>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		{#if anyPlanned}
			<!-- The caption names the planned figures' model, pill, and looter basis; the basis is the control. -->
			<p class="mt-2 text-[11px] leading-relaxed text-text-tertiary">
				Planned routes: community model v1 ·
				<LooterBasisSelect
					levels={looterLevels}
					onchange={(basis) => {
						void setLooterBasis(basis);
						hub.reloadForecast();
					}}
				/>
			</p>
		{/if}

		{#if hub.selectedRow}
			<SkillingRouteDetail {hub} row={hub.selectedRow} />
		{/if}
	{/if}
</SkillingSection>
