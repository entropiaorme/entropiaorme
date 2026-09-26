<script lang="ts">
	import { onMount } from 'svelte';
	import { Button, ErrorNotice, Tabs } from '$lib/components';
	import EquipmentFormModal from '$lib/features/equipment/EquipmentFormModal.svelte';
	import EquipmentListView from '$lib/features/equipment/EquipmentListView.svelte';
	import { createLibraryModel, type EquipmentFormType } from '$lib/features/equipment/libraryModel.svelte';
	import ProtectionTab from '$lib/features/protection/ProtectionTab.svelte';
	import { createProtectionModel } from '$lib/features/protection/protectionModel.svelte';
	import { closeGuide, openGuide } from '$lib/guide/engine';
	import { guideState, registerDemoApi, unregisterDemoApi } from '$lib/guide/state.svelte';
	import { equipmentSurface } from '$lib/guide/surfaces/equipment';
	import { getPreference } from '$lib/preferences';
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

	let guideSeen = $state(true);

	// Reload data on initial mount and whenever guide-mode toggles.
	$effect(() => {
		void model.loadData(guideState.isActive);
		if (inDevelopment.visible) void protection.load(guideState.isActive);
	});

	onMount(() => {
		void (async () => {
			guideSeen = await getPreference<boolean>('guide_seen_equipment', false);
		})();
		registerDemoApi('equipment', {
			setActiveTab: (tab: string) => {
				activeTab = tab;
			},
			expandRow: (id: string) => {
				model.expandedId = id;
			},
			collapseRow: () => {
				model.expandedId = null;
			},
			openAddModal: (type: EquipmentFormType = 'weapon') => {
				model.openAddModal(undefined, type);
			},
			closeAddModal: () => {
				model.showAddModal = false;
			}
		});
		return () => {
			unregisterDemoApi('equipment');
			model.destroy();
		};
	});

	function toggleSurfaceGuide(): void {
		if (guideState.isActive) {
			closeGuide();
		} else {
			guideSeen = true;
			void openGuide(equipmentSurface);
		}
	}
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
			<button
				type="button"
				onclick={toggleSurfaceGuide}
				title={guideState.isActive ? 'Exit guide' : 'Open guide'}
				aria-label={guideState.isActive ? 'Exit guide' : 'Open guide for this page'}
				class="relative h-8 w-8 rounded-full border border-border bg-surface hover:bg-surface-hover text-text-secondary hover:text-text transition-colors flex items-center justify-center text-sm font-semibold {guideState.isActive ? 'z-[9100]' : ''}"
			>
				{#if guideState.isActive}
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="w-3.5 h-3.5" aria-hidden="true">
						<path d="M5.28 4.22a.75.75 0 00-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 101.06 1.06L8 9.06l2.72 2.72a.75.75 0 101.06-1.06L9.06 8l2.72-2.72a.75.75 0 00-1.06-1.06L8 6.94 5.28 4.22z" />
					</svg>
				{:else}
					?
				{/if}
				{#if !guideSeen}
					<span class="absolute -top-0.5 -right-0.5 h-2 w-2 rounded-full bg-accent"></span>
				{/if}
			</button>
			{#if activeTab === 'library'}
				<Button size="sm" onclick={() => model.openAddModal()} data-guide-anchor="add-equipment-button">
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
	<div data-guide-anchor="equipment-tabs">
		<Tabs {tabs} active={activeTab} onchange={(id) => (activeTab = id)} />
	</div>

	{#if activeTab === 'hotbar'}
		<!-- In the guide the hotbar is demo data, so nothing it shows is saved. -->
		<HotbarTab
			equipment={model.allEquipment}
			hotbar={model.hotbar}
			carriedWeaponIds={model.carriedWeaponIds}
			hotbarHooksEnabled={model.hotbarHooksEnabled}
			enabled={!guideState.isActive}
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
