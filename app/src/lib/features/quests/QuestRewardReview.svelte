<script lang="ts">
	import { onMount } from 'svelte';
	import { Button, ErrorNotice } from '$lib/components';
	import type { UnresolvedQuestReward } from '$lib/api';
	import { getUnresolvedQuestRewards, reviewQuestReward } from '$lib/api';
	import { describeError } from '$lib/view/errorState';
	import { formatPed } from '$lib/utils/format';

	let rows = $state<UnresolvedQuestReward[]>([]);
	let loading = $state(true);
	let savingId = $state<number | null>(null);
	let error = $state<string | null>(null);
	let selected = $state<Record<number, number[]>>({});

	async function load(): Promise<void> {
		loading = true;
		error = null;
		try {
			rows = await getUnresolvedQuestRewards();
		} catch (cause) {
			error = describeError(cause, 'Failed to load unresolved rewards');
		} finally {
			loading = false;
		}
	}

	function toggle(completionId: number, index: number): void {
		const current = selected[completionId] ?? [];
		selected = {
			...selected,
			[completionId]: current.includes(index)
				? current.filter((value) => value !== index)
				: [...current, index]
		};
	}

	async function resolve(row: UnresolvedQuestReward, declareNone: boolean): Promise<void> {
		savingId = row.completionId;
		error = null;
		try {
			await reviewQuestReward({
				completionId: row.completionId,
				selectedIndices: declareNone ? [] : (selected[row.completionId] ?? []),
				declareNone
			});
			rows = rows.filter((candidate) => candidate.completionId !== row.completionId);
		} catch (cause) {
			error = describeError(cause, 'The reward could not be resolved safely');
		} finally {
			savingId = null;
		}
	}

	onMount(() => void load());
</script>

<div class="space-y-4">
	<div>
		<h2 class="text-sm font-medium text-text">Unresolved reward evidence</h2>
		<p class="mt-1 text-sm text-text-secondary">
			Only confirm lines that were additional quest rewards. A confirmation proceeds only when
			each line maps to one exact recorded acquisition; otherwise the data remains untouched.
		</p>
	</div>

	{#if error}<ErrorNotice message={error} />{/if}
	{#if loading}
		<p class="py-8 text-center text-sm text-text-tertiary">Loading reward evidence...</p>
	{:else if rows.length === 0}
		<p class="py-8 text-center text-sm text-text-tertiary">No unresolved quest rewards.</p>
	{:else}
		<div class="divide-y divide-border">
			{#each rows as row (row.completionId)}
				<section class="py-4 first:pt-0">
					<div class="flex flex-wrap items-start justify-between gap-3">
						<div>
							<h3 class="font-medium text-text">{row.questName}</h3>
							<p class="text-xs text-text-tertiary">
								{new Date(row.completedAt * 1000).toLocaleString()} · {row.reason ?? 'Capture was ambiguous'}
							</p>
						</div>
						{#if row.isolated !== null}
							<span class="text-xs text-text-secondary">{row.isolated ? 'Isolated tick' : 'Mixed tick'}</span>
						{/if}
					</div>
					<div class="mt-3 space-y-1.5">
						{#each row.loot as item, index (`${row.completionId}-${index}`)}
							<label class="flex cursor-pointer items-center gap-3 py-1 text-sm">
								<input
									type="checkbox"
									checked={(selected[row.completionId] ?? []).includes(index)}
									onchange={() => toggle(row.completionId, index)}
									class="accent-accent"
								/>
								<span class="min-w-0 flex-1 text-text">{item.itemName} × {item.quantity}</span>
								<span class="tabular-nums text-text-secondary">{formatPed(item.valuePed)}</span>
							</label>
						{/each}
					</div>
					<div class="mt-3 flex flex-wrap gap-2">
						<Button
							size="sm"
							disabled={(selected[row.completionId] ?? []).length === 0 || savingId !== null}
							loading={savingId === row.completionId}
							onclick={() => void resolve(row, false)}
						>Confirm selected</Button>
						<Button
							size="sm"
							variant="secondary"
							disabled={savingId !== null}
							onclick={() => void resolve(row, true)}
						>No separate reward</Button>
					</div>
				</section>
			{/each}
		</div>
	{/if}
</div>
