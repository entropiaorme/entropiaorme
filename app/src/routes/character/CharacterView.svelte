<script lang="ts">
	import { type UnlistenFn } from '@tauri-apps/api/event';
	import { onMount } from 'svelte';
	import Button from '$lib/components/Button.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import Tabs from '$lib/components/Tabs.svelte';
	import AttributesTable from '$lib/features/character/AttributesTable.svelte';
	import { createCharacterModel } from '$lib/features/character/characterModel.svelte';
	import OptimizerView from '$lib/features/character/OptimizerView.svelte';
	import ProfessionsTable from '$lib/features/character/ProfessionsTable.svelte';
	import ProspectView from '$lib/features/character/ProspectView.svelte';
	import RecommenderView from '$lib/features/character/RecommenderView.svelte';
	import SkillsTable from '$lib/features/character/SkillsTable.svelte';
	import {
		characterDemoOptimizerProfession,
		characterDemoOptimizerTargetLevel,
		characterDemoPathOptimizer,
		characterDemoProspectProfession,
		characterDemoProspectResult,
		characterDemoProspectTargetLevel,
	} from '$lib/guide/fixtures/character';
	import { guideState, registerDemoApi, unregisterDemoApi } from '$lib/guide/state.svelte';
	import {
		hydrate as hydrateScan,
		scanStatus as scanStatusStore,
		subscribeScan,
	} from '$lib/stores/scanStore.svelte';
	import { formatDateFull } from '$lib/utils/format';
	import CodexTab from './CodexTab.svelte';
	import ScanInFlightView from './ScanInFlightView.svelte';

	const model = createCharacterModel();
	const optimizer = model.optimizer;
	const prospect = model.prospect;

	// ── Tab state ───────────────────────────────────────────────────────────

	let mainTab = $state<'stats' | 'prospect' | 'optimizer' | 'recommender' | 'codex'>('stats');
	let statsSubTab = $state<'attributes' | 'skills' | 'professions'>('attributes');

	// Guide-mode fake skill scanner (only consulted when guideState.isActive)
	let demoFakeScannerVisible = $state(false);
	// Guide-mode codex seed flag, passed as a prop to CodexTab.
	let demoCodexSeedActive = $state(false);

	onMount(() => {
		registerDemoApi('character', {
			setMainTab: (tab: string) => {
				mainTab = tab as 'stats' | 'prospect' | 'optimizer' | 'recommender' | 'codex';
			},
			setStatsSubTab: (tab: string) => {
				statsSubTab = tab as 'attributes' | 'skills' | 'professions';
			},
			setFakeScannerVisible: (visible: boolean) => {
				demoFakeScannerVisible = visible;
			},
			setProspectSeed: (seed: boolean) => {
				if (seed) {
					optimizer.selectedProfession = characterDemoProspectProfession;
					prospect.targetInput = characterDemoProspectTargetLevel;
					prospect.sliceType = 'global';
					prospect.result = characterDemoProspectResult;
				} else {
					optimizer.selectedProfession = '';
					prospect.targetInput = '';
					prospect.result = null;
				}
			},
			setOptimizerSeed: (seed: boolean) => {
				if (seed) {
					optimizer.mode = 'profession';
					optimizer.selectedProfession = characterDemoOptimizerProfession;
					optimizer.pathTargetInput = characterDemoOptimizerTargetLevel;
					optimizer.pathResult = characterDemoPathOptimizer;
				} else {
					optimizer.mode = 'profession';
					optimizer.selectedProfession = '';
					optimizer.pathTargetInput = '';
					optimizer.pathResult = null;
				}
			},
			setCodexSeed: (seed: boolean) => {
				demoCodexSeedActive = seed;
			}
		});
		return () => unregisterDemoApi('character');
	});

	// ── Manual scan status (drives in-flight view) ──────────────────────────────

	// Scan status from the shared event-driven store, suppressed while the guide
	// is active (the guide owns this view then). The effect below hydrates once
	// and subscribes when the guide is inactive; the store re-reads on each
	// backend scan frame the shell bridges, replacing the retired 500ms poll.
	let scanStatus = $derived(guideState.isActive ? null : scanStatusStore.current);
	let scanInFlight = $derived(scanStatus !== null && scanStatus.phase !== 'idle');

	$effect(() => {
		if (scanInFlight) statsSubTab = 'skills';
	});

	$effect(() => {
		if (guideState.isActive) return;
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
		void model.loadCharacterData(guideState.isActive);
	}

	// Keep the prospect slice selection valid as its options or type change.
	$effect(() => {
		if (prospect.sliceType === 'global') {
			prospect.sliceValue = '';
			return;
		}
		if (!prospect.currentOptions.some((option) => option.value === prospect.sliceValue)) {
			prospect.sliceValue = prospect.currentOptions[0]?.value ?? '';
		}
	});

	// ── Load on mount ───────────────────────────────────────────────────────────

	$effect(() => {
		void model.loadCharacterData(guideState.isActive);
	});

	// Refresh after the user returns from the scan overlay window.
	$effect(() => {
		if (guideState.isActive) return;
		const onFocus = () => { void model.loadCharacterData(false); };
		window.addEventListener('focus', onFocus);
		return () => window.removeEventListener('focus', onFocus);
	});
</script>

<div class="space-y-5">
	{#if guideState.isActive && demoFakeScannerVisible}
		<div class="fixed top-20 left-12 right-0 z-10 flex justify-center pointer-events-none">
			<img
				data-guide-anchor="character-scanner-spawn"
				src="/guide-assets/skill-scanner.png"
				alt=""
				class="block"
			/>
		</div>
	{/if}

	<!-- Main tab toggle -->
	<Tabs
		tabs={[
			{ id: 'stats', label: 'Stats' },
			{ id: 'prospect', label: 'Prospect' },
			{ id: 'optimizer', label: 'Optimiser' },
			{ id: 'recommender', label: 'Activity Recommender' },
			{ id: 'codex', label: 'Codex' }
		]}
		active={mainTab}
		onchange={(id) => (mainTab = id as 'stats' | 'prospect' | 'optimizer' | 'recommender' | 'codex')}
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

	{#if mainTab === 'prospect'}
		<ProspectView {model} />
	{/if}

	{#if mainTab === 'optimizer'}
		<OptimizerView {model} />
	{/if}

	{#if mainTab === 'recommender'}
		<RecommenderView {model} />
	{/if}

	{#if mainTab === 'codex'}
		<CodexTab seedActive={demoCodexSeedActive} />
	{/if}
</div>
