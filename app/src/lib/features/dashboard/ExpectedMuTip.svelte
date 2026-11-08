<script lang="ts">
	import type { LiveExpectedCycle } from '$lib/api';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { atLooterBasis, type LooterBasis, looterBasisCaption } from '$lib/looterBasis.svelte';
	import { formatPed, formatPercent } from '$lib/utils/format';

	/**
	 * What the Expected MU Rate rests on: the modelled loot over the whole
	 * Cycled at the declared looter basis, the range the unknown-efficiency
	 * spend leaves, and the markup it is valued at. It discloses the model,
	 * because an expected figure must never read as a measured one.
	 */
	let {
		cycle,
		basis,
		lootMuFactor,
		cycledPed,
	}: {
		cycle: LiveExpectedCycle;
		basis: LooterBasis;
		lootMuFactor: number;
		cycledPed: number;
	} = $props();

	const at = $derived(atLooterBasis(cycle.bases, basis));
	const rate = (loot: number) => (cycledPed > 0 ? (loot * lootMuFactor) / cycledPed : 0);
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
	<span class="block text-xs font-medium text-text">Expected loot at its markup, over Cycled</span>
	<span class="mt-1 block text-xs leading-relaxed text-text-secondary tabular-nums">
		{formatPed(at.expectedLootTtPed)} PED expected TT at {formatPercent(lootMuFactor)} Loot MU, over
		{formatPed(cycledPed)} PED cycled. Range {formatPercent(rate(at.lowPed))} to
		{formatPercent(rate(at.highPed))}.
	</span>
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
	</span>
</InfoTip>
