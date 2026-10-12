<script lang="ts">
	/**
	 * Sale intake that starts from the game's window rather than from a row.
	 *
	 * The existing sale flow starts by picking a holding, which assumes the
	 * player already knows which tracked position they are selling. In front
	 * of the game's sale window they are reading it off the screen instead, so
	 * this flow runs the other way: record what the window says, resolve which
	 * holding it refers to, then review before anything is written.
	 *
	 * Naming the item is a typeahead over what is actually held, so the usual
	 * case resolves by being chosen rather than by being matched. A name typed
	 * free still resolves on the way out of the field, conservatively: an
	 * ambiguous one is a question put to the player, never a quiet choice of
	 * cost basis, because the wrong holding attributes real money to gameplay
	 * that did not earn it.
	 *
	 * The item comes first and the rest of the form waits on it, because the
	 * item is what makes the rest meaningful. The TT value is entered rather
	 * than worked out from recorded stock: stock holds an average across
	 * everything logged for that item, which drifts from what the stack in
	 * hand is actually worth, and a figure that quietly disagrees with the
	 * one on screen is worse than no figure at all.
	 *
	 * Nothing is complained about until the player tries to file the listing.
	 * A form that opens already scolding the reader for not having filled it
	 * in yet teaches them to ignore its red text.
	 *
	 * The capture buttons are the same flow with the fields filled by reading
	 * the game's window instead of by typing. They fill this same draft and
	 * commit nothing, so typing remains a complete path rather than a
	 * fallback, and a capture is reviewed here before anything is written.
	 */
	import SearchHighlight from '$lib/components/SearchHighlight.svelte';
	import { onDestroy } from 'svelte';
	import { rankByQuery } from '$lib/view/search';
	import Button from '$lib/components/Button.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import Input from '$lib/components/Input.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import PickerInput from '$lib/components/PickerInput.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import { showSaleCaptureOverlay } from '$lib/api';
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { formatPed } from '$lib/utils/format';
	import { createTypeahead } from '$lib/view/typeahead.svelte';
	import type { InventoryHoldingCandidate, SaleWindowCapture } from '$lib/api/commands.gen';
	import {
		capturedDraft,
		draftIssues,
		EMPTY_DRAFT,
		type HoldingOption,
		impliedMarkupPct,
		type ListingDraftFields,
		previewNetMarkup,
	} from './listingIntake';

	let {
		open = $bindable(false),
		holdings,
		onresolve,
		oncapture,
		oncollect,
		onsubmit,
	}: {
		open?: boolean;
		/** Everything currently held, loot and assets alike, for the typeahead. */
		holdings: HoldingOption[];
		/** Candidate holdings for a name typed rather than chosen. */
		onresolve: (
			name: string,
			channel: 'auction' | 'trade',
		) => Promise<{
			candidates: InventoryHoldingCandidate[];
			resolved: InventoryHoldingCandidate | null;
		}>;
		/** One read of the game's sale window. */
		oncapture: () => Promise<SaleWindowCapture>;
		/** A read taken from the overlay while this form was not on screen,
		 * or null if there was none. Collected once. */
		oncollect: () => Promise<SaleWindowCapture | null>;
		onsubmit: (input: {
			fields: ListingDraftFields;
			channel: 'auction' | 'trade';
			holding: InventoryHoldingCandidate;
			/** A full local timestamp, or null to mean now. */
			occurredAt: string | null;
		}) => Promise<void>;
	} = $props();

	let fields = $state<ListingDraftFields>({ ...EMPTY_DRAFT });
	let channel = $state<'auction' | 'trade'>('auction');
	// Null means now, resolved at the moment of filing rather than pinned when
	// the form opened: a form left sitting for ten minutes should still record
	// the listing as made when it was actually made.
	let occurredAt = $state<string | null>(null);
	let saving = $state(false);
	let error = $state<string | null>(null);
	let attempted = $state(false);

	let resolving = $state(false);
	let candidates = $state<HoldingOption[]>([]);
	let chosen = $state<HoldingOption | null>(null);
	/** The name the free-text resolver last answered for, so a stale answer
	 * cannot be mistaken for one about what is in the box now. */
	let resolvedFor = $state<string | null>(null);
	/** Emitted by the shell when the overlay has read a window for this form. */
	const CAPTURE_READY_EVENT = 'sale-capture-ready';

	let capturing = $state(false);
	/** What the last read could not make out, so the gaps are visible rather
	 * than left looking like fields the window did not have. */
	let unread = $state<string[]>([]);

	const picker = createTypeahead<HoldingOption>({
		search: async (query) => rankByQuery(holdings, query, (row) => row.name, 12),
		debounceMs: 0,
		minLength: 1,
		labelOf: (row) => row.name,
	});

	const pickerModel = {
		get query() {
			return picker.query;
		},
		set query(value: string) {
			picker.query = value;
			fields.itemName = value;
			// The match belongs to the name it was made for; editing the name
			// must drop it, or a sale could bind to the holding of a word the
			// player has since typed over.
			chosen = null;
			candidates = [];
			resolvedFor = null;
		},
		get results() {
			return picker.results;
		},
		get selected() {
			return picker.selected;
		},
		get loading() {
			return picker.loading;
		},
		get error() {
			return picker.error;
		},
		select(row: HoldingOption) {
			picker.select(row);
			fields.itemName = row.name;
			chosen = row;
			candidates = [];
			resolvedFor = row.name;
		},
		clear() {
			// `clear` empties the picker's own query directly, so the draft's
			// copy of the name has to be emptied with it or the form would
			// still be holding a name the box no longer shows.
			picker.clear();
			fields.itemName = '';
			chosen = null;
			candidates = [];
			resolvedFor = null;
		},
	};

	const issues = $derived(draftIssues(fields, channel));
	const blocking = $derived(issues.filter((issue) => issue.severity === 'blocking'));
	const advisories = $derived(issues.filter((issue) => issue.severity === 'advisory'));
	const netPreview = $derived(previewNetMarkup(fields, channel));
	const impliedSb = $derived(impliedMarkupPct(fields.startingBid, fields.ttValue));
	const impliedBo = $derived(impliedMarkupPct(fields.buyout, fields.ttValue));
	const named = $derived(fields.itemName.trim() !== '');
	// A name that resolved to nothing is a legitimate sale of untracked stock,
	// so it may proceed; it just cannot claim any activity's provenance. Only
	// true once the resolver has actually answered about THIS name: an empty
	// candidate list means nothing before it has been asked.
	const untracked = $derived(
		named && !chosen && !resolving && resolvedFor === fields.itemName.trim() && candidates.length === 0,
	);
	// The rest of the form waits on the item, which is what gives the other
	// figures their meaning.
	const itemSettled = $derived(chosen !== null || untracked);

	function reset() {
		fields = { ...EMPTY_DRAFT };
		channel = 'auction';
		occurredAt = null;
		error = null;
		attempted = false;
		candidates = [];
		chosen = null;
		resolvedFor = null;
		unread = [];
		picker.clear();
	}


	$effect(() => {
		if (open) return;
		reset();
	});

	// A capture taken from the overlay was taken for this form, so the form
	// collects it. There is nothing to confirm: the figures landing in the
	// fields is the review, exactly as it is for the button.
	//
	// The overlay says when it has one, rather than the form noticing on its
	// next focus. A second screen showing this form passively is the case
	// that makes the difference: nothing there would ever regain focus, and
	// an unchanged form reads exactly like a capture that failed.
	$effect(() => {
		if (!open) return;
		collect();

		let unlisten: UnlistenFn | undefined;
		let disposed = false;
		void listen(CAPTURE_READY_EVENT, () => {
			if (!disposed) collect();
		}).then((fn) => {
			if (disposed) fn();
			else unlisten = fn;
		});
		return () => {
			disposed = true;
			unlisten?.();
		};
	});

	async function collect() {
		try {
			const waiting = await oncollect();
			if (waiting) await fill(waiting);
		} catch {
			// Nothing waiting, or it could not be collected. The form opens
			// empty, which is what it would have done anyway.
		}
	}

	onDestroy(() => picker.destroy());

	/** Enrich a resolved candidate with what is known about the holding it
	 * names, so a name matched by typing behaves like one chosen from the list. */
	function enrich(candidate: InventoryHoldingCandidate): HoldingOption {
		const known = holdings.find((row) => row.holdingId === candidate.holdingId);
		return { ...candidate, heldQty: known?.heldQty ?? null };
	}

	/** The window's own name for a field, for saying what did not read. */
	function fieldLabel(field: string): string {
		return (
			{
				item_name: 'the item name',
				quantity: 'the quantity',
				tt_value: 'the TT value',
				auction_fee: 'the auction fee',
				auction_days: 'the number of days',
				starting_bid: 'the starting bid',
				buyout: 'the buyout',
			}[field] ?? field
		);
	}

	/** Read the sale window and fill the form from it.
	 *
	 * Nothing here commits, and nothing it fills is trusted further than a
	 * typed figure would be: the same validation, the same review, the same
	 * button. A field the read refused stays empty and is named below the
	 * buttons, because an empty box invites a correction while a plausible
	 * wrong number does not.
	 *
	 * Filling replaces what is in the form. A capture is a fresh look at the
	 * window, so merging it into half-typed values would produce a listing
	 * that matches neither the screen nor what the player entered. */
	async function capture() {
		if (capturing) return;
		capturing = true;
		error = null;
		try {
			await fill(await oncapture());
			// Its own read is already in the fields; leaving the held copy
			// behind would re-fill the form the next time this window came
			// back into focus.
			await oncollect();
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Could not read the sale window';
		} finally {
			capturing = false;
		}
	}

	/** Put a read into the form. The same path whichever button took it. */
	async function fill(read: SaleWindowCapture) {
		if (read.error !== null) {
			error = read.error;
			return;
		}
		unread = read.unread;
		attempted = false;
		candidates = [];
		chosen = null;
		resolvedFor = null;
		fields = capturedDraft(read);
		// The box has to show the name the draft is holding, or the form
		// would carry a name nothing on screen accounts for. It is filled
		// rather than typed: a read is not a search, and routing it through
		// the query would flash the suggestion list open over the form and
		// shut it again a moment later.
		picker.fill(fields.itemName);
		if (fields.itemName === '') return;
		await resolveTyped();
		// A name the window gave and the resolver settled is as chosen as one
		// picked by hand, so settle it the same way and put the list away
		// rather than leaving it open over the form.
		if (chosen) pickerModel.select(chosen);
	}

	/** Resolve a name the player typed instead of choosing.
	 *
	 * Guarded against its own lateness. Clicking a suggestion blurs the field
	 * first, so this can already be in flight when the click lands; without
	 * the guard its answer would return a moment later and overwrite the
	 * choice the player just made with a list of near-misses. */
	async function resolveTyped() {
		const name = fields.itemName.trim();
		if (name === '' || chosen || resolving) return;
		resolving = true;
		try {
			const outcome = await onresolve(name, channel);
			if (chosen || fields.itemName.trim() !== name) return;
			candidates = outcome.candidates.map(enrich);
			chosen = outcome.resolved ? enrich(outcome.resolved) : null;
			resolvedFor = name;
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Could not match that name to a holding';
		} finally {
			resolving = false;
		}
	}

	async function commit() {
		attempted = true;
		if (saving || blocking.length > 0) return;
		await resolveTyped();
		// An unresolved ambiguity is the one thing the player must settle;
		// everything else the form already knows how to proceed without.
		if (!chosen && candidates.length > 0) return;
		const holding: InventoryHoldingCandidate = chosen ?? {
			kind: 'loot',
			holdingId: fields.itemName.trim(),
			name: fields.itemName.trim(),
			score: 0,
		};
		saving = true;
		error = null;
		try {
			await onsubmit({ fields, channel, holding, occurredAt });
			open = false;
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Failed to record the sale';
		} finally {
			saving = false;
		}
	}

	/** A local `datetime-local` value for now, for the moment the player
	 * chooses to pin the time rather than leave it running. */
	function nowLocalValue(): string {
		const now = new Date();
		const pad = (value: number) => String(value).padStart(2, '0');
		return (
			`${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}` +
			`T${pad(now.getHours())}:${pad(now.getMinutes())}`
		);
	}
</script>

<Modal bind:open title="Create listing" class="max-w-xl">
	<div class="space-y-5">
		<div class="flex flex-wrap items-center gap-2">
			<Button variant="secondary" size="sm" onclick={capture} disabled={capturing}>
				{capturing ? 'Reading the window' : 'Capture from game'}
			</Button>
			<Button variant="ghost" size="sm" onclick={() => showSaleCaptureOverlay()}>
				Capture overlay
			</Button>
		</div>
		{#if unread.length > 0}
			<p class="text-sm text-text-tertiary">
				Could not make out {unread.map(fieldLabel).join(', ')}. Fill {unread.length === 1
					? 'it'
					: 'them'} in yourself.
			</p>
		{/if}

		<SegmentedControl
			options={[
				{ id: 'auction', label: 'Auction' },
				{ id: 'trade', label: 'Trade' },
			]}
			active={channel}
			onchange={(id) => (channel = id as 'auction' | 'trade')}
		/>

		<div class="space-y-1">
			<span class="eyebrow text-text-tertiary">Item</span>
			<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
			<div onfocusout={resolveTyped}>
				<PickerInput
					id="create-listing-item"
					model={pickerModel}
					class="relative"
					dropdownClass="absolute left-0 right-0 z-30 shadow-lg"
				>
					{#snippet result({ item, query })}
						<span class="truncate"><SearchHighlight text={item.name} {query} /></span>
						{#if item.kind === 'equipment'}
							<span class="ml-3 shrink-0 text-xs text-text-tertiary">Asset</span>
						{/if}
					{/snippet}
					{#snippet selection({ item })}
						<span class="truncate">{item.name}</span>
					{/snippet}
				</PickerInput>
			</div>

			{#if !chosen && candidates.length > 0}
				<div class="space-y-1 pt-1">
					<p class="text-xs text-warning">
						More than one holding could be meant. Choose which one this sale takes from.
					</p>
					<div class="flex flex-wrap gap-1.5">
						{#each candidates as candidate (candidate.holdingId)}
							<button
								type="button"
								class="rounded border border-border px-2 py-1 text-xs text-text-secondary
									hover:border-border-bright hover:text-text"
								onclick={() => (chosen = candidate)}
							>
								{candidate.name}
							</button>
						{/each}
					</div>
				</div>
			{:else if untracked}
				<p class="text-xs text-text-tertiary">
					Nothing tracked matches that name. The sale will still be recorded, but no activity can
					claim its markup.
				</p>
			{/if}
		</div>

		<div class="grid grid-cols-2 gap-3">
			<label class="block space-y-1">
				<span class="eyebrow text-text-tertiary">Quantity</span>
				<Input
					type="number"
					min="0"
					step="1"
					align="right"
					disabled={!itemSettled}
					bind:value={fields.quantity}
				/>
				{#if chosen?.heldQty !== null && chosen?.heldQty !== undefined}
					<span class="block text-right text-[10px] tabular-nums text-text-tertiary">
						{chosen.heldQty} held
					</span>
				{/if}
			</label>
			<label class="block space-y-1">
				<span class="eyebrow text-text-tertiary">TT value (PED)</span>
				<Input
					type="number"
					min="0"
					step="0.01"
					align="right"
					disabled={!itemSettled}
					bind:value={fields.ttValue}
				/>
			</label>
		</div>

		{#if channel === 'auction'}
			<!-- Markup is what the bids come to against TT, exactly as the game
				shows it: read-only there, so read-only here. -->
			<div class="grid grid-cols-2 gap-3">
				<label class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Starting bid (PED)</span>
					<Input type="number" min="0" step="0.01" align="right" disabled={!itemSettled} bind:value={fields.startingBid} />
					<span class="block text-right text-[10px] tabular-nums text-text-tertiary">
						{impliedSb !== null ? `${impliedSb.toFixed(2)}% of TT` : ' '}
					</span>
				</label>
				<label class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Buyout (PED)</span>
					<Input type="number" min="0" step="0.01" align="right" disabled={!itemSettled} bind:value={fields.buyout} />
					<span class="block text-right text-[10px] tabular-nums text-text-tertiary">
						{impliedBo !== null ? `${impliedBo.toFixed(2)}% of TT` : ' '}
					</span>
				</label>
			</div>

			<div class="grid grid-cols-3 gap-3">
				<label class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Auction fee (PED)</span>
					<Input type="number" min="0" step="0.01" align="right" disabled={!itemSettled} bind:value={fields.auctionFee} />
				</label>
				<label class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Runs for (days)</span>
					<Input type="number" min="1" step="1" align="right" disabled={!itemSettled} bind:value={fields.auctionDays} />
				</label>
				<div class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Listed</span>
					{#if occurredAt === null}
						<button
							type="button"
							class="h-9 w-full rounded-md border border-border bg-surface/70 px-3 text-left text-sm
								text-text transition-[border-color] hover:border-border-bright
								disabled:cursor-not-allowed disabled:opacity-50"
							disabled={!itemSettled}
							onclick={() => (occurredAt = nowLocalValue())}
						>
							Now
						</button>
					{:else}
						<div class="flex items-center gap-1">
							<Input type="datetime-local" class="flex-1" bind:value={occurredAt} />
							<button
								type="button"
								class="shrink-0 px-1 text-xs text-accent underline-offset-2 hover:underline"
								onclick={() => (occurredAt = null)}
							>
								Now
							</button>
						</div>
					{/if}
				</div>
			</div>
		{:else}
			<div class="grid grid-cols-2 gap-3">
				<label class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Sold for (PED)</span>
					<Input type="number" min="0" step="0.01" align="right" disabled={!itemSettled} bind:value={fields.buyout} />
				</label>
				<div class="block space-y-1">
					<span class="eyebrow text-text-tertiary">Sold</span>
					{#if occurredAt === null}
						<button
							type="button"
							class="h-9 w-full rounded-md border border-border bg-surface/70 px-3 text-left text-sm
								text-text transition-[border-color] hover:border-border-bright
								disabled:cursor-not-allowed disabled:opacity-50"
							disabled={!itemSettled}
							onclick={() => (occurredAt = nowLocalValue())}
						>
							Now
						</button>
					{:else}
						<div class="flex items-center gap-1">
							<Input type="datetime-local" class="flex-1" bind:value={occurredAt} />
							<button
								type="button"
								class="shrink-0 px-1 text-xs text-accent underline-offset-2 hover:underline"
								onclick={() => (occurredAt = null)}
							>
								Now
							</button>
						</div>
					{/if}
				</div>
			</div>
		{/if}

		{#if netPreview !== null}
			<div class="flex items-center justify-between border-t border-border/50 pt-3 text-sm">
				<span class="text-text-secondary">
					{channel === 'auction' ? 'Net markup if it clears' : 'Realised markup'}
				</span>
				<span class="tabular-nums font-medium {netPreview >= 0 ? 'text-success' : 'text-error'}">
					{formatPed(netPreview)} PED
				</span>
			</div>
		{/if}

		{#if attempted}
			{#each blocking as issue (issue.field + issue.message)}
				<p class="text-xs text-error">{issue.message}</p>
			{/each}
		{/if}
		{#each advisories as issue (issue.field + issue.message)}
			<p class="text-xs text-warning">{issue.message}</p>
		{/each}

		<ErrorNotice message={error} />

		<div class="flex items-center justify-end gap-2 pt-1">
			<Button variant="ghost" onclick={() => (open = false)} disabled={saving}>Cancel</Button>
			<Button onclick={commit} loading={saving}>
				{channel === 'auction' ? 'List on auction' : 'Record trade'}
			</Button>
		</div>
	</div>
</Modal>
