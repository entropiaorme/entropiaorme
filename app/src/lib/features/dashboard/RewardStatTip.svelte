<script lang="ts">
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { formatPed, formatTtOrCount, isZeroTt, NO_TT_TITLE } from '$lib/utils/format';
	import type { RewardValuation } from './liveLootValuation.svelte';

	/**
	 * What the session's Reward and Reward MU are made of: each line a
	 * hand-in paid, at TT, and the whole at market. A reward is a fixed
	 * amount handed in, never loot and never a return on what was cycled, so
	 * the tip says so where the figure could read as one.
	 */
	let { reward }: { reward: RewardValuation } = $props();

	const lines = $derived(
		[...reward.lines].sort((a, b) => b.valuePed - a.valuePed || a.itemName.localeCompare(b.itemName)),
	);
</script>

<InfoTip label="What the quest reward is made of" align="left" width="w-80">
	<span class="block text-xs font-medium text-text">Quest rewards handed in this session</span>
	<span class="mt-1 block text-xs leading-relaxed text-text-secondary tabular-nums">
		{formatPed(reward.ttPed)} PED at TT, {formatPed(reward.muPed)} PED at market.
	</span>
	<span class="mt-2 flex flex-col gap-1" data-testid="reward-lines">
		{#each lines as line, index (`${line.itemName}:${index}`)}
			<span class="flex items-baseline gap-3 text-xs">
				<span class="min-w-0 flex-1 truncate text-text-secondary">{line.itemName}</span>
				<span
					class="w-16 text-right tabular-nums text-text"
					title={isZeroTt(line.valuePed) ? NO_TT_TITLE : undefined}
				>{formatTtOrCount(line.valuePed, line.quantity)}</span>
			</span>
		{/each}
	</span>
	<span class="mt-2 block text-[11px] leading-relaxed text-text-tertiary">
		Ammo counts at face value and items at their market value. A fixed amount for the hand-in,
		not loot and not a return on what you cycled: it weighs less the more a session cycles.
	</span>
</InfoTip>
