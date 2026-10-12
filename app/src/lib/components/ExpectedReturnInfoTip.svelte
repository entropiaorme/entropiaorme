<script lang="ts">
	import { formatPercent } from '$lib/utils/format';
	import { looterBasisCaption, type LooterBasis } from '$lib/looterBasis.svelte';
	import InfoTip from './InfoTip.svelte';

	let {
		looterLevel = null,
		looterBasis = 'three_looter_mean',
		coverage = null,
		incomplete = false,
	}: {
		looterLevel?: number | null;
		/** The basis `looterLevel` was read at (analytics replays the mean);
		 * null for a level chosen between the bases. */
		looterBasis?: LooterBasis | null;
		coverage?: number | null;
		incomplete?: boolean;
	} = $props();
</script>

<InfoTip label="What Expected Return includes" width="w-96">
	<p class="text-xs font-semibold leading-relaxed text-text">Offensive spend only</p>
	<p class="mt-1 text-xs leading-relaxed text-text-secondary">
		Models weapon and amplifier spend with known Efficiency. Healing, armour, harvesting, and
		other unmodelled costs are excluded because their return mechanics are not yet known. This is
		not a whole-activity forecast.
	</p>
	{#if looterLevel !== null || coverage !== null || incomplete}
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
		</p>
	{/if}
</InfoTip>
