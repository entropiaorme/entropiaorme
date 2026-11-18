<script lang="ts">
	// A measured session minute by minute, collapsed until asked: one dense
	// row for each step between two readings (loot arrived beside looted,
	// the weapon and healing spend measured beside tracked, armour
	// measured), and under a row whatever in it is worth a look, said as a
	// sentence with what can be done about it. The toggle counts what the
	// player can act on; opened, only the minutes worth a look are listed
	// until the player asks for every minute.
	//
	// Nothing shows until the minutes are read, and nothing at all for a
	// session the readings give no step of.
	import { onMount, tick, untrack } from 'svelte';
	import type { InventoryItem } from '$lib/api/commands.gen';
	import { addAssetPurchase } from '$lib/api/inventory';
	import { Button, ErrorNotice } from '$lib/components';
	import AssetPurchaseModal from '$lib/features/inventory/AssetPurchaseModal.svelte';
	import InventoryItemFormModal from '$lib/features/inventory/InventoryItemFormModal.svelte';
	import type { PurchaseDraft } from '$lib/features/inventory/purchaseProposals.svelte';
	import { lootEntriesPrompt } from '$lib/features/tracking/lootEntries';
	import { localDateOf } from '$lib/utils/format';
	import { formatMeasured, formatTracked } from './measure';
	import {
		archivedUnits,
		flagCount,
		measuredCell,
		minuteRows,
		minutesClock,
		type MinuteFlag,
		type MinuteLaneKind,
		type MinuteRow,
		notProposedNote,
		rowsToShow,
		standingRecordOf,
		toggleLabel,
		trackedText,
		unconfirmedLine,
	} from './minutes';
	import {
		createSessionMinutesModel,
		type DecidableArrival,
		decidable,
	} from './sessionMinutesModel.svelte';

	let {
		sessionId,
		onremoveloot = null,
		reviews = null,
		refreshKey = 0,
	}: {
		sessionId: string;
		/** Take every line of an item out of the session's loot, through the
		 * session's own loot correction; `null` where it is not offered. */
		onremoveloot?: ((itemName: string) => Promise<void>) | null;
		/** Show the session's own review of a lane, where it has one. */
		reviews?: Partial<Record<MinuteLaneKind, (() => void) | null>> | null;
		/** Changes when the session's loot changed elsewhere in its detail,
		 * so the minutes are read again. */
		refreshKey?: number;
	} = $props();

	const model = createSessionMinutesModel({
		sessionId: () => sessionId,
		onremoveloot: () => onremoveloot,
	});
	onMount(() => model.connect());
	$effect(() => {
		void sessionId;
		void refreshKey;
		void model.refresh();
	});

	/** Rows listed at once; a long session lists more when asked. */
	const PAGE = 120;

	const minutes = $derived(model.minutes);
	const clock = $derived(minutes ? minutesClock(minutes) : null);
	const rows = $derived(minutes && clock ? minuteRows(minutes, clock) : []);
	const count = $derived(flagCount(rows));
	const worthALook = $derived(rows.some((row) => row.needsLook));
	const unconfirmed = $derived(minutes ? unconfirmedLine(minutes) : null);

	let open = $state(false);
	/** The player's choice of filter; until they choose, it is on when the
	 * first read of the session had anything worth a look. */
	let onlyChosen = $state<boolean | null>(null);
	let onlyDefault = $state<{ sessionId: string; on: boolean } | null>(null);
	$effect(() => {
		if (minutes && onlyDefault?.sessionId !== minutes.sessionId) {
			onlyDefault = { sessionId: minutes.sessionId, on: worthALook };
			onlyChosen = null;
		}
	});
	const onlyFlagged = $derived(onlyChosen ?? onlyDefault?.on ?? false);
	const shown = $derived(rowsToShow(rows, onlyFlagged, model.touched));
	let limit = $state(PAGE);
	const visible = $derived(shown.slice(0, limit));

	const listId = `session-minutes-${crypto.randomUUID()}`;
	let list = $state<HTMLElement | null>(null);
	/** What the last decision did, said once. */
	let status = $state('');
	type Asking = 'purchase' | 'unknownOrigin' | 'remove';
	/** The flag asking first: recording again one that may be recorded, or
	 * removing loot. */
	let confirming = $state<{ key: string; action: Asking } | null>(null);

	// ── Recording as a purchase, through the asset forms ──────────────

	/** What the asset forms open with. What was paid is the player's to
	 * give, except for something the PED Card bought at its TT. */
	type ArrivalDraft = Omit<PurchaseDraft, 'markupPaid'> & { markupPaid: number | null };

	let purchaseFor = $state<{
		item: DecidableArrival;
		asset: InventoryItem;
		draft: ArrivalDraft;
	} | null>(null);
	let newAssetFor = $state<{ item: DecidableArrival; draft: ArrivalDraft } | null>(null);
	let newAssetOpen = $state(false);
	/** The flag a form was opened from: focus returns to it once the form
	 * closes, saved or not. */
	let formFor: string | null = null;

	// Another session shown starts the view afresh.
	let shownFor: string | null = null;
	$effect(() => {
		const id = sessionId;
		untrack(() => {
			if (shownFor !== null && shownFor !== id) {
				status = '';
				confirming = null;
				purchaseFor = null;
				newAssetFor = null;
				newAssetOpen = false;
				formFor = null;
				limit = PAGE;
				open = false;
				onlyChosen = null;
			}
			shownFor = id;
		});
	});

	function flagElement(key: string): HTMLElement | null {
		const flags = list?.querySelectorAll<HTMLElement>('[data-flag]') ?? [];
		return Array.from(flags).find((element) => element.dataset.flag === key) ?? null;
	}

	/** Focus a flag once the list has settled; the list itself when the
	 * flag has gone (loot removed leaves no flag behind). */
	async function focusFlag(key: string) {
		await tick();
		setTimeout(() => (flagElement(key) ?? list)?.focus(), 0);
	}

	/** Say what a decision did: cleared first, so the same words twice are
	 * still announced. */
	async function say(text: string) {
		status = '';
		await tick();
		status = text;
	}

	async function settled(key: string, done: Promise<boolean>, said: string) {
		if (await done) await say(said);
		void focusFlag(key);
	}

	/** Ask first, with focus on the answer. */
	async function ask(key: string, action: Asking) {
		confirming = { key, action };
		await tick();
		flagElement(key)?.querySelector<HTMLElement>('[data-confirm-focus]')?.focus();
	}

	function isConfirming(key: string, action: Asking): boolean {
		return confirming?.key === key && confirming.action === action;
	}

	function draftOf(item: DecidableArrival, row: MinuteRow): ArrivalDraft {
		return {
			key: `arrival-${item.proposalId}`,
			name: item.arrival.itemName,
			quantity: item.arrival.units,
			ttValue: item.arrival.tt.ped,
			markupPaid: item.arrival.boughtAtTt ? 0 : null,
			acquiredAt: localDateOf(row.step.between.to),
		};
	}

	async function recordPurchase(item: DecidableArrival, row: MinuteRow) {
		if (item.possiblyRecorded && !isConfirming(item.key, 'purchase')) {
			await ask(item.key, 'purchase');
			return;
		}
		confirming = null;
		const target = await model.purchaseTarget(item);
		if (!target || target.kind === 'several') {
			void focusFlag(item.key);
			return;
		}
		const draft = draftOf(item, row);
		formFor = item.key;
		if (target.kind === 'asset') {
			purchaseFor = { item, asset: target.asset, draft };
		} else {
			newAssetFor = { item, draft };
			newAssetOpen = true;
		}
	}

	async function savePurchase(input: Parameters<typeof addAssetPurchase>[1]) {
		const chosen = purchaseFor;
		if (!chosen) return;
		await model.recordPurchase(chosen.item, () => addAssetPurchase(chosen.asset.id, input));
		void say(`${chosen.item.arrival.itemName} recorded as a purchase.`);
	}

	function formClosed() {
		const key = formFor;
		formFor = null;
		if (key) void focusFlag(key);
	}

	$effect(() => {
		if (!newAssetOpen && newAssetFor) {
			newAssetFor = null;
			formClosed();
		}
	});

	// ── The other decisions ───────────────────────────────────────────

	async function recordUnknownOrigin(item: DecidableArrival) {
		if (item.possiblyRecorded && !isConfirming(item.key, 'unknownOrigin')) {
			await ask(item.key, 'unknownOrigin');
			return;
		}
		confirming = null;
		if (await model.recordUnknownOrigin(item)) {
			const made = model.decidedHere(item.key);
			await say(
				made?.kind === 'unknownOrigin' && !made.linked
					? `${item.arrival.itemName} added to stock, but not marked done.`
					: `${item.arrival.itemName} added to stock as unknown origin.`,
			);
		}
		void focusFlag(item.key);
	}

	/** The question stays, its answer pending, until the loot is removed. */
	async function removeLoot(key: string, itemName: string) {
		const removed = model.removeLoot(key, itemName);
		await removed.finally(() => {
			confirming = null;
		});
		await settled(key, removed, `${itemName} removed from this session's loot.`);
	}

	function cancelConfirm(key: string) {
		confirming = null;
		void focusFlag(key);
	}

	function onConfirmKey(event: KeyboardEvent, key: string) {
		if (event.key === 'Escape') {
			event.stopPropagation();
			cancelConfirm(key);
		}
	}

	const LANE_REVIEW: Record<MinuteLaneKind, string> = {
		weapon: 'Open weapon attribution',
		healing: 'Open healing evidence',
	};

	function needsLook(flag: MinuteFlag): boolean {
		return flag.kind !== 'arrival' || flag.needsLook;
	}
</script>

{#if minutes && minutes.steps.length > 0}
	<div class="pt-3" data-testid="session-minutes">
		<button
			type="button"
			class="inline-flex items-center gap-1 rounded-sm text-xs hover:text-text
				transition-colors duration-[var(--duration-fast)]
				focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent
				{count > 0 ? 'text-text-secondary' : 'text-text-tertiary'}"
			aria-expanded={open}
			aria-controls={open ? listId : undefined}
			onclick={() => (open = !open)}
		>
			<svg
				viewBox="0 0 20 20"
				fill="currentColor"
				class="h-3 w-3 transition-transform duration-[var(--duration-fast)] {open ? 'rotate-90' : ''}"
				aria-hidden="true"
			>
				<path
					fill-rule="evenodd"
					d="M7.21 14.77a.75.75 0 01.02-1.06L11.17 10 7.23 6.29a.75.75 0 111.04-1.08l4.5 4.25a.75.75 0 010 1.08l-4.5 4.25a.75.75 0 01-1.06-.02z"
					clip-rule="evenodd"
				/>
			</svg>
			{toggleLabel(count)}
		</button>

		<!-- Always in place, so what a decision did is announced; seen only
			 while it says something. -->
		<p
			class="text-xs text-text-secondary {status ? 'mt-2' : 'sr-only'}"
			role="status"
			aria-live="polite"
			data-testid="minutes-status"
		>{status}</p>

		{#if open}
			<div id={listId} class="mt-3 space-y-3 text-xs" data-testid="minutes-detail">
				<div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
					<p class="text-text-tertiary">
						Each figure is in PED: loot reads arrived / looted, weapon and healing measured /
						tracked, armour measured.
					</p>
					{#if worthALook || onlyFlagged}
						<button
							type="button"
							class="filter-chip {onlyFlagged ? 'is-active' : ''}
								focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent"
							aria-pressed={onlyFlagged}
							onclick={() => {
								onlyChosen = !onlyFlagged;
								limit = PAGE;
							}}
						>
							Only minutes that need a look
						</button>
					{/if}
				</div>

				<ErrorNotice message={model.error} onDismiss={() => model.dismissError()} />
				{#if unconfirmed}
					<p class="text-text-tertiary" data-testid="minutes-unconfirmed">{unconfirmed}</p>
				{/if}

				<div
					class="grid grid-cols-[minmax(0,8rem)_repeat(4,minmax(0,1fr))] gap-x-4 pb-1 eyebrow"
					aria-hidden="true"
				>
					<span>Minute</span>
					<span class="text-right">Loot</span>
					<span class="text-right">Weapon</span>
					<span class="text-right">Healing</span>
					<span class="text-right">Armour</span>
				</div>

				<!-- The column heads are drawn once for the eye; each cell names its
					 own figures for a screen reader. -->
				<ol
					bind:this={list}
					tabindex="-1"
					class="divide-y divide-border/40 border-y border-border/40 focus:outline-none"
					aria-label="The session minute by minute"
				>
					{#each visible as row (row.key)}
						{@const weapon = measuredCell(row.step.weapon.measured)}
						{@const healing = measuredCell(row.step.healing.measured)}
						{@const armour = measuredCell(row.step.armour)}
						{@const lootFlagged = row.flags.some((flag) => flag.kind !== 'stretch' && needsLook(flag))}
						<li data-testid="minute-row">
							<div class="grid grid-cols-[minmax(0,8rem)_repeat(4,minmax(0,1fr))] items-baseline gap-x-4 py-1.5 tabular-nums">
								<span class="min-w-0 text-text-secondary" title={row.span}>
									<span class="sr-only">Minute to</span>
									{row.time}
									{#if row.aside}<span class="block text-[11px] text-text-tertiary">{row.aside}</span>{/if}
								</span>
								<span class="text-right" title="Arrived / looted">
									<span class="sr-only">Loot arrived / looted:</span>
									<span class={lootFlagged ? 'text-warning' : 'text-text'}>{formatMeasured(row.step.arrived)}</span>
									<span class="text-text-tertiary">/ {formatTracked(row.step.looted, row.step.arrived.precision)}</span>
								</span>
								<span class="text-right">
									<span class="sr-only">Weapon measured / tracked:</span>
									<span
										class={row.drifting.weapon ? 'text-warning' : weapon.measured ? 'text-text' : 'text-text-tertiary'}
										title={weapon.title}>{weapon.text}</span
									>
									<span class="text-text-tertiary">/ {trackedText(row.step.weapon.tracked, row.step.weapon.measured)}</span>
								</span>
								<span class="text-right">
									<span class="sr-only">Healing measured / tracked:</span>
									<span
										class={row.drifting.healing ? 'text-warning' : healing.measured ? 'text-text' : 'text-text-tertiary'}
										title={healing.title}>{healing.text}</span
									>
									<span class="text-text-tertiary">/ {trackedText(row.step.healing.tracked, row.step.healing.measured)}</span>
								</span>
								<span class="text-right {armour.measured ? 'text-text' : 'text-text-tertiary'}" title={armour.title}>
									<span class="sr-only">Armour measured:</span>
									{armour.text}
								</span>
							</div>

							{#each row.flags as flag (flag.key)}
								{@const acting = model.actingOn(flag.key)}
								{@const busy = model.acting !== null}
								{@const failed = model.errorOf(flag.key)}
								<div
									data-flag={flag.key}
									data-testid="minute-flag"
									tabindex="-1"
									class="mb-2 ml-4 space-y-1.5 border-l pl-3 focus:outline-none focus-visible:ring-1 focus-visible:ring-accent
										{needsLook(flag) ? 'border-warning/60' : 'border-border/40'}"
								>
									<p class={needsLook(flag) ? 'text-text' : 'text-text-secondary'}>{flag.text}</p>

									{#if flag.kind === 'arrival'}
										{@const disposition = flag.arrival.disposition}
										{@const item = decidable(flag.key, flag.arrival)}
										{@const made = model.decidedHere(flag.key)}
										{@const unlinked = model.unlinkedOf(flag.key)}
										{@const standing =
											made?.kind === 'unknownOrigin' && !made.linked
												? made.recordId
												: standingRecordOf(flag.arrival)}
										{#if disposition.state === 'recorded'}
											{#if disposition.as === 'unknownOrigin'}
												<div class="flex flex-wrap items-center gap-2">
													<span class="text-text-secondary">Added to stock as unknown origin.</span>
													<Button
														size="sm"
														variant="ghost"
														loading={acting === 'undo'}
														disabled={busy && acting !== 'undo'}
														aria-label={`Take back ${flag.arrival.itemName} from stock`}
														onclick={() =>
															void settled(
																flag.key,
																model.undoUnknownOrigin(flag.key, disposition.recordId),
																`${flag.arrival.itemName} taken back from stock.`,
															)}
													>
														Undo
													</Button>
												</div>
											{:else}
												{@const assetName =
													made?.kind === 'purchase' ? made.assetName : disposition.assetName}
												<p class="text-text-secondary" data-testid="recorded-purchase">
													Recorded as a purchase{assetName ? ` of ${assetName}` : ''}.
													To take it back, remove that purchase from the asset in
													<a href="/inventory" class="linklet underline underline-offset-2">Inventory</a>.
												</p>
											{/if}
										{:else if disposition.state === 'ignored'}
											<div class="flex flex-wrap items-center gap-2">
												<span class="text-text-tertiary">Ignored: not offered again.</span>
												<Button
													size="sm"
													variant="ghost"
													loading={acting === 'unignore'}
													disabled={busy && acting !== 'unignore'}
													aria-label={`Offer ${flag.arrival.itemName} again`}
													onclick={() =>
														void settled(
															flag.key,
															model.unignore(flag.key, disposition.proposalId),
															`${flag.arrival.itemName} offered again.`,
														)}
												>
													Offer again
												</Button>
											</div>
										{:else if disposition.state === 'notProposed'}
											<p class="text-text-tertiary">{notProposedNote(disposition.reason)}</p>
										{:else if item && standing}
											<!-- The stock stands, but the arrival is not marked done: it is
												 either marked now or taken back, never recorded twice. -->
											<p class="text-warning">Added to stock, but not marked done.</p>
											<div class="flex flex-wrap items-center gap-2">
												<Button
													size="sm"
													variant="secondary"
													loading={acting === 'link'}
													disabled={busy && acting !== 'link'}
													aria-label={`Mark as recorded: ${flag.arrival.itemName}`}
													onclick={() =>
														void settled(
															flag.key,
															model.markUnknownOriginRecorded(flag.key, item.proposalId, standing),
															`${flag.arrival.itemName} marked as recorded.`,
														)}
												>
													Mark as recorded
												</Button>
												<Button
													size="sm"
													variant="ghost"
													loading={acting === 'undo'}
													disabled={busy && acting !== 'undo'}
													aria-label={`Take back ${flag.arrival.itemName} from stock`}
													onclick={() =>
														void settled(
															flag.key,
															model.undoUnknownOrigin(flag.key, standing),
															`${flag.arrival.itemName} taken back from stock.`,
														)}
												>
													Undo
												</Button>
											</div>
										{:else if item && unlinked}
											<p class="text-warning">Recorded in {unlinked.assetName}, but not marked done.</p>
											<Button
												size="sm"
												variant="secondary"
												loading={acting === 'link'}
												disabled={busy && acting !== 'link'}
												aria-label={`Mark as recorded: ${flag.arrival.itemName}`}
												onclick={() =>
													void settled(
														flag.key,
														model.markRecorded(flag.key),
														`${flag.arrival.itemName} marked as recorded.`,
													)}
											>
												Mark as recorded
											</Button>
										{:else if item}
											{#if confirming?.key === flag.key}
												<!-- svelte-ignore a11y_no_static_element_interactions -->
												<div
													class="flex flex-wrap items-center gap-2"
													onkeydown={(event) => onConfirmKey(event, flag.key)}
												>
													<span class="text-warning">
														This may already be recorded. Check your assets; record it again only if it
														is not there.
													</span>
													<Button
														size="sm"
														variant="secondary"
														data-confirm-focus
														onclick={() =>
															confirming?.action === 'purchase'
																? void recordPurchase(item, row)
																: void recordUnknownOrigin(item)}
													>
														Record again
													</Button>
													<Button size="sm" variant="ghost" onclick={() => cancelConfirm(flag.key)}>
														Cancel
													</Button>
												</div>
											{:else}
												{#if item.possiblyRecorded}
													<p class="text-warning">This may already be recorded; check your assets first.</p>
												{/if}
												<div class="flex flex-wrap items-center gap-2">
													<Button
														size="sm"
														variant="secondary"
														loading={acting === 'prepare' || acting === 'purchase'}
														disabled={busy && acting !== 'prepare' && acting !== 'purchase'}
														aria-label={`Record ${flag.arrival.itemName} as a purchase`}
														onclick={() => void recordPurchase(item, row)}
													>
														Record as a purchase
													</Button>
													<Button
														size="sm"
														variant="ghost"
														loading={acting === 'unknownOrigin'}
														disabled={busy && acting !== 'unknownOrigin'}
														aria-label={`Add ${flag.arrival.itemName} to stock as unknown origin`}
														onclick={() => void recordUnknownOrigin(item)}
													>
														Add to stock as unknown origin
													</Button>
													<Button
														size="sm"
														variant="ghost"
														loading={acting === 'ignore'}
														disabled={busy && acting !== 'ignore'}
														aria-label={`Ignore ${flag.arrival.itemName}`}
														onclick={() =>
															void settled(flag.key, model.ignore(item), `${flag.arrival.itemName} ignored.`)}
													>
														Ignore
													</Button>
												</div>
											{/if}
										{/if}
									{:else if flag.kind === 'missing'}
										{@const correction = flag.missing.correction}
										{#if correction.kind === 'offered' && model.removesLoot && correction.sessionId === sessionId}
											{#if isConfirming(flag.key, 'remove')}
												<!-- svelte-ignore a11y_no_static_element_interactions -->
												<div
													class="flex flex-wrap items-center gap-2"
													onkeydown={(event) => onConfirmKey(event, flag.key)}
												>
													<span class="text-text-secondary">
														{lootEntriesPrompt(
															'Archive',
															minutes ? archivedUnits(minutes, correction.itemName) : flag.missing.units,
															correction.itemName,
														)}
													</span>
													<Button
														size="sm"
														variant="ghost"
														data-confirm-focus
														onclick={() => cancelConfirm(flag.key)}
													>
														Cancel
													</Button>
													<Button
														size="sm"
														variant="secondary"
														loading={acting === 'remove'}
														disabled={busy && acting !== 'remove'}
														onclick={() => void removeLoot(flag.key, correction.itemName)}
													>
														Yes
													</Button>
												</div>
											{:else}
												<Button
													size="sm"
													variant="ghost"
													disabled={busy}
													aria-label={`Remove ${correction.itemName} from this session's loot`}
													onclick={() => void ask(flag.key, 'remove')}
												>
													Remove from this session's loot
												</Button>
											{/if}
										{:else if flag.note}
											<p class="text-text-tertiary">{flag.note}</p>
										{/if}
									{:else if flag.kind === 'stretch'}
										{@const review = reviews?.[flag.lane] ?? null}
										{#if review}
											<button type="button" class="linklet" onclick={review}>
												{LANE_REVIEW[flag.lane]}
											</button>
										{/if}
									{/if}

									{#if failed}
										<ErrorNotice message={failed} onDismiss={() => model.dismissRowError()} />
									{/if}
								</div>
							{/each}
						</li>
					{/each}
				</ol>

				{#if shown.length === 0}
					<p class="text-text-tertiary" data-testid="minutes-empty">
						Nothing in this session needs a look.
					</p>
				{:else if shown.length > visible.length}
					<button type="button" class="linklet" onclick={() => (limit += PAGE)}>
						Show {Math.min(PAGE, shown.length - visible.length)} more minutes
					</button>
				{/if}
			</div>
		{/if}
	</div>
{:else if model.error && !model.answered}
	<div class="flex flex-wrap items-center gap-3 pt-3 text-xs" data-testid="session-minutes">
		<span class="text-negative" role="alert">{model.error}</span>
		<button type="button" class="linklet" onclick={() => void model.refresh()}>Try again</button>
	</div>
{/if}

<AssetPurchaseModal
	item={purchaseFor?.asset ?? null}
	purchase={null}
	prefill={purchaseFor?.draft ?? null}
	onsubmit={savePurchase}
	oncancel={() => {
		purchaseFor = null;
		formClosed();
	}}
/>
<InventoryItemFormModal
	bind:open={newAssetOpen}
	item={null}
	prefill={newAssetFor?.draft ?? null}
	around={newAssetFor
		? (save) => {
				const chosen = newAssetFor;
				if (!chosen) return save();
				return model.recordPurchase(chosen.item, save).then((saved) => {
					void say(`${chosen.item.arrival.itemName} recorded as a purchase.`);
					return saved;
				});
			}
		: null}
	onsaved={() => undefined}
/>
