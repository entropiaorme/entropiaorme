<script lang="ts">
	/**
	 * How one recording was spread: each session it reached, by hits taken,
	 * and within a session each stretch of play its hits landed in. The
	 * session figures are exact; a stretch's share is the hit-weighted
	 * estimate the recording model accepts in exchange for never asking what
	 * was worn.
	 */
	import type { ProtectionCostAllocation, ProtectionCostWindow } from '$lib/api';
	import { formatPed } from '$lib/utils/format';
	import { formatSessionStart } from './armourRecording';

	let { window }: { window: ProtectionCostWindow } = $props();

	function sessionLabel(allocation: ProtectionCostAllocation): string {
		return allocation.sessionName ?? allocation.definitionName ?? 'Session';
	}

	function hitsText(hits: number): string {
		return `${hits.toLocaleString()} ${hits === 1 ? 'hit' : 'hits'}`;
	}

	function percent(share: number): string {
		return `${(share * 100).toFixed(share < 0.1 ? 1 : 0)}%`;
	}

	/** Stretches worth listing: more than one, or a single named one. */
	function stretches(allocation: ProtectionCostAllocation) {
		const named = allocation.contexts.some((context) => context.label !== null);
		return allocation.contexts.length > 1 || named ? allocation.contexts : [];
	}
</script>

<div class="mt-2.5 ml-3 border-l border-border/70 pl-4" data-testid="protection-recording-detail">
	{#each window.allocations as allocation (allocation.sessionId)}
		<div class="py-1.5">
			<div class="grid grid-cols-[minmax(0,1fr)_90px_50px_90px] items-baseline gap-4">
				<div class="min-w-0 truncate">
					<span class="text-text-secondary">{sessionLabel(allocation)}</span>
					<span class="text-text-tertiary"> · {formatSessionStart(allocation.startedAt)}</span>
				</div>
				<span class="text-right tabular-nums text-text-tertiary">{hitsText(allocation.hitCount)}</span>
				<span class="text-right tabular-nums text-text-tertiary">{percent(allocation.allocationShare)}</span>
				<span class="text-right tabular-nums text-text-secondary">{formatPed(allocation.costPed)} PED</span>
			</div>
			{#each stretches(allocation) as context, index (index)}
				<div class="grid grid-cols-[minmax(0,1fr)_90px_50px_90px] items-baseline gap-4 pl-4 text-[11px]">
					<span class="min-w-0 truncate text-text-tertiary">{context.label ?? 'Outside any segment'}</span>
					<span class="text-right tabular-nums text-text-tertiary">{hitsText(context.hitCount)}</span>
					<span></span>
					<span class="text-right tabular-nums text-text-tertiary">{formatPed(context.costPed)} PED</span>
				</div>
			{/each}
		</div>
	{/each}
</div>
