<script lang="ts">
	import {
		cancelQuestHandIn,
		confirmQuestHandIn,
		getQuestHandInState,
		waitForNextQuestHandInClump,
		type QuestHandInState,
	} from '$lib/api';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import { formatPed } from '$lib/utils/format';
	import { untrack } from 'svelte';

	let {
		initialState,
		onComplete,
		onCancel,
	}: {
		initialState: QuestHandInState;
		onComplete: () => void | Promise<void>;
		onCancel: () => void | Promise<void>;
	} = $props();

	let handIn = $state<QuestHandInState>(untrack(() => ({ ...initialState })));
	let busy = $state(false);
	let error = $state<string | null>(null);
	let polling = false;

	function describe(problem: unknown, fallback: string) {
		return problem instanceof Error ? problem.message : fallback;
	}

	async function refreshWaiting() {
		if (!handIn.waiting || busy || polling) return;
		polling = true;
		try {
			handIn = await getQuestHandInState(handIn.questId);
			error = null;
		} catch (problem) {
			error = describe(problem, 'Could not check for the reward clump');
		} finally {
			polling = false;
		}
	}

	$effect(() => {
		if (!handIn.waiting) return;
		return useVisiblePoll(refreshWaiting, { intervalMs: 500 });
	});

	async function waitNext() {
		const candidate = handIn.candidate;
		if (!candidate || busy) return;
		busy = true;
		error = null;
		try {
			handIn = await waitForNextQuestHandInClump(handIn.questId, candidate.id);
		} catch (problem) {
			error = describe(problem, 'Could not wait for the next clump');
		} finally {
			busy = false;
		}
	}

	async function confirm() {
		const candidate = handIn.candidate;
		if (!candidate || busy) return;
		busy = true;
		error = null;
		try {
			await confirmQuestHandIn(handIn.questId, candidate.id);
			await onComplete();
		} catch (problem) {
			error = describe(problem, 'Could not confirm the quest reward');
			busy = false;
		}
	}

	async function cancel() {
		if (busy) return;
		busy = true;
		error = null;
		try {
			await cancelQuestHandIn(handIn.questId);
			await onCancel();
		} catch (problem) {
			error = describe(problem, 'Could not cancel the hand-in');
			busy = false;
		}
	}

	function observedTime(value: string) {
		const parsed = new Date(value);
		if (Number.isNaN(parsed.getTime())) return value;
		return new Intl.DateTimeFormat(undefined, {
			hour: '2-digit',
			minute: '2-digit',
			second: '2-digit',
		}).format(parsed);
	}
</script>

<div class="hand-in-panel" aria-live="polite" data-overlay-surface>
	<div class="hand-in-heading">
		<div class="hand-in-kicker">Hand in quest</div>
		<div class="hand-in-name">{handIn.questName}</div>
	</div>

	{#if handIn.candidate}
		<div class="hand-in-question">Is this your quest reward?</div>
		<div class="hand-in-meta">
			<span>{observedTime(handIn.candidate.observedAt)}</span>
			<span class="tabular-nums">{formatPed(handIn.candidate.totalPed)} PED TT</span>
		</div>
		<div class="hand-in-items">
			{#each handIn.candidate.items as item, index (`${item.itemName}:${index}`)}
				<div class="hand-in-item">
					<span class="hand-in-item-name">{item.itemName}</span>
					<span class="hand-in-item-value tabular-nums">
						x{item.quantity.toLocaleString()} · {formatPed(item.valuePed)} PED
					</span>
				</div>
			{/each}
		</div>
	{:else}
		<div class="hand-in-waiting">
			<span class="waiting-dot" aria-hidden="true"></span>
			<div>
				<div class="hand-in-question">Hand in the quest now</div>
				<div class="hand-in-copy">The next loot clump will appear here for confirmation.</div>
			</div>
		</div>
	{/if}

	{#if error}
		<div class="hand-in-error" role="alert">{error}</div>
	{/if}

	<div class="hand-in-actions">
		{#if handIn.candidate}
			<button type="button" class="action-primary" disabled={busy} onclick={confirm}>
				Confirm reward
			</button>
			<button type="button" class="action-secondary" disabled={busy} onclick={waitNext}>
				No, wait for the next clump
			</button>
		{/if}
		<button type="button" class="action-quiet" disabled={busy} onclick={cancel}>Cancel</button>
	</div>
</div>

<style>
	.hand-in-panel {
		display: flex;
		flex-direction: column;
		gap: 10px;
		width: 100%;
		max-height: 220px;
		overflow-y: auto;
		padding: 12px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		border-radius: 8px;
		background: rgba(11, 15, 25, 0.97);
		color: rgba(255, 255, 255, 0.9);
	}

	.hand-in-heading,
	.hand-in-items {
		display: flex;
		flex-direction: column;
	}

	.hand-in-kicker {
		color: rgba(125, 211, 252, 0.86);
		font-size: 10px;
		font-weight: 700;
		letter-spacing: 0.08em;
		text-transform: uppercase;
	}

	.hand-in-name {
		overflow: hidden;
		color: rgba(255, 255, 255, 0.72);
		font-size: 11px;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.hand-in-question {
		font-size: 13px;
		font-weight: 650;
	}

	.hand-in-meta,
	.hand-in-item,
	.hand-in-actions,
	.hand-in-waiting {
		display: flex;
		align-items: center;
	}

	.hand-in-meta {
		justify-content: space-between;
		color: rgba(255, 255, 255, 0.52);
		font-size: 10px;
	}

	.hand-in-items {
		border-block: 1px solid rgba(255, 255, 255, 0.08);
	}

	.hand-in-item {
		justify-content: space-between;
		gap: 12px;
		padding: 6px 0;
		font-size: 11px;
	}

	.hand-in-item + .hand-in-item {
		border-top: 1px solid rgba(255, 255, 255, 0.05);
	}

	.hand-in-item-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.hand-in-item-value {
		flex-shrink: 0;
		color: rgba(255, 255, 255, 0.62);
	}

	.hand-in-waiting {
		gap: 9px;
		padding: 6px 0;
	}

	.waiting-dot {
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: rgb(56, 189, 248);
		box-shadow: 0 0 0 4px rgba(56, 189, 248, 0.12);
	}

	.hand-in-copy,
	.hand-in-error {
		font-size: 10px;
	}

	.hand-in-copy {
		margin-top: 2px;
		color: rgba(255, 255, 255, 0.48);
	}

	.hand-in-error {
		color: rgb(252, 165, 165);
	}

	.hand-in-actions {
		flex-wrap: wrap;
		gap: 6px;
	}

	.hand-in-actions button {
		padding: 6px 8px;
		border: 0;
		border-radius: 5px;
		font-size: 10px;
		font-weight: 650;
		cursor: pointer;
	}

	.hand-in-actions button:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.action-primary {
		background: rgb(14, 165, 233);
		color: rgb(3, 7, 18);
	}

	.action-secondary {
		background: rgba(255, 255, 255, 0.09);
		color: rgba(255, 255, 255, 0.82);
	}

	.action-quiet {
		margin-left: auto;
		background: transparent;
		color: rgba(255, 255, 255, 0.52);
	}
</style>
