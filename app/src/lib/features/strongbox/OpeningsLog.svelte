<script lang="ts">
	/**
	 * The runs already recorded, newest first, each with what it opened into
	 * as it stands now, and a way to take one back whole.
	 */
	import Button from '$lib/components/Button.svelte';
	import type { StrongboxOpeningEntry } from '$lib/api/commands.gen';
	import { formatLedgerDate, formatPed, formatQuantity, listNames } from '$lib/utils/format';

	let {
		openings,
		undoing,
		onundo,
		highlight = null,
	}: {
		openings: StrongboxOpeningEntry[];
		undoing: string | null;
		onundo: (id: string) => void;
		/** The run just recorded, marked so it is found at a glance. */
		highlight?: string | null;
	} = $props();

	function kept(run: StrongboxOpeningEntry): string[] {
		return run.contents.filter((item) => item.nanocubes === null).map((item) => item.itemName);
	}

	function cubes(run: StrongboxOpeningEntry): number {
		return run.contents.reduce((sum, item) => sum + (item.nanocubes ?? 0), 0);
	}

	function confirmUndo(run: StrongboxOpeningEntry) {
		const message = `Take back the opening of ${formatQuantity(run.boxCount)} ${run.boxItem}? The boxes return and everything they became is unmade.`;
		if (window.confirm(message)) onundo(run.id);
	}
</script>

<ul class="divide-y divide-border/50">
	{#each openings as run (run.id)}
		{@const items = kept(run)}
		{@const made = cubes(run)}
		<li
			class="grid grid-cols-[5.5rem_minmax(0,1fr)_auto] items-baseline gap-x-5 py-3 {run.undone
				? 'opacity-50'
				: ''} {run.id === highlight ? 'bg-accent-faint -mx-3 px-3' : ''}"
		>
			<span class="text-xs tabular-nums text-text-tertiary">{formatLedgerDate(run.openedAt)}</span>
			<span class="min-w-0">
				<span class="block truncate text-sm text-text {run.undone ? 'line-through' : ''}">
					<span class="tabular-nums">{formatQuantity(run.boxCount)}</span> {run.boxItem}
				</span>
				<span class="block truncate text-xs text-text-tertiary">
					{#if items.length > 0}
						{listNames(items.slice(0, 3))}{items.length > 3 ? ` and ${items.length - 3} more` : ''}
						&middot;
					{/if}
					{#if made > 0}{formatQuantity(made)} Nanocubes &middot;{/if}
					{formatPed(run.ammoPed)} PED ammo
					{#if run.boxMarkupPaid > 0}&middot; boxes cost {formatPed(run.boxMarkupPaid)} PED above TT{/if}
				</span>
			</span>
			{#if run.undone}
				<span class="text-xs text-text-tertiary">Undone</span>
			{:else}
				<Button
					size="sm"
					variant="ghost"
					loading={undoing === run.id}
					disabled={undoing !== null && undoing !== run.id}
					onclick={() => confirmUndo(run)}
				>
					Undo
				</Button>
			{/if}
		</li>
	{/each}
</ul>
