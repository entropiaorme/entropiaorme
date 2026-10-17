<script lang="ts">
	import Menu from '$lib/components/Menu.svelte';
	import type { QuestRewardReviewInput } from '$lib/types';
	import { formatTimeUntil } from '../cooldown';
	import QuestSettlePanel from './QuestSettlePanel.svelte';
	import { type BoardRow, formatBackAt } from './questBoard';

	let {
		row,
		now,
		busy,
		settling,
		copied,
		onCopyWaypoint,
		onStart,
		onComplete,
		onAbandon,
		onResetCooldown,
		onUndoReward,
		onToggleSettle,
		onSettle,
		onEdit,
		onDelete
	}: {
		row: BoardRow;
		now: number;
		busy: boolean;
		settling: boolean;
		copied: boolean;
		onCopyWaypoint: () => void;
		onStart: () => void;
		onComplete: () => void;
		onAbandon: () => void;
		onResetCooldown: () => void;
		onUndoReward: () => void;
		onToggleSettle: () => void;
		onSettle: (input: QuestRewardReviewInput) => void;
		onEdit: () => void;
		onDelete: () => void;
	} = $props();

	/** Abandon asks once: giving a mission up holds its slot until the next
	 * one can be collected, and a slot's timer cannot be taken back. */
	let confirmingAbandon = $state(false);
	/** Deleting one of the player's own quests asks once too. */
	let confirmingDelete = $state(false);
	$effect(() => {
		if (row.state !== 'running' && row.state !== 'recording' && row.state !== 'waiting') {
			confirmingAbandon = false;
		}
	});

	const running = $derived(row.state === 'running' || row.state === 'recording' || row.state === 'waiting');
	const standingRun = $derived(row.trigger === 'signal_item' || row.trigger === 'manual_hand_in');

	const dotClass = $derived(
		row.state === 'ready'
			? 'signal-dot positive'
			: row.state === 'cooling'
				? 'signal-dot idle'
				: row.state === 'running'
					? 'signal-dot'
					: 'signal-dot animate-pulse'
	);

	/** The row's one line of status, in the player's words. */
	const status = $derived.by((): string => {
		switch (row.state) {
			case 'ready':
				if (row.kind === 'slot') return "Pick up today's mission";
				if (standingRun) return 'Ready';
				return row.giver ? `Pick up from ${row.giver}` : 'Ready to pick up';
			case 'running':
				return standingRun ? 'Run open' : 'In your mission log';
			case 'recording':
				return 'Recording now';
			case 'waiting':
				return 'Waiting for the reward clump';
			case 'cooling':
				return row.abandoned ? 'Abandoned' : 'Done';
		}
	});

	const menuItems = $derived.by(() => {
		const items: { label: string; danger?: boolean; onSelect: () => void }[] = [];
		const quest = row.quest;
		if (running && row.trigger === 'mission_log') {
			items.push({ label: 'Mark complete', onSelect: onComplete });
		}
		if (row.state === 'cooling' && quest && quest.cooldownDurationHours) {
			items.push({ label: 'Reset cooldown', onSelect: onResetCooldown });
		}
		if (row.state === 'cooling' && quest?.rewardUndoAvailable) {
			items.push({ label: 'Undo reward', danger: true, onSelect: onUndoReward });
		}
		if (row.custom && row.kind === 'quest') {
			items.push({ label: 'Edit', onSelect: onEdit });
			items.push({ label: 'Delete', danger: true, onSelect: () => (confirmingDelete = true) });
		}
		return items;
	});

	const settleLabel = $derived(
		row.unresolved.length === 1 ? 'Settle reward' : `Settle ${row.unresolved.length} rewards`
	);
</script>

<li class="board-row group" data-testid="quest-board-row" data-state={row.state}>
	<span class="shrink-0 {dotClass}" aria-hidden="true"></span>

	<div class="min-w-0">
		<p class="flex items-baseline gap-2 min-w-0">
			<span class="truncate text-sm font-medium {row.state === 'cooling' ? 'text-text-secondary' : 'text-text'}">
				{row.label}
			</span>
			{#if row.variant}
				<span class="truncate text-sm text-text-secondary">{row.variant}</span>
			{/if}
			{#if row.detail}
				<span class="shrink-0 text-xs text-text-tertiary">{row.detail}</span>
			{/if}
		</p>
		<p class="mt-0.5 flex items-baseline gap-1.5 min-w-0 text-xs text-text-tertiary">
			<span class="truncate {row.state === 'recording' ? 'text-accent' : ''}">{status}</span>
			{#if row.rewardLabel}
				<span class="shrink-0" aria-hidden="true">·</span>
				<span class="truncate">{row.rewardLabel}</span>
			{/if}
			{#if row.unresolved.length > 0}
				<span class="shrink-0" aria-hidden="true">·</span>
				<button
					type="button"
					class="shrink-0 font-medium text-warning hover:text-text cursor-pointer transition-colors"
					aria-expanded={settling}
					onclick={onToggleSettle}
				>{settleLabel}</button>
			{/if}
		</p>
	</div>

	<div class="text-right">
		{#if row.state === 'cooling' && row.backAt != null}
			<p class="text-sm tabular-nums text-text-secondary" title="Available again {formatBackAt(row.backAt, now)}">
				{formatBackAt(row.backAt, now)}
			</p>
			<p class="text-[11px] tabular-nums text-text-tertiary">
				{formatTimeUntil(row.backAt / 1000, now) ?? 'now'}
			</p>
		{/if}
	</div>

	<div class="flex items-center justify-end gap-1">
		{#if confirmingDelete}
			<span class="text-xs text-text-secondary">Delete this quest?</span>
			<button type="button" class="row-verb text-negative" disabled={busy} onclick={() => { confirmingDelete = false; onDelete(); }}>
				Delete
			</button>
			<button type="button" class="row-verb" disabled={busy} onclick={() => (confirmingDelete = false)}>
				Keep
			</button>
		{:else if confirmingAbandon}
			<span class="text-xs text-text-secondary">Abandon it in game too?</span>
			<button type="button" class="row-verb text-negative" disabled={busy} onclick={() => { confirmingAbandon = false; onAbandon(); }}>
				Abandon
			</button>
			<button type="button" class="row-verb" disabled={busy} onclick={() => (confirmingAbandon = false)}>
				Keep
			</button>
		{:else}
			{#if row.waypoint && row.state !== 'cooling'}
				<button
					type="button"
					class="row-verb"
					title="Copy the waypoint"
					aria-label="Copy the waypoint for {row.label}"
					onclick={onCopyWaypoint}
				>{copied ? 'Copied' : 'WP'}</button>
			{/if}
			<div class="reveal flex items-center gap-1">
				{#if running}
					<button type="button" class="row-verb" disabled={busy} onclick={() => (confirmingAbandon = true)}>
						Abandon
					</button>
				{:else if row.state === 'ready' && standingRun && row.quest}
					<button type="button" class="row-verb" disabled={busy} onclick={onStart}>Start</button>
				{/if}
				{#if menuItems.length > 0}
					<Menu ariaLabel="More for {row.label}" items={menuItems} overlay align="right" panelClass="min-w-40" />
				{/if}
			</div>
		{/if}
	</div>

	{#if settling && row.unresolved.length > 0}
		<div class="col-span-full">
			<QuestSettlePanel rewards={row.unresolved} {busy} {onSettle} />
		</div>
	{/if}
</li>

<style>
	/* One quest per line: status, what it is, when it is back, what you can
	   do. Hairline-separated, never boxed. */
	.board-row {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto auto;
		align-items: center;
		column-gap: 0.875rem;
		padding: 0.625rem 0.25rem;
		border-bottom: 1px solid color-mix(in oklab, var(--color-border) 40%, transparent);
	}
	.board-row:last-child {
		border-bottom: 0;
	}
	/* Secondary verbs stay quiet until the row is pointed at or focused, so
	   the board reads as state first and controls second. */
	.reveal {
		opacity: 0;
		transition: opacity var(--duration-base) var(--ease-out);
	}
	.board-row:hover .reveal,
	.board-row:focus-within .reveal {
		opacity: 1;
	}
	.row-verb {
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--color-accent);
		padding: 0.25rem 0.5rem;
		border-radius: var(--radius-sm);
		cursor: pointer;
		transition:
			color var(--duration-base) var(--ease-out),
			background-color var(--duration-base) var(--ease-out);
	}
	.row-verb:hover:not(:disabled) {
		color: var(--color-accent-hover);
		background: color-mix(in oklab, var(--color-surface-hover) 50%, transparent);
	}
	.row-verb:disabled {
		color: var(--color-text-tertiary);
		cursor: default;
	}
	.row-verb.text-negative {
		color: var(--color-negative);
	}
	@media (prefers-reduced-motion: reduce) {
		.reveal {
			transition: none;
		}
	}
</style>
