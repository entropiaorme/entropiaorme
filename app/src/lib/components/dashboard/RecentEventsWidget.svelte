<script lang="ts">
	import type { RecentEvent } from '$lib/api';
	import { eventToneClass, formatEventTime } from '$lib/features/dashboard/recentEvents';
	import { formatPed } from '$lib/utils/format';

	let { events }: { events: RecentEvent[] } = $props();
</script>

<div class="flex-1 min-h-0 flex flex-col" data-testid="dashboard-recent-events">
	{#if events.length > 0}
		<ul class="flex-1 min-h-0 overflow-y-auto divide-y divide-border/40">
			{#each events as event (event.id)}
				{@const time = formatEventTime(event.timestamp)}
				<li class="flex items-center gap-3 py-2 text-sm">
					<span class="w-1.5 h-1.5 rounded-full shrink-0 {eventToneClass(event.type)}"></span>
					<span class="text-text-secondary truncate">{event.description}</span>
					<span class="ml-auto flex items-baseline gap-4 shrink-0">
						{#if event.value}
							<span class="text-xs text-text font-medium tabular-nums">{formatPed(event.value)} PED</span>
						{/if}
						{#if time}
							<time class="w-16 text-right text-xs text-text-tertiary tabular-nums" datetime={event.timestamp ?? undefined}>{time}</time>
						{/if}
					</span>
				</li>
			{/each}
		</ul>
	{:else}
		<div class="flex-1 flex items-center justify-center">
			<p class="text-text-tertiary text-sm">No recent events.</p>
		</div>
	{/if}
</div>
