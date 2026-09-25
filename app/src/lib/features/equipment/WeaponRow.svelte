<script lang="ts">
	import { Badge, Button, DataTable, StatDisplay } from '$lib/components';
	import ExpectedReturnInfoTip from '$lib/components/ExpectedReturnInfoTip.svelte';
	import type { Equipment } from '$lib/types';
	import { NO_DATA, formatPercent } from '$lib/utils/format';
	import { enrichmentColor, enrichmentLabel, formatPec } from './display';
	import EffectiveEfficiencyInfoTip from '$lib/components/EffectiveEfficiencyInfoTip.svelte';
	import type { LibraryModel } from './libraryModel.svelte';
	import { describeEffectProfile } from './weaponEffect';

	let { model, item }: { model: LibraryModel; item: Equipment } = $props();

	const detail = $derived(model.detailCache[item.id] ?? null);

	// Absorption annotation for a breakdown line, mirroring the markup
	// multiplier tag: the split devices show the share of weapon decay they
	// take (the absorber acts on what the implant leaves), and the weapon
	// line shows the fraction it keeps.
	const pct = (value: number) => `${Number(value.toFixed(1))}%`;
	function absorptionTag(component: string): string | null {
		const implantShare = detail?.implant?.absorptionPercent ?? 0;
		const absorberShare = detail?.absorber?.absorptionPercent ?? 0;
		if (component === 'Implant decay' && implantShare > 0) {
			return `${pct(implantShare)} of decay`;
		}
		if (component === 'Absorber decay' && absorberShare > 0) {
			return implantShare > 0
				? `${pct(absorberShare)} of remainder`
				: `${pct(absorberShare)} of decay`;
		}
		if (component === 'Weapon decay' && (implantShare > 0 || absorberShare > 0)) {
			const kept = (1 - implantShare / 100) * (1 - absorberShare / 100) * 100;
			return `${pct(kept)} kept`;
		}
		return null;
	}

	const breakdownRows = $derived(
		(detail?.costBreakdown ?? []).map((line) => ({
			component: line.component,
			absorption: absorptionTag(line.component) ?? '',
			costPec: line.costPec,
			markupMultiplier: line.markupMultiplier,
			effectiveCostPec: line.effectiveCostPec,
		})),
	);
	const effectiveEfficiencyValue = $derived.by(() => {
		const effective = detail?.expectedReturn?.effectiveEfficiency;
		if (!effective) return NO_DATA;
		switch (effective.status) {
			case 'within_model_range':
				return effective.efficiencyPct === null
					? NO_DATA
					: `${effective.efficiencyPct.toFixed(1)}%`;
			case 'below_model_range':
				return 'Below model range';
			case 'above_model_range':
				return 'Above model range';
		}
	});
</script>

{#snippet expectedReturnTip()}
	<ExpectedReturnInfoTip
		looterLevel={detail?.expectedReturn?.looterLevel}
		coverage={detail?.expectedReturn?.coverage}
		incomplete={detail?.expectedReturn?.incomplete}
	/>
{/snippet}

{#snippet effectiveEfficiencyTip()}
		<EffectiveEfficiencyInfoTip
			effectiveEfficiency={detail?.expectedReturn?.effectiveEfficiency ?? null}
			weightedEfficiencyPct={detail?.expectedReturn?.weightedEfficiencyPct ?? null}
			consumedPremiumLabel={formatPec((detail?.expectedReturn?.consumedPremiumPerUse ?? 0) * 100)}
			looterLevel={detail?.expectedReturn?.looterLevel ?? 0}
		/>
{/snippet}

<!-- Equipment row -->
<button
	type="button"
	data-guide-anchor="library-row-{item.id}"
	class="w-full text-left px-4 py-3 rounded-md transition-colors duration-[var(--duration-fast)]
		cursor-pointer
		{model.expandedId === item.id
		? 'bg-surface-hover'
		: 'hover:bg-surface-hover/50'}"
	onclick={() => model.toggleExpand(item.id)}
>
	<div class="flex items-center gap-3">
		<!-- Type icon -->
		<div class="shrink-0 h-8 w-8 rounded-md bg-surface flex items-center justify-center">
			<div class="h-2 w-2 rounded-full bg-accent"></div>
		</div>

		<!-- Name + amp -->
		<div class="flex-1 min-w-0">
			<div class="flex items-center gap-2">
				<span class="text-sm font-medium text-text truncate">{item.name}</span>
			</div>
			{#if item.amplifierName}
				<p class="text-xs text-text-tertiary mt-0.5 truncate">
					+ {item.amplifierName}
				</p>
			{/if}
			{#if item.lifestealPercent}
				<p class="text-xs text-positive mt-0.5">{item.lifestealPercent}% lifesteal</p>
			{/if}
			{#if item.effectProfile}
				<p class="text-xs text-text-tertiary mt-0.5 truncate" data-testid="weapon-effect-summary">
					{describeEffectProfile(item.effectProfile)}
				</p>
			{/if}
		</div>

		<!-- Cost -->
		<div class="text-right shrink-0">
			<span class="text-sm font-medium tabular-nums text-text">
				{formatPec(item.costPerUse)}
			</span>
			<span class="text-xs text-text-tertiary ml-0.5">PEC</span>
		</div>

		<!-- Enrichment badge -->
		<span data-guide-anchor="enrichment-badge-{item.id}" class="shrink-0">
			<Badge variant={enrichmentColor(item.enrichmentLevel)} class="shrink-0">
				{enrichmentLabel(item.enrichmentLevel)}
			</Badge>
		</span>

		<!-- Chevron -->
		<svg
			xmlns="http://www.w3.org/2000/svg"
			viewBox="0 0 20 20"
			fill="currentColor"
			class="h-4 w-4 text-text-tertiary transition-transform duration-[var(--duration-base)]
				{model.expandedId === item.id ? 'rotate-180' : ''}"
		>
			<path
				fill-rule="evenodd"
				d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
				clip-rule="evenodd"
			/>
		</svg>
	</div>
</button>

<!-- Inline detail panel -->
{#if model.expandedId === item.id}
	{#if detail}
		<div class="ml-11 mr-4 mb-2 p-4 bg-surface rounded-md border border-border/50">
			<!-- Cost breakdown -->
			<h3 class="eyebrow mb-3">
				Cost Breakdown
			</h3>
			<div class="mb-4">
				<DataTable
					columns={[
						{ key: 'component', label: 'Component' },
						{ key: 'absorption', label: 'Absorption' },
						{ key: 'costPec', label: 'Base PEC', align: 'right' },
						{ key: 'markupMultiplier', label: 'Markup', align: 'right' },
						{ key: 'effectiveCostPec', label: 'Per Use PEC', align: 'right' }
					]}
					rows={breakdownRows}
				>
					{#snippet cell({ row, column })}
						{#if column.key === 'component'}
							<span class="text-text-secondary">{row.component}</span>
						{:else if column.key === 'absorption'}
							{#if row.absorption}
								<span class="text-positive text-xs">{row.absorption}</span>
							{/if}
						{:else if column.key === 'costPec'}
							<span class="text-text-tertiary text-xs">{formatPec(row.costPec)}</span>
						{:else if column.key === 'markupMultiplier'}
							{#if row.markupMultiplier !== 1}
								<span class="text-warning text-xs">x{row.markupMultiplier.toFixed(2)}</span>
							{/if}
						{:else}
							<span class="text-text font-medium">{formatPec(row.effectiveCostPec)}</span>
						{/if}
					{/snippet}
				</DataTable>
				<div class="flex items-center justify-between text-sm font-medium px-3 py-2.5">
					<span class="text-text">Total per use</span>
					<span class="text-accent tabular-nums">
						{formatPec(detail.totalCostPerUse)} PEC
					</span>
				</div>
			</div>

			{#if detail.expectedReturn}
				{@const expected = detail.expectedReturn}
				<div
					class="grid grid-cols-3 gap-5 border-y border-border/35 py-3 mb-4"
					data-guide-anchor="expected-return-{item.id}"
				>
					<StatDisplay
						label="Expected Return"
						value={expected.expectedTtRate !== null ? formatPercent(expected.expectedTtRate) : NO_DATA}
						valueClass={expected.expectedTtRate !== null ? 'text-text' : 'text-text-tertiary'}
						emphasis="secondary"
						labelSuffix={expectedReturnTip}
					/>
					<StatDisplay
						label="Break-even loot MU"
						value={expected.breakEvenLootMarkup !== null ? formatPercent(expected.breakEvenLootMarkup) : NO_DATA}
						valueClass="text-text"
						emphasis="secondary"
					/>
					<StatDisplay
						label="Effective Efficiency"
						value={effectiveEfficiencyValue}
						valueClass={expected.effectiveEfficiency !== null ? 'text-text' : 'text-text-tertiary'}
						emphasis="secondary"
						labelSuffix={effectiveEfficiencyTip}
					/>
				</div>
				<p class="-mt-2 mb-4 text-[11px] leading-relaxed text-text-tertiary">
					Community model v1 · three-looter mean {expected.looterLevel.toFixed(1)}
					{#if expected.incomplete}
						· partial Efficiency coverage ({formatPercent(expected.coverage)})
					{/if}
				</p>
			{/if}

			<!-- Component list -->
			<h3 class="eyebrow mb-2">
				Components
			</h3>
			<div class="space-y-1.5 text-sm mb-4">
				<div class="flex items-center justify-between">
					<span class="text-text">
						{detail.weapon.name}
					</span>
					<span class="text-text-secondary text-xs tabular-nums">
						{detail.weapon.efficiencyPct !== null ? `Efficiency ${detail.weapon.efficiencyPct.toFixed(1)}% · ` : 'Efficiency unknown · '}
						Decay {formatPec(detail.weapon.decay)} · Ammo {formatPec(detail.weapon.ammoBurn)} PEC
					</span>
				</div>
				{#if detail.weapon.damageEnhancers > 0}
					<div class="flex items-center justify-between">
						<span class="text-text">Damage enhancers</span>
						<span class="text-text-secondary text-xs tabular-nums">
							{detail.weapon.damageEnhancers} slot{detail.weapon.damageEnhancers === 1 ? '' : 's'}
						</span>
					</div>
				{/if}
				{#if detail.amplifier}
					<div class="flex items-center justify-between">
						<span class="text-text">
							{detail.amplifier.name}
						</span>
						<span class="text-text-secondary text-xs tabular-nums">
							{detail.amplifier.efficiencyPct !== null ? `Efficiency ${detail.amplifier.efficiencyPct.toFixed(1)}% · ` : 'Efficiency unknown · '}
							Decay {formatPec(detail.amplifier.decay)} · Ammo
							{formatPec(detail.amplifier.ammoBurn)} PEC
						</span>
					</div>
				{/if}
				{#if detail.scope}
					<div class="flex items-center justify-between">
						<span class="text-text">
							{detail.scope.name}
						</span>
						<span class="text-text-secondary text-xs tabular-nums">
							Decay {formatPec(detail.scope.decay)}
							{#if detail.scope.markupPercent !== 100}
								· {detail.scope.markupPercent}%
							{/if}
						</span>
					</div>
				{/if}
				{#if detail.absorber}
					<div class="flex items-center justify-between">
						<span class="text-text">
							{detail.absorber.name}
						</span>
						<span class="text-text-secondary text-xs tabular-nums">
							-{detail.absorber.absorptionPercent}% weapon decay
							{#if detail.absorber.markupPercent !== 100}
								· {detail.absorber.markupPercent}%
							{/if}
						</span>
					</div>
				{/if}
				{#if detail.implant}
					<div class="flex items-center justify-between">
						<span class="text-text">
							{detail.implant.name}
						</span>
						<span class="text-text-secondary text-xs tabular-nums">
							-{detail.implant.absorptionPercent}% weapon decay
							{#if detail.implant.markupPercent !== 100}
								· {detail.implant.markupPercent}%
							{/if}
						</span>
					</div>
				{/if}
			</div>

			<!-- Actions -->
			<div class="flex items-center gap-2">
				<Button size="sm" variant="ghost" onclick={() => model.openEditModal(item.id)}>
					Edit
				</Button>
				<Button size="sm" variant="danger" onclick={() => model.removeEquipment(item.id)}>
					Remove
				</Button>
			</div>
		</div>
	{:else}
		<!-- Loading detail -->
		<div class="ml-11 mr-4 mb-2 p-4 bg-surface rounded-md border border-border/50">
			<p class="text-xs text-text-tertiary">Loading…</p>
		</div>
	{/if}
{/if}
