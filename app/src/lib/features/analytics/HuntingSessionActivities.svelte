<script lang="ts">
	import InfoTip from '$lib/components/InfoTip.svelte';
	import EffectiveEfficiencyInfoTip from '$lib/components/EffectiveEfficiencyInfoTip.svelte';
	import ExpectedReturnInfoTip from '$lib/components/ExpectedReturnInfoTip.svelte';
	import RewardMuInfoTip from '$lib/components/RewardMuInfoTip.svelte';
	import { NO_DATA } from '$lib/utils/format';
	import ActivityLootComposition from './ActivityLootComposition.svelte';
	import CompletionRewardContext from './CompletionRewardContext.svelte';
	import EconomicOutcomeHorizon from './EconomicOutcomeHorizon.svelte';
	import ExpectedEconomicsEquation from './ExpectedEconomicsEquation.svelte';
	import HuntingActivityPicker from './HuntingActivityPicker.svelte';
	import { rewardContextOf, type HuntingActivitySection } from './huntingModel.svelte';

	let {
		activities,
		marketAvailable,
	}: {
		activities: HuntingActivitySection[];
		marketAvailable: boolean;
	} = $props();

	let selectedKey = $state<string | null>(null);

	function findActivity(rows: HuntingActivitySection[], key: string): HuntingActivitySection | null {
		for (const row of rows) {
			if (row.key === key) return row;
			const nested = findActivity(row.variants, key);
			if (nested) return nested;
		}
		return null;
	}

	const selected = $derived(
		(selectedKey ? findActivity(activities, selectedKey) : null) ??
			activities.find((activity) => !activity.isUnscoped) ??
			activities[0] ??
			null,
	);

	const muRate = (activity: HuntingActivitySection) =>
		activity.muProjectedReturns !== null && activity.cycled > 0
			? activity.muProjectedReturns / activity.cycled
			: null;
	const effectiveEfficiencyValue = (activity: HuntingActivitySection) => {
		const effective = activity.expected?.effectiveEfficiency;
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
	};
	const kindLabel = (activity: HuntingActivitySection) => {
		switch (activity.kind) {
			case 'quest_family':
				return 'Quest family';
			case 'bundle':
				return 'Joint activity';
			case 'segment':
				return 'Segment';
			case 'ambient':
				return 'Unscoped';
			default:
				return 'Quest';
		}
	};
</script>

{#snippet estimateTip()}
	<InfoTip label="What MU figures are">
		<p class="text-xs font-semibold leading-relaxed text-text">Estimated, not realised</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			What current market markup would add if this activity's loot sold at it. Confirmed rewards
			remain outside this estimate.
		</p>
	</InfoTip>
{/snippet}

{#snippet realisedTip()}
	<InfoTip label="What realised figures are">
		<p class="text-xs font-semibold leading-relaxed text-text">Loot TT plus confirmed economic outcomes</p>
		<p class="mt-1 text-xs leading-relaxed text-text-secondary">
			Confirmed reward TT is added exactly once. Later realised markup from ordinary loot and stock
			rewards returns through its immutable activity provenance. A ledger entry pegged to the session
			is shared across its activities by what each cycled.
		</p>
	</InfoTip>
{/snippet}

{#snippet rewardMuTip()}
	<RewardMuInfoTip />
{/snippet}

{#snippet expectedReturnTip()}
	<ExpectedReturnInfoTip
		looterLevel={selected?.expected?.looterLevel}
		coverage={selected?.expected?.coverage}
		incomplete={selected?.expected?.incomplete}
	/>
{/snippet}

{#snippet effectiveEfficiencyTip()}
	<EffectiveEfficiencyInfoTip
		effectiveEfficiency={selected?.expected?.effectiveEfficiency ?? null}
		looterLevel={selected?.expected?.looterLevel}
		coverage={selected?.expected?.coverage}
		incomplete={selected?.expected?.incomplete}
		scope="activity"
	/>
{/snippet}

{#if selected}
	<div>
		<div class="min-w-0">
			<div class="w-full min-w-0">
				<HuntingActivityPicker
					{activities}
					{selected}
					onselect={(key) => (selectedKey = key)}
				/>
				<p class="mt-0.5 text-xs text-text-tertiary">{kindLabel(selected)}</p>
			</div>
		</div>

		{#if selected.isUnscoped}
			<p class="mt-5 rounded-lg border border-border/40 bg-surface-hover/20 p-4 text-sm leading-relaxed text-text-secondary">
				This evidence was recorded without a declared quest or segment. It remains in the session total but cannot support a repeatable activity comparison.
			</p>
		{:else}
			<div class="mt-6">
				<EconomicOutcomeHorizon
					cycled={selected.cycled}
					ttNet={selected.returns - selected.cycled}
					ttRate={selected.lootRate}
					muNet={selected.muProjectedReturns !== null
						? selected.muProjectedReturns - selected.cycled
						: null}
					muRate={muRate(selected)}
					realisedNet={selected.rewardStatus === 'unverified'
						? null
						: selected.rewardedReturns - selected.cycled}
					realisedRate={selected.rewardStatus === 'unverified' ? null : selected.rewardedRate}
					muTip={estimateTip}
					realisedTip={realisedTip}
					testid="activity-economic-grid"
				/>
			</div>

			{#if selected.rewardStatus !== 'none'}
				<div class="mt-4">
					<CompletionRewardContext
						ttPed={selected.confirmedRewardPed}
						muPed={selected.rewardMuPed}
						treatments={rewardContextOf(selected).treatments}
						scope="activity"
					/>
				</div>
			{/if}

			<ExpectedEconomicsEquation
				effectiveEfficiency={effectiveEfficiencyValue(selected)}
				lootMarkupFactor={selected.lootMarkupFactor}
				expectedTtRate={selected.expectedTtRate}
				expectedMarketRate={selected.expectedMarketRate}
				rewardMuRate={selected.rewardMuRate}
				expectedTotalRate={selected.expectedTotalRate}
				efficiencyTip={effectiveEfficiencyTip}
				estimateTip={estimateTip}
				expectedTip={expectedReturnTip}
				rewardTip={rewardMuTip}
				testid="activity-expected-economics"
			/>

			<ActivityLootComposition
				items={selected.items}
				{marketAvailable}
				emptyLabel="No itemised loot was recorded for this activity."
				disclosure="activity"
			/>
		{/if}
	</div>
{:else}
	<p class="px-2.5 py-4 text-center text-xs text-text-tertiary">
		No quest or segment evidence was recorded for this session.
	</p>
{/if}
