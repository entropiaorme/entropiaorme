<script lang="ts">
	/**
	 * List held stock on the auction.
	 *
	 * A listing is not a sale. The quantity leaves holdings now, because in
	 * game it has left the player's inventory, and the starting-bid fee is
	 * spent now whether or not the item ever sells; but no markup is realised
	 * until the auction closes at a price. The modal therefore asks only for
	 * what is known at listing time.
	 *
	 * The fee is entered rather than modelled: the game quotes it live, so a
	 * recorded figure is exact where a predicted one would not be.
	 */
	import Modal from '$lib/components/Modal.svelte';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import { formatPed, todayDate } from '$lib/utils/format';
	import type { ActivityListingDraft, ActivityTradeDraft } from './treeCuttingModel.svelte';
	import type { TreeCuttingStock } from './treeCuttingModel.svelte';

	let {
		item,
		onlist,
		ontrade,
		oncancel,
		// The activity the excess warning names, so the Hunting tab hosts the
		// identical modal over its own vocabulary.
		activityAttributionNoun = 'a tree cutting activity',
	}: {
		item: TreeCuttingStock | null;
		onlist: (input: ActivityListingDraft) => Promise<void>;
		ontrade: (input: ActivityTradeDraft) => Promise<void>;
		oncancel: () => void;
		activityAttributionNoun?: string;
	} = $props();

	let quantity = $state(0);
	let startingBid = $state(0);
	let buyout = $state<number | null>(null);
	let listingFee = $state(0.5);
	let listedAt = $state('');
	let soldFor = $state(0);
	let saleMethod = $state<'auction' | 'trade'>('auction');
	let saving = $state(false);
	let error = $state<string | null>(null);

	let modalOpen = $state(false);
	let initialisedFor = $state<string | null>(null);

	const unitTt = $derived(item && item.heldQty > 0 ? item.heldTt / item.heldQty : 0);
	const listedTt = $derived(quantity * unitTt);
	// Anything beyond the tracked position is real value with no activity
	// claim on it, so the split is stated up front rather than discovered
	// afterwards in the figures.
	const excess = $derived(item ? Math.max(quantity - item.heldQty, 0) : 0);
	// Net of the listing fee, because the fee is spent the moment this listing
	// is created: a preview that quoted the gross would promise more than the
	// player can ever end up holding, and by a margin that matters on a
	// low-markup clear.
	// Markup paid to produce converted stock is recovered before anything
	// counts as gained, in proportion to how much of it is being sold.
	const paidShare = $derived(
		item?.paidMarkup && item.heldQty > 0
			? item.paidMarkup * Math.min(quantity / item.heldQty, 1)
			: 0,
	);
	const netMarkup = $derived(
		(saleMethod === 'trade' ? soldFor : (buyout ?? startingBid)) -
			listedTt -
			paidShare -
			(saleMethod === 'auction' ? listingFee || 0 : 0),
	);

	$effect(() => {
		if (item && initialisedFor !== item.itemName) {
			quantity = item.heldQty;
			startingBid = 0;
			buyout = null;
			listingFee = 0.5;
			soldFor = 0;
			saleMethod = 'auction';
			// Prefilled rather than left blank: listing today is the case, and
			// an empty field reads as unset when the effect is today's date.
			listedAt = todayDate();
			error = null;
			initialisedFor = item.itemName;
			modalOpen = true;
		}
		if (!item) {
			initialisedFor = null;
			modalOpen = false;
		}
	});

	$effect(() => {
		if (item && !modalOpen) oncancel();
	});

	async function confirm() {
		if (!item || saving || quantity <= 0) return;
		saving = true;
		error = null;
		try {
			if (saleMethod === 'auction') {
				await onlist({
					itemName: item.itemName,
					quantity,
					startingBid,
					buyout,
					listingFee,
					listedAt: listedAt || null,
				});
			} else {
				await ontrade({
					itemName: item.itemName,
					quantity,
					soldFor,
					soldAt: listedAt || null,
				});
			}
			modalOpen = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to record the sale';
		} finally {
			saving = false;
		}
	}
</script>

{#if item}
	<Modal bind:open={modalOpen} title={`Sell ${item.itemName}`}>
		<div class="space-y-4">
			<SegmentedControl
				options={[{ id: 'auction', label: 'Auction' }, { id: 'trade', label: 'Trade' }]}
				active={saleMethod}
				onchange={(id) => (saleMethod = id as 'auction' | 'trade')}
			/>
			<div class="bg-surface/50 rounded-md border border-border/50 px-3 py-2 space-y-1.5 text-sm">
				<div class="flex items-center justify-between">
					<span class="text-text-secondary">Held</span>
					<span class="tabular-nums text-text">{item.heldQty} ({formatPed(item.heldTt)} PED)</span>
				</div>
				<div class="flex items-center justify-between">
					<span class="text-text-secondary">{saleMethod === 'auction' ? 'Listing TT' : 'Sale TT'}</span>
					<span class="tabular-nums text-text">{formatPed(listedTt)} PED</span>
				</div>
				{#if paidShare > 0}
					<div class="flex items-center justify-between">
						<span class="text-text-secondary">Markup paid</span>
						<span class="tabular-nums text-text">{formatPed(paidShare)} PED</span>
					</div>
				{/if}
				{#if (saleMethod === 'trade' ? soldFor : (buyout ?? startingBid)) > 0}
					<div class="flex items-center justify-between pt-1.5 border-t border-border/50">
						<span class="text-text font-medium">
							{saleMethod === 'auction' ? 'Net markup if it clears' : 'Realised markup'}
						</span>
						<span class="tabular-nums font-medium {netMarkup >= 0 ? 'text-success' : 'text-error'}">
							{formatPed(netMarkup)} PED
						</span>
					</div>
				{/if}
			</div>

			<label class="block space-y-1">
				<span class="eyebrow text-text-tertiary">Quantity</span>
				<Input type="number" min="0" step="1" bind:value={quantity} />
			</label>

			{#if excess > 0}
				<p class="text-xs text-warning">
					{excess} beyond tracked stock. Its value counts in your ledger, but it cannot be
					attributed to {activityAttributionNoun}, so it will not reach the Realised figures.
				</p>
			{/if}

			{#if saleMethod === 'auction'}
			<div class="grid grid-cols-2 gap-3">
				<label class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Starting bid (PED)</span>
					<Input type="number" min="0" step="0.01" bind:value={startingBid} />
				</label>
				<label class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Buyout (PED, optional)</span>
					<Input type="number" min="0" step="0.01" bind:value={buyout} />
				</label>
			</div>

			<div class="grid grid-cols-2 gap-3">
				<label class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Listing fee (PED)</span>
					<Input type="number" min="0" step="0.01" bind:value={listingFee} />
				</label>
				<label class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Listed on</span>
					<Input type="date" bind:value={listedAt} />
				</label>
			</div>
			<p class="text-xs text-text-tertiary">
				The fee is charged now and stays spent even if the listing expires unsold. Any further fee
				charged when it sells is entered at confirmation.
			</p>
			{:else}
				<div class="grid grid-cols-2 gap-3">
					<label class="block space-y-1">
						<span class="eyebrow text-text-tertiary">Sold for (PED)</span>
						<Input type="number" min="0" step="0.01" bind:value={soldFor} />
					</label>
					<label class="block space-y-1">
						<span class="eyebrow text-text-tertiary">Sold on</span>
						<Input type="date" bind:value={listedAt} />
					</label>
				</div>
			{/if}

			{#if error}
				<p class="text-xs text-error">{error}</p>
			{/if}

			<div class="flex items-center justify-end gap-2 pt-2">
				<Button variant="ghost" onclick={oncancel} disabled={saving}>Cancel</Button>
				<Button onclick={confirm} loading={saving} disabled={quantity <= 0}>
					{saleMethod === 'auction' ? 'List on auction' : 'Record trade'}
				</Button>
			</div>
		</div>
	</Modal>
{/if}
