<script lang="ts">
	import type { SessionQuestReward } from '$lib/api';
	import Divider from '$lib/components/Divider.svelte';
	import { formatPed, formatTtOrCount, isZeroTt, NO_TT_TITLE } from '$lib/utils/format';

	/**
	 * What a recorded session's hand-ins paid: one line per completed quest,
	 * when it was handed in, its total at TT, and what it was made of. A
	 * reward is never loot, so it stands apart from the loot breakdown and
	 * outside Loot TT, Net and Rate. Renders nothing for a session that
	 * handed nothing in.
	 */
	let { rewards }: { rewards: readonly SessionQuestReward[] } = $props();

	const rows = $derived(
		rewards.map((reward) => ({
			...reward,
			totalPed: reward.items.reduce((sum, item) => sum + item.valuePed, 0),
			at: new Date(reward.completedAt * 1000).toLocaleTimeString([], {
				hour: '2-digit',
				minute: '2-digit',
			}),
		})),
	);
</script>

{#if rows.length > 0}
	<Divider />
	<section aria-label="Quest rewards" data-testid="session-quest-rewards">
		<h3 class="eyebrow mb-3">Quest rewards</h3>
		<ul class="flex flex-col gap-2 pl-1">
			{#each rows as reward (reward.completionId)}
				<li class="text-xs">
					<p class="flex items-baseline gap-2 min-w-0">
						<span class="truncate font-medium text-text">{reward.questName || 'Quest'}</span>
						<span class="shrink-0 tabular-nums text-text-tertiary">{reward.at}</span>
						<span class="ml-auto shrink-0 tabular-nums text-text">{formatPed(reward.totalPed)} PED</span>
					</p>
					<p class="mt-0.5 flex flex-wrap gap-x-3 gap-y-0.5 text-text-secondary">
						{#each reward.items as item, index (`${item.itemName}:${index}`)}
							<span>
								{item.itemName}
								<span
									class="tabular-nums text-text-tertiary"
									title={isZeroTt(item.valuePed) ? NO_TT_TITLE : undefined}
								>{formatTtOrCount(item.valuePed, item.quantity)}</span>
							</span>
						{/each}
					</p>
				</li>
			{/each}
		</ul>
		<p class="mt-2 pl-1 text-[11px] text-text-tertiary">
			Not loot: Loot TT, Net and Return leave it out.
		</p>
	</section>
{/if}
