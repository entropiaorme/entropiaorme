/**
 * What the account readings propose for listings still on auction: that
 * one sold (with the PED Card's rise in the same step, when it can be read)
 * or came back. A
 * proposal is a guess. It fills the listing's existing confirmation, and
 * only once the listing's own command has recorded the outcome is the
 * proposal marked recorded; "Not this listing" dismisses it.
 *
 * Kept current by the account topic. Read from the main window only: the
 * account commands are refused to every other window.
 */
import { ACCOUNT_TOPIC, dismissProposal, getProposals, linkProposals } from '$lib/api/account';
import type { AccountProposal } from '$lib/api/commands.gen';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { formatPed, formatSeenAt } from '$lib/utils/format';

export type ListingProposal = AccountProposal & {
	outcome: Extract<AccountProposal['outcome'], { kind: 'listingSold' | 'listingReturned' }>;
};

export function isListingProposal(proposal: AccountProposal): proposal is ListingProposal {
	return proposal.outcome.kind === 'listingSold' || proposal.outcome.kind === 'listingReturned';
}

/** The proposal for each listing: the latest seen, as the list comes. */
export function proposalsByListing(proposals: AccountProposal[]): Map<string, ListingProposal> {
	const found = new Map<string, ListingProposal>();
	for (const proposal of proposals) {
		if (isListingProposal(proposal) && !found.has(proposal.outcome.listingId)) {
			found.set(proposal.outcome.listingId, proposal);
		}
	}
	return found;
}

/** What the account shows, in one sentence, with the window it happened
 * in. */
export function proposalSentence(proposal: ListingProposal, now: Date = new Date()): string {
	const between = `between ${formatSeenAt(proposal.between.from, now)} and ${formatSeenAt(proposal.between.to, now)}`;
	if (proposal.outcome.kind === 'listingReturned') {
		return `Your account shows this came back from the auction ${between}.`;
	}
	const left = `Your account shows this left the auction ${between}.`;
	const proceeds = proposal.outcome.proceeds;
	// The Card's rise, never called the sale price: the auction may have
	// taken its fee first.
	return proceeds
		? `${left} Your PED Card rose by ${formatPed(proceeds.ped)} PED between those two readings: that is the figure filled in, so correct it if the auction took a fee first.`
		: left;
}

export function createListingProposals() {
	const store = createSnapshotStore<AccountProposal[]>(ACCOUNT_TOPIC, () => getProposals(null));
	const byListing = $derived(proposalsByListing(store.current ?? []));

	return {
		/** The pending proposal for a listing, if there is one. */
		forListing(listingId: string): ListingProposal | null {
			return byListing.get(listingId) ?? null;
		},
		refresh(): Promise<void> {
			return store.hydrate();
		},
		/** Mark a proposal recorded once its listing's outcome is. A failure
		 * changes nothing that matters: the outcome stands, and a listing no
		 * longer on auction has nothing pending. */
		async markRecorded(proposal: ListingProposal): Promise<void> {
			try {
				await linkProposals({ ids: [proposal.id], recordId: proposal.outcome.listingId });
			} catch {
				// The listing's outcome is recorded either way.
			}
			await store.hydrate();
		},
		/** "Not this listing". */
		async dismiss(proposal: ListingProposal): Promise<void> {
			await dismissProposal({ id: proposal.id });
			await store.hydrate();
		},
		/** Attach the topic listener, then read; returns the detach. */
		connect(): () => void {
			let detach: (() => void) | undefined;
			let closed = false;
			void store.subscribe().then((unlisten) => {
				if (closed) unlisten();
				else detach = unlisten;
			});
			void store.hydrate();
			return () => {
				closed = true;
				detach?.();
			};
		},
	};
}

export type ListingProposals = ReturnType<typeof createListingProposals>;
