<script lang="ts">
	/**
	 * What looks like a sale to another player in the account readings,
	 * waiting to be recorded or dismissed: one quiet line each, saying what
	 * left, its TT, what the Card rose by, and where recording it would take
	 * it from. Recording opens the trade form, filled in; nothing is saved
	 * from here. Nothing shows while none waits.
	 *
	 * A row stays where it is, still, from the moment it is done with until
	 * the list read again drops it, so nothing jumps under the pointer.
	 */
	import type { InventoryItem } from '$lib/api/commands.gen';
	import Button from '$lib/components/Button.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import type { TreeCuttingStock } from '$lib/features/analytics/treeCuttingModel.svelte';
	import { formatPed, formatSeenAt, formatTt } from '$lib/utils/format';
	import {
		type SaleProposal,
		type SaleProposals,
		type SaleTarget,
		saleTargetFor,
		saleTargetPhrase,
		saleWhat,
	} from './saleProposals.svelte';

	let {
		model,
		loot,
		assets,
		onrecord,
		isNew = () => false,
	}: {
		model: SaleProposals;
		/** The loot and the assets held now, to say where each sale is from. */
		loot: TreeCuttingStock[];
		assets: InventoryItem[];
		/** Open the trade form for a proposal, filled in. */
		onrecord: (proposal: SaleProposal, target: SaleTarget) => void;
		/** Found since the player last looked: marked until they leave. */
		isNew?: (proposal: SaleProposal) => boolean;
	} = $props();

	const now = new Date();
	/** The row asking whether to record again one that may be recorded. */
	let confirming = $state<number | null>(null);

	function record(proposal: SaleProposal, target: SaleTarget) {
		if (proposal.possiblyRecorded && confirming !== proposal.id) {
			confirming = proposal.id;
			return;
		}
		confirming = null;
		onrecord(proposal, target);
	}
</script>

{#if model.pending.length > 0 || model.error}
	<section aria-label="Sales read from your account" class="space-y-2">
		<span class="eyebrow">Looks like a sale to a player</span>
		<ErrorNotice message={model.error} onDismiss={model.dismissError} />
		{#if model.pending.length > 0}
			<ul class="divide-y divide-border/50 border-y border-border/50">
				{#each model.pending as proposal (proposal.id)}
					{@const what = saleWhat(proposal)}
					{@const target = saleTargetFor(proposal.outcome.itemName, loot, assets)}
					{@const settled = model.isSettled(proposal.id)}
					{@const unlinked = model.isUnlinked(proposal.id)}
					<li
						class="flex items-center gap-3 py-2.5 transition-opacity duration-[var(--duration-base)]
							{settled ? 'opacity-50' : ''}"
						aria-busy={settled}
					>
						<div class="min-w-0 flex-1">
							<p class="truncate text-sm text-text-secondary" title={what}>
								{#if isNew(proposal)}<span
										class="mr-1.5 inline-block h-1.5 w-1.5 -translate-y-px rounded-full bg-warning align-middle"
										data-testid="proposal-new"
										><span class="sr-only">New: </span></span
									>{/if}<span class="text-text">{what}</span>, {formatSeenAt(proposal.between.to, now)}
							</p>
							<p class="text-xs tabular-nums text-text-tertiary">
								{formatTt(proposal.outcome.tt.ped)} PED TT, the Card rose by
								{formatPed(proposal.outcome.received.ped)} PED, {saleTargetPhrase(target)}
							</p>
							{#if unlinked}
								<p class="text-xs text-warning">Recorded, but not marked done.</p>
							{:else if confirming === proposal.id}
								<p class="text-xs text-warning">
									This may already be recorded. Check your history; record it again only if it is
									not there.
								</p>
							{:else if proposal.possiblyRecorded}
								<p class="text-xs text-warning">
									This may already be recorded; check your history first.
								</p>
							{/if}
						</div>
						{#if unlinked}
							<Button
								size="sm"
								variant="secondary"
								loading={model.linking === proposal.id}
								disabled={settled || (model.busy && model.linking !== proposal.id)}
								aria-label={`Mark as recorded: ${what}`}
								onclick={() => void model.markRecorded(proposal)}
							>
								Mark as recorded
							</Button>
						{:else}
							{#if target.kind === 'loot' || target.kind === 'asset'}
								<Button
									size="sm"
									variant={proposal.possiblyRecorded ? 'ghost' : 'secondary'}
									loading={model.recording === proposal.id}
									disabled={settled || (model.busy && model.recording !== proposal.id)}
									aria-label={confirming === proposal.id
										? `Record again: ${what}`
										: `Record sale of ${what}`}
									onclick={() => record(proposal, target)}
								>
									{confirming === proposal.id ? 'Record again' : 'Record sale'}
								</Button>
							{/if}
							{#if confirming === proposal.id}
								<Button size="sm" variant="ghost" onclick={() => (confirming = null)}>
									Cancel
								</Button>
							{:else}
								<Button
									size="sm"
									variant="ghost"
									loading={model.dismissing === proposal.id}
									disabled={settled || (model.busy && model.dismissing !== proposal.id)}
									aria-label={`Dismiss ${what}`}
									onclick={() => void model.dismiss(proposal)}
								>
									Dismiss
								</Button>
							{/if}
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</section>
{/if}
