<script lang="ts">
	import type { ComparedSetup } from '$lib/api/comparison';
	import EffectiveEfficiencyInfoTip from '$lib/components/EffectiveEfficiencyInfoTip.svelte';
	import { NO_DATA } from '$lib/utils/format';
	import { formatPec } from '../display';
	import SetupCell from './SetupCell.svelte';
	import type { Comparator, SetupDraft } from './compareModel.svelte';
	import {
		type PlanFigures,
		type Payback,
		formatCompact,
		formatKillPec,
		isLimitedSetup,
		payback,
		planFigures,
		setupLabels,
		partsUsedUp,
		describeUnmodelled,
		formatCount,
		formatDuration,
		formatPed,
	} from './comparisonView';

	/**
	 * The compared setups: the chart's legend, its figures, and where each
	 * setup is edited, in one table. Rows keep the order they were added in;
	 * the trailing empty row adds the next one.
	 */
	let { comparator }: { comparator: Comparator } = $props();

	const ROLE_NOUN = {
		weapon: 'weapons',
		amplifier: 'amps',
		absorber: 'absorbers',
		implant: 'implants',
	} as const;

	type Row = { draft: SetupDraft; setup: ComparedSetup | null };
	const rows = $derived<Row[]>(
		comparator.drafts.map((draft) => ({ draft, setup: comparator.figuresFor(draft.id) })),
	);
	// The empty row's weapon pick lists it and opens the next empty row.
	$effect(() => {
		if (comparator.blank?.pickers.weapon.selected) comparator.commitBlank();
	});

	const columns = [
		{ key: 'dpp', label: 'Damage / PEC' },
		{ key: 'effective', label: 'Eff. Efficiency' },
		{ key: 'cycled', label: 'PED cycled' },
		{ key: 'lost', label: 'Expected loss' },
		{ key: 'time', label: 'Time' },
	] as const;

	// Every row's answers for the kill count and the pinned mob HP.
	const answers = $derived(
		new Map(
			rows.map((row) => [
				row.draft.id,
				row.setup ? planFigures(row.setup, comparator.killCount, comparator.pinnedHp) : null,
			]),
		),
	);
	// The least PED cycled among the setups: the others show how much more.
	const cycledMin = $derived.by(() => {
		const values = [...answers.values()].flatMap((answer) => (answer ? [answer.cycledPed] : []));
		return values.length > 1 ? Math.min(...values) : null;
	});
	const lostMax = $derived(
		Math.max(0, ...[...answers.values()].map((answer) => answer?.lossPed ?? 0)),
	);

	// The best setup per figure is emphasised, so the table reads at a
	// glance without a sentence.
	type Figure = 'dpp' | 'effective' | 'lost' | 'cycled';
	function figure(row: Row, key: Figure): number | null {
		const economics = row.setup?.economics;
		if (!economics) return null;
		const answer = answers.get(row.draft.id) ?? null;
		switch (key) {
			case 'dpp':
				return economics.damagePerPec;
			case 'effective':
				return economics.effectiveEfficiency?.status === 'within_model_range'
					? economics.effectiveEfficiency.efficiencyPct
					: null;
			case 'lost':
				return answer?.lossPed ?? null;
			case 'cycled':
				return answer?.cycledPed ?? null;
		}
	}
	const lowerIsBetter: Record<Figure, boolean> = {
		dpp: false,
		effective: false,
		lost: true,
		cycled: true,
	};
	function isBest(row: Row, key: Figure): boolean {
		const values = rows.flatMap((other) => {
			const value = figure(other, key);
			return value === null ? [] : [value];
		});
		const own = figure(row, key);
		if (own === null || values.length < 2) return false;
		if (values.every((value) => value === values[0])) return false;
		const best = lowerIsBetter[key] ? Math.min(...values) : Math.max(...values);
		return own === best;
	}
	const cell = $derived(
		`whitespace-nowrap px-3 py-2.5 text-right tabular-nums transition-opacity ${
			comparator.current ? '' : 'opacity-50'
		}`,
	);
	const tone = (row: Row, key: Figure) =>
		isBest(row, key) ? 'font-semibold text-text' : 'text-text-secondary';

	// Capital: an unlimited setup's markup is weighed against the best
	// limited setup, or the one picked, at a yearly rate.
	const labels = $derived(comparator.comparison ? setupLabels(comparator.comparison) : []);
	const showPayback = $derived(rows.some((row) => row.draft.capitalPed > 0));
	const limitedRows = $derived(rows.filter((row) => row.setup && isLimitedSetup(row.setup)));
	const referenceRow = $derived(
		rows.find((row) => row.draft.id === comparator.referenceDraftId) ?? null,
	);
	const labelOf = (row: Row) => (row.setup ? (labels[row.setup.index] ?? row.setup.name) : '');
	function paybackOf(row: Row): Payback | null {
		const reference = referenceRow;
		if (!row.setup || !reference?.setup || reference === row || row.draft.capitalPed <= 0)
			return null;
		return payback(
			row.setup,
			reference.setup,
			comparator.pinnedHp,
			row.draft.capitalPed,
			comparator.capitalRate,
		);
	}
	function paybackTitle(row: Row, result: Payback): string {
		const rate = Math.round(comparator.capitalRate * 1000) / 10;
		const needs = `Owning ties up ${formatPed(row.draft.capitalPed)} PED; at ${rate}% that is ${formatPed(result.capitalCostPed)} PED a year it needs to save to be worth holding.`;
		const against = referenceRow ? labelOf(referenceRow) : 'the reference';
		const amount = comparator.killCount;
		const volume = `${formatCount(amount)} mobs a year`;
		if (result.kind === 'never') {
			return `${needs} It loses at least as much as ${against} per kill, so no amount of hunting saves toward it.`;
		}
		const perUnit = `${formatKillPec(result.saving * 100)} PEC per kill`;
		const saved = result.saving * amount;
		const gap = result.capitalCostPed - saved;
		const verdict =
			gap > 0
				? `${formatPed(gap)} PED short; it takes ${formatCompact(result.perYear)} a year to cover.`
				: `that covers it with ${formatPed(-gap)} PED to spare.`;
		return `${needs} Against ${against} it saves ${perUnit}: at ${volume}, ${formatPed(saved)} PED, ${verdict}`;
	}

	function lostTitle(answer: PlanFigures): string {
		const looter = comparator.comparison?.assumptions.looterLevel;
		return [
			`House edge${looter === undefined ? '' : ` at looter level ${looter.toFixed(1)}`} ${formatPed(answer.houseEdgePed)} PED`,
			answer.cycledPed > 0
				? `Gear leak ${formatPed((answer.leakPed / answer.cycledPed) * 100)} PED per 100 PED cycled`
				: null,
			`Markup ${formatPed(answer.markupPed)} PED`,
			`Efficiency below 100% ${formatPed(answer.efficiencyPed)} PED`,
			answer.unmodelledPed > 0 ? `Absorber and implant decay ${formatPed(answer.unmodelledPed)} PED` : null,
		]
			.filter(Boolean)
			.join('\n');
	}

	function effectiveText(setup: ComparedSetup): string {
		const effective = setup.economics?.effectiveEfficiency;
		if (!effective) return NO_DATA;
		if (effective.status === 'below_model_range') return 'Below range';
		if (effective.status === 'above_model_range') return 'Above range';
		return effective.efficiencyPct === null ? NO_DATA : `${effective.efficiencyPct.toFixed(1)}%`;
	}
</script>

{#snippet net(result: Payback)}
	<!-- Reading the kill slider as a year's hunting: how much of the yearly
	     charge on the capital the setup's saving covers. -->
	{@const amount = comparator.killCount}
	{@const saved = Math.max(0, result.saving * amount)}
	{@const covered = result.capitalCostPed > 0 ? Math.min(1, saved / result.capitalCostPed) : 1}
	{#if result.saving < 0}
		<!-- Behind the reference: how far, so a near tie reads as one. -->
		<span class="block text-xs text-text-tertiary tabular-nums">
			At {formatCompact(amount)}/yr: costs
			<span class="text-text-secondary">{formatCompact(-result.saving * amount)} more</span>
			than the reference
		</span>
	{:else}
	<span class="block text-xs text-text-tertiary tabular-nums">
		At {formatCompact(amount)}/yr: saves
		<span class={covered >= 1 ? 'text-positive' : 'text-text-secondary'}>
			{saved > 0 ? formatCompact(saved) : 'nothing'}
		</span>
		of {formatCompact(result.capitalCostPed)} needed
	</span>
	<span class="mt-1 ml-auto block h-1 w-24 overflow-hidden rounded-full bg-border/60" aria-hidden="true">
		<span
			class="block h-full rounded-full {covered >= 1 ? 'bg-positive' : 'bg-text-tertiary'}"
			style="width: {covered * 100}%"
		></span>
	</span>
	{/if}
{/snippet}

{#snippet swatch(slot: number, faint = false)}
	<svg width="16" height="8" class="mt-2 shrink-0" aria-hidden="true">
		<line
			x1="1"
			y1="4"
			x2="15"
			y2="4"
			stroke="var(--compare-series-{slot})"
			stroke-width="2"
			stroke-linecap="round"
			stroke-dasharray={faint ? '2 3' : undefined}
		/>
	</svg>
{/snippet}

<table class="w-full text-sm">
	<thead>
		<tr class="border-b border-border/50">
			<th class="eyebrow py-2 pr-4 text-left font-normal">Setup</th>
			{#each columns as column (column.key)}
				<th class="eyebrow whitespace-nowrap px-3 py-2 text-right font-normal">{column.label}</th>
			{/each}
			{#if showPayback}
				<th class="whitespace-nowrap px-3 py-2 text-right font-normal">
					<span class="eyebrow block">Pays back at</span>
					<span class="text-[11px] text-text-tertiary">
						vs
						<select
							class="cursor-pointer appearance-none border-b border-dotted border-text-tertiary/60 bg-transparent text-text-secondary hover:text-text focus:outline-none"
							aria-label="Limited setup to weigh capital against"
							value={comparator.referenceId ?? 'best'}
							onchange={(event) => {
								const raw = event.currentTarget.value;
								comparator.referenceId = raw === 'best' ? null : Number(raw);
							}}
						>
							<option value="best">best limited</option>
							{#each limitedRows as row (row.draft.id)}
								<option value={row.draft.id}>{labelOf(row)}</option>
							{/each}
						</select>
						at
						<input
							class="w-8 border-b border-dotted border-text-tertiary/60 bg-transparent text-right text-text-secondary tabular-nums hover:text-text focus:border-accent/60 focus:outline-none"
							aria-label="Yearly rate on capital, percent"
							value={Math.round(comparator.capitalRate * 1000) / 10}
							onchange={(event) => {
								const parsed = Number(event.currentTarget.value);
								if (Number.isFinite(parsed) && parsed > 0) comparator.capitalRate = parsed / 100;
							}}
						/>%
					</span>
				</th>
			{/if}
			<th class="w-8"><span class="sr-only">Remove</span></th>
		</tr>
	</thead>
	<tbody>
		{#each rows as row (row.draft.id)}
			{@const setup = row.setup}
			{@const economics = setup?.economics ?? null}
			{@const answer = answers.get(row.draft.id) ?? null}
			<tr class="border-b border-border/30 align-top">
				<td class="py-2.5 pr-4">
					<div class="flex min-w-0 items-start gap-2">
						{@render swatch(row.draft.slot)}
						<div class="min-w-0 flex-1 space-y-1">
							<SetupCell draft={row.draft} />
							{#if setup && describeUnmodelled(setup)}
								<p class="text-xs text-warning">{describeUnmodelled(setup)}</p>
							{/if}
							{#if setup && answer}
								{@const usedUp = partsUsedUp(setup, answer.uses).filter((part) => part.count >= 0.05)}
								{#if usedUp.length > 0}
									<p class="text-xs text-text-tertiary tabular-nums">
										Uses up ≈ {usedUp
											.map((part) => `${part.count >= 100 ? formatCompact(part.count) : part.count.toFixed(1)} ${ROLE_NOUN[part.role]}`)
											.join(' · ')}
									</p>
								{/if}
							{/if}
							{#if economics && economics.ampDamageWasted > 0}
								<p class="text-xs text-text-tertiary" data-testid="amp-wasted">
									{economics.ampDamageWasted.toFixed(1)} amplifier damage above half the weapon's is paid
									for and never dealt
								</p>
							{/if}
						</div>
					</div>
				</td>
				<!-- The figures dim in place while a newer answer is computed. -->
				<td
					class="{cell} {tone(row, 'dpp')}"
					title={economics
						? `Max damage ${economics.maxDamage.toFixed(1)}, ${formatPec(economics.costPerUsePec)} PEC a use`
						: undefined}
				>
					{economics ? economics.damagePerPec.toFixed(2) : NO_DATA}
					{#if answer?.overkillShare != null && answer.overkillShare >= 0.005}
						<span class="block text-xs font-normal text-text-tertiary">
							{(answer.overkillShare * 100).toFixed(answer.overkillShare < 0.1 ? 1 : 0)}% overkill
						</span>
					{/if}
				</td>
				<td class="{cell} {tone(row, 'effective')}">
					<span class="inline-flex items-center justify-end gap-1.5">
						{setup ? effectiveText(setup) : NO_DATA}
						{#if economics && comparator.comparison}
							<EffectiveEfficiencyInfoTip
								effectiveEfficiency={economics.effectiveEfficiency}
								weightedEfficiencyPct={economics.weightedEfficiencyPct}
								consumedPremiumLabel={formatPec(economics.consumedPremiumPec)}
								looterLevel={comparator.comparison.assumptions.looterLevel}
							/>
						{/if}
					</span>
				</td>
				<td class="{cell} {tone(row, 'cycled')}">
					{answer ? `${formatPed(answer.cycledPed)} PED` : NO_DATA}
					{#if answer && cycledMin !== null && answer.cycledPed - cycledMin >= 0.5}
						<!-- What the looter level's house edge is charged on, between setups. -->
						<span class="block text-xs font-normal text-text-tertiary">
							cycles {formatCompact(answer.cycledPed - cycledMin)} more
						</span>
					{/if}
				</td>
				<td class="{cell} {tone(row, 'lost')}" title={answer ? lostTitle(answer) : undefined}>
					{#if answer}
						<span>{formatPed(answer.lossPed)} PED</span>
						<!-- Where it goes: the house edge, markup, Efficiency, and parts
						     with no modelled return. -->
						{#if lostMax > 0}
							<span class="mt-1 ml-auto flex h-1 overflow-hidden rounded-full" style="width: {Math.max(2, (answer.lossPed / lostMax) * 100)}%" aria-hidden="true">
								<span class="h-full bg-text-tertiary/25" style="flex: {answer.houseEdgePed} 0 0"></span>
								<span class="h-full bg-text-secondary" style="flex: {answer.markupPed} 0 0"></span>
								<span class="h-full bg-text-tertiary/60" style="flex: {answer.efficiencyPed} 0 0"></span>
								<span class="h-full bg-border" style="flex: {answer.unmodelledPed} 0 0"></span>
							</span>
						{/if}
					{:else}
						{NO_DATA}
					{/if}
				</td>
				<td class="{cell} text-text-secondary">
					{answer ? formatDuration(answer.seconds) : NO_DATA}
				</td>
				{#if showPayback}
					{@const result = paybackOf(row)}
					<td class="{cell} text-text-secondary" title={result ? paybackTitle(row, result) : undefined}>
						{#if result?.kind === 'pays_back'}
							<span class="block font-semibold text-text">
								{formatCompact(result.perYear)} mobs / yr
							</span>
							{@render net(result)}
						{:else if result?.kind === 'never'}
							<span class="block text-text-tertiary">Never</span>
							{@render net(result)}
						{:else if referenceRow === row}
							<span class="text-xs text-text-tertiary">reference</span>
						{:else if row.draft.capitalPed > 0 && !referenceRow}
							<span class="text-xs text-text-tertiary">Add a limited setup</span>
						{/if}
					</td>
				{/if}
				<td class="py-2.5 text-right">
					<button
						type="button"
						class="mt-0.5 inline-flex h-6 w-6 items-center justify-center rounded text-text-tertiary hover:bg-surface-hover hover:text-text cursor-pointer"
						aria-label="Remove {row.draft.pickers.weapon.selected?.name ?? 'this setup'}"
						title="Remove"
						onclick={() => comparator.remove(row.draft.id)}
					>
						<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
							<path d="M1 1 L9 9 M9 1 L1 9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
						</svg>
					</button>
				</td>
			</tr>
		{/each}
		{#if comparator.blank}
			<tr class="align-top">
				<td class="py-2.5 pr-4">
					<div class="flex min-w-0 items-start gap-2">
						{@render swatch(comparator.blank.slot, true)}
						<div class="min-w-0 flex-1">
							{#key comparator.blank.id}
								<SetupCell draft={comparator.blank} blank />
							{/key}
						</div>
					</div>
				</td>
				<td colspan={columns.length + (showPayback ? 2 : 1)}></td>
			</tr>
		{/if}
	</tbody>
</table>
