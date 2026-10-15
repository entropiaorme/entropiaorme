<script lang="ts">
	import MarketItemList from '$lib/features/market/MarketItemList.svelte';
	import MarketPaste from '$lib/features/market/MarketPaste.svelte';
	import { createOverviewModel } from '$lib/features/market/overviewModel.svelte';

	const overview = createOverviewModel();

	$effect(() => {
		void overview.loadData();
	});
</script>

<div class="flex h-full min-h-0 flex-col gap-8">
	<div class="shrink-0 space-y-3">
		<p class="text-sm text-text-tertiary">
			Markup observations from the in-game market ledger. Estimates only: they never enter your
			P&amp;L.
		</p>
		<MarketPaste onimported={() => void overview.loadData()} />
	</div>

	<MarketItemList model={overview} />
</div>
