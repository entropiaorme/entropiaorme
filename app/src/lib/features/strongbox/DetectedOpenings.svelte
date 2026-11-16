<script lang="ts">
	/**
	 * Openings the account readings found that wait to be reviewed: one quiet
	 * line each, to review as a pasted export would be, or to dismiss. An
	 * opening the readings could not settle says so, and one set aside for a
	 * recording that was never linked to it says it may be recorded already;
	 * nothing here is recorded until the player records it from the review.
	 */
	import Button from '$lib/components/Button.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import { detectedWhat, detectedWhen } from './detectedOpenings';
	import type { OpeningModel } from './openingModel.svelte';

	let { model }: { model: OpeningModel } = $props();

	const now = new Date();
</script>

{#if model.detected.length > 0 || model.proposalError}
	<section aria-label="Openings read from your account" class="space-y-2">
		<span class="eyebrow">Read from your account</span>
		<ErrorNotice message={model.proposalError} />
		<ul class="divide-y divide-border/50 border-y border-border/50">
			{#each model.detected as proposal (proposal.id)}
				{@const unsettled = proposal.outcome.settlement === 'unsettled'}
				<li class="flex items-center gap-3 py-2.5">
					<div class="min-w-0 flex-1">
						<p class="truncate text-sm text-text-secondary" title={detectedWhat(proposal)}>
							<span class="text-text">{detectedWhat(proposal)}</span>, {detectedWhen(proposal, now)}
							{#if unsettled}<span class="text-warning">, not settled</span>{/if}
						</p>
						{#if proposal.possiblyRecorded}
							<p class="text-xs text-warning">This may already be recorded; check your openings first.</p>
						{/if}
					</div>
					<Button
						size="sm"
						variant={proposal.possiblyRecorded ? 'ghost' : 'secondary'}
						disabled={model.reading || model.dismissing !== null}
						aria-label={`Review ${detectedWhat(proposal)}`}
						onclick={() => void model.reviewProposal(proposal.id)}
					>
						Review
					</Button>
					<Button
						size="sm"
						variant="ghost"
						loading={model.dismissing === proposal.id}
						disabled={model.dismissing !== null && model.dismissing !== proposal.id}
						aria-label={`Dismiss ${detectedWhat(proposal)}`}
						onclick={() => void model.dismissProposal(proposal.id)}
					>
						Dismiss
					</Button>
				</li>
			{/each}
		</ul>
	</section>
{/if}
