<script lang="ts">
	/**
	 * The auction worklist and its history.
	 *
	 * Open listings are goods in transit: the stock has left the player's
	 * inventory at a price nobody knows yet. This panel is the only place that
	 * position is visible, and the only place a sale becomes real.
	 *
	 * It takes the sub-activity surface's own two-pane shape (selectable list on
	 * the left, detail on the right) because the toggle swaps between them:
	 * the same frame with different contents reads as one surface, where a
	 * second layout would read as a different page.
	 *
	 * Confirming asks for the price the auction actually fetched and the
	 * additional fee charged at the point of sale, because neither is knowable
	 * at listing time.
	 */
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import Input from '$lib/components/Input.svelte';
	import StatDisplay from '$lib/components/StatDisplay.svelte';
	import type { AuctionListing } from '$lib/types/analytics';
	import {
		NO_DATA,
		NO_TT_TITLE,
		formatLedgerDate,
		formatPed,
		formatPercent,
		formatQuantity,
		isZeroTt,
	} from '$lib/utils/format';
	import { hasRunOut } from './listingLifecycle';

	let {
		open,
		resolved,
		onresolve,
		// The activity-specific nouns in the explanatory copy, so the Hunting
		// tab hosts the identical surface over its own vocabulary. Defaults
		// keep Tree Cutting's established wording.
		activityNoun = 'tree cutting',
		sourceNounPlural = 'the board activities',
		sourceNounIndefinite = 'a board activity',
		emptyLead = 'Selling harvested stock',
		expiredChargeNote = 'No board activity is charged for it: not selling describes the market ' +
			'and the price you asked, not the harvesting that produced the stock.',
		embedded = false,
		central = false,
	}: {
		open: AuctionListing[];
		resolved: AuctionListing[];
		onresolve: (
			listingId: string,
			outcome:
				| { sold: true; finalPrice: number; saleFee: number; resolvedAt?: string }
				| { sold: false; resolvedAt?: string },
		) => Promise<void>;
		activityNoun?: string;
		sourceNounPlural?: string;
		sourceNounIndefinite?: string;
		emptyLead?: string;
		expiredChargeNote?: string;
		embedded?: boolean;
		central?: boolean;
	} = $props();

	const signedPed = (value: number) => `${value >= 0 ? '+' : ''}${formatPed(value)}`;
	const netTone = (value: number) => (value >= 0 ? 'text-positive' : 'text-negative');

	// The list's column widths, declared once so the header and the rows cannot
	// drift apart. Each gives ground in proportion to its own width as the pane
	// narrows, rather than the item name collapsing to nothing and spilling
	// over its neighbour. Kept in step with the sub-activity list the toggle
	// swaps from.
	const COL_NAME = 'min-w-0 flex-[1_1_6rem]';
	const COL_QTY = 'min-w-0 flex-[0_1_3rem]';
	const COL_TT = 'min-w-0 flex-[0_1_3.5rem]';
	const COL_LISTED = 'min-w-0 flex-[0_1_5.75rem]';

	// Open auctions lead: they are the only rows that still need a decision.
	const allListings = $derived([...open, ...resolved]);

	// Read once per mount rather than per row: a panel that changed its mind
	// about what time it is halfway down the list would be worse than one that
	// is a few hours stale, and the question it raises is not time-critical.
	const now = Date.now();

	let selectedId = $state<string | null>(null);
	// A stale id (a listing resolved out of the open group, or a reload)
	// degrades to the first row rather than an empty pane.
	const selected = $derived(
		allListings.find((listing) => listing.id === selectedId) ?? allListings[0] ?? null,
	);

	// The whole gain the sale produced, and the part of it an activity may
	// claim. They differ only when the listing ran past tracked stock, and the
	// difference is worth naming rather than leaving as an unexplained gap
	// between the ledger and the activity's Realised figures.
	const netMarkup = $derived(
		selected && selected.status === 'sold'
			? (selected.grossMarkup ?? 0) - selected.listingFee - (selected.saleFee ?? 0)
			: 0,
	);
	const unattributedMarkup = $derived(netMarkup - (selected?.activityNetMarkup ?? 0));
	const listingNet = (listing: AuctionListing) =>
		(listing.grossMarkup ?? 0) - listing.listingFee - (listing.saleFee ?? 0);

	// What the auction actually fetched, as a rate on the listing's TT: the
	// same 100%-is-TT reading the rest of Analytics uses for markup, so a sale
	// can be held against the market figure that motivated it. Gross of fees,
	// because it describes the price, not what was kept from it.
	const saleMarkupRate = $derived(
		selected && selected.status === 'sold' && !isZeroTt(selected.ttValue)
			? (selected.finalPrice ?? 0) / selected.ttValue
			: null,
	);
	/** A listing of an item with no TT is read by count: a rate on TT says
	 * nothing about it, and what each unit fetched says everything. */
	const selectedNoTt = $derived(selected !== null && isZeroTt(selected.ttValue));
	const pricePerUnit = $derived(
		selected && selected.status === 'sold' && selected.quantity > 0
			? (selected.finalPrice ?? 0) / selected.quantity
			: null,
	);
	/** What was listed: how many, wherever that is not simply one. */
	const listingTitle = (listing: AuctionListing) =>
		listing.subjectKind === 'equipment' && Math.abs(listing.quantity - 1) < 1e-9
			? listing.itemName
			: `${formatQuantity(listing.quantity)} \u00d7 ${listing.itemName}`;

	let confirming = $state(false);
	let finalPrice = $state(0);
	let saleFee = $state(0);
	let resolvedAt = $state('');
	let busy = $state(false);
	let error = $state<string | null>(null);

	function select(listing: AuctionListing) {
		selectedId = listing.id;
		confirming = false;
		error = null;
	}

	function startConfirm() {
		if (!selected) return;
		// Pre-filled with the buyout when there was one, since an auction that
		// clears instantly is the common case. Still editable.
		finalPrice = selected.buyout ?? selected.startingBid;
		saleFee = 0;
		resolvedAt = '';
		error = null;
		confirming = true;
	}

	async function resolve(
		outcome:
			| { sold: true; finalPrice: number; saleFee: number; resolvedAt?: string }
			| { sold: false; resolvedAt?: string },
	) {
		if (!selected || busy) return;
		busy = true;
		error = null;
		try {
			await onresolve(selected.id, outcome);
			confirming = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to resolve the listing';
		} finally {
			busy = false;
		}
	}

	const STATUS_LABEL: Record<string, string> = {
		pending: 'On auction',
		sold: 'Sold',
		expired: 'Expired',
	};
</script>

{#snippet listingRow(listing: AuctionListing, isSelected: boolean)}
	<li>
		<button
			type="button"
			aria-pressed={isSelected}
			onclick={() => select(listing)}
			class="w-full flex items-center gap-2 px-3 py-2 text-left
				transition-[background-color,border-color] duration-[var(--duration-base)] ease-[var(--ease-out)]
				{central
				? `border-b border-border/30 ${isSelected ? 'border-l-2 border-l-accent bg-accent/[0.07]' : 'border-l-2 border-l-transparent hover:bg-surface-hover/40'}`
				: `rounded-lg border ${isSelected ? 'border-accent/40 bg-accent/[0.08]' : 'border-transparent hover:border-border/40 hover:bg-surface-hover/40'}`}"
		>
			<span
				class="{COL_NAME} truncate text-sm font-medium tracking-tight
					{listing.status === 'pending' ? 'text-text' : 'text-text-tertiary'}"
				title={listing.itemName}
			>
				{listing.itemName}
			</span>
			<span class="{COL_QTY} truncate text-right text-xs tabular-nums text-text-secondary">
				{formatQuantity(listing.quantity)}
			</span>
			{#if isZeroTt(listing.ttValue)}
				<span class="{COL_TT} truncate text-right text-xs text-text-tertiary" title={NO_TT_TITLE}>
					No TT
				</span>
			{:else}
				<span class="{COL_TT} truncate text-right text-xs tabular-nums text-text">
					{formatPed(listing.ttValue)}
				</span>
			{/if}
			<span class="{COL_LISTED} truncate text-right text-xs tabular-nums font-medium">
				{#if listing.status === 'sold'}
					<span class={netTone(central ? listingNet(listing) : (listing.activityNetMarkup ?? 0))}>
						{signedPed(central ? listingNet(listing) : (listing.activityNetMarkup ?? 0))}
					</span>
				{:else if listing.status === 'expired'}
					<span class="text-text-tertiary">{NO_DATA}</span>
				{:else if hasRunOut(listing, now)}
					<span class="font-normal text-warning/80">Ran out</span>
				{:else}
					<span class="text-text-tertiary">{formatLedgerDate(listing.listedAt)}</span>
				{/if}
			</span>
		</button>
	</li>
{/snippet}

{#snippet content()}
	{#if allListings.length === 0}
		<div class="flex min-h-40 items-center justify-center p-6">
			<div class="flex items-center gap-1.5 text-sm text-text-tertiary">
				<span>Nothing has been listed on the auction yet.</span>
				<InfoTip label="How listings work" width="w-80">
					<p class="text-xs font-semibold leading-relaxed text-text">{emptyLead}</p>
					<p class="mt-1 text-xs leading-relaxed text-text-secondary">
						Sell an item from Inventory to list it here. The position leaves your inventory
						straight away, because in game it has left your inventory, and the starting-bid fee is
						spent whether or not it sells.
					</p>
					<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
						Nothing is realised while a listing is open: an auction has no price until it closes.
						Confirm it when it sells, or mark it expired when it comes back.
					</p>
				</InfoTip>
			</div>
		</div>
	{:else}
		<!-- Kept in step with the sub-activity surface the toggle swaps from: same
			frame, so the hairline must not move when the contents do, and both
			panes narrow together as the card does. -->
		<div class="grid sm:grid-cols-[46%_minmax(0,1fr)]">
			<div class="min-w-0 border-b border-border/40 sm:border-b-0 sm:border-r">
				<div class="px-2 pt-4">
					<div class="flex items-center gap-2 px-3 pb-2 text-text-tertiary">
						<span class="eyebrow {COL_NAME}">Item</span>
						<span class="eyebrow {COL_QTY} text-right">Qty</span>
						<span class="eyebrow {COL_TT} text-right">TT</span>
						<span class="eyebrow {COL_LISTED} text-right">Listed / MU</span>
					</div>
				</div>

				<div class="flex max-h-[32rem] flex-col overflow-y-auto px-2 pb-3">
					{#if open.length > 0}
						<span class="eyebrow px-3 pb-1 text-text-tertiary">On auction</span>
						<ul class={central ? 'flex flex-col' : 'flex flex-col gap-1'}>
							{#each open as listing (listing.id)}
								{@render listingRow(listing, listing.id === selected?.id)}
							{/each}
						</ul>
					{/if}
					{#if resolved.length > 0}
						<span class="eyebrow px-3 pb-1 pt-3 text-text-tertiary">Resolved</span>
						<ul class={central ? 'flex flex-col' : 'flex flex-col gap-1'}>
							{#each resolved as listing (listing.id)}
								{@render listingRow(listing, listing.id === selected?.id)}
							{/each}
						</ul>
					{/if}
				</div>
			</div>

			{#if selected}
				<div class="min-w-0 p-5">
					<div class="mb-4 flex items-baseline justify-between gap-3">
						<div class="min-w-0">
							<p class="truncate text-sm font-medium tracking-tight text-text">
								{listingTitle(selected)}
							</p>
							<p class="mt-0.5 text-xs text-text-tertiary">
								Listed {formatLedgerDate(selected.listedAt)}{selected.resolvedAt
									? `, ${selected.status === 'sold' ? 'sold' : 'returned'} ${formatLedgerDate(selected.resolvedAt)}`
									: ''}
							</p>
						</div>
						<span
							class="shrink-0 text-xs font-medium uppercase tracking-wide
								{selected.status === 'sold'
								? 'text-positive'
								: selected.status === 'pending'
									? 'text-accent'
									: 'text-text-tertiary'}"
						>
							{STATUS_LABEL[selected.status] ?? selected.status}
						</span>
					</div>

					<!-- A resolved sale reports what it got; an open one reports what
						it asked. Carrying the asking prices into a sold listing pushes
						the figures that matter onto a third row for context nobody
						re-reads once the price is known. -->
					{#if selected.status === 'sold'}
						<div class="grid grid-cols-3 gap-x-5 gap-y-4">
							{#if selected.subjectKind !== 'equipment' && selectedNoTt}
								<StatDisplay label="Quantity" value={formatQuantity(selected.quantity)} />
							{:else}
								<StatDisplay
									label={selected.subjectKind === 'equipment' ? 'Total cost' : 'Listing TT'}
									value={formatPed(selected.costBasis ?? selected.ttValue)}
									unit="PED"
								/>
							{/if}
							<StatDisplay label="Sold for" value={formatPed(selected.finalPrice ?? 0)} unit="PED" />
							<StatDisplay
								label="Fees"
								value={formatPed(selected.listingFee + (selected.saleFee ?? 0))}
								unit="PED"
								emphasis="secondary"
							/>

							<!-- Only net markup is toned. The rate and the credited share
								describe how the sale divides, not whether it went well, and
								colouring all three makes the one figure that carries a
								verdict indistinguishable from its neighbours. -->
							{#if selectedNoTt}
								<StatDisplay
									label="Each"
									value={pricePerUnit !== null ? formatPed(pricePerUnit) : NO_DATA}
									unit="PED"
								/>
							{:else}
								<StatDisplay
									label="MU rate"
									value={saleMarkupRate !== null ? formatPercent(saleMarkupRate) : NO_DATA}
								/>
							{/if}
							<StatDisplay
								label="Net markup"
								value={signedPed(netMarkup)}
								unit="PED"
								valueClass={netTone(netMarkup)}
							/>
							{#if selected.subjectKind === 'loot'}
							<StatDisplay
								label={central ? 'Attributed' : 'Credited'}
								value={signedPed(selected.activityNetMarkup ?? 0)}
								unit="PED"
							>
								{#snippet labelSuffix()}
									<InfoTip label="How the credited amount is worked out" width="w-80">
										<p class="text-xs font-semibold leading-relaxed text-text">
											The part {activityNoun} can claim
										</p>
										<p class="mt-1 text-xs leading-relaxed text-text-secondary">
											The share of net markup covered by stock this activity is recorded as
											producing, split across {sourceNounPlural} that supplied it in proportion
											to what each contributed.
										</p>
										{#if selected.unattributedQty > 0}
											<p class="mt-2 text-xs leading-relaxed text-text-secondary">
												{formatQuantity(selected.unattributedQty)} of the {formatQuantity(
													selected.quantity,
												)}
												listed was beyond tracked stock, so {formatPed(
													(selected.unattributedQty / selected.quantity) * 100,
												)}% of this sale, {signedPed(unattributedMarkup)} PED, cannot be credited
												to {sourceNounIndefinite}. Its value is still yours and still in your ledger.
											</p>
										{:else}
											<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
												It matches net markup when the whole listing came from tracked stock, as
												this one did.
											</p>
										{/if}
									</InfoTip>
								{/snippet}
							</StatDisplay>
							{:else}
								<StatDisplay label="Position" value="Whole item" emphasis="secondary" />
							{/if}
						</div>
					{:else}
						<!-- What the listing cost above what it asks: the two spent
							figures lead, the two asked ones follow, and all four carry
							the same weight because none of them is yet an outcome. -->
						<div class="grid grid-cols-2 gap-x-5 gap-y-4">
							{#if selectedNoTt}
								<StatDisplay label="Quantity" value={formatQuantity(selected.quantity)} />
							{:else}
								<StatDisplay label="Listing TT" value={formatPed(selected.ttValue)} unit="PED" />
							{/if}
							<StatDisplay label="Fee paid" value={formatPed(selected.listingFee)} unit="PED">
								{#snippet labelSuffix()}
									{#if selected.status === 'expired'}
										<InfoTip label="Why an expired listing still cost you" width="w-80">
											<p class="text-xs font-semibold leading-relaxed text-text">
												Expired, and what it cost
											</p>
											<p class="mt-1 text-xs leading-relaxed text-text-secondary">
											The stock returned to your inventory in full. The listing fee stays spent.
											</p>
											<p class="mt-2 text-xs leading-relaxed text-text-tertiary">
												{expiredChargeNote}
											</p>
										</InfoTip>
									{/if}
								{/snippet}
							</StatDisplay>

							<StatDisplay
								label="Starting bid"
								value={formatPed(selected.startingBid)}
								unit="PED"
							/>
							<StatDisplay
								label="Buyout"
								value={selected.buyout !== null ? formatPed(selected.buyout) : NO_DATA}
								unit={selected.buyout !== null ? 'PED' : ''}
							/>
						</div>
					{/if}

					{#if selected.status === 'pending'}
						{#if hasRunOut(selected, now)}
							<!-- The clock is up, and that is all we know. Which way it went
								is the player's to say: guessing either outcome would write a
								price, or a return of stock, that never happened. -->
							<p class="mt-4 text-xs leading-relaxed text-warning/90">
								Its {selected.auctionDays} days are up. What became of it?
							</p>
						{/if}
						<div class="mt-5 border-t border-border/40 pt-4">
							{#if confirming}
								<div class="space-y-3">
									<div class="grid grid-cols-3 gap-3">
										<label class="block space-y-1">
											<span class="eyebrow text-text-tertiary">Sold for (PED)</span>
											<Input type="number" min="0" step="0.01" bind:value={finalPrice} />
										</label>
										<label class="block space-y-1">
											<span class="eyebrow text-text-tertiary">Extra fee (PED)</span>
											<Input type="number" min="0" step="0.01" bind:value={saleFee} />
										</label>
										<label class="block space-y-1">
											<span class="eyebrow text-text-tertiary">Sold on</span>
											<Input type="date" bind:value={resolvedAt} />
										</label>
									</div>
									<p class="text-xs leading-relaxed text-text-tertiary">
										The extra fee is the one charged at the point of sale when an item clears above
										its starting bid; the game sends it to you by in-game mail. Leave it at zero if
										there was none.
									</p>
									<div class="flex items-center justify-end gap-2">
										<Button variant="ghost" size="sm" onclick={() => (confirming = false)}>
											Cancel
										</Button>
										<Button
											size="sm"
											loading={busy}
											onclick={() =>
												resolve({
													sold: true,
													finalPrice,
													saleFee,
													resolvedAt: resolvedAt || undefined,
												})}
										>
											Confirm sale
										</Button>
									</div>
								</div>
							{:else}
								<div class="flex items-center justify-end gap-3">
									<div class="flex shrink-0 items-center gap-2">
										<Button size="sm" onclick={startConfirm}>It sold</Button>
										<Button
											size="sm"
											variant="ghost"
											loading={busy}
											onclick={() => resolve({ sold: false })}
										>
											It expired
										</Button>
									</div>
								</div>
							{/if}
						</div>
					{/if}

					{#if error}
						<p class="mt-3 text-xs text-error">{error}</p>
					{/if}
				</div>
			{/if}
		</div>
	{/if}
{/snippet}

{#if embedded}
	<div>{@render content()}</div>
{:else}
	<Card class="hover:z-20">{@render content()}</Card>
{/if}
