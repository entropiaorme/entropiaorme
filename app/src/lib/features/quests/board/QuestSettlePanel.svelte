<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import type { QuestRewardReviewInput, UnresolvedQuestReward } from '$lib/types';
	import { variantOf } from './questBoard';

	let {
		rewards,
		busy,
		onSettle
	}: {
		rewards: UnresolvedQuestReward[];
		busy: boolean;
		onSettle: (input: QuestRewardReviewInput) => void;
	} = $props();

	/** The completion-tick lines picked for a clump reward, per completion. */
	let picked = $state<Record<number, number[]>>({});

	function togglePick(completionId: number, index: number) {
		const current = picked[completionId] ?? [];
		picked = {
			...picked,
			[completionId]: current.includes(index)
				? current.filter((entry) => entry !== index)
				: [...current, index]
		};
	}

	function when(completedAt: number): string {
		const at = new Date(completedAt * 1000);
		return `${at.toLocaleDateString([], { day: 'numeric', month: 'short' })}, ${at.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
	}

	function what(reward: UnresolvedQuestReward): string {
		return reward.expectedItems.length > 0
			? reward.expectedItems.join(', ')
			: 'the reward';
	}

	function quantity(value: number): string {
		return value === 1 ? '' : ` x${value.toLocaleString()}`;
	}
</script>

<!-- One settlement per unresolved completion, newest first. Each offers only
	 the routes it can honour: the exact loot line the reward arrived as, the
	 completion's own recorded lines, or a declaration that it was paid. -->
<div class="settle" data-testid="quest-settle-panel">
	{#each rewards as reward (reward.completionId)}
		<div class="settle-item">
			<div class="flex items-baseline justify-between gap-3">
				<p class="text-xs text-text-secondary">
					<span class="text-text">{variantOf(reward.questName)}</span>
					<span class="text-text-tertiary"> · {when(reward.completedAt)}</span>
				</p>
				<Button
					size="sm"
					variant="ghost"
					disabled={busy}
					onclick={() => onSettle({ completionId: reward.completionId, declareNone: true })}
				>
					{#snippet children()}No reward{/snippet}
				</Button>
			</div>

			{#if reward.candidates.length > 0}
				<p class="mt-1 text-xs text-text-tertiary">
					{what(reward)} was recorded as loot. Use the line it arrived as:
				</p>
				<ul class="mt-1.5 flex flex-col">
					{#each reward.candidates as line (line.id)}
						<li class="candidate">
							<span class="flex-1 min-w-0 truncate text-xs text-text-secondary">
								{line.itemName}{quantity(line.quantity)}
								{#if line.mobName}<span class="text-text-tertiary"> · from {line.mobName}</span>{/if}
							</span>
							<button
								type="button"
								class="settle-action"
								disabled={busy}
								onclick={() => onSettle({ completionId: reward.completionId, lootItemIds: [line.id] })}
							>Use this</button>
						</li>
					{/each}
				</ul>
			{:else if reward.loot.length > 0}
				<p class="mt-1 text-xs text-text-tertiary">
					{reward.isolated === false
						? 'The completion arrived with other loot. Pick the lines that were the reward:'
						: 'Pick the lines that were the reward:'}
				</p>
				<ul class="mt-1.5 flex flex-col">
					{#each reward.loot as line, index (index)}
						<li class="candidate">
							<label class="flex flex-1 min-w-0 items-center gap-2 text-xs text-text-secondary cursor-pointer">
								<input
									type="checkbox"
									class="accent-[var(--color-accent)]"
									checked={(picked[reward.completionId] ?? []).includes(index)}
									disabled={busy}
									onchange={() => togglePick(reward.completionId, index)}
								/>
								<span class="truncate">{line.itemName}{quantity(line.quantity)}</span>
								<span class="ml-auto tabular-nums text-text-tertiary">{line.valuePed.toFixed(2)} PED</span>
							</label>
						</li>
					{/each}
				</ul>
				<div class="mt-2 flex justify-end">
					<Button
						size="sm"
						disabled={busy || (picked[reward.completionId] ?? []).length === 0}
						onclick={() =>
							onSettle({
								completionId: reward.completionId,
								selectedIndices: picked[reward.completionId] ?? []
							})}
					>
						{#snippet children()}Confirm reward{/snippet}
					</Button>
				</div>
			{:else if reward.declarable}
				<div class="mt-1 flex items-center justify-between gap-3">
					<p class="text-xs text-text-tertiary">
						Nothing recorded shows {what(reward)}. If it was paid, say so; it is kept as declared.
					</p>
					<Button
						size="sm"
						variant="secondary"
						disabled={busy}
						onclick={() => onSettle({ completionId: reward.completionId, declarePaid: true })}
					>
						{#snippet children()}It was paid{/snippet}
					</Button>
				</div>
			{:else}
				<p class="mt-1 text-xs text-text-tertiary">
					{reward.reason ?? 'Nothing recorded can show this reward.'}
				</p>
			{/if}
		</div>
	{/each}
</div>

<style>
	/* Settlement hangs off its row on a rail rather than in a box: the rail
	   and the indentation say "this belongs to the quest above". */
	.settle {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
		margin: 0 0 0.75rem 1.25rem;
		padding: 0.25rem 0 0.25rem 0.875rem;
		border-left: 1px solid color-mix(in oklab, var(--color-accent) 35%, transparent);
	}
	.candidate {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: 0.3rem 0;
		border-bottom: 1px solid color-mix(in oklab, var(--color-border) 35%, transparent);
	}
	.candidate:last-child {
		border-bottom: 0;
	}
	.settle-action {
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--color-accent);
		cursor: pointer;
		transition: color var(--duration-base) var(--ease-out);
	}
	.settle-action:hover:not(:disabled) {
		color: var(--color-accent-hover);
	}
	.settle-action:disabled {
		color: var(--color-text-tertiary);
		cursor: default;
	}
</style>
