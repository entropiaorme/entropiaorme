<script lang="ts">
	import Tabs from '$lib/components/Tabs.svelte';
	import OverviewTab from './OverviewTab.svelte';
	import LedgerTab from './LedgerTab.svelte';
	import HuntingTab from './HuntingTab.svelte';
	import TreeCuttingTab from './TreeCuttingTab.svelte';

	const tabs = [
		{ id: 'overview', label: 'Overview' },
		{ id: 'ledger', label: 'Ledger' },
		{ id: 'hunting', label: 'Hunting' },
		{ id: 'treecutting', label: 'Tree Cutting' }
	];

	let activeTab = $state('overview');
</script>

<div class="px-6 pb-6 space-y-6">
	<!-- Page header -->
	<div class="flex items-center justify-between">
		<header class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold text-text tracking-tight">Analytics</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="text-sm text-text-secondary mt-0.5">Lifetime totals, ledger activity, and per-profession performance</p>
		</header>
	</div>

	<!-- Tab bar -->
	<Tabs {tabs} active={activeTab} onchange={(id) => (activeTab = id)} />

	<!-- Tab content (lazy-loaded). -->
	<div>
		{#if activeTab === 'overview'}
			<OverviewTab />
		{:else if activeTab === 'ledger'}
			<LedgerTab />
		{:else if activeTab === 'hunting'}
			<HuntingTab />
		{:else if activeTab === 'treecutting'}
			<TreeCuttingTab />
		{/if}
	</div>
</div>
