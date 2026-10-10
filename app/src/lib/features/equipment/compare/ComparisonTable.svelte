<script lang="ts">
	import type { ComparedSetup } from '$lib/api/comparison';
	import EffectiveEfficiencyInfoTip from '$lib/components/EffectiveEfficiencyInfoTip.svelte';
	import { NO_DATA } from '$lib/utils/format';
	import { formatPec } from '../display';
	import SetupCell from './SetupCell.svelte';
	import type { Comparator, SetupDraft } from './compareModel.svelte';
	import { describeUnmodelled, formatKillPec } from './comparisonView';

	/**
	 * The compared setups: the chart's legend, its figures, and where each
	 * setup is edited, in one table. Rows keep the order they were added in;
	 * the trailing empty row adds the next one.
	 */
	let { comparator }: { comparator: Comparator } = $props();

	type Row = { draft: SetupDraft; setup: ComparedSetup | null };
	const rows = $derived<Row[]>(
		comparator.drafts.map((draft) => ({ draft, setup: comparator.figuresFor(draft.id) })),
	);
	// The empty row's weapon pick lists it and opens the next empty row.
	$effect(() => {
		if (comparator.blank?.pickers.weapon.selected) comparator.commitBlank();
	});

	const columns = [
		{ key: 'maxDamage', label: 'Max damage' },
		{ key: 'cost', label: 'Cost / use' },
		{ key: 'dpp', label: 'Damage / PEC' },
		{ key: 'effective', label: 'Eff. Efficiency' },
		{ key: 'lossPer100', label: 'PEC lost / 100 HP' },
		{ key: 'dps', label: 'Damage / s' },
	] as const;

	// The best setup per figure is emphasised, so the table reads at a
	// glance without a sentence.
	type Figure = 'maxDamage' | 'dpp' | 'effective' | 'lossPer100' | 'dps';
	function figure(setup: ComparedSetup | null, key: Figure): number | null {
		const economics = setup?.economics;
		if (!economics) return null;
		switch (key) {
			case 'maxDamage':
				return economics.maxDamage;
			case 'dpp':
				return economics.damagePerPec;
			case 'effective':
				return economics.effectiveEfficiency?.status === 'within_model_range'
					? economics.effectiveEfficiency.efficiencyPct
					: null;
			case 'lossPer100':
				return economics.lossPerHpPec * 100;
			case 'dps':
				return economics.damagePerSecond;
		}
	}
	const lowerIsBetter: Record<Figure, boolean> = {
		maxDamage: false,
		dpp: false,
		effective: false,
		lossPer100: true,
		dps: false,
	};
	function isBest(setup: ComparedSetup | null, key: Figure): boolean {
		const values = rows.flatMap((row) => {
			const value = figure(row.setup, key);
			return value === null ? [] : [value];
		});
		const own = figure(setup, key);
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
	const tone = (setup: ComparedSetup | null, key: Figure) =>
		isBest(setup, key) ? 'font-semibold text-text' : 'text-text-secondary';

	function effectiveText(setup: ComparedSetup): string {
		const effective = setup.economics?.effectiveEfficiency;
		if (!effective) return NO_DATA;
		if (effective.status === 'below_model_range') return 'Below range';
		if (effective.status === 'above_model_range') return 'Above range';
		return effective.efficiencyPct === null ? NO_DATA : `${effective.efficiencyPct.toFixed(1)}%`;
	}
</script>

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
			<th class="w-8"><span class="sr-only">Remove</span></th>
		</tr>
	</thead>
	<tbody>
		{#each rows as row (row.draft.id)}
			{@const setup = row.setup}
			{@const economics = setup?.economics ?? null}
			<tr class="border-b border-border/30 align-top">
				<td class="py-2.5 pr-4">
					<div class="flex min-w-0 items-start gap-2">
						{@render swatch(row.draft.slot)}
						<div class="min-w-0 flex-1 space-y-1">
							<SetupCell draft={row.draft} />
							{#if setup && describeUnmodelled(setup)}
								<p class="text-xs text-warning">{describeUnmodelled(setup)}</p>
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
				<td class="{cell} {tone(setup, 'maxDamage')}">
					{economics ? economics.maxDamage.toFixed(1) : NO_DATA}
				</td>
				<td class="{cell} text-text-secondary">
					{economics ? `${formatPec(economics.costPerUsePec)} PEC` : NO_DATA}
				</td>
				<td class="{cell} {tone(setup, 'dpp')}">
					{economics ? economics.damagePerPec.toFixed(2) : NO_DATA}
				</td>
				<td class="{cell} {tone(setup, 'effective')}">
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
				<td class="{cell} {tone(setup, 'lossPer100')}">
					{economics ? formatKillPec(economics.lossPerHpPec * 100) : NO_DATA}
				</td>
				<td class="{cell} {tone(setup, 'dps')}">
					{economics?.damagePerSecond != null ? economics.damagePerSecond.toFixed(1) : NO_DATA}
				</td>
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
				<td colspan={columns.length + 1}></td>
			</tr>
		{/if}
	</tbody>
</table>
