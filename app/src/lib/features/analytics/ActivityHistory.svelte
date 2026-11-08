<script lang="ts">
	/**
	 * What this activity has done to its stock, and the way back out of it.
	 *
	 * A review surface: you come here because something is wrong, so it is
	 * built for finding the entry you mean and understanding it at a glance.
	 * Each row says what happened in a line, carries the one figure that matters
	 * on the right, and keeps its undo quiet until asked.
	 *
	 * One list, so no two-pane frame. The sibling views split because a row
	 * there selects something with more to say about it; a history entry says
	 * all it has in its own row, and a detail pane would sit empty beside it.
	 *
	 * A listing appears once however far it got. Creating it and selling it are
	 * the same listing at two moments, so confirming a sale changes the entry
	 * rather than adding another beneath it.
	 *
	 * Undo confirms in place rather than in a second modal, so the row being
	 * acted on stays in front of the reader. A sold listing offers its two ways
	 * back side by side.
	 *
	 * An undone entry stays on the list, struck through and inert. Its effects
	 * are all reversed, so it is a record rather than a state: it says what was
	 * taken back, and offers nothing to do about it. Putting something back is
	 * recording it again, not reviving this.
	 */
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import type { ActivityHistoryEntry } from '$lib/types/analytics';
	import { derivedFromLabel } from '$lib/features/inventory/conversionDraft';
	import { formatLedgerDate, formatPed } from '$lib/utils/format';

	let {
		entries,
		loading,
		onundo,
		embedded = false,
	}: {
		entries: ActivityHistoryEntry[];
		loading: boolean;
		onundo: (entry: ActivityHistoryEntry, revertSale: boolean) => Promise<void>;
		embedded?: boolean;
	} = $props();

	// Which row is asking to be confirmed, and which row is mid-undo. Only one
	// of each: a confirmation is a question about one entry.
	let confirming = $state<string | null>(null);
	let busy = $state<string | null>(null);

	const signedPed = (value: number) => `${value >= 0 ? '+' : ''}${formatPed(value)}`;

	const STATUS_LABEL: Record<string, string> = {
		pending: 'On auction',
		sold: 'Sold',
		expired: 'Expired',
		converted: 'Converted',
		removed: 'Removed',
	};

	const STATUS_TONE: Record<string, string> = {
		pending: 'text-accent',
		sold: 'text-positive',
		expired: 'text-text-tertiary',
		converted: 'text-text-secondary',
		removed: 'text-text-tertiary',
	};

	/** What happened, in a line. The figures a row leads with are on the
	 * right, so this says what was done to how much. */
	function summary(entry: ActivityHistoryEntry): string {
		const tt = `${formatPed(entry.ttValue)} PED TT`;
		if (entry.kind === 'conversion') {
			if (entry.itemised) {
				const used = entry.conversionLines
					.filter((line) => line.side === 'input')
					.map((line) => `${line.quantity} ${line.itemName}`)
					.join(', ');
				return `From ${used}`;
			}
			return `${tt} into ${entry.targetItem ?? 'another item'}`;
		}
		const origin = derivedFromLabel(entry.derivedFrom);
		const suffix = origin ? `, ${origin.charAt(0).toLowerCase()}${origin.slice(1)}` : '';
		if (entry.kind === 'removal') return `${entry.quantity} removed, ${tt}`;
		if (entry.kind === 'trade') return `${entry.quantity} sold by trade, ${tt}${suffix}`;
		if (entry.status === 'sold') return `${entry.quantity} sold, ${tt}${suffix}`;
		if (entry.status === 'expired') return `${entry.quantity} returned, ${tt}`;
		return `${entry.quantity} on auction, ${tt}`;
	}

	/** An itemised conversion leads with what it produced. */
	function title(entry: ActivityHistoryEntry): string {
		if (entry.kind !== 'conversion' || !entry.itemised) return entry.itemName;
		const made = entry.conversionLines
			.filter((line) => line.side === 'output')
			.map((line) => line.itemName);
		if (made.length === 0) return `${formatPed(entry.pedAmount ?? 0)} PED`;
		return made.length === 1 ? made[0] : `${made[0]} and ${made.length - 1} more`;
	}

	function startConfirm(entry: ActivityHistoryEntry) {
		confirming = confirming === entry.id ? null : entry.id;
	}

	async function undo(entry: ActivityHistoryEntry, revertSale: boolean) {
		if (busy) return;
		busy = entry.id;
		try {
			await onundo(entry, revertSale);
			confirming = null;
		} catch {
			// The model surfaces the reason on the tab; the row stays open so
			// the choice that failed is still in front of the reader.
		} finally {
			busy = null;
		}
	}
</script>

{#snippet content()}
	{#if loading}
		<p class="py-10 text-center text-sm text-text-tertiary">Reading what has been recorded...</p>
	{:else if entries.length === 0}
		<div class="flex min-h-40 items-center justify-center p-6">
			<p class="max-w-sm text-center text-sm leading-relaxed text-text-tertiary">
				Nothing recorded yet. Sales, conversions, and removals appear here, and can be
				taken back from here if you record one by mistake.
			</p>
		</div>
	{:else}
		<ul class="flex max-h-[32rem] flex-col overflow-y-auto px-5 py-2">
			{#each entries as entry (entry.id)}
				{@const isConfirming = confirming === entry.id}
				<li
					class="border-b border-border/30 py-3 last:border-b-0
						transition-colors duration-[var(--duration-base)]
						{isConfirming ? 'bg-surface-hover/30' : ''}
						{entry.undone ? 'line-through decoration-border-bright opacity-45' : ''}"
				>
					<div class="flex items-center gap-3">
						<span
							class="w-20 shrink-0 text-[0.625rem] font-medium uppercase tracking-wide
								{entry.undone
								? 'text-text-tertiary'
								: (STATUS_TONE[entry.status] ?? 'text-text-tertiary')}"
						>
							{STATUS_LABEL[entry.status] ?? entry.status}
						</span>

						<div class="min-w-0 flex-1">
							<p class="truncate text-sm font-medium tracking-tight text-text">
								{title(entry)}
							</p>
							<p class="mt-0.5 truncate text-xs text-text-tertiary">{summary(entry)}</p>
						</div>

						<div class="w-24 shrink-0 text-right">
							{#if entry.netMarkup !== null}
								<p class="text-sm font-medium tabular-nums text-text">
									{signedPed(entry.netMarkup)}
								</p>
								<p class="mt-0.5 text-[0.625rem] uppercase tracking-wider text-text-tertiary">
									Net markup
								</p>
							{:else}
								<p class="text-sm tabular-nums text-text-tertiary">
									{formatPed(entry.ttValue)}
								</p>
								<p class="mt-0.5 text-[0.625rem] uppercase tracking-wider text-text-tertiary">
									PED
								</p>
							{/if}
						</div>

						<span class="w-14 shrink-0 text-right text-xs tabular-nums text-text-tertiary">
							{formatLedgerDate(entry.occurredAt)}
						</span>

						<div class="flex w-20 shrink-0 items-center justify-end">
							{#if entry.undone}
								<!-- Where the action was, since there is none left to take.
									`inline-block` keeps the row's strike off it: the label
									says what the row is, it is not part of the record being
									struck. -->
								<span class="inline-block text-xs text-text-tertiary">Undone</span>
							{:else if !entry.canDelete && !entry.canRevertSale}
								<span class="flex items-center gap-1 text-xs text-text-tertiary">
									Held
									<InfoTip align="right" label="Why this cannot be undone" width="w-80">
										<p class="text-xs font-semibold leading-relaxed text-text">
											Something else depends on this
										</p>
										<p class="mt-1 text-xs leading-relaxed text-text-secondary">
											{entry.undoBlockedReason}
										</p>
									</InfoTip>
								</span>
							{:else}
								<Button
									variant="ghost"
									size="sm"
									loading={busy === entry.id}
									onclick={() => startConfirm(entry)}
								>
									Undo
								</Button>
							{/if}
						</div>
					</div>

					{#if isConfirming}
						<!-- Under the row it belongs to, right-aligned on the action
							column: the only place with room, and what is about to be
							undone stays in front of the reader. -->
						<div class="mt-2 flex items-center justify-end gap-2">
							{#if entry.canRevertSale}
								<Button
									variant="danger"
									size="sm"
									loading={busy === entry.id}
									onclick={() => undo(entry, true)}
								>
									Undo sale
								</Button>
							{/if}
							{#if entry.canDelete}
								<Button
									variant="danger"
									size="sm"
									loading={busy === entry.id}
									onclick={() => undo(entry, false)}
								>
									{entry.canRevertSale ? 'Undo listing' : 'Confirm undo'}
								</Button>
							{/if}
							<Button variant="ghost" size="sm" onclick={() => (confirming = null)}>
								Cancel
							</Button>
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
{/snippet}

{#if embedded}
	<div>{@render content()}</div>
{:else}
	<Card class="hover:z-20">{@render content()}</Card>
{/if}
