<script lang="ts">
	import { type UnlistenFn } from '@tauri-apps/api/event';
	import { onMount } from 'svelte';
	import Button from '$lib/components/Button.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import Tabs from '$lib/components/Tabs.svelte';
	import AttributesTable from '$lib/features/character/AttributesTable.svelte';
	import { createCharacterModel } from '$lib/features/character/characterModel.svelte';
	import ProfessionsTable from '$lib/features/character/ProfessionsTable.svelte';
	import SkillingHub from '$lib/features/character/SkillingHub.svelte';
	import SkillsTable from '$lib/features/character/SkillsTable.svelte';
	import {
		hydrate as hydrateScan,
		scanStatus as scanStatusStore,
		subscribeScan,
	} from '$lib/stores/scanStore.svelte';
	import { formatDateFull } from '$lib/utils/format';
	import CodexTab from './CodexTab.svelte';
	import ScanInFlightView from './ScanInFlightView.svelte';

	const model = createCharacterModel();

	// ── Tab state ───────────────────────────────────────────────────────────

	let mainTab = $state<'stats' | 'skilling' | 'codex'>('stats');
	let statsSubTab = $state<'attributes' | 'skills' | 'professions'>('attributes');

	// ── Manual scan status (drives in-flight view) ──────────────────────────────

	// Scan status from the shared event-driven store. The effect below hydrates
	// once and subscribes; the store re-reads on each backend scan frame the
	// shell bridges.
	let scanStatus = $derived(scanStatusStore.current);
	let scanInFlight = $derived(scanStatus !== null && scanStatus.phase !== 'idle');

	$effect(() => {
		if (scanInFlight) statsSubTab = 'skills';
	});

	$effect(() => {
		let unlisten: UnlistenFn | undefined;
		let disposed = false;
		// Attach the listener BEFORE the first hydrate: a status change between
		// the hydrate GET and the listener attaching would otherwise be lost (if
		// it were the last transition). Hydrating inside the resolve keeps the
		// listener live first, so any later frame re-hydrates and heals it.
		void subscribeScan().then((fn) => {
			if (disposed) {
				fn();
				return;
			}
			unlisten = fn;
			void hydrateScan();
		});
		return () => {
			disposed = true;
			unlisten?.();
		};
	});

	function onScanReviewComplete() {
		void model.loadCharacterData();
	}

	// ── Load on mount ───────────────────────────────────────────────────────────

	onMount(() => {
		void model.loadCharacterData();
		// Refresh after the user returns from the scan overlay window.
		const onFocus = () => { void model.loadCharacterData(); };
		window.addEventListener('focus', onFocus);
		return () => window.removeEventListener('focus', onFocus);
	});
</script>

<div class="space-y-5">
	<!-- Main tab toggle -->
	<Tabs
		tabs={[
			{ id: 'stats', label: 'Stats' },
			{ id: 'skilling', label: 'Skilling' },
			{ id: 'codex', label: 'Codex' }
		]}
		active={mainTab}
		onchange={(id) => (mainTab = id as 'stats' | 'skilling' | 'codex')}
	/>

	<ErrorNotice message={model.error} />

	{#if mainTab === 'stats'}
		<!-- Sub-tab toggle + compact scan status / button -->
		<div class="flex items-center justify-between gap-4">
			<SegmentedControl
				size="md"
				options={[
					{ id: 'attributes', label: 'Attributes', disabled: scanInFlight },
					{ id: 'skills', label: 'Skills', disabled: scanInFlight },
					{ id: 'professions', label: 'Professions', disabled: scanInFlight }
				]}
				active={statsSubTab}
				onchange={(id) => (statsSubTab = id as 'attributes' | 'skills' | 'professions')}
			/>
			<div class="flex items-center gap-3">
				<div class="flex items-center gap-2 text-xs text-text-tertiary whitespace-nowrap">
					<span class="h-2 w-2 rounded-full {model.calibration.calibrated ? 'bg-success' : 'bg-warning'}"></span>
					<span>Last scanned</span>
					<span class="text-text">
						{model.calibration.calibrated && model.calibration.lastCalibration
							? formatDateFull(model.calibration.lastCalibration)
							: 'never'}
					</span>
				</div>
				{#if scanInFlight}
					<span class="rounded-md bg-surface px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-text-secondary whitespace-nowrap">
						{scanStatus?.phase === 'capturing' ? 'Capturing' : scanStatus?.phase === 'processing' ? 'Processing' : 'Awaiting review'}
					</span>
				{:else}
					<Button size="sm" variant="secondary" onclick={model.openScanOverlay}>
						{#snippet children()}Scan skills{/snippet}
					</Button>
				{/if}
			</div>
		</div>

	{#if scanInFlight && scanStatus}
		<ScanInFlightView
			status={scanStatus}
			onComplete={onScanReviewComplete}
		/>
	{:else}

	<!-- Attributes sub-tab -->
	{#if statsSubTab === 'attributes'}
		<AttributesTable {model} />
	{/if}

	<!-- Skills sub-tab -->
	{#if statsSubTab === 'skills'}
		<SkillsTable {model} />
	{/if}

	<!-- Professions sub-tab -->
	{#if statsSubTab === 'professions'}
		<ProfessionsTable {model} />
	{/if}

	{/if}

	{/if}

	{#if mainTab === 'skilling'}
		<SkillingHub {model} />
	{/if}

	{#if mainTab === 'codex'}
		<CodexTab />
	{/if}
</div>
