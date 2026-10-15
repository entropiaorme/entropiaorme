<script lang="ts">
	import MarketItemList from '$lib/features/market/MarketItemList.svelte';
	import MarketPaste from '$lib/features/market/MarketPaste.svelte';
	import { createOverviewModel } from '$lib/features/market/overviewModel.svelte';

	const overview = createOverviewModel();

	$effect(() => {
		void overview.loadData();
	});
</script>

<div class="px-6 pb-6 space-y-8">
	<header class="flex flex-col gap-1.5">
		<h1 class="text-xl font-semibold text-text tracking-tight">Market</h1>
		<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
		<p class="text-sm text-text-secondary mt-0.5">
			Markup observations from the in-game market ledger. Estimates only: they never enter your
			P&amp;L.
		</p>
	</header>

	<MarketPaste onimported={() => void overview.loadData()} />

	<MarketItemList model={overview} />
</div>
