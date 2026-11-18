<script lang="ts">
	import Card from '$lib/components/Card.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import Input from '$lib/components/Input.svelte';
	import { createCodexMetaModel } from './codexMetaModel.svelte';

	// The meta codex: claim a meta reward into an attribute, and read (or
	// calibrate) the metas earned against the cap.
	const model = createCodexMetaModel();
	$effect(() => {
		void model.load();
	});

	const stepButton =
		'flex h-7 w-7 cursor-pointer items-center justify-center rounded text-sm text-text-secondary transition-colors ' +
		'hover:bg-surface-hover hover:text-text disabled:cursor-default disabled:opacity-30';
</script>

<Card class="space-y-4 p-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<h3 class="text-sm font-medium text-text">Meta Codex Reward</h3>
		{#if model.progress}
			{@const progress = model.progress}
			<div class="flex items-center gap-3">
				{#if model.calibrating}
					<div class="flex items-center gap-1 text-sm text-text-secondary">
						<button
							type="button"
							class={stepButton}
							aria-label="One fewer meta earned"
							disabled={model.saving || progress.earned <= 0}
							onclick={() => model.step(-1)}
						>&minus;</button>
						<label for="codex-meta-earned" class="sr-only">Metas earned</label>
						<Input
							id="codex-meta-earned"
							class="w-20"
							type="number"
							inputmode="numeric"
							min="0"
							max={progress.cap}
							step="1"
							align="right"
							disabled={model.saving}
							bind:value={model.draft}
							onchange={() => void model.commitDraft()}
							onkeydown={(e) => {
								if (e.key === 'Enter') e.currentTarget.blur();
							}}
						/>
						<button
							type="button"
							class={stepButton}
							aria-label="One more meta earned"
							disabled={model.saving || progress.earned >= progress.cap}
							onclick={() => model.step(1)}
						>+</button>
						<span class="ml-1">of <span class="tabular-nums">{progress.cap}</span> metas earned</span>
					</div>
				{:else}
					<p class="text-sm text-text-secondary">
						<span class="font-medium tabular-nums text-text">{progress.earned}</span>
						of <span class="tabular-nums">{progress.cap}</span> metas earned
					</p>
				{/if}
				<button
					type="button"
					class="cursor-pointer whitespace-nowrap rounded-md px-3 py-1.5 text-xs font-medium transition-colors
						{model.calibrating ? 'bg-warning/20 text-warning' : 'bg-surface-hover text-text-secondary hover:text-text'}"
					aria-pressed={model.calibrating}
					onclick={model.toggleCalibrating}
				>
					{model.calibrating ? 'Done' : 'Calibrate'}
				</button>
			</div>
		{/if}
	</div>

	{#if model.calibrating && model.progress}
		{@const progress = model.progress}
		<p class="text-xs text-text-tertiary">
			Set this to the count the game shows. Your species ranks account for
			<span class="tabular-nums">{progress.tracked}</span>{#if model.adjustment > 0}, plus
				<span class="tabular-nums">{model.adjustment}</span> calibrated{:else if model.adjustment < 0}, less
				<span class="tabular-nums">{-model.adjustment}</span> calibrated{/if}; every meta you reach from
			here still counts.
			{#if model.adjustment !== 0}
				<button
					type="button"
					class="linklet ml-1"
					disabled={model.saving}
					onclick={() => void model.calibrate(progress.tracked)}
				>
					Match your ranks
				</button>
			{/if}
		</p>
	{/if}

	{#if model.loadError}
		<ErrorNotice message={model.loadError} />
	{:else if model.loading}
		<p class="py-4 text-center text-sm text-text-tertiary">Loading...</p>
	{:else}
		<div class="space-y-1">
			{#each model.attributes as attr (attr.name)}
				<div
					class="group flex items-center justify-between rounded px-3 py-2 transition-colors hover:bg-surface-hover/50"
				>
					<div class="flex items-center gap-3">
						<span class="w-24 text-sm font-medium text-text">{attr.name}</span>
						{#if attr.currentLevel != null}
							<span class="text-xs tabular-nums text-text-secondary">Lv {attr.currentLevel.toFixed(1)}</span>
						{:else}
							<span class="text-xs text-text-tertiary">Not scanned</span>
						{/if}
					</div>
					<button
						type="button"
						class="cursor-pointer rounded bg-accent/10 px-3 py-1 text-xs font-medium text-accent opacity-0 transition-colors hover:bg-accent/20 focus-visible:opacity-100 group-hover:opacity-100"
						onclick={() => model.claim(attr.name)}
					>Claim 1 PES</button>
				</div>
			{/each}
		</div>
	{/if}

	{#if model.notice}
		<div class="text-center text-sm {model.notice.tone === 'error' ? 'text-negative' : 'text-positive'}">
			{model.notice.text}
		</div>
	{/if}
</Card>
