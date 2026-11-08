<script lang="ts">
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { formatPed, formatPercent } from '$lib/utils/format';
	import type { LiveValuationView } from './liveLootValuation.svelte';

	/**
	 * What the live markup figures are made of: how much of the loot was
	 * valued at its own market reading, at the player's own markup, at the
	 * Nanocube floor, or by a Nanocube mark. Shared by Loot MU and MU Rate,
	 * which differ only in their denominator.
	 */
	let {
		valuation,
		cycledPed = null,
	}: {
		valuation: LiveValuationView;
		/** The session's Cycled, for MU Rate; absent for Loot MU. */
		cycledPed?: number | null;
	} = $props();

	const CONFIDENCE: Record<LiveValuationView['confidence'], string> = {
		liquid: 'high-volume markets only',
		liquidMiddling: 'high- and medium-volume markets',
		all: 'every market reading',
	};

	const lines = $derived.by(() => {
		const { coverage, lootTt } = valuation;
		const share = (tt: number) => (lootTt > 0 ? tt / lootTt : 0);
		return [
			{ label: 'At market readings', tt: coverage.marketTt },
			{ label: 'At your own markup', tt: coverage.manualTt },
			{ label: 'At the Nanocube floor', tt: coverage.flooredTt },
			{ label: 'Marked as Nanocube', tt: coverage.markedTt },
			{ label: 'Shrapnel at its conversion', tt: coverage.shrapnelTt },
		]
			.filter((line) => line.tt > 0)
			.map((line) => ({ ...line, share: share(line.tt) }));
	});
</script>

<InfoTip label={cycledPed === null ? 'What Loot MU is made of' : 'What MU Rate is made of'} align="left" width="w-80">
	<span class="block text-xs font-medium text-text">
		{cycledPed === null ? 'Loot at market, over its TT' : 'Loot at market, over Cycled'}
	</span>
	<span class="mt-1 block text-xs leading-relaxed text-text-secondary tabular-nums">
		{formatPed(valuation.marketValuePed)} PED at market
		{#if cycledPed === null}
			for {formatPed(valuation.lootTt)} PED of loot TT.
		{:else}
			against {formatPed(cycledPed)} PED cycled.
		{/if}
	</span>
	{#if valuation.coverage.noTtValuePed > 0}
		<span class="mt-1.5 flex items-baseline gap-3 text-xs" data-testid="no-tt-value">
			<span class="min-w-0 flex-1 text-text-secondary">Items with no TT, at your value</span>
			<span class="tabular-nums text-text">+{formatPed(valuation.coverage.noTtValuePed)}</span>
			<span class="w-10"></span>
		</span>
	{/if}
	{#if lines.length > 0}
		<span class="mt-2 flex flex-col gap-1" data-testid="markup-coverage">
			<span class="eyebrow">Loot TT valued</span>
			{#each lines as line (line.label)}
				<span class="flex items-baseline gap-3 text-xs">
					<span class="min-w-0 flex-1 text-text-secondary">{line.label}</span>
					<span class="tabular-nums text-text">{formatPed(line.tt)}</span>
					<span class="w-10 text-right tabular-nums text-text-tertiary">{formatPercent(line.share)}</span>
				</span>
			{/each}
		</span>
	{/if}
	<span class="mt-2 block text-[11px] leading-relaxed text-text-tertiary">
		Trusting {CONFIDENCE[valuation.confidence]}; the rest is valued at the Nanocube markup. Set it
		in Loot Composition. An estimate: it never enters Net or Rate.
	</span>
</InfoTip>
