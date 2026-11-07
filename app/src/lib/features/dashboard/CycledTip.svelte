<script lang="ts">
	import type { CycledLine } from '$lib/api';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { formatPed } from '$lib/utils/format';
	import { groupCycledBreakdown, hasArmourLine } from './cycledBreakdown';

	let {
		lines,
		totalPed,
		incomplete
	}: {
		lines: readonly CycledLine[];
		totalPed: number;
		/** What the figure is known to leave out (unpriced shots). */
		incomplete?: string;
	} = $props();

	const groups = $derived(groupCycledBreakdown(lines, totalPed));
	const armourRecorded = $derived(hasArmourLine(lines));
</script>

<InfoTip label="What this session's Cycled is made of" align="left" width="w-80">
	<span class="block text-xs font-medium text-text">Cycled this session</span>
	{#if groups.length === 0}
		<span class="mt-1.5 block text-xs leading-relaxed text-text-secondary">Nothing spent yet.</span>
	{:else}
		<span class="mt-2 flex flex-col gap-2.5" data-testid="cycled-breakdown">
			{#each groups as group (group.bucket)}
				<span class="flex flex-col gap-1">
					<span class="flex items-baseline justify-between gap-3">
						<span class="eyebrow">{group.title}</span>
						{#if group.items.length > 1}
							<span class="text-[11px] tabular-nums text-text-tertiary">{formatPed(group.subtotalPed)}</span>
						{/if}
					</span>
					{#each group.items as item (item.label)}
						<span class="flex items-baseline gap-3 text-xs">
							<span class="min-w-0 flex-1 truncate text-text-secondary">{item.label}</span>
							<span class="tabular-nums text-text">{formatPed(item.costPed)}</span>
							<span class="w-10 text-right tabular-nums text-text-tertiary">{item.sharePct.toFixed(0)}%</span>
						</span>
					{/each}
				</span>
			{/each}
		</span>
		<span class="mt-2.5 flex items-baseline justify-between gap-3 border-t border-border/50 pt-2 text-xs">
			<span class="text-text-secondary">Total</span>
			<span class="tabular-nums font-medium text-text">{formatPed(totalPed)} PED</span>
		</span>
	{/if}
	{#if incomplete}
		<span class="mt-2 block text-xs leading-relaxed text-warning">{incomplete}.</span>
	{/if}
	{#if !armourRecorded}
		<span class="mt-2 block text-[11px] leading-relaxed text-text-tertiary">
			Armour and plates join once you record a repair or Trade Terminal reading.
		</span>
	{/if}
</InfoTip>
