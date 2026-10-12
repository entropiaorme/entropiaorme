<script lang="ts">
	import type { EquipmentEffectiveEfficiency } from '$lib/api/commands.gen';
	import { formatPercent } from '$lib/utils/format';
	import { looterBasisCaption, type LooterBasis } from '$lib/looterBasis.svelte';
	import InfoTip from './InfoTip.svelte';

	let {
		effectiveEfficiency,
		looterLevel = null,
		looterBasis = 'three_looter_mean',
		weightedEfficiencyPct = null,
		consumedPremiumLabel = null,
		coverage = null,
		incomplete = false,
		scope = 'setup',
	}: {
		effectiveEfficiency: EquipmentEffectiveEfficiency | null;
		looterLevel?: number | null;
		/** The basis `looterLevel` was read at (analytics replays the mean);
		 * null for a level chosen between the bases. */
		looterBasis?: LooterBasis | null;
		weightedEfficiencyPct?: number | null;
		consumedPremiumLabel?: string | null;
		coverage?: number | null;
		incomplete?: boolean;
		scope?: 'setup' | 'activity';
	} = $props();

</script>

<InfoTip label="What Effective Efficiency means" width="w-96">
	<p class="text-xs font-semibold leading-relaxed text-text">Unlimited economic equivalent</p>
	{#if scope === 'activity'}
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			The Efficiency an otherwise identical unlimited offensive setup would need to match this
			scope’s combined premium-adjusted expected return. Weapon and amplifier streams are
			weighted by raw TT; consumed limited markup remains in economic cost.
		</p>
	{:else}
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			The Efficiency an otherwise identical unlimited offensive setup would need to match this
			setup’s premium-adjusted expected return at the same looter level. Limited markup changes
			economic cost, not any component’s in-game Efficiency.
		</p>
	{/if}

	{#if effectiveEfficiency?.status === 'within_model_range' && effectiveEfficiency.efficiencyPct !== null}
		{#if scope === 'setup' && weightedEfficiencyPct !== null && consumedPremiumLabel !== null}
			<p class="mt-2 text-[11px] leading-relaxed text-text-tertiary">
				{weightedEfficiencyPct.toFixed(1)}% TT-weighted in-game Efficiency to
				{effectiveEfficiency.efficiencyPct.toFixed(1)}% effective · {consumedPremiumLabel} PEC
				premium per use
			</p>
		{:else}
			<p class="mt-2 text-[11px] leading-relaxed text-text-tertiary">
				{effectiveEfficiency.efficiencyPct.toFixed(1)}% effective Efficiency
			</p>
		{/if}
	{:else if effectiveEfficiency?.status === 'below_model_range'}
		<p class="mt-2 text-[11px] leading-relaxed text-text-tertiary">
			The premium-adjusted return is below Community Model v1’s 0% Efficiency boundary, so no
			plausible percentage is shown.
		</p>
	{:else if effectiveEfficiency?.status === 'above_model_range'}
		<p class="mt-2 text-[11px] leading-relaxed text-text-tertiary">
			The premium-adjusted return is above Community Model v1’s 100% Efficiency boundary, so no
			plausible percentage is shown.
		</p>
	{:else}
		<p class="mt-2 text-[11px] leading-relaxed text-text-tertiary">
			Captured offensive Efficiency evidence is required before this comparison can be calculated.
		</p>
	{/if}

	<p class="mt-2 text-[11px] leading-relaxed text-text-tertiary">
		Community model v1
		{#if looterLevel !== null}
			· {looterBasisCaption(looterBasis, looterLevel)}
		{/if}
		{#if coverage !== null}
			· {formatPercent(coverage)} offensive basis coverage
		{/if}
		{#if incomplete}
			· partial basis
		{/if}
		· expected rates remain offensive-only
	</p>
</InfoTip>
