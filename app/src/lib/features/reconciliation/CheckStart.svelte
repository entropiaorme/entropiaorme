<script lang="ts">
	/**
	 * Where the comparison comes from: the latest reading of the account, or
	 * an inventory pasted from the account site's Copy CSV, which needs no
	 * connection and gives the same check.
	 */
	import Button from '$lib/components/Button.svelte';
	import { formatSeenAt } from '$lib/utils/format';
	import type { ReconciliationModel } from './reconciliationModel.svelte';

	let { model }: { model: ReconciliationModel } = $props();

	let text = $state('');

	function onpaste(event: ClipboardEvent) {
		const pasted = event.clipboardData?.getData('text') ?? '';
		if (!pasted.trim() || model.pasteRefusal) return;
		event.preventDefault();
		text = pasted;
		void model.planFromPaste(pasted);
	}
</script>

<div class="max-w-3xl space-y-10">
	<section aria-labelledby="check-from-reading" class="space-y-3">
		<h3 id="check-from-reading" class="eyebrow">From your account</h3>
		<p class="text-sm text-text-secondary">
			{#if model.lastReadingAt}
				Compare your stock with your account as it was last read, {formatSeenAt(model.lastReadingAt)}.
			{:else}
				Compare your stock with your account as it was last read.
			{/if}
		</p>
		<div class="flex flex-wrap items-center gap-4">
			<Button
				disabled={model.readingRefusal !== null || model.planning === 'paste'}
				loading={model.planning === 'reading'}
				aria-describedby={model.readingRefusal || model.readingError ? 'check-reading-note' : undefined}
				onclick={() => void model.planFromReading()}
			>
				Compare with the latest reading
			</Button>
			{#if model.readingRefusal}
				<p id="check-reading-note" class="text-xs text-text-tertiary">{model.readingRefusal}</p>
			{:else if model.readingError}
				<p id="check-reading-note" class="text-xs text-error" role="alert">{model.readingError}</p>
			{/if}
		</div>
	</section>

	<section aria-labelledby="check-from-paste" class="space-y-4 border-t border-border/50 pt-8">
		<h3 id="check-from-paste" class="eyebrow">Or paste an inventory</h3>
		<ol class="grid gap-5 sm:grid-cols-2">
			<li class="space-y-1.5">
				<span class="block text-3xl font-light tabular-nums text-text-tertiary">1</span>
				<p class="text-sm text-text">Open your inventory on the account site.</p>
				<p class="text-xs text-text-tertiary">Everything you carry and store, in one list.</p>
			</li>
			<li class="space-y-1.5">
				<span class="block text-3xl font-light tabular-nums text-text-tertiary">2</span>
				<p class="text-sm text-text">Press Copy CSV, then paste it below.</p>
			</li>
		</ol>

		<div class="space-y-2">
			<label for="check-paste" class="sr-only">Inventory pasted from the account site</label>
			<div class="relative">
				<textarea
					id="check-paste"
					bind:value={text}
					{onpaste}
					rows="6"
					spellcheck="false"
					disabled={model.pasteRefusal !== null}
					placeholder="Paste the inventory here (Ctrl+V)"
					class="block w-full resize-none rounded-lg border border-dashed bg-surface/40 px-4 py-3.5 font-mono text-xs leading-relaxed text-text-secondary placeholder:font-sans placeholder:text-sm placeholder:text-text-tertiary focus:outline-none disabled:cursor-not-allowed disabled:opacity-50 {model.pasteError
						? 'border-error/50'
						: 'border-border-bright/70 focus:border-accent/60 focus:bg-surface/70'}"
					aria-invalid={model.pasteError ? true : undefined}
					aria-describedby={model.pasteError || model.pasteRefusal ? 'check-paste-note' : undefined}
				></textarea>
				{#if model.planning === 'paste'}
					<div
						class="absolute inset-0 flex items-center justify-center rounded-lg bg-base/60 text-sm text-text-secondary"
						role="status"
					>
						Comparing...
					</div>
				{/if}
			</div>
			<div class="flex items-start justify-between gap-4">
				{#if model.pasteError}
					<p id="check-paste-note" class="text-xs text-error" role="alert">{model.pasteError}</p>
				{:else if model.pasteRefusal}
					<p id="check-paste-note" class="text-xs text-text-tertiary">{model.pasteRefusal}</p>
				{:else}
					<span></span>
				{/if}
				{#if text.trim() && model.planning === null && !model.pasteRefusal}
					<Button size="sm" variant="secondary" onclick={() => void model.planFromPaste(text)}>
						Compare
					</Button>
				{/if}
			</div>
		</div>
	</section>
</div>
