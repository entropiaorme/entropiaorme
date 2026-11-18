<script lang="ts">
	/**
	 * What applying came to: the items settled, the ones left as they are,
	 * the ones left for the next check and why, and one Undo for the whole
	 * check when it recorded anything. After an undo the focus moves to the
	 * line saying so.
	 */
	import { tick } from 'svelte';
	import Button from '$lib/components/Button.svelte';
	import { formatQuantity, listNames } from '$lib/utils/format';
	import { REFUSAL_LABEL } from './reconciliation';
	import type { ReconciliationModel } from './reconciliationModel.svelte';

	let {
		model,
		ondone,
	}: {
		model: ReconciliationModel;
		/** Leave for the Inventory. */
		ondone: () => void;
	} = $props();

	const outcome = $derived(model.outcome);
	const changed = $derived(model.settled.changed);
	const leftAsIs = $derived(model.settled.leftAsIs);
	const refused = $derived(outcome?.outcome === 'applied' ? outcome.refused : []);

	let said = $state<HTMLParagraphElement | null>(null);

	async function undo() {
		await model.undo();
		if (!model.undone) return;
		await tick();
		said?.focus();
	}

	function names(items: string[]): string {
		return items.length <= 12 ? `${listNames(items)}.` : '';
	}
</script>

<section aria-label="Applied" class="max-w-2xl space-y-6">
	{#if outcome?.outcome === 'alreadyApplied'}
		<p class="text-sm text-text-secondary">This check was applied before; nothing changed.</p>
	{:else if model.undone}
		<p bind:this={said} tabindex="-1" class="text-sm text-text-secondary focus:outline-none">
			Taken back: your stock holds what it held before the check.
		</p>
	{:else}
		<div class="space-y-2">
			<p class="text-sm leading-relaxed text-text-secondary">
				{#if changed.length > 0}
					{formatQuantity(changed.length)}
					{changed.length === 1 ? 'item is' : 'items are'} settled against your account.
				{:else}
					Nothing was recorded.
				{/if}
				{#if changed.length > 0 && refused.length === 0 && leftAsIs.length === 0}
					Your stock now matches what was compared.
				{/if}
			</p>
			{#if changed.length > 0}
				<p class="text-xs text-text-tertiary">{names(changed)}</p>
			{/if}
			{#if leftAsIs.length > 0}
				<p class="text-sm text-text-secondary">
					{formatQuantity(leftAsIs.length)}
					{leftAsIs.length === 1 ? 'item is left as it is' : 'items are left as they are'}: the app
					still holds a different quantity from your account.
				</p>
				<p class="text-xs text-text-tertiary">{names(leftAsIs)}</p>
			{/if}
		</div>
	{/if}

	{#if refused.length > 0 && !model.undone}
		<div class="space-y-2">
			<span class="eyebrow">Left for the next check</span>
			<ul class="divide-y divide-border/50 border-y border-border/50">
				{#each refused as row (row.itemName)}
					<li class="grid grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)] gap-x-4 py-2">
						<span class="truncate text-sm text-text" title={row.itemName}>{row.itemName}</span>
						<span class="text-xs text-text-tertiary">{REFUSAL_LABEL[row.reason]}</span>
					</li>
				{/each}
			</ul>
		</div>
	{/if}

	<div class="flex flex-wrap items-center gap-3 border-t border-border/50 pt-5">
		{#if model.canUndo && !model.undone}
			<Button variant="secondary" loading={model.undoing} onclick={() => void undo()}>
				Undo this check
			</Button>
		{/if}
		<Button onclick={ondone}>Back to Inventory</Button>
		<Button variant="ghost" onclick={model.startOver}>Check again</Button>
		{#if model.undoError}
			<p class="basis-full text-xs text-error" role="alert">{model.undoError}</p>
		{/if}
	</div>
</section>
