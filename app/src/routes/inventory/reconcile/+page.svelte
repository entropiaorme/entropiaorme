<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import StockCheck from '$lib/features/reconciliation/StockCheck.svelte';
	import { createReconciliationModel } from '$lib/features/reconciliation/reconciliationModel.svelte';

	const model = createReconciliationModel();

	onMount(() => {
		void model.load();
		// The tracking gate lifts the moment tracking stops; a reading stored
		// meanwhile becomes the latest one to compare with.
		let stop: (() => void) | undefined;
		let disposed = false;
		void model.subscribe().then((unlisten) => {
			if (disposed) unlisten();
			else stop = unlisten;
		});
		return () => {
			disposed = true;
			stop?.();
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
			<h1 class="text-xl font-semibold tracking-tight text-text">Check against your account</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="mt-0.5 text-sm text-text-secondary">
				Settle what the app holds against what your account shows. A missing item leaves your
				stock, never a session's returns; an extra one is stock no activity looted.
			</p>
		</div>
	</header>

	<StockCheck {model} ondone={() => goto('/inventory')} />
</div>
