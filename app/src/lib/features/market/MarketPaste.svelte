<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import { createImportModel } from './importModel.svelte';

	let { onimported }: { onimported: () => void } = $props();

	const model = createImportModel();

	let quoteOpen = $state(false);
	const quoteId = 'market-unit-quote';

	const readyCount = $derived(model.preview?.rows.length ?? 0);
	const skipped = $derived(model.preview?.skipped ?? []);

	async function handleCommit() {
		if (await model.commit()) onimported();
	}

	const inputClass =
		'w-full rounded-md border border-border bg-surface/70 px-3 py-2 text-sm text-text placeholder:text-text-tertiary focus:border-accent/60 focus:outline-none focus:[box-shadow:var(--shadow-glow)]';
</script>

<section class="space-y-3" aria-label="Import market observations">
	<textarea
		value={model.text}
		oninput={(e) => model.setText(e.currentTarget.value)}
		rows="4"
		spellcheck="false"
		placeholder="Paste the in-game market-ledger export"
		aria-label="Market-ledger paste"
		class="{inputClass} font-mono resize-y"
	></textarea>

	<div class="flex items-center justify-between gap-3">
		<p class="text-sm text-text-secondary" aria-live="polite">
			{#if model.previewing}
				Reading the paste
			{:else if readyCount > 0}
				{readyCount}
				{readyCount === 1 ? 'item' : 'items'} ready{skipped.length > 0
					? `, ${skipped.length} ${skipped.length === 1 ? 'line' : 'lines'} skipped`
					: ''}
			{:else if model.committed}
				<span class="text-success">
					Imported {model.committed.itemCount}
					{model.committed.itemCount === 1 ? 'item' : 'items'}
				</span>
			{/if}
		</p>
		<Button
			variant="primary"
			size="sm"
			disabled={!model.canCommit}
			loading={model.committing}
			onclick={() => void handleCommit()}
		>
			Import
		</Button>
	</div>

	{#if model.error}
		<ErrorNotice message={model.error} />
	{/if}

	{#if skipped.length > 0}
		<ul class="space-y-1.5 border-l border-border/60 pl-3">
			{#each skipped as line (line.lineNumber)}
				<li class="text-xs">
					<span class="text-text-tertiary">Line {line.lineNumber}:</span>
					<span class="text-text-secondary">{line.reason}</span>
					<code class="mt-0.5 block truncate text-text-tertiary">{line.content}</code>
				</li>
			{/each}
		</ul>
	{/if}

	<div>
		<button
			type="button"
			class="text-xs text-text-tertiary transition-colors duration-[var(--duration-fast)] hover:text-text focus:outline-none focus-visible:[box-shadow:var(--shadow-glow)]"
			aria-expanded={quoteOpen}
			aria-controls={quoteId}
			onclick={() => (quoteOpen = !quoteOpen)}
		>
			{quoteOpen ? 'Hide the unit quote' : 'Add a PED-per-unit quote'}
		</button>

		{#if quoteOpen}
			<div id={quoteId} class="mt-3 space-y-2">
				<p class="text-xs text-text-tertiary">For zero-TT or unit-priced items.</p>
				<div class="grid gap-3 sm:grid-cols-[minmax(0,1fr)_10rem_auto] sm:items-end">
					<label class="block">
						<span class="mb-1 block text-xs font-medium text-text-secondary">Item</span>
						<input
							value={model.unitItemName}
							disabled={model.unitPriceSaving}
							oninput={(event) => model.setUnitItemName(event.currentTarget.value)}
							placeholder="Item name"
							class={inputClass}
						/>
					</label>
					<label class="block">
						<span class="mb-1 block text-xs font-medium text-text-secondary">PED per unit</span>
						<input
							type="number"
							disabled={model.unitPriceSaving}
							min="0"
							step="0.01"
							value={model.unitPriceInput}
							oninput={(event) => model.setUnitPriceInput(event.currentTarget.value)}
							placeholder="0.00"
							class="{inputClass} tabular-nums"
						/>
					</label>
					<Button
						variant="secondary"
						size="sm"
						disabled={!model.canSaveUnitPrice}
						loading={model.unitPriceSaving}
						onclick={() => void model.saveUnitPrice()}
					>
						Save quote
					</Button>
				</div>
				{#if model.unitPriceError}
					<ErrorNotice message={model.unitPriceError} />
				{:else if model.unitPriceSaved}
					<p class="text-sm text-success">
						Saved {model.unitPriceSaved.itemName} at {model.unitPriceSaved.pedPerUnit.toFixed(2)}
						PED per unit.
					</p>
				{/if}
			</div>
		{/if}
	</div>
</section>
