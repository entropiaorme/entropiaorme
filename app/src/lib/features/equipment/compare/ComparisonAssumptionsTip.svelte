<script lang="ts">
	import type { ComparisonAssumptionsView } from '$lib/api/comparison';
	import InfoTip from '$lib/components/InfoTip.svelte';

	let { assumptions }: { assumptions: ComparisonAssumptionsView } = $props();

	const pct = (ratio: number) => `${Number((ratio * 100).toFixed(1))}%`;
</script>

<!-- Every comparison figure is a planning estimate: the model and each
     assumption behind it are stated where the figures are read. -->
<InfoTip label="How these figures are estimated" width="w-96">
	<p class="text-xs font-semibold leading-relaxed text-text">Planning estimate, not tracked results</p>
	<p class="mt-1 text-xs leading-relaxed text-text-secondary">
		Expected loot follows Community model v1 over the weapon and amplifier at a three-looter mean of
		{assumptions.looterLevel.toFixed(1)}. Absorber and implant decay counts as cost with no
		modelled return. Kills are solved exactly from these assumptions, not simulated:
	</p>
	<ul class="mt-1.5 space-y-0.5 text-xs leading-relaxed text-text-secondary">
		<li>{pct(assumptions.hitRate)} of uses hit</li>
		<li>
			Each hit rolls evenly from {pct(assumptions.damageFloor)} to 100% of max damage
		</li>
		<li>
			{pct(assumptions.criticalChance)} of hits are critical, at {assumptions.criticalMultiplier}x the
			rolled damage; critical effects you have active are not read
		</li>
		<li>Amplifier damage counts up to half the weapon's</li>
		<li>Damage per PEC counts misses and criticals: it is damage dealt, not rolled</li>
	</ul>
	<p class="mt-2 text-[11px] leading-relaxed text-text-tertiary">
		Loot is priced at TT. The game's loot design favours setups that kill for less, so the cheaper
		setup per kill likely holds a larger advantage than shown.
		Armour, healing and time under fire are outside the model. Unlimited items run at TT; a limited
		item's markup is consumed with its decay.
	</p>
</InfoTip>
