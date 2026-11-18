<script lang="ts">
	/**
	 * The Armour tab: the limited armour and plates your account shows you
	 * holding, each with the markup you paid for it. A session bills each
	 * piece's wear, read from your account, at the markup it had when the
	 * session started; one with none is billed at TT. Unlimited armour needs
	 * nothing here: its wear is billed at TT. A piece the app does not
	 * recognise by its name can be declared from either list.
	 */
	import { onMount } from 'svelte';
	import { ErrorNotice } from '$lib/components';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import { formatSeenAt } from '$lib/utils/format';
	import ArmourPieceRow from './ArmourPieceRow.svelte';
	import { heldIn, noLongerHeld, type PieceList } from './armour';
	import type { ArmourModel } from './armourModel.svelte';
	import DeclarePiece from './DeclarePiece.svelte';

	let { model }: { model: ArmourModel } = $props();

	onMount(() => model.connect());

	let showGone = $state(false);
	const gone = $derived(noLongerHeld(model.view));
	const lists: { id: PieceList; title: string; empty: string; declare: string }[] = [
		{
			id: 'armour',
			title: 'Armour',
			empty: 'Your latest reading holds no limited armour.',
			declare: 'Declare an armour piece',
		},
		{
			id: 'plates',
			title: 'Plates',
			empty: 'Your latest reading holds no limited plates.',
			declare: 'Declare a plate',
		},
	];

	function errorFor(item: string): string | null {
		return model.failure?.item === item ? model.failure.message : null;
	}
</script>

<div class="space-y-8">
	<div>
		<div class="flex items-center gap-2.5">
			<h2 class="text-lg font-semibold text-text">Armour</h2>
			<InDevelopmentMark id="limited-protection" />
		</div>
		<p class="mt-1 max-w-2xl text-sm text-text-secondary">
			Your limited armour and plates, as your account shows them. Type the markup you paid for each:
			a session bills its wear at that markup, and a piece without one at TT. A new markup applies
			from then on. Unlimited armour needs nothing here.
		</p>
		{#if model.view?.readAt}
			<p class="mt-1 text-xs text-text-tertiary">From your account, {formatSeenAt(model.view.readAt)}.</p>
		{:else if model.view}
			<p class="mt-1 text-xs text-warning">
				Nothing has been read from your account yet: sign in to see your armour.
			</p>
		{/if}
	</div>

	{#if model.error}
		<ErrorNotice message={model.error} />
	{:else if model.loading}
		<div class="py-14 text-center text-sm text-text-tertiary animate-pulse">Loading armour...</div>
	{:else}
		{#each lists as list (list.id)}
			{@const pieces = heldIn(model.view, list.id)}
			<section aria-labelledby={`armour-${list.id}-heading`}>
				<div class="mb-1 grid grid-cols-[minmax(0,1fr)_6rem_9rem] items-baseline gap-x-4 border-b border-border/60 pb-2">
					<h3 id={`armour-${list.id}-heading`} class="text-sm font-semibold text-text">{list.title}</h3>
					<span class="eyebrow text-right">TT now</span>
					<span class="eyebrow text-right">Markup paid</span>
				</div>
				{#if pieces.length === 0}
					<p class="py-4 text-xs text-text-tertiary">{list.empty}</p>
				{:else}
					<ul class="divide-y divide-border/40" aria-label={`Limited ${list.title.toLowerCase()}`}>
						{#each pieces as piece (piece.item)}
							<ArmourPieceRow
								{piece}
								saving={model.saving === piece.item}
								busy={model.saving !== null}
								error={errorFor(piece.item)}
								onsave={(markup) => model.setMarkup(piece.item, markup)}
								onundeclare={() => void model.undeclare(piece.item)}
							/>
						{/each}
					</ul>
				{/if}
				<button
					type="button"
					class="mt-2 text-xs text-text-tertiary hover:text-text-secondary disabled:opacity-50"
					disabled={model.saving !== null}
					onclick={() => (model.declaring = list.id)}
				>
					{list.declare}
				</button>
			</section>
		{/each}

		{#if gone.length > 0}
			<section aria-label="No longer held">
				<button
					type="button"
					class="flex items-center gap-2 text-xs text-text-secondary hover:text-text"
					aria-expanded={showGone}
					aria-controls="armour-gone"
					onclick={() => (showGone = !showGone)}
				>
					<span class="inline-block transition-transform {showGone ? 'rotate-90' : ''}" aria-hidden="true">&rsaquo;</span>
					No longer held ({gone.length})
				</button>
				{#if showGone}
					<p class="mt-2 text-xs text-text-tertiary">
						Sessions that wore them keep the markup they were billed at.
					</p>
					<ul id="armour-gone" class="mt-2 divide-y divide-border/40 border-y border-border/40">
						{#each gone as piece (piece.item)}
							<ArmourPieceRow
								{piece}
								saving={model.saving === piece.item}
								busy={model.saving !== null}
								error={errorFor(piece.item)}
								onsave={(markup) => model.setMarkup(piece.item, markup)}
								onundeclare={() => void model.undeclare(piece.item)}
							/>
						{/each}
					</ul>
				{/if}
			</section>
		{/if}
	{/if}
</div>

<DeclarePiece {model} />
