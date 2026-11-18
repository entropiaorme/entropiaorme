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
	 *
	 * An entry the account readings recorded on their own says so quietly
	 * after its line, with when it happened on hover or focus. Its Undo is
	 * the same as any other, and says that the readings will not record it
	 * again.
	 *
	 * An entry a stock check recorded is undone only with the whole check:
	 * its Undo offers that instead, naming what the check takes back.
	 */
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import type { ActivityHistoryEntry } from '$lib/types/analytics';
	import { derivedFromLabel } from '$lib/features/inventory/conversionDraft';
	import { FROM_ACCOUNT_UNDO, fromAccountSentence } from '$lib/features/inventory/fromAccount';
	import {
		NO_TT_TITLE,
		formatCount,
		formatLedgerDate,
		formatPed,
		formatQuantity,
		isZeroTt,
		listNames,
	} from '$lib/utils/format';

	let {
		entries,
		loading,
		onundo,
		onundocheck,
		embedded = false,
	}: {
		entries: ActivityHistoryEntry[];
		loading: boolean;
		onundo: (entry: ActivityHistoryEntry, revertSale: boolean) => Promise<void>;
		/** Undo the stock check an entry was recorded by, as a whole. */
		onundocheck?: (reconciliationId: string) => Promise<void>;
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
		held: 'Added',
	};

	const STATUS_TONE: Record<string, string> = {
		pending: 'text-accent',
		sold: 'text-positive',
		expired: 'text-text-tertiary',
		converted: 'text-text-secondary',
		removed: 'text-text-tertiary',
		held: 'text-text-secondary',
	};

	/** What happened, in a line. The figures a row leads with are on the
	 * right, so this says what was done to how much. */
	function summary(entry: ActivityHistoryEntry): string {
		// An item with no TT is told by its count alone; a TT of 0.00 says nothing.
		const noTt = isZeroTt(entry.ttValue);
		const tt = `${formatPed(entry.ttValue)} PED TT`;
		const withTt = noTt ? '' : `, ${tt}`;
		const count = formatQuantity(entry.quantity);
		if (entry.kind === 'conversion') {
			if (entry.itemised) {
				const used = entry.conversionLines
					.filter((line) => line.side === 'input')
					.map((line) => `${line.quantity} ${line.itemName}`)
					.join(', ');
				return `From ${used}`;
			}
			return `${noTt ? count : tt} into ${entry.targetItem ?? 'another item'}`;
		}
		const origin = derivedFromLabel(entry.derivedFrom);
		const suffix = origin ? `, ${origin.charAt(0).toLowerCase()}${origin.slice(1)}` : '';
		if (entry.kind === 'removal') return `${count} removed${withTt}`;
		if (entry.kind === 'unknownOrigin') return `${count} held as stock of unknown origin${withTt}`;
		if (entry.kind === 'trade') return `${count} sold by trade${withTt}${suffix}`;
		if (entry.status === 'sold') return `${count} sold${withTt}${suffix}`;
		if (entry.status === 'expired') return `${count} returned${withTt}`;
		return `${count} on auction${withTt}`;
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

	/** The stock check an entry is undone with, when it is one's and the
	 * check can be undone from here. */
	function checkOf(entry: ActivityHistoryEntry): string | null {
		return onundocheck && !entry.undone ? entry.reconciliationId : null;
	}

	/** What undoing a check takes back: what this list shows of it, and
	 * whatever else it recorded that the list does not show. */
	function checkSentence(reconciliationId: string): string {
		const names = entries
			.filter((row) => row.reconciliationId === reconciliationId && !row.undone)
			.map((row) => `${title(row)} (${STATUS_LABEL[row.status]?.toLowerCase() ?? row.status})`);
		const shown = names.length > 4 ? [...names.slice(0, 4), `${names.length - 4} more`] : names;
		const listed = [...shown, 'anything else the check recorded'];
		return `Part of a stock check: undo the check as a whole. It takes back ${listNames(listed)}.`;
	}

	async function undoCheck(entry: ActivityHistoryEntry, reconciliationId: string) {
		if (busy || !onundocheck) return;
		busy = entry.id;
		try {
			await onundocheck(reconciliationId);
			confirming = null;
		} catch {
			// As for any undo: the reason is on the tab and the row stays open.
		} finally {
			busy = null;
		}
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
				{@const check = checkOf(entry)}
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
							<p class="mt-0.5 flex min-w-0 items-baseline gap-1.5 text-xs text-text-tertiary">
								<span class="truncate">{summary(entry)}</span>
								{#if entry.recordedFromAccount}
									{@const step = entry.recordedFromAccount}
									<span class="shrink-0" data-testid="from-account">
										<InfoTip label="Recorded from your account" align="left" width="w-64">
											{#snippet trigger()}<span
													class="underline decoration-dotted underline-offset-2 hover:text-text-secondary"
													>from your account</span
												>{/snippet}
											<span class="block text-xs leading-relaxed text-text-secondary"
												>{fromAccountSentence(step)}</span
											>
										</InfoTip>
									</span>
								{/if}
							</p>
						</div>

						<div class="w-24 shrink-0 text-right">
							{#if entry.netMarkup !== null}
								<p class="text-sm font-medium tabular-nums text-text">
									{signedPed(entry.netMarkup)}
								</p>
								<p class="mt-0.5 text-[0.625rem] uppercase tracking-wider text-text-tertiary">
									Net markup
								</p>
							{:else if isZeroTt(entry.ttValue)}
								<p class="text-sm tabular-nums text-text-tertiary" title={NO_TT_TITLE}>
									{formatCount(entry.quantity)}
								</p>
								<p class="mt-0.5 text-[0.625rem] uppercase tracking-wider text-text-tertiary">
									No TT
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
							{:else if check}
								<Button
									variant="ghost"
									size="sm"
									aria-label="Undo the stock check"
									loading={busy === entry.id}
									onclick={() => startConfirm(entry)}
								>
									Undo
								</Button>
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

					{#if isConfirming && check}
						<div class="mt-2 flex items-center justify-end gap-2">
							<p class="mr-auto text-xs text-text-tertiary">{checkSentence(check)}</p>
							<Button
								variant="danger"
								size="sm"
								loading={busy === entry.id}
								onclick={() => undoCheck(entry, check)}
							>
								Undo the check
							</Button>
							<Button variant="ghost" size="sm" onclick={() => (confirming = null)}>
								Cancel
							</Button>
						</div>
					{:else if isConfirming}
						<!-- Under the row it belongs to, right-aligned on the action
							column: the only place with room, and what is about to be
							undone stays in front of the reader. -->
						<div class="mt-2 flex items-center justify-end gap-2">
							{#if entry.recordedFromAccount}
								<p class="mr-auto text-xs text-text-tertiary">{FROM_ACCOUNT_UNDO}</p>
							{/if}
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
