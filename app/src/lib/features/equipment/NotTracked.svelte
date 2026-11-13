<script lang="ts">
	// The names the player said are not a tool they track, at the foot of
	// the library while the held tool is read off the screen: one quiet
	// line naming them, opened to bring each back in one action. Nothing is
	// shown when nothing is set aside.
	import { listNames } from '$lib/utils/format';
	import { createNotTrackedModel, type NotTrackedModel } from './notTrackedModel.svelte';

	let {
		readingOn,
		model = createNotTrackedModel(),
	}: {
		/** The held tool is read off the screen (and in-development surfaces show). */
		readingOn: boolean;
		model?: NotTrackedModel;
	} = $props();

	let open = $state(false);

	$effect(() => {
		if (readingOn) void model.refresh();
	});
</script>

{#if readingOn && model.names.length > 0}
	<section class="mt-4 text-xs" data-testid="not-tracked">
		<button
			type="button"
			class="linklet text-left"
			aria-expanded={open}
			aria-controls="not-tracked-names"
			onclick={() => (open = !open)}
		>
			Not tracked: {listNames(model.names.map((entry) => entry.name))}
		</button>
		{#if open}
			<ul id="not-tracked-names" class="mt-2 space-y-1">
				{#each model.names as entry (entry.name)}
					<li class="flex items-baseline gap-3">
						<span class="text-text-secondary">{entry.name}</span>
						<button
							type="button"
							class="linklet disabled:opacity-40"
							aria-label="Track {entry.name} again"
							disabled={model.busy}
							onclick={() => void model.trackAgain(entry.name)}
						>
							Track again
						</button>
					</li>
				{/each}
			</ul>
		{/if}
		{#if model.error}
			<p class="mt-2 text-negative" role="alert">{model.error}</p>
		{/if}
	</section>
{/if}
