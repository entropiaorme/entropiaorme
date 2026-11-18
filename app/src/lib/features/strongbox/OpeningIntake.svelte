<script lang="ts">
	/**
	 * The opening itself happens in game; this is where its result comes in.
	 * With the account connected, the account is read before and after the
	 * boxes are opened, and the opening the readings show is the result.
	 * Otherwise the account site's Inventory Changes page is the one record
	 * the game gives of what a run of boxes produced, so the default is
	 * pasting its export. Entering the result by hand is the fallback, and
	 * asks only for the items: the ammo is whatever the keys did not pay
	 * back in them.
	 */
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import { formatPed, formatQuantity } from '$lib/utils/format';
	import type { IntakeMode, OpeningModel } from './openingModel.svelte';
	import { manualAmmo, manualItemsTt } from './openingDraft';

	let { model }: { model: OpeningModel } = $props();

	const CHANGES_URL = 'account.entropiauniverse.com/account/inventory/changes';
	const modes = $derived([
		...(model.accountConnected || model.intake === 'account'
			? [{ id: 'account', label: 'Read from your account' }]
			: []),
		{ id: 'export', label: 'Paste the export' },
		{ id: 'manual', label: 'Enter by hand' },
	]);

	let text = $state('');
	let copied = $state(false);
	let zone = $state<HTMLTextAreaElement | null>(null);

	$effect(() => {
		if (model.intake === 'export') zone?.focus();
	});

	function onpaste(event: ClipboardEvent) {
		const pasted = event.clipboardData?.getData('text') ?? '';
		if (!pasted.trim()) return;
		event.preventDefault();
		text = pasted;
		void model.readExport(pasted);
	}

	async function copyLink() {
		try {
			await navigator.clipboard.writeText(`https://${CHANGES_URL}`);
			copied = true;
			setTimeout(() => (copied = false), 1600);
		} catch {
			copied = false;
		}
	}

	const keys = $derived((model.manual.boxes ?? 0) * model.manual.keyUnitTt);
</script>

<div class="space-y-8">
	<SegmentedControl
		options={modes}
		active={model.intake}
		size="md"
		label="How the result comes in"
		onchange={(id) => (model.intake = id as IntakeMode)}
	/>

	{#if model.intake === 'account'}
		{@const run = model.run}
		<div class="max-w-3xl space-y-5">
			{#if run.phase === 'starting'}
				<p class="text-sm text-text-secondary" role="status">Reading your account before you start...</p>
			{:else if run.phase === 'open'}
				<ol class="grid gap-5 sm:grid-cols-2">
					<li class="space-y-1.5">
						<span class="block text-3xl font-light tabular-nums text-text-tertiary">1</span>
						<p class="text-sm text-text">Open your {model.boxItem} in game.</p>
					</li>
					<li class="space-y-1.5">
						<span class="block text-3xl font-light tabular-nums text-text-tertiary">2</span>
						<p class="text-sm text-text">Then press Done.</p>
						<p class="text-xs text-text-tertiary">Your account is read once more, which can take up to a minute.</p>
					</li>
				</ol>
				<div class="flex justify-end">
					<Button onclick={() => void model.finishRun()}>Done</Button>
				</div>
			{:else if run.phase === 'reading'}
				<p class="text-sm text-text-secondary" role="status">
					Reading your account, which can take up to a minute...
				</p>
			{:else if run.phase === 'nothing'}
				<div class="space-y-3" role="status">
					<p class="text-sm text-text-secondary">
						Your account does not show any {model.boxItem} opened yet. The site can take a minute or
						two to show them: check the next reading, or paste the export or enter the result by hand.
					</p>
					<div class="flex flex-wrap items-center gap-3">
						<Button size="sm" onclick={() => void model.checkNextReading()}>Check the next reading</Button>
						<Button size="sm" variant="ghost" onclick={() => void model.readAccountAgain()}>
							Start again
						</Button>
					</div>
				</div>
			{:else if run.phase === 'failed'}
				<div class="flex flex-wrap items-start justify-between gap-4" role="status">
					<p class="text-sm text-error">
						{run.message} Paste the export or enter the result by hand instead.
					</p>
					<Button size="sm" variant="ghost" onclick={() => void model.readAccountAgain()}>
						Start again
					</Button>
				</div>
			{/if}
		</div>
	{:else if model.intake === 'export'}
		<ol class="grid max-w-3xl gap-5 sm:grid-cols-3">
			<li class="space-y-1.5">
				<span class="block text-3xl font-light tabular-nums text-text-tertiary">1</span>
				<p class="text-sm text-text">Reset Inventory Changes on the account site.</p>
				<button
					type="button"
					class="text-xs text-accent hover:text-text"
					onclick={copyLink}
					title={`https://${CHANGES_URL}`}
				>
					{copied ? 'Link copied' : 'Copy the page link'}
				</button>
			</li>
			<li class="space-y-1.5">
				<span class="block text-3xl font-light tabular-nums text-text-tertiary">2</span>
				<p class="text-sm text-text">Open your {model.boxItem} in game.</p>
				<p class="text-xs text-text-tertiary">The page catches up within a minute.</p>
			</li>
			<li class="space-y-1.5">
				<span class="block text-3xl font-light tabular-nums text-text-tertiary">3</span>
				<p class="text-sm text-text">Copy CSV, then paste it below.</p>
			</li>
		</ol>

		<div class="max-w-3xl space-y-2">
			<label for="strongbox-export" class="sr-only">Inventory Changes export</label>
			<div class="relative">
				<textarea
					id="strongbox-export"
					bind:this={zone}
					bind:value={text}
					{onpaste}
					rows="7"
					spellcheck="false"
					placeholder="Paste the export here (Ctrl+V)"
					class="zone block w-full resize-none rounded-lg border border-dashed bg-surface/40 px-4 py-3.5 font-mono text-xs leading-relaxed text-text-secondary placeholder:font-sans placeholder:text-sm placeholder:text-text-tertiary focus:outline-none {model.readError
						? 'border-error/50'
						: 'border-border-bright/70 focus:border-accent/60 focus:bg-surface/70'}"
					aria-describedby={model.readError ? 'strongbox-export-error' : undefined}
				></textarea>
				{#if model.reading}
					<div class="absolute inset-0 flex items-center justify-center rounded-lg bg-base/60 text-sm text-text-secondary">
						Reading the export...
					</div>
				{/if}
			</div>
			<div class="flex items-start justify-between gap-4">
				{#if model.readError}
					<p id="strongbox-export-error" class="text-xs text-error">{model.readError}</p>
				{:else}
					<span></span>
				{/if}
				{#if text.trim() && !model.reading}
					<Button size="sm" variant="secondary" onclick={() => void model.readExport(text)}>
						Read it
					</Button>
				{/if}
			</div>
		</div>
	{:else}
		<div class="max-w-3xl space-y-6">
			<div class="flex flex-wrap items-end gap-6">
				<label class="block space-y-1.5">
					<span class="eyebrow">Boxes opened</span>
					<Input
						type="number"
						min="1"
						step="1"
						class="w-32"
						align="right"
						bind:value={model.manual.boxes}
					/>
				</label>
				<label class="block space-y-1.5">
					<span class="eyebrow">Key TT</span>
					<Input
						type="number"
						min="0"
						step="0.01"
						class="w-28"
						align="right"
						bind:value={model.manual.keyUnitTt}
					/>
				</label>
			</div>

			<div class="space-y-2">
				<div class="flex items-baseline justify-between">
					<span class="eyebrow">What came out besides ammo</span>
					<span class="text-xs text-text-tertiary">Already made Nanocubes? Enter them as Nanocube.</span>
				</div>
				<div class="space-y-1.5">
					{#each model.manual.lines as line (line.id)}
						<div class="flex items-center gap-2">
							<Input class="min-w-0 flex-1" placeholder="Item" aria-label="Item" bind:value={line.itemName} />
							<Input
								type="number"
								min="0"
								class="w-24 shrink-0"
								align="right"
								placeholder="Qty"
								aria-label="Quantity"
								bind:value={line.quantity}
							/>
							<Input
								type="number"
								min="0"
								step="0.01"
								class="w-28 shrink-0"
								align="right"
								placeholder="TT"
								aria-label="TT value"
								bind:value={line.ttValue}
							/>
							<button
								type="button"
								class="h-6 w-6 shrink-0 text-text-tertiary hover:text-text"
								aria-label="Remove line"
								onclick={() => model.removeManualLine(line.id)}
							>
								&times;
							</button>
						</div>
					{/each}
				</div>
				<button type="button" class="text-xs font-medium text-accent hover:text-text" onclick={model.addManualLine}>
					Add an item
				</button>
			</div>

			<div class="flex flex-wrap items-baseline gap-x-6 gap-y-1 border-t border-border/50 pt-4 text-sm">
				<span class="text-text-secondary">
					Universal Ammo
					<span class="ml-1 tabular-nums text-text">{formatPed(manualAmmo(model.manual))}</span>
				</span>
				<span class="text-xs text-text-tertiary">
					the rest of {formatQuantity(model.manual.boxes ?? 0)} keys' {formatPed(keys)} PED, after
					{formatPed(manualItemsTt(model.manual))} in items
				</span>
			</div>

			<div class="flex items-center justify-between gap-4">
				<p class="text-xs text-text-tertiary">
					{model.manualRefusal ?? ''}
				</p>
				<Button
					disabled={model.manualRefusal !== null}
					loading={model.reading}
					onclick={() => void model.reviewManual()}
				>
					Review
				</Button>
			</div>
		</div>
	{/if}
</div>

<style>
	.zone {
		transition:
			border-color var(--duration-base) var(--ease-out),
			background-color var(--duration-base) var(--ease-out);
	}
</style>
