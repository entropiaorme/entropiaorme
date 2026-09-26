<script lang="ts">
	import { onMount } from 'svelte';
	import { listen } from '@tauri-apps/api/event';
	import { CONSUMABLES_TOPIC } from '$lib/api';
	import { Button, ErrorNotice, Tabs } from '$lib/components';
	import EquipmentFormModal from '$lib/features/equipment/EquipmentFormModal.svelte';
	import EquipmentListView from '$lib/features/equipment/EquipmentListView.svelte';
	import { createLibraryModel } from '$lib/features/equipment/libraryModel.svelte';
	import ProtectionTab from '$lib/features/protection/ProtectionTab.svelte';
	import { createProtectionModel } from '$lib/features/protection/protectionModel.svelte';
	import { inDevelopment } from '$lib/inDevelopment';
	import type { Hotbar } from '$lib/types/settings';
	import GuardrailsTab from './GuardrailsTab.svelte';
	import EffectsTab from './EffectsTab.svelte';
	import HotbarTab from './HotbarTab.svelte';

	const model = createLibraryModel();
	const protection = createProtectionModel();

	const tabs = $derived([
		{ id: 'library', label: 'Library' },
		...(inDevelopment.visible ? [{ id: 'protection', label: 'Armour' }] : []),
		{ id: 'effects', label: 'Effects' },
		{ id: 'hotbar', label: 'Hotbar' },
		{ id: 'guardrails', label: 'Guardrails' }
	]);
	let activeTab = $state('library');

	// Load on mount, and the Armour tab's data once it is visible.
	$effect(() => {
		void model.loadData();
		if (inDevelopment.visible) void protection.load();
	});

	// A dose starting or ending moves the reload speed every figure here is
	// priced under.
	$effect(() => {
		let unlisten: (() => void) | undefined;
		let closed = false;
		void listen(CONSUMABLES_TOPIC, () => void model.refreshPricing()).then((stop) => {
			if (closed) stop();
			else unlisten = stop;
		});
		return () => {
			closed = true;
			unlisten?.();
		};
	});

	onMount(() => () => model.destroy());
</script>

<ErrorNotice
	class="mx-6 mt-6"
	message={model.error}
	onDismiss={() => (model.error = null)}
/>

<div class="px-6 pb-6 space-y-6">
	<!-- Page header -->
	<div class="flex items-center justify-between">
		<header class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold text-text tracking-tight">Equipment</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="text-sm text-text-secondary mt-0.5">
				Gear library with automatic cost-per-use calculation
			</p>
		</header>
		<div class="flex items-center gap-2">
			{#if activeTab === 'library'}
				<Button size="sm" onclick={() => model.openAddModal()}>
					<svg
						xmlns="http://www.w3.org/2000/svg"
						viewBox="0 0 20 20"
						fill="currentColor"
						class="h-3.5 w-3.5"
					>
						<path
							d="M10.75 4.75a.75.75 0 00-1.5 0v4.5h-4.5a.75.75 0 000 1.5h4.5v4.5a.75.75 0 001.5 0v-4.5h4.5a.75.75 0 000-1.5h-4.5v-4.5z"
						/>
					</svg>
					Add Equipment
				</Button>
			{/if}
		</div>
	</div>

	<!-- Tabs -->
	<div>
		<Tabs {tabs} active={activeTab} onchange={(id) => (activeTab = id)} />
	</div>

	{#if activeTab === 'hotbar'}
		<HotbarTab
			equipment={model.allEquipment}
			hotbar={model.hotbar}
			carriedWeaponIds={model.carriedWeaponIds}
			hotbarHooksEnabled={model.hotbarHooksEnabled}
			onchange={(value: Hotbar) => {
				model.hotbar = { ...value };
			}}
			oncarriedchange={(ids) => {
				model.carriedWeaponIds = ids;
			}}
		/>
	{:else if activeTab === 'guardrails'}
		<GuardrailsTab
			equipment={model.allEquipment}
			guardrail={model.harvestGuardrail}
			onchange={(value) => {
				model.harvestGuardrail = value;
			}}
		/>
	{:else if activeTab === 'effects'}
		<EffectsTab
			sources={model.passiveEffectSources}
			reloadSpeed={model.reloadSpeed}
			onchange={(settings) => model.effectsSaved(settings)}
		/>
	{:else if activeTab === 'protection' && inDevelopment.visible}
		<ProtectionTab model={protection} />
	{:else}
		<EquipmentListView {model} />
	{/if}
</div>

<EquipmentFormModal {model} />
