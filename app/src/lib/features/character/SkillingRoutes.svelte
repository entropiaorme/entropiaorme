<script lang="ts">
	import { tick } from 'svelte';
	import type { EquipmentSearchResult, ManualMobSuggestion } from '$lib/api';
	import { getManualMobSuggestions, searchEquipmentItems } from '$lib/api';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import Input from '$lib/components/Input.svelte';
	import LooterBasisSelect from '$lib/components/LooterBasisSelect.svelte';
	import PickerInput from '$lib/components/PickerInput.svelte';
	import SearchHighlight from '$lib/components/SearchHighlight.svelte';
	import { looterLevelsFrom, setLooterBasis } from '$lib/looterBasis.svelte';
	import type { ProfessionLevel } from '$lib/types/analytics';
	import { NO_DATA } from '$lib/utils/format';
	import { createTypeahead } from '$lib/view/typeahead.svelte';
	import { targetLabel } from './codexRankingTarget';
	import { routeStatusLabel } from './routesView';
	import SkillingPanel from './SkillingPanel.svelte';
	import SkillingPremium from './SkillingPremium.svelte';
	import SkillingRouteDetail from './SkillingRouteDetail.svelte';
	import {
		formatHours,
		formatKills,
		formatOverkill,
		formatRoutePed,
		formatRoutePercent,
	} from './skillingFormat';
	import { MAX_SKILL_BOOST, type SkillingModel } from './skillingModel.svelte';

	// Skilling methods compared: each row a weapon on a mob you set up in
	// place, priced to the goal before any of it is played. The premium over
	// the cheapest is what a more comfortable method costs. Once any route
	// expects its loot to sell above TT, routes compare on the loss after
	// that markup.
	let { hub, professions }: { hub: SkillingModel; professions: ProfessionLevel[] } = $props();
	const looterLevels = $derived(looterLevelsFrom(professions));
	const showMarkup = $derived(hub.routes.some((route) => route.lootMarkupPercent != null));
	const figureColumns = $derived(showMarkup ? 7 : 6);


	const weaponPicker = () =>
		createTypeahead<EquipmentSearchResult>({
			search: (query) => searchEquipmentItems(query, 'weapon'),
			labelOf: (item) => item.name,
		});
	const mobPicker = () =>
		createTypeahead<ManualMobSuggestion>({
			search: (query) => getManualMobSuggestions(query),
			labelOf: (item) => item.display,
		});

	// The trailing blank row: a route is added once both halves are picked.
	const newWeapon = weaponPicker();
	const newMob = mobPicker();
	// One cell of an existing row being changed.
	let editing = $state<{ index: number; field: 'weapon' | 'mob' } | null>(null);
	const editWeapon = weaponPicker();
	const editMob = mobPicker();

	$effect(() => () => {
		for (const picker of [newWeapon, newMob, editWeapon, editMob]) picker.destroy();
	});

	$effect(() => {
		const weapon = newWeapon.selected;
		const mob = newMob.selected;
		if (!weapon?.catalogId || !mob) return;
		hub.addRoute({
			weaponId: weapon.catalogId,
			weaponName: weapon.name,
			species: mob.species,
			maturity: mob.maturity,
			display: mob.display,
		});
		newWeapon.clear();
		newMob.clear();
	});

	$effect(() => {
		const weapon = editWeapon.selected;
		if (!editing || editing.field !== 'weapon' || !weapon?.catalogId) return;
		const stored = hub.routes[editing.index];
		if (stored) {
			hub.updateRoute(editing.index, {
				...stored,
				weaponId: weapon.catalogId,
				weaponName: weapon.name,
			});
		}
		editing = null;
		editWeapon.clear();
	});

	$effect(() => {
		const mob = editMob.selected;
		if (!editing || editing.field !== 'mob' || !mob) return;
		const stored = hub.routes[editing.index];
		if (stored) {
			hub.updateRoute(editing.index, {
				...stored,
				species: mob.species,
				maturity: mob.maturity,
				display: mob.display,
			});
		}
		editing = null;
		editMob.clear();
	});

	async function edit(index: number, field: 'weapon' | 'mob') {
		editWeapon.clear();
		editMob.clear();
		editing = { index, field };
		await tick();
		document.getElementById(`skilling-route-edit-${field}`)?.focus();
	}

	const COL_FIGURE = 'px-3 py-2.5 text-right tabular-nums';
</script>

{#snippet weaponResult({ item, query }: { item: EquipmentSearchResult; query: string })}
	<span class="truncate text-text"><SearchHighlight text={item.name} {query} /></span>
{/snippet}
{#snippet weaponSelection({ item }: { item: EquipmentSearchResult })}
	<span class="flex-1 truncate text-text">{item.name}</span>
{/snippet}
{#snippet mobResult({ item, query }: { item: ManualMobSuggestion; query: string })}
	<span class="truncate text-text"><SearchHighlight text={item.display} {query} /></span>
{/snippet}
{#snippet mobSelection({ item }: { item: ManualMobSuggestion })}
	<span class="flex-1 truncate text-text">{item.display}</span>
{/snippet}

<SkillingPanel id="skilling-routes" label="Routes" busy={hub.forecastLoading}>
	{#snippet actions()}
		{#if hub.goalActive && hub.huntingTarget}
			<div class="flex items-center gap-1.5">
				<label class="flex items-center gap-2">
					<span class="eyebrow">Skill boost</span>
					<Input
						class="w-16"
						align="right"
						type="number"
						min="0"
						max={MAX_SKILL_BOOST}
						step="1"
						inputmode="numeric"
						aria-label="Skill boost percent"
						value={hub.boostInput}
						oninput={(event) => hub.setSkillBoost(event.currentTarget.value)}
						onblur={hub.commitSkillBoost}
						onkeydown={(event) => {
							if (event.key === 'Enter') hub.commitSkillBoost();
						}}
					/>
					<span class="text-xs text-text-tertiary">%</span>
				</label>
				<InfoTip label="About the skill boost" width="w-80">
					<p class="text-xs font-semibold leading-relaxed text-text">Capped at 100%</p>
					<p class="mt-1 text-xs leading-relaxed text-text-secondary">
						Stacking skill pills and other boosts can show a total above 100% in the game, but that is a
						display bug: the game never applies more than 100%. A figure above 100 here prices at 100.
					</p>
					<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
						Skill per kill was measured with no boost, at 50% and at 100%; a boost in between is read
						between those measurements.
					</p>
				</InfoTip>
			</div>
			<span class="text-xs">
				<LooterBasisSelect
					levels={looterLevels}
					onchange={(basis) => {
						void setLooterBasis(basis);
						hub.reloadForecast();
					}}
				/>
			</span>
		{/if}
	{/snippet}

	{#if !hub.goalActive}
		<p class="text-sm text-text-tertiary">Set a goal above where you stand.</p>
	{:else if hub.forecastError}
		<ErrorNotice message={hub.forecastError} />
	{:else if !hub.huntingTarget}
		<p class="max-w-2xl text-sm leading-relaxed text-text-tertiary">
			Hunting trains {targetLabel(hub.target)} only as a byproduct of a weapon's own professions, so
			Routes does not price it. Activities shows what trains it, and Sessions what your recorded play
			has done for it.
		</p>
	{:else}
		<div class="overflow-x-auto">
			<table class="w-full text-sm">
				<thead>
					<tr class="border-b border-border/60">
						<th class="py-2 pr-3 text-left eyebrow">Weapon</th>
						<th class="px-3 py-2 text-left eyebrow">Mob</th>
						<th class="px-3 py-2 text-right eyebrow">Cycled</th>
						<th class="px-3 py-2 text-right eyebrow">TT lost</th>
						{#if showMarkup}
							<th class="px-3 py-2 text-right eyebrow">
								<span class="inline-flex items-center justify-end gap-1.5">
									After markup
									<InfoTip label="About the expected markup" width="w-80">
										<p class="text-xs leading-relaxed text-text-secondary">
											TT lost less the markup you expect the route's loot to sell at, which you set in a
											route's detail. Routes without one stand at TT. Below zero, the markup outweighs the
											loss.
										</p>
										<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
											It is your planning assumption, not a measurement: markup is only realised when the
											loot sells, after auction fees, and heavy selling can push it down.
										</p>
									</InfoTip>
								</span>
							</th>
						{/if}
						<th class="px-3 py-2 text-right eyebrow">Time</th>
						<th class="px-3 py-2 text-right eyebrow">Kills</th>
						<th class="px-3 py-2 text-right eyebrow">
							<span class="inline-flex items-center justify-end gap-1.5">
								Overkill
								<InfoTip label="About overkill" width="w-80">
									<p class="text-xs leading-relaxed text-text-secondary">
										The share of the damage fired that lands past the kill, and so of the TT lost and the
										firing time. Beneath it, what that comes to over the route, after any expected markup:
										the most a weapon that fits the mob better could save.
									</p>
									<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
										It is a ceiling, not a promise: a lighter weapon often deals less damage per PEC or
										returns less, and even a perfect fit overshoots a little on the last hit.
									</p>
								</InfoTip>
							</span>
						</th>
						<th class="py-2 pl-3 text-right eyebrow">Over cheapest</th>
						<th class="w-6"><span class="sr-only">Remove</span></th>
					</tr>
				</thead>
				<tbody>
					{#each hub.rows as row (row.key)}
						{@const current = row.key === hub.selectedRow?.key}
						{@const route = row.route}
						<tr
							class="cursor-pointer border-b border-border/30 transition-colors hover:bg-surface-hover/40
								{current ? 'bg-accent/[0.06]' : ''}"
							aria-selected={current}
							onclick={() => hub.selectRow(row.key)}
						>
							<td class="py-2 pr-3">
								{#if editing?.index === row.index && editing.field === 'weapon'}
									<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
									<div onclick={(event) => event.stopPropagation()}>
										<PickerInput
											id="skilling-route-edit-weapon"
											model={editWeapon}
											placeholder="Search weapons"
											result={weaponResult}
											selection={weaponSelection}
											class="min-w-48"
										/>
									</div>
								{:else}
									<button
										type="button"
										class="max-w-56 cursor-pointer truncate text-left hover:text-accent
											{current ? 'text-accent' : 'text-text'}"
										title="Change {row.stored.weaponName}"
										onclick={(event) => {
											event.stopPropagation();
											void edit(row.index, 'weapon');
										}}
									>
										{row.stored.weaponName}
									</button>
								{/if}
							</td>
							<td class="px-3 py-2">
								{#if editing?.index === row.index && editing.field === 'mob'}
									<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
									<div onclick={(event) => event.stopPropagation()}>
										<PickerInput
											id="skilling-route-edit-mob"
											model={editMob}
											placeholder="Search mobs"
											result={mobResult}
											selection={mobSelection}
											class="min-w-44"
										/>
									</div>
								{:else}
									<button
										type="button"
										class="max-w-56 cursor-pointer truncate text-left text-text hover:text-accent"
										title="Change {row.stored.display}"
										onclick={(event) => {
											event.stopPropagation();
											void edit(row.index, 'mob');
										}}
									>
										{row.stored.display}
									</button>
								{/if}
							</td>
							{#if route === null}
								<td colspan={figureColumns} class="py-2.5 pl-3 text-right text-xs text-text-tertiary">
									{hub.forecastLoading ? 'Pricing…' : NO_DATA}
								</td>
							{:else if route.status !== 'ready'}
								<td colspan={figureColumns} class="py-2.5 pl-3 text-right text-xs text-text-tertiary">
									{routeStatusLabel(route.status)}
								</td>
							{:else}
								<td class="{COL_FIGURE} text-text-secondary">{formatRoutePed(route.cycledPed)}</td>
								<td class="{COL_FIGURE} {showMarkup ? 'text-text-secondary' : 'font-medium text-text'}">
									{formatRoutePed(route.pedLost)}
								</td>
								{#if showMarkup}
									<td class="{COL_FIGURE} whitespace-nowrap">
										{#if route.netCost === null}
											<span class="text-text-tertiary" title="No markup set: at TT">
												{formatRoutePed(route.pedLost)}
											</span>
										{:else if route.netCost < 0}
											<span class="font-medium text-success">{formatRoutePed(-route.netCost)} gained</span>
										{:else}
											<span class="font-medium text-text">{formatRoutePed(route.netCost)}</span>
										{/if}
										{#if row.stored.lootMarkupPercent != null}
											<span class="block text-xs text-text-tertiary">
												at {formatRoutePercent(row.stored.lootMarkupPercent)}
											</span>
										{/if}
									</td>
								{/if}
								<td class="{COL_FIGURE} whitespace-nowrap text-text-secondary">
									{route.hours !== null ? formatHours(route.hours) : NO_DATA}
									{#if row.stored.huntingSharePercent != null && row.stored.huntingSharePercent < 100}
										<span class="block text-xs text-text-tertiary">
											{formatRoutePercent(row.stored.huntingSharePercent)} hunting
										</span>
									{/if}
								</td>
								<td class="{COL_FIGURE} text-text-secondary">{formatKills(route.kills)}</td>
								<td class="{COL_FIGURE} whitespace-nowrap text-text-secondary">
									{#if row.overkill}
										{formatOverkill(row.overkill.share)}
										{#if row.overkill.kind !== 'no_cost'}
											<span class="block text-xs text-text-tertiary">
												up to {formatRoutePed(row.overkill.ped)} PED
											</span>
										{/if}
									{:else}
										{NO_DATA}
									{/if}
								</td>
								<td class="whitespace-nowrap py-2.5 pl-3 text-right tabular-nums">
									<SkillingPremium premium={row.premium} perHour={row.premiumPerHour} />
								</td>
							{/if}
							<td class="py-2.5 pl-2 text-right">
								<button
									type="button"
									class="cursor-pointer text-text-tertiary hover:text-text"
									aria-label="Remove {row.stored.weaponName} on {row.stored.display}"
									title="Remove"
									onclick={(event) => {
										event.stopPropagation();
										hub.removeRoute(row.index);
									}}
								>
									<svg width="8" height="8" viewBox="0 0 10 10" aria-hidden="true">
										<path d="M1 1 L9 9 M9 1 L1 9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
									</svg>
								</button>
							</td>
						</tr>
					{/each}
					<tr>
						<td class="py-2 pr-3">
							<PickerInput
								id="skilling-route-new-weapon"
								model={newWeapon}
								placeholder="Add a weapon"
								result={weaponResult}
								selection={weaponSelection}
								class="min-w-48"
							/>
						</td>
						<td class="px-3 py-2">
							<PickerInput
								id="skilling-route-new-mob"
								model={newMob}
								placeholder="Add a mob"
								result={mobResult}
								selection={mobSelection}
								class="min-w-44"
							/>
						</td>
						<td colspan={figureColumns + 1}></td>
					</tr>
				</tbody>
			</table>
		</div>

		{#if hub.selectedRow?.route}
			<SkillingRouteDetail {hub} row={hub.selectedRow} route={hub.selectedRow.route} />
		{/if}
	{/if}
</SkillingPanel>
