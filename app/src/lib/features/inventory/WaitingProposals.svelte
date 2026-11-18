<script lang="ts">
	/**
	 * What the readings found that waits for the player, besides purchases:
	 * a strongbox opening, a listing's outcome the readings could not
	 * settle, and something that arrived in a tracked session that nothing
	 * explains. One quiet line each, saying what was found and when, with
	 * the way to where it is decided: the strongbox review, the listings,
	 * or the session's minutes. Nothing is recorded from here.
	 */
	import type { AccountProposal } from '$lib/api/commands.gen';
	import Button from '$lib/components/Button.svelte';
	import { formatPed, formatQuantity, formatSeenAt, formatTt } from '$lib/utils/format';

	let {
		proposals,
		isNew = () => false,
		onopenings,
		onlistings,
		onsessions,
	}: {
		proposals: AccountProposal[];
		isNew?: (proposal: AccountProposal) => boolean;
		/** Go to the strongbox review, where openings are recorded. */
		onopenings: () => void;
		/** Go to the listings, where a listing's outcome is confirmed. */
		onlistings: () => void;
		/** Go to the sessions, where an arrival is decided in its minutes. */
		onsessions: () => void;
	} = $props();

	const now = new Date();

	function what(proposal: AccountProposal): string {
		const { outcome } = proposal;
		switch (outcome.kind) {
			case 'opening': {
				const boxes = outcome.boxes.map((box) => `${formatQuantity(box.count)} ${box.name}`);
				return `${boxes.join(' and ')} opened`;
			}
			case 'listingSold':
				return `${formatQuantity(outcome.units)} ${outcome.itemName} sold at auction`;
			case 'listingReturned':
				return `${formatQuantity(outcome.units)} ${outcome.itemName} came back from the auction`;
			case 'arrival':
				return `${formatQuantity(outcome.units)} ${outcome.itemName} arrived in a tracked session`;
			case 'purchase':
				return `${formatQuantity(outcome.units)} ${outcome.itemName} bought`;
		}
	}

	function detail(proposal: AccountProposal): string {
		const { outcome } = proposal;
		switch (outcome.kind) {
			case 'opening':
				return outcome.ammo
					? `${formatPed(outcome.ammo.amount.ped)} PED of ammo back; review it to record`
					: 'it does not balance yet; review what came out to record it';
			case 'listingSold':
				return outcome.proceeds
					? `the Card rose by ${formatPed(outcome.proceeds.ped)} PED; confirm the price`
					: 'the price is yours to say';
			case 'listingReturned':
				return 'confirm it came back';
			case 'arrival':
				return `${formatTt(outcome.tt.ped)} PED TT; decide it in that session's minutes`;
			case 'purchase':
				return '';
		}
	}

	function act(proposal: AccountProposal): { label: string; go: () => void } {
		switch (proposal.outcome.kind) {
			case 'opening':
				return { label: 'Review', go: onopenings };
			case 'listingSold':
			case 'listingReturned':
				return { label: 'Confirm', go: onlistings };
			default:
				return { label: 'Open sessions', go: onsessions };
		}
	}
</script>

{#if proposals.length > 0}
	<ul class="divide-y divide-border/50 border-y border-border/50" data-testid="waiting-proposals">
		{#each proposals as proposal (proposal.id)}
			{@const action = act(proposal)}
			<li class="flex items-center gap-3 py-2.5">
				<div class="min-w-0 flex-1">
					<p class="truncate text-sm text-text-secondary">
						{#if isNew(proposal)}<span
								class="mr-1.5 inline-block h-1.5 w-1.5 -translate-y-px rounded-full bg-warning align-middle"
								data-testid="proposal-new"
								><span class="sr-only">New: </span></span
							>{/if}<span class="text-text">{what(proposal)}</span>, {formatSeenAt(
							proposal.between.to,
							now,
						)}
					</p>
					<p class="truncate text-xs tabular-nums text-text-tertiary">{detail(proposal)}</p>
				</div>
				<Button size="sm" variant="secondary" onclick={action.go}>{action.label}</Button>
			</li>
		{/each}
	</ul>
{/if}
