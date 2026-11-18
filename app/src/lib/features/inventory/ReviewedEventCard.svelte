<script lang="ts">
	/**
	 * One thing the player applied after a review: a stock check, with
	 * everything it recorded, or a strongbox opening, with the boxes and
	 * keys that went in and the ammo and items that came out. Either is
	 * taken back whole.
	 */
	import Button from '$lib/components/Button.svelte';
	import FlowList from './FlowList.svelte';
	import { openingFlow, type FlowLine, type ReviewedEvent } from './accountActivity';
	import type { ActivityHistoryEntry } from '$lib/api/commands.gen';
	import { formatLedgerDate, formatPed } from '$lib/utils/format';

	let {
		event,
		onundo,
	}: {
		event: ReviewedEvent;
		/** Take the event back whole; rejects with the reason shown on the page. */
		onundo: (event: ReviewedEvent) => Promise<void>;
	} = $props();

	let confirming = $state(false);
	let busy = $state(false);

	const undone = $derived(
		event.kind === 'opening'
			? event.opening.undone
			: event.entries.every((entry) => entry.undone),
	);

	const VERB: Record<string, string> = {
		removal: 'Removed',
		unknownOrigin: 'Held, of unknown origin',
		conversion: 'Converted',
		trade: 'Sold',
		listing: 'Listed',
	};

	/** A check's records, grouped by what each did. */
	function checkGroups(entries: ActivityHistoryEntry[]): { label: string; lines: FlowLine[] }[] {
		const groups = new Map<string, FlowLine[]>();
		for (const entry of entries) {
			const label = VERB[entry.kind] ?? entry.kind;
			const lines = groups.get(label) ?? [];
			lines.push({ name: entry.itemName, quantity: entry.quantity, tt: entry.ttValue });
			groups.set(label, lines);
		}
		return [...groups.entries()].map(([label, lines]) => ({ label, lines }));
	}

	const sentence = $derived(
		event.kind === 'check'
			? 'Every record the check made is taken back; the differences it settled are open again.'
			: 'The boxes and keys return and everything they became is unmade.',
	);

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
	class="rounded-lg bg-surface-hover/25 px-4 py-3 {undone ? 'opacity-50' : ''}"
	data-testid="reviewed-event"
	data-kind={event.kind}
>
	<header class="flex items-baseline gap-3">
		<h3 class="text-sm font-medium tracking-tight text-text {undone ? 'line-through' : ''}">
			{event.kind === 'check' ? 'Stock check' : 'Strongbox opening'}
		</h3>
		<span class="min-w-0 flex-1 truncate text-xs tabular-nums text-text-tertiary">
			{formatLedgerDate(event.at)}
		</span>
		<div class="flex shrink-0 items-center">
			{#if undone}
				<span class="text-xs text-text-tertiary">Undone</span>
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

	{#if event.kind === 'check'}
		<div class="mt-2.5 grid grid-cols-1 gap-3 sm:grid-cols-2">
			{#each checkGroups(event.entries) as group (group.label)}
				<FlowList label={group.label} lines={group.lines} struck={undone} />
			{/each}
		</div>
	{:else}
		{@const opening = event.opening}
		{@const flow = openingFlow(opening)}
		<div class="mt-2.5 grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-start gap-x-4">
			<FlowList
				label="In"
				lines={[
					{ name: opening.boxItem, quantity: opening.boxCount, tt: 0 },
					{ name: 'Keys', quantity: null, tt: opening.keyTt },
				]}
				struck={undone}
			/>
			<span aria-hidden="true" class="pt-4 text-text-tertiary">&rarr;</span>
			<FlowList
				label="Out"
				lines={[
					{ name: 'Ammo', quantity: null, tt: opening.ammoPed },
					...flow.kept,
					...(flow.nanocubes > 0
						? [{ name: 'Nanocube', quantity: flow.nanocubes, tt: flow.recycledTt }]
						: []),
				]}
				struck={undone}
			/>
		</div>
		{#if opening.boxMarkupPaid > 0 || opening.keyMarkupPaid > 0}
			<p class="mt-2 text-xs tabular-nums text-text-tertiary">
				{#if opening.boxMarkupPaid > 0}Boxes cost {formatPed(opening.boxMarkupPaid)} PED above TT{/if}{#if opening.boxMarkupPaid > 0 && opening.keyMarkupPaid > 0};
				{/if}{#if opening.keyMarkupPaid > 0}keys {formatPed(opening.keyMarkupPaid)} PED above TT{/if}.
			</p>
		{/if}
	{/if}

	{#if confirming && !undone}
		<div class="mt-2.5 flex items-center justify-end gap-2 border-t border-border/30 pt-2.5">
			<p class="mr-auto text-xs leading-relaxed text-text-tertiary">{sentence}</p>
			<Button variant="danger" size="sm" loading={busy} onclick={confirm}>
				{event.kind === 'check' ? 'Undo the check' : 'Undo the opening'}
			</Button>
			<Button variant="ghost" size="sm" onclick={() => (confirming = false)}>Cancel</Button>
		</div>
	{/if}
</article>
