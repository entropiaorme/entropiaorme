<script lang="ts">
	/**
	 * The Inventory's history, split by who decided.
	 *
	 * "From your account" is what the readings did with nobody looking: a
	 * "Waiting for you" group of what they found that needs a decision,
	 * counted until decided, then one designed event for each thing they
	 * recorded on their own. "Reviewed" is what the player applied after a
	 * review: each stock check, and each strongbox opening. "Your entries"
	 * is what the player entered by hand. The Loot and Assets filter holds
	 * inside each, except over what waits: a decision is never hidden behind
	 * a filter.
	 *
	 * Opening "From your account" tells the app the player has seen what is
	 * shown, which clears the marks on the sidebar and the History tab. What
	 * was new when the player arrived keeps its own mark until they leave
	 * the view. Anything recorded while they look is marked new as it
	 * arrives, and is seen in turn.
	 */
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { ACCOUNT_TOPIC, getProposals } from '$lib/api/account';
	import type { AccountProposal, InventoryItem } from '$lib/api/commands.gen';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import ActivityHistory from '$lib/features/analytics/ActivityHistory.svelte';
	import { activityMarks } from '$lib/features/account/activityMarks.svelte';
	import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
	import AccountEventCard from './AccountEventCard.svelte';
	import PurchaseProposals from './PurchaseProposals.svelte';
	import ReviewedEventCard from './ReviewedEventCard.svelte';
	import SaleProposals from './SaleProposals.svelte';
	import WaitingProposals from './WaitingProposals.svelte';
	import {
		accountEventShowsUnder,
		isNewSince,
		newestShown,
		recordedAt,
		reviewedEventShowsUnder,
		splitHistory,
		waitingOf,
		type AccountEvent,
		type HistorySpace,
		type ReviewedEvent,
	} from './accountActivity';
	import type { InventoryKind, InventoryModel } from './inventoryModel.svelte';
	import type {
		PurchaseProposal,
		PurchaseProposals as PurchaseProposalsModel,
		PurchaseTarget,
	} from './purchaseProposals.svelte';
	import type {
		SaleProposal,
		SaleProposals as SaleProposalsModel,
		SaleTarget,
	} from './saleProposals.svelte';
	import type { TreeCuttingStock } from '$lib/features/analytics/treeCuttingModel.svelte';

	let {
		model,
		purchases,
		sales,
		loot,
		assets,
		onrecordpurchase,
		onrecordsale,
		onlistings,
	}: {
		model: InventoryModel;
		purchases: PurchaseProposalsModel;
		sales: SaleProposalsModel;
		loot: TreeCuttingStock[];
		assets: InventoryItem[];
		/** Open the asset form for a purchase proposal, filled in. */
		onrecordpurchase: (proposal: PurchaseProposal, target: PurchaseTarget) => void;
		/** Open the trade form for a sale proposal, filled in. */
		onrecordsale: (proposal: SaleProposal, target: SaleTarget) => void;
		/** Show the listings, where a listing's outcome is confirmed. */
		onlistings: () => void;
	} = $props();

	let space = $state<HistorySpace>('account');

	/** Every proposal still open, of every kind but purchases and sales:
	 * those have their own rows, with the asset and trade forms behind them. */
	const proposals = createSnapshotStore<AccountProposal[]>(ACCOUNT_TOPIC, () =>
		getProposals(null),
	);
	onMount(() => proposals.connect());

	const split = $derived(splitHistory(model.allHistory, model.openings, model.allListings));
	const allWaiting = $derived(
		waitingOf(
			(proposals.current ?? []).filter(
				(p) => p.outcome.kind !== 'purchase' && p.outcome.kind !== 'sale',
			),
		),
	);
	const side = $derived(model.kind);
	const accountEvents = $derived(split.account.filter((e) => accountEventShowsUnder(e, side)));
	const reviewedEvents = $derived(split.reviewed.filter((e) => reviewedEventShowsUnder(e, side)));
	const yours = $derived.by(() => {
		const ids = new Set(split.yours.map((entry) => entry.id));
		return model.history.filter((entry) => ids.has(entry.id));
	});

	/** How far the player had seen when they opened the space: what came
	 * after is marked until they leave it. */
	let seenAtEntry = $state<string | null>(null);
	$effect(() => {
		if (space !== 'account') {
			seenAtEntry = null;
			return;
		}
		const marks = activityMarks.current;
		if (seenAtEntry === null && marks) seenAtEntry = marks.seenThrough;
	});

	/** Everything shown in the space is seen: the watermark moves to the
	 * newest of it, whichever side the filter shows. */
	$effect(() => {
		if (space !== 'account' || seenAtEntry === null || !model.historyLoaded) return;
		const newest = newestShown(split.account, [
			...allWaiting,
			...purchases.pending,
			...sales.pending,
		]);
		const seen = activityMarks.current?.seenThrough ?? null;
		if (newest && isNewSince(newest, seen)) void activityMarks.seen(newest);
	});

	const markFrom = $derived(space === 'account' ? seenAtEntry : (activityMarks.current?.seenThrough ?? null));
	const eventIsNew = (event: AccountEvent) => isNewSince(recordedAt(event), markFrom);
	const proposalIsNew = (proposal: AccountProposal) => isNewSince(proposal.detectedAt, markFrom);

	/** What the "From your account" toggle counts: what waits, until
	 * decided, and what is new. */
	const accountCount = $derived(
		allWaiting.length +
			purchases.pending.length +
			sales.pending.length +
			split.account.filter((event) => eventIsNew(event)).length,
	);

	const spaces = $derived([
		{ id: 'account', label: 'From your account', attention: accountCount },
		{ id: 'reviewed', label: 'Reviewed' },
		{ id: 'yours', label: 'Your entries' },
	]);
	const kindOptions = [
		{ id: 'loot', label: 'Loot' },
		{ id: 'equipment', label: 'Assets' },
	];

	async function undoAccountEvent(event: AccountEvent) {
		switch (event.kind) {
			case 'recycling':
			case 'shrapnel':
				return model.undo(event.entry, false);
			case 'sale':
				for (const entry of event.entries) {
					if (!entry.undone && entry.canDelete) await model.undo(entry, false);
				}
				return;
			case 'listed':
				return model.undo(event.entry, false);
			case 'listingSold':
				return model.undo(event.entry, true);
			case 'listingReturned':
				return model.reopenListing(event.entry.id);
		}
	}

	async function undoReviewed(event: ReviewedEvent) {
		if (event.kind === 'check') return model.undoCheck(event.reconciliationId);
		return model.undoOpening(event.opening.id);
	}

	const waitingCount = $derived(
		allWaiting.length + purchases.pending.length + sales.pending.length,
	);
</script>

<div class="flex flex-wrap items-center justify-between gap-3 py-3">
	<SegmentedControl
		options={spaces}
		active={space}
		size="md"
		label="Whose history"
		onchange={(id) => (space = id as HistorySpace)}
	/>
	<SegmentedControl
		options={kindOptions}
		active={model.kind}
		size="md"
		label="Loot or assets"
		onchange={(id) => (model.kind = id as InventoryKind)}
	/>
</div>

{#if model.historyLoading && !model.historyLoaded}
	<p class="py-10 text-center text-sm text-text-tertiary">Reading what has been recorded...</p>
{:else if space === 'account'}
	<div class="flex max-h-[36rem] flex-col gap-6 overflow-y-auto pb-2" data-testid="account-space">
		{#if waitingCount > 0 || purchases.error || sales.error}
			<section aria-label="Waiting for you" class="space-y-3">
				<h2 class="flex items-baseline gap-2 text-sm font-medium tracking-tight text-text">
					Waiting for you
					<span class="text-xs tabular-nums text-warning">{waitingCount}</span>
				</h2>
				<PurchaseProposals
					model={purchases}
					{assets}
					onrecord={onrecordpurchase}
					isNew={proposalIsNew}
				/>
				<SaleProposals
					model={sales}
					{loot}
					{assets}
					onrecord={onrecordsale}
					isNew={proposalIsNew}
				/>
				<WaitingProposals
					proposals={allWaiting}
					isNew={proposalIsNew}
					onopenings={() => goto('/inventory/strongboxes')}
					{onlistings}
					onsessions={() => goto('/analytics')}
				/>
			</section>
		{/if}

		{#if accountEvents.length > 0}
			<section aria-label="Recorded from your account" class="flex flex-col gap-2.5">
				{#if waitingCount > 0}
					<h2 class="text-sm font-medium tracking-tight text-text">Recorded on its own</h2>
				{/if}
				{#each accountEvents as event (event.key)}
					<AccountEventCard {event} isNew={eventIsNew(event)} onundo={undoAccountEvent} />
				{/each}
			</section>
		{:else if waitingCount === 0}
			<div class="flex min-h-40 items-center justify-center p-6">
				<p class="max-w-sm text-center text-sm leading-relaxed text-text-tertiary">
					Nothing from your account here yet. What your readings record on their own, and
					what they find for you to decide, shows here.
				</p>
			</div>
		{/if}
	</div>
{:else if space === 'reviewed'}
	{#if reviewedEvents.length > 0}
		<div class="flex max-h-[36rem] flex-col gap-2.5 overflow-y-auto pb-2" data-testid="reviewed-space">
			{#each reviewedEvents as event (event.key)}
				<ReviewedEventCard {event} onundo={undoReviewed} />
			{/each}
		</div>
	{:else}
		<div class="flex min-h-40 items-center justify-center p-6">
			<p class="max-w-sm text-center text-sm leading-relaxed text-text-tertiary">
				No stock checks or strongbox openings recorded yet.
			</p>
		</div>
	{/if}
{:else}
	<ActivityHistory
		entries={yours}
		loading={model.historyLoading}
		onundo={model.undo}
		onundocheck={model.undoCheck}
		embedded
	/>
{/if}
