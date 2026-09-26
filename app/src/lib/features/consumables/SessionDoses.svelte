<script lang="ts">
	import { ErrorNotice } from '$lib/components';
	import { formatPed } from '$lib/utils/format';
	import { describeDoseCost, describeEffects, describeSource, formatDuration } from './doses';
	import type { SessionDosesModel } from './sessionDosesModel.svelte';

	let { model }: { model: SessionDosesModel } = $props();

	function clock(epochSeconds: number): string {
		return new Date(epochSeconds * 1000).toLocaleTimeString([], {
			hour: '2-digit',
			minute: '2-digit',
		});
	}
</script>

{#if model.doses.length > 0}
	<section class="mt-4" data-testid="session-doses">
		<div class="flex items-baseline justify-between gap-4 mb-2">
			<h3 class="eyebrow">Doses</h3>
			{#if model.bookedPed > 0}
				<span class="text-xs text-text-secondary tabular-nums">{formatPed(model.bookedPed)} booked</span>
			{/if}
		</div>
		{#if model.error}
			<ErrorNotice message={model.error} onDismiss={model.dismissError} />
		{/if}
		<ul class="divide-y divide-border/40">
			{#each model.doses as dose (dose.id)}
				{@const removed = dose.removedAt !== null}
				<li class="flex items-center gap-4 py-2 text-xs" data-testid="session-dose">
					<span class="tabular-nums text-text-tertiary shrink-0 w-12">{clock(dose.startedAt)}</span>
					<div class="min-w-0 flex-1 {removed ? 'opacity-50' : ''}">
						<div class="text-sm text-text truncate {removed ? 'line-through' : ''}">{dose.itemName}</div>
						<div class="text-text-tertiary truncate">
							{describeEffects(dose.effects) || 'No evaluated effect'} · {formatDuration(dose.endsAt - dose.startedAt)}{dose.replaced
								? ' (replaced by a re-dose)'
								: ''} · {describeSource(dose)}
						</div>
					</div>
					<span class="tabular-nums shrink-0 {removed ? 'text-text-tertiary' : 'text-text-secondary'}">
						{removed ? 'Removed' : describeDoseCost(dose)}
					</span>
					{#if dose.source === 'on_use'}
						<!-- A heal's buff goes with its heal: its correction is in Healing. -->
						<span class="w-14 shrink-0"></span>
					{:else if removed}
						<button
							type="button"
							class="linklet shrink-0 w-14 text-right"
							disabled={model.busy !== null}
							onclick={() => void model.restore(dose)}
						>
							Restore
						</button>
					{:else}
						<button
							type="button"
							class="linklet linklet-danger shrink-0 w-14 text-right"
							disabled={model.busy !== null}
							title="Remove this dose (a misclick): its cost comes off the session"
							onclick={() => void model.remove(dose)}
						>
							Remove
						</button>
					{/if}
				</li>
			{/each}
		</ul>
	</section>
{/if}
