<script lang="ts">
	import { onMount } from 'svelte';
	import StrongboxOpening from '$lib/features/strongbox/StrongboxOpening.svelte';
	import { createOpeningModel } from '$lib/features/strongbox/openingModel.svelte';

	const model = createOpeningModel();

	onMount(() => {
		void model.load();
		// The tracking gate follows the live session, so it lifts the moment
		// tracking stops; what the account proposes follows the account.
		let stop: (() => void) | undefined;
		let disposed = false;
		void model.subscribe().then((unlisten) => {
			if (disposed) unlisten();
			else stop = unlisten;
		});
		return () => {
			disposed = true;
			stop?.();
			// A run read from the account stops with the view.
			model.dispose();
		};
	});
</script>

<div class="flex h-full min-h-0 flex-col gap-6 overflow-y-auto px-6 pb-10">
	<header class="shrink-0">
		<a
			href="/inventory"
			class="mb-2 inline-flex items-center gap-1 text-xs text-text-tertiary hover:text-text"
		>
			<span aria-hidden="true">&larr;</span> Inventory
		</a>
		<div class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold tracking-tight text-text">Open strongboxes</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="mt-0.5 text-sm text-text-secondary">
				Record a run of boxes and decide what to keep; what sells later credits the play that looted
				them.
			</p>
		</div>
	</header>

	<StrongboxOpening {model} />
</div>
