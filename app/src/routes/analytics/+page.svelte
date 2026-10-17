<script lang="ts">
	import Tabs from '$lib/components/Tabs.svelte';
	import OverviewTab from './OverviewTab.svelte';
	import LedgerTab from './LedgerTab.svelte';
	import MarketTab from './MarketTab.svelte';
	import HuntingTab from './HuntingTab.svelte';
	import TreeCuttingTab from './TreeCuttingTab.svelte';
	import QuestsTab from './QuestsTab.svelte';

	const tabs = [
		{ id: 'overview', label: 'Overview' },
		{ id: 'ledger', label: 'Ledger' },
		{ id: 'market', label: 'Market' },
		{ id: 'hunting', label: 'Hunting' },
		{ id: 'treecutting', label: 'Tree Cutting' },
		{ id: 'quests', label: 'Quests' }
	];

	let activeTab = $state('overview');

	// The Market tab scrolls its item list inside the tab, so the page
	// becomes a viewport-height column with the header and tabs fixed.
	const fillsViewport = $derived(activeTab === 'market');
</script>

<div class="px-6 pb-6 {fillsViewport ? 'flex h-full min-h-0 flex-col gap-6' : 'space-y-6'}">
	<!-- Page header -->
	<div class="flex shrink-0 items-center justify-between">
		<header class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold text-text tracking-tight">Analytics</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="text-sm text-text-secondary mt-0.5">Lifetime totals, ledger activity, market observations, per-profession performance, and quest rewards</p>
		</header>
	</div>

	<!-- Tab bar -->
	<Tabs {tabs} active={activeTab} onchange={(id) => (activeTab = id)} class="shrink-0" />

	<!-- Tab content (lazy-loaded). -->
	<div class={fillsViewport ? 'min-h-0 flex-1' : ''}>
		{#if activeTab === 'overview'}
			<OverviewTab />
		{:else if activeTab === 'ledger'}
			<LedgerTab />
		{:else if activeTab === 'market'}
			<MarketTab />
		{:else if activeTab === 'hunting'}
			<HuntingTab />
		{:else if activeTab === 'treecutting'}
			<TreeCuttingTab />
		{:else if activeTab === 'quests'}
			<QuestsTab />
		{/if}
	</div>
</div>
