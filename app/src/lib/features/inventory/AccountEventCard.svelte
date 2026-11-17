<script lang="ts">
	/**
	 * One thing the account recorded on its own, laid out as what went in
	 * and what came out, with when the readings saw it and the way back.
	 *
	 * Each kind reads its own way: a recycling lists what went in and the
	 * Nanocubes that came out, with the balance; Shrapnel to ammo shows the
	 * Trade Terminal's 1%; a sale to the Trade Terminal gathers everything
	 * the step sold beside the Card's rise; a listing that sold shows its
	 * price and markup, and one that came back the item back and the fee
	 * lost. A new event carries a mark until the player leaves the view.
	 *
	 * Undo confirms in place, saying what it does, as the rest of the
	 * history does.
	 */
	import Button from '$lib/components/Button.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import FlowList from './FlowList.svelte';
	import {
		accountEventUndo,
		recyclingFlow,
		saleFlow,
		shrapnelFlow,
		type AccountEvent,
	} from './accountActivity';
	import { fromAccountWindow } from './fromAccount';
	import { formatPed, formatQuantity } from '$lib/utils/format';

	let {
		event,
		isNew = false,
		onundo,
	}: {
		event: AccountEvent;
		isNew?: boolean;
		/** Take the event back; rejects with the reason shown on the page. */
		onundo: (event: AccountEvent) => Promise<void>;
	} = $props();

	let confirming = $state(false);
	let busy = $state(false);

	const undo = $derived(accountEventUndo(event));
	const undone = $derived(undo.kind === 'undone');

	const TITLE: Record<AccountEvent['kind'], string> = {
		recycling: 'Recycled',
		shrapnel: 'Shrapnel to ammo',
		sale: 'Sold to the Trade Terminal',
		listingSold: 'Listing sold',
		listingReturned: 'Listing came back',
	};

	const signed = (value: number) => `${value >= 0 ? '+' : '−'}${formatPed(Math.abs(value))}`;

	async function confirm() {
		if (busy) return;
		busy = true;
		try {
			await onundo(event);
			confirming = false;
		} catch {
			// The page says why; the confirmation stays open on what failed.
		} finally {
			busy = false;
		}
	}
</script>

<article
	class="relative rounded-lg px-4 py-3 transition-colors duration-[var(--duration-base)]
		{isNew ? 'bg-warning/[0.06]' : 'bg-surface-hover/25'}
		{undone ? 'opacity-50' : ''}"
	data-testid="account-event"
	data-kind={event.kind}
>
	{#if isNew}
		<span
			aria-hidden="true"
			class="absolute left-0 top-3 bottom-3 w-0.5 rounded-r-full bg-warning
				[box-shadow:0_0_8px_color-mix(in_oklab,var(--color-warning)_60%,transparent)]"
		></span>
	{/if}

	<header class="flex items-baseline gap-3">
		<h3 class="text-sm font-medium tracking-tight text-text {undone ? 'line-through' : ''}">
			{TITLE[event.kind]}
		</h3>
		{#if isNew}
			<span class="text-[0.625rem] font-semibold uppercase tracking-wider text-warning" data-testid="event-new"
				>New</span
			>
		{/if}
		<span class="min-w-0 flex-1 truncate text-xs text-text-tertiary">
			Seen {fromAccountWindow(event.step)}
		</span>
		<div class="flex shrink-0 items-center">
			{#if undo.kind === 'undone'}
				<span class="text-xs text-text-tertiary">Undone</span>
			{:else if undo.kind === 'held'}
				<span class="flex items-center gap-1 text-xs text-text-tertiary">
					Held
					<InfoTip align="right" label="Why this cannot be undone" width="w-72">
						<p class="text-xs leading-relaxed text-text-secondary">{undo.reason}</p>
					</InfoTip>
				</span>
			{:else}
				<Button
					variant="ghost"
					size="sm"
					loading={busy}
					onclick={() => (confirming = !confirming)}
				>
					Undo
				</Button>
			{/if}
		</div>
	</header>

	<div class="mt-2.5 grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-start gap-x-4">
		{#if event.kind === 'recycling'}
			{@const flow = recyclingFlow(event.entry)}
			<FlowList label="In" lines={flow.inputs} struck={undone} />
			<span aria-hidden="true" class="pt-4 text-text-tertiary">&rarr;</span>
			<FlowList label="Out" lines={flow.outputs} struck={undone} />
		{:else if event.kind === 'shrapnel'}
			{@const flow = shrapnelFlow(event.entry)}
			<FlowList
				label="In"
				lines={[{ name: 'Shrapnel', quantity: flow.shrapnel, tt: flow.tt }]}
				struck={undone}
			/>
			<span aria-hidden="true" class="pt-4 text-text-tertiary">&rarr;</span>
			<FlowList
				label="Out"
				lines={[{ name: 'Universal Ammo', quantity: null, tt: flow.ammo }]}
				struck={undone}
			/>
		{:else if event.kind === 'sale'}
			{@const flow = saleFlow(event.entries)}
			<FlowList label="Sold" lines={flow.items} struck={undone} />
			<span aria-hidden="true" class="pt-4 text-text-tertiary">&rarr;</span>
			<FlowList
				label="PED Card"
				lines={[{ name: 'Rose by', quantity: null, tt: event.step.cardChange ?? flow.tt }]}
				struck={undone}
			/>
		{:else}
			{@const entry = event.entry}
			<FlowList
				label="On auction"
				lines={[{ name: entry.itemName, quantity: entry.quantity, tt: entry.ttValue }]}
				struck={undone}
			/>
			<span aria-hidden="true" class="pt-4 text-text-tertiary">&rarr;</span>
			{#if event.kind === 'listingSold'}
				<FlowList
					label="Sold for"
					lines={[
						{
							name: 'PED Card',
							quantity: null,
							tt: event.listing?.finalPrice ?? entry.ttValue + (entry.netMarkup ?? 0),
						},
					]}
					struck={undone}
				/>
			{:else}
				<FlowList
					label="Back in your storage"
					lines={[{ name: entry.itemName, quantity: entry.quantity, tt: entry.ttValue }]}
					struck={undone}
				/>
			{/if}
		{/if}
	</div>

	<p class="mt-2 text-xs tabular-nums text-text-tertiary" data-testid="event-balance">
		{#if event.kind === 'recycling'}
			{@const flow = recyclingFlow(event.entry)}
			{#if Math.abs(flow.balance) < 0.005}
				Balanced: the Nanocubes are worth what went in.
			{:else}
				Balance {signed(flow.balance)} PED.
			{/if}
		{:else if event.kind === 'shrapnel'}
			{@const flow = shrapnelFlow(event.entry)}
			{signed(flow.bonus)} PED, the Trade Terminal's 1% on {formatQuantity(flow.shrapnel)} Shrapnel.
		{:else if event.kind === 'sale'}
			{@const flow = saleFlow(event.entries)}
			{#if event.step.cardChange !== null && Math.abs(event.step.cardChange - flow.tt) >= 0.005}
				At TT: {formatPed(flow.tt)} PED; the Card rose by {formatPed(event.step.cardChange)} PED in
				the same step.
			{:else}
				At TT: {formatPed(flow.tt)} PED for {event.entries.length === 1
					? 'one item'
					: `${event.entries.length} items`}.
			{/if}
		{:else if event.kind === 'listingSold'}
			{#if event.entry.netMarkup !== null}
				Markup {signed(event.entry.netMarkup)} PED after fees{#if event.listing && (event.listing.saleFee ?? 0) > 0}
					(a sale fee of {formatPed(event.listing.saleFee ?? 0)} PED){/if}.
			{/if}
		{:else}
			Listing fee lost: {formatPed(event.listing?.listingFee ?? 0)} PED.
		{/if}
	</p>

	{#if confirming && undo.kind === 'available'}
		<div class="mt-2.5 flex items-center justify-end gap-2 border-t border-border/30 pt-2.5">
			<p class="mr-auto text-xs leading-relaxed text-text-tertiary">{undo.sentence}</p>
			<Button variant="danger" size="sm" loading={busy} onclick={confirm}>{undo.action}</Button>
			<Button variant="ghost" size="sm" onclick={() => (confirming = false)}>Cancel</Button>
		</div>
	{/if}
</article>
