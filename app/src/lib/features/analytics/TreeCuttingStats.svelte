<script lang="ts">
	import InfoTip from '$lib/components/InfoTip.svelte';
	import EffectiveEfficiencyInfoTip from '$lib/components/EffectiveEfficiencyInfoTip.svelte';
	import ExpectedReturnInfoTip from '$lib/components/ExpectedReturnInfoTip.svelte';
	import RewardMuInfoTip from '$lib/components/RewardMuInfoTip.svelte';
	import type { ExpectedHuntingEconomics } from '$lib/api';
	import type { RealisedDerivedMarkup } from '$lib/api/commands.gen';
	import { formatPed, NO_DATA } from '$lib/utils/format';
	import CompletionRewardContext from './CompletionRewardContext.svelte';
	import EconomicOutcomeHorizon from './EconomicOutcomeHorizon.svelte';
	import ExpectedEconomicsEquation from './ExpectedEconomicsEquation.svelte';
	import type { RewardContext } from './huntingModel.svelte';

	let {
		cycled,
		returns,
		lootRate,
		muProjectedReturns,
		muRate,
		lootMarkupFactor,
		expectedTtRate,
		expectedMarketRate,
		expectedEconomics,
		realisedReturns,
		realisedRate,
		overhead,
		unassignedOverhead = 0,
		realisedDerived = [],
		reward = null,
		rewardScope = 'session',
		rewardMuRate = null,
		expectedTotalRate = null,
	}: {
		cycled: number;
		returns: number;
		lootRate: number;
		muProjectedReturns: number | null;
		muRate: number | null;
		lootMarkupFactor?: number | null;
		expectedTtRate?: number | null;
		expectedMarketRate?: number | null;
		expectedEconomics?: ExpectedHuntingEconomics | null;
		realisedReturns: number;
		/** Realised returns over Cycled and the overhead. */
		realisedRate: number;
		/** The sessions' overhead shared to this scope: a cost beside Cycled,
		 * which Realised Net takes off. */
		overhead: number;
		/** The part of `overhead` on no board activity: in Overall only. */
		unassignedOverhead?: number;
		/** The part of the realised figure that came through conversions. */
		realisedDerived?: RealisedDerivedMarkup[];
		/** Completion rewards aggregated over the scope's activities. Absent
		 * for activities that do not have them at all, Tree Cutting included. */
		reward?: RewardContext | null;
		/** The surface this line is rendering: one session, or every session
		 * in the period. The reward disclosure states which it covers. */
		rewardScope?: 'session' | 'overall';
		rewardMuRate?: number | null;
		expectedTotalRate?: number | null;
	} = $props();

	const effectiveEfficiencyValue = $derived.by(() => {
		const effective = expectedEconomics?.effectiveEfficiency;
		if (!effective) return NO_DATA;
		switch (effective.status) {
			case 'within_model_range':
				return effective.efficiencyPct !== null
					? `${effective.efficiencyPct.toFixed(1)}%`
					: NO_DATA;
			case 'below_model_range':
				return 'Below model range';
			case 'above_model_range':
				return 'Above model range';
		}
	});
</script>

{#snippet estimateTip()}
	<InfoTip label="What MU figures are">
		<p class="text-xs font-semibold leading-relaxed text-text">Estimated, not realised</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			What current market markup would add if this loot sold at it. Nothing here is money
			until a sale is confirmed.
		</p>
	</InfoTip>
{/snippet}

{#snippet expectedReturnTip()}
	<ExpectedReturnInfoTip
		looterLevel={expectedEconomics?.looterLevel}
		coverage={expectedEconomics?.coverage}
		incomplete={expectedEconomics?.incomplete}
	/>
{/snippet}

{#snippet effectiveEfficiencyTip()}
	<EffectiveEfficiencyInfoTip
		effectiveEfficiency={expectedEconomics?.effectiveEfficiency ?? null}
		looterLevel={expectedEconomics?.looterLevel}
		coverage={expectedEconomics?.coverage}
		incomplete={expectedEconomics?.incomplete}
		scope="activity"
	/>
{/snippet}

{#snippet realisedTip()}
	<InfoTip label="What realised figures are">
		<p class="text-xs font-semibold leading-relaxed text-text">
			{reward ? 'Loot TT, confirmed reward TT and realised markup' : 'Loot TT plus realised markup'}
		</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary" data-testid="realised-meaning">
			{#if reward}
				Confirmed reward TT, plus only markup from a recorded sale or conversion and ledger entries
				pegged to a session. Realised Net also takes off the sessions' overhead, and the Realised
				Rate sets the realised returns against Cycled and that overhead: until a sale or a pegged
				entry realises a gain or loss, Realised Net reads as TT Net plus the confirmed reward TT,
				less the overhead.
			{:else}
				Only markup from a recorded sale or conversion, plus ledger entries pegged to a session.
				Realised Net also takes off the sessions' overhead, and the Realised Rate sets the realised
				returns against Cycled and that overhead: until a sale or a pegged entry realises a gain or
				loss, Realised Net reads as TT Net less the overhead.
			{/if}
		</p>
		{#if overhead > 0}
			<p class="mt-2 flex items-baseline justify-between gap-3 border-t border-border/50 pt-2 text-xs text-text-secondary" data-testid="realised-overhead">
				<span>Overhead taken off</span>
				<span class="shrink-0 tabular-nums text-text">{formatPed(overhead)} PED</span>
			</p>
			{#if unassignedOverhead > 0}
				<p class="mt-1 text-xs text-text-tertiary" data-testid="realised-unassigned-overhead">
					Of which {formatPed(unassignedOverhead)} PED came from sessions that cycled nothing on a
					board activity, so it is counted here only.
				</p>
			{/if}
		{/if}
		{#if realisedDerived.length > 0}
			<ul class="mt-2 space-y-1 border-t border-border/50 pt-2" data-testid="realised-derived">
				{#each realisedDerived as row (row.originItem)}
					<li class="flex items-baseline justify-between gap-3 text-xs text-text-secondary">
						<span class="min-w-0">
							{row.originItem} (derived){row.quantity >= 0.5 ? ` ×${Math.round(row.quantity)}` : ''}
						</span>
						<span class="shrink-0 tabular-nums text-text">{row.netMarkup >= 0 ? '+' : ''}{formatPed(row.netMarkup)} PED</span>
					</li>
				{/each}
			</ul>
		{/if}
	</InfoTip>
{/snippet}

{#snippet rewardMuTip()}
	<RewardMuInfoTip />
{/snippet}

<EconomicOutcomeHorizon
	{cycled}
	ttNet={returns - cycled}
	ttRate={lootRate}
	muNet={muProjectedReturns !== null ? muProjectedReturns - cycled : null}
	{muRate}
	realisedNet={realisedReturns - cycled - overhead}
	{realisedRate}
	muTip={estimateTip}
	realisedTip={realisedTip}
/>

{#if reward && reward.treatments.length > 0}
	<div class="mt-5">
		<CompletionRewardContext
			ttPed={reward.rewardTtPed}
			muPed={reward.rewardMuPed}
			treatments={reward.treatments}
			scope={rewardScope}
		/>
	</div>
{/if}

{#if lootMarkupFactor !== undefined || expectedTtRate !== undefined}
	<ExpectedEconomicsEquation
		effectiveEfficiency={effectiveEfficiencyValue}
		lootMarkupFactor={lootMarkupFactor ?? null}
		expectedTtRate={expectedTtRate ?? null}
		expectedMarketRate={expectedMarketRate ?? null}
		{rewardMuRate}
		{expectedTotalRate}
		efficiencyTip={effectiveEfficiencyTip}
		estimateTip={estimateTip}
		expectedTip={expectedReturnTip}
		rewardTip={rewardMuTip}
	/>
{/if}
