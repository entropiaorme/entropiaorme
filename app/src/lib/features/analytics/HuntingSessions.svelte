<script lang="ts">
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { formatPed } from '$lib/utils/format';
	import ActivityLootComposition from './ActivityLootComposition.svelte';
	import HuntingSessionActivities from './HuntingSessionActivities.svelte';
	import type { HuntingSessionSection } from './huntingModel.svelte';

	let { selected }: { selected: HuntingSessionSection } = $props();

	const hasDeclaredActivities = $derived(
		selected?.activities.some((activity) => !activity.isUnscoped) ?? false,
	);
</script>

{#if selected.isUnassigned}
	<div class="flex min-h-28 items-center justify-center border-t border-border/50 pt-5">
		<div class="flex items-center gap-1.5 text-sm text-text-secondary">
			<span>Hunting outside a defined session cannot join the routine comparison.</span>
			<InfoTip label="What unassigned hunting means" width="w-80">
				<p class="text-xs font-semibold leading-relaxed text-text">Hunting without a repeatable routine</p>
				<p class="mt-1 text-xs leading-relaxed text-text-secondary">Its cost, overhead and loot still count in Overall. Without a session definition there is no deliberate activity to rank, so this diagnostic row carries no economic claim of its own.</p>
			</InfoTip>
		</div>
	</div>
	{#if selected.overhead > 0}
		<!-- What these sessions cost beyond their hunting: a cost Overall
		     takes off its Realised Net, shown so the bucket's share is seen. -->
		<p class="text-center text-xs text-text-tertiary" data-testid="unassigned-overhead">
			Overhead of these sessions: <span class="tabular-nums text-text-secondary">{formatPed(selected.overhead)} PED</span>,
			taken off Overall's Realised Net.
		</p>
	{/if}
{:else}
	<ActivityLootComposition
		items={selected.items}
		marketAvailable={selected.muProjectedReturns !== null}
		emptyLabel="No loot recorded for this session yet."
		disclosure="session"
	/>
	{#if hasDeclaredActivities}
		<div class="border-t border-border/50 pt-5">
			<HuntingSessionActivities
				activities={selected.activities}
				marketAvailable={selected.muProjectedReturns !== null}
			/>
		</div>
	{/if}
{/if}
