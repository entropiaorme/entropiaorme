<script lang="ts">
	import { onMount } from 'svelte';
	import { Button, ErrorNotice } from '$lib/components';
	import { formatDay } from './armourRecording';
	import { createSetPiecesModel } from './setPiecesModel.svelte';

	let { setId, onConfirmed }: { setId: string; onConfirmed?: () => void } = $props();

	// The editor mounts this afresh for each set it opens.
	const pieces = createSetPiecesModel(
		() => setId,
		() => onConfirmed?.(),
	);

	onMount(() => {
		void pieces.load();
	});

	const suggested = $derived(pieces.choices.filter((choice) => choice.suggested));
	const others = $derived(pieces.choices.filter((choice) => !choice.suggested));
	const readingDay = $derived(
		pieces.pieces?.readingTakenAt != null ? formatDay(pieces.pieces.readingTakenAt) : null,
	);
</script>

<section aria-labelledby={`set-pieces-${setId}`} class="border-t border-border/70 pt-4" data-testid="set-pieces">
	<div class="flex items-baseline justify-between gap-4">
		<h3 id={`set-pieces-${setId}`} class="eyebrow">Pieces</h3>
		{#if pieces.view === 'confirmed'}
			<Button variant="ghost" size="sm" onclick={pieces.edit}>Change</Button>
		{/if}
	</div>

	{#if pieces.error}
		<div class="mt-2"><ErrorNotice message={pieces.error} onDismiss={() => (pieces.error = null)} /></div>
	{/if}

	{#if pieces.loading}
		<p class="mt-2 text-xs text-text-tertiary animate-pulse">Reading your account...</p>
	{:else if pieces.view === 'empty'}
		<p class="mt-2 text-xs text-text-tertiary">Sign in to your account and track a session wearing the set</p>
	{:else if pieces.view === 'confirmed' && pieces.pieces}
		<p class="mt-1 text-xs text-text-tertiary">
			Sessions read from your account bill this set by what these pieces lose, at its markup.
		</p>
		<ul class="mt-2 divide-y divide-border/60 border-y border-border/70">
			{#each pieces.pieces.confirmed as piece (piece.name)}
				<li class="py-1.5 text-sm text-text">{piece.name}</li>
			{/each}
		</ul>
	{:else if pieces.view === 'choosing'}
		<p class="mt-1 text-xs text-text-tertiary">
			{readingDay ? `From your account reading of ${readingDay}. ` : ''}Tick the pieces that make up this set: sessions read from your account bill it by what they lose.
		</p>
		<ul class="mt-2 divide-y divide-border/60 border-y border-border/70" aria-label="Suggested pieces">
			{#each suggested as choice (choice.name)}
				<li>
					<label class="flex items-center gap-2.5 py-1.5 text-sm text-text">
						<input type="checkbox" checked={pieces.selection.has(choice.name)} onchange={() => pieces.toggle(choice.name)} disabled={pieces.saving} />
						<span class="truncate">{choice.name}</span>
					</label>
				</li>
			{/each}
		</ul>
		{#if others.length > 0}
			<h4 class="mt-3 text-[11px] text-text-tertiary">Other limited armour and plates in that reading</h4>
			<ul class="mt-1 divide-y divide-border/60 border-y border-border/70" aria-label="Other limited armour and plates">
				{#each others as choice (choice.name)}
					<li>
						<label class="flex items-center gap-2.5 py-1.5 text-sm text-text-secondary">
							<input type="checkbox" checked={pieces.selection.has(choice.name)} onchange={() => pieces.toggle(choice.name)} disabled={pieces.saving} />
							<span class="truncate">{choice.name}</span>
						</label>
					</li>
				{/each}
			</ul>
		{/if}
		<div class="mt-3 flex justify-end gap-2">
			{#if pieces.editing}
				<Button variant="secondary" size="sm" onclick={pieces.cancel} disabled={pieces.saving}>Cancel</Button>
			{/if}
			<Button size="sm" onclick={pieces.confirm} disabled={!pieces.canConfirm} loading={pieces.saving}>Confirm pieces</Button>
		</div>
	{/if}
</section>
