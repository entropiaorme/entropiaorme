<script lang="ts">
	import { onMount } from 'svelte';
	import Tabs from '$lib/components/Tabs.svelte';
	import { createAccountStatus } from '$lib/features/account/accountStatus.svelte';
	import { inDevelopment } from '$lib/inDevelopment';
	import OverviewTab from './OverviewTab.svelte';
	import LedgerTab from './LedgerTab.svelte';
	import MarketTab from './MarketTab.svelte';
	import HuntingTab from './HuntingTab.svelte';
	import TreeCuttingTab from './TreeCuttingTab.svelte';
	import QuestsTab from './QuestsTab.svelte';
	import MeasuredTab from './MeasuredTab.svelte';

	const baseTabs = [
		{ id: 'overview', label: 'Overview' },
		{ id: 'ledger', label: 'Ledger' },
		{ id: 'market', label: 'Market' },
		{ id: 'hunting', label: 'Hunting' },
		{ id: 'treecutting', label: 'Tree Cutting' },
		{ id: 'quests', label: 'Quests' }
	];

	// The Measured tab appears once the player has connected the account.
	const account = inDevelopment.visible ? createAccountStatus() : null;
	onMount(() => account?.connect());
	const measured = $derived(account?.current?.hasConnected === true);
	const tabs = $derived(measured ? [...baseTabs, { id: 'measured', label: 'Measured' }] : baseTabs);

	let chosenTab = $state('overview');
	const activeTab = $derived(chosenTab === 'measured' && !measured ? 'overview' : chosenTab);

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
	<Tabs {tabs} active={activeTab} onchange={(id) => (chosenTab = id)} class="shrink-0" />

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
		{:else if activeTab === 'measured'}
			<MeasuredTab />
		{/if}
	</div>
</div>
