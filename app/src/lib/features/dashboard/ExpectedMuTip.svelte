<script lang="ts">
	import type { LiveExpectedCycle } from '$lib/api';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { atLooterBasis, type LooterBasis, looterBasisCaption } from '$lib/looterBasis.svelte';
	import { formatPed, formatPercent } from '$lib/utils/format';

	/**
	 * What the Expected MU Rate rests on: the modelled loot over the whole
	 * Cycled at the declared looter basis, the range the unknown-efficiency
	 * spend leaves, and the markup it is valued at, plus the quest rewards
	 * confirmed so far at their market value. It discloses the model,
	 * because an expected figure must never read as a measured one, and it
	 * names the reward's part, because a hand-in is a fixed amount and must
	 * never read as what the hunting pays.
	 */
	let {
		cycle,
		basis,
		lootMuFactor,
		cycledPed,
		rewardMuPed = 0,
		trackedCost = false,
	}: {
		cycle: LiveExpectedCycle;
		basis: LooterBasis;
		lootMuFactor: number;
		cycledPed: number;
		/** The confirmed quest rewards at market, PED; 0 before a hand-in. */
		rewardMuPed?: number;
		/** The session's Cycled comes from the account readings: the figure
		 * here stays over the tracked cost the model was given, and says
		 * so, so it never contradicts the Cycled tile. */
		trackedCost?: boolean;
	} = $props();

	const over = $derived(trackedCost ? 'the tracked cost' : 'Cycled');

	const at = $derived(atLooterBasis(cycle.bases, basis));
	const rewardRate = $derived(cycledPed > 0 ? rewardMuPed / cycledPed : 0);
	const rate = (loot: number) =>
		cycledPed > 0 ? (loot * lootMuFactor) / cycledPed + rewardRate : 0;
	const lines = $derived(
		[
			{
				label: 'Weapons, amps, scopes (modelled)',
				cost: cycle.modelledCostPed,
				loot: at.modelledExpectedLootTtPed,
			},
			{
				label: 'Unknown Efficiency',
				cost: cycle.unknownEfficiencyTtPed,
				loot: at.expectedLootTtPed - at.modelledExpectedLootTtPed,
			},
			{ label: 'Lost (markup over TT, doses)', cost: cycle.lostPed, loot: 0 },
		].filter((line) => line.cost > 0),
	);
</script>

<InfoTip label="What Expected MU Rate rests on" align="left" width="w-96">
	<span class="block text-xs font-medium text-text">
		{rewardMuPed > 0
			? `Expected loot at its markup, and the quest rewards, over ${over}`
			: `Expected loot at its markup, over ${over}`}
	</span>
	<span class="mt-1 block text-xs leading-relaxed text-text-secondary tabular-nums">
		{formatPed(at.expectedLootTtPed)} PED expected TT at {formatPercent(lootMuFactor)} Loot MU, over
		{formatPed(cycledPed)} PED {trackedCost ? 'of the tracked cost' : 'cycled'}. Range {formatPercent(rate(at.lowPed))} to
		{formatPercent(rate(at.highPed))}.
	</span>
	{#if rewardMuPed > 0}
		<span class="mt-1.5 flex flex-col gap-1 text-xs tabular-nums" data-testid="expected-reward-split">
			<span class="flex items-baseline gap-3">
				<span class="min-w-0 flex-1 text-text-secondary">From the loot</span>
				<span class="text-text">{formatPercent(rate(at.expectedLootTtPed) - rewardRate)}</span>
			</span>
			<span class="flex items-baseline gap-3">
				<span class="min-w-0 flex-1 text-text-secondary">
					From quest rewards ({formatPed(rewardMuPed)} PED at market)
				</span>
				<span class="text-text">+{formatPercent(rewardRate)}</span>
			</span>
		</span>
	{/if}
	<span class="mt-2 flex flex-col gap-1" data-testid="expected-cycle-split">
		<span class="flex items-baseline gap-3">
			<span class="eyebrow min-w-0 flex-1">Cycled</span>
			<span class="eyebrow w-16 text-right">PED</span>
			<span class="eyebrow w-16 text-right">Exp. TT</span>
		</span>
		{#each lines as line (line.label)}
			<span class="flex items-baseline gap-3 text-xs">
				<span class="min-w-0 flex-1 text-text-secondary">{line.label}</span>
				<span class="w-16 text-right tabular-nums text-text">{formatPed(line.cost)}</span>
				<span class="w-16 text-right tabular-nums text-text-tertiary">{formatPed(line.loot)}</span>
			</span>
		{/each}
	</span>
	<span class="mt-2 block text-[11px] leading-relaxed text-text-tertiary">
		Community model v1 at {looterBasisCaption(basis, at.looterLevel)}, with variance taken out:
		each weapon, amplifier, and scope at its own Efficiency, weighted by the TT it cycled.
		Armour, healing, absorbers, implants, harvesting, and weapons without an Efficiency return
		somewhere across the model's Efficiency span, which is the range. Markup paid over TT and doses
		return nothing. An estimate of what this play returns at this markup; never Net or Rate.
		{#if rewardMuPed > 0}
			A quest reward is a fixed amount for the hand-in, not something the hunting pays: its part
			shrinks the more the session cycles.
		{/if}
	</span>
</InfoTip>
