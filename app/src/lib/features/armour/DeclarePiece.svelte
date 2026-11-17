<script lang="ts">
	/**
	 * Declare an item of the latest reading a limited piece, for the list it
	 * was opened from: the likeliest first (an item that wore and nothing
	 * places, then attached or tagged ones), and a filter for the rest. The
	 * piece is then billed at the markup set for it, or at TT.
	 */
	import { Modal } from '$lib/components';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import { candidatesFor, HINT_LABEL } from './armour';
	import type { ArmourModel } from './armourModel.svelte';

	let { model }: { model: ArmourModel } = $props();

	let query = $state('');
	const open = $derived(model.declaring !== null);
	const list = $derived(model.declaring ?? 'armour');
	const candidates = $derived(candidatesFor(model.view, list, query));
	const failure = $derived(
		model.failure && candidates.some((candidate) => candidate.item === model.failure?.item)
			? model.failure
			: null,
	);

	function close(next: boolean) {
		if (!next) {
			model.declaring = null;
			query = '';
		}
	}
</script>

<Modal
	bind:open={() => open, close}
	title={list === 'plates' ? 'Declare a plate' : 'Declare an armour piece'}
	class="max-w-lg"
>
	<div class="space-y-4">
		<p class="text-sm text-text-secondary">
			{list === 'plates'
				? 'Pick a plate the app did not recognise by its name. Its wear is then billed as armour, at the markup you set for it.'
				: 'Pick a limited piece the app did not recognise by its name. Its wear is then billed as armour, at the markup you set for it.'}
		</p>
		<SearchInput bind:value={query} placeholder="Filter your items" aria-label="Filter your items" />
		{#if candidates.length === 0}
			<p class="py-6 text-center text-xs text-text-tertiary">
				{query.trim() ? 'No item matches that filter.' : 'Your latest reading holds nothing else to declare.'}
			</p>
		{:else}
			<ul class="divide-y divide-border/40 border-y border-border/40" aria-label="Items you hold">
				{#each candidates as candidate (candidate.item)}
					<li class="flex items-center gap-3 py-2">
						<div class="min-w-0 flex-1">
							<span class="block truncate text-sm text-text" title={candidate.item}>{candidate.item}</span>
							{#if HINT_LABEL[candidate.hint]}
								<span class="block text-xs text-text-tertiary">{HINT_LABEL[candidate.hint]}</span>
							{/if}
						</div>
						<button
							type="button"
							class="shrink-0 text-xs font-medium text-accent hover:text-text disabled:opacity-50"
							disabled={model.saving !== null}
							onclick={() => void model.declare(candidate.item, list)}
						>
							{model.saving === candidate.item ? 'Adding...' : 'Add'}
						</button>
					</li>
				{/each}
			</ul>
		{/if}
		{#if failure}
			<p class="text-xs text-error" role="alert">{failure.message}</p>
		{/if}
	</div>
</Modal>
