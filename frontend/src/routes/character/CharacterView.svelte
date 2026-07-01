<script lang="ts">
	import type {
		SkillLevel,
		ProfessionLevel,
		HpOptimizerSkill,
		HpOptimizerAttribute,
		PathOptimizerResult,
		CharacterProspectOptions,
		ProspectResult,
		ProspectSliceType,
		ProspectOption,
	} from '$lib/types/analytics';
	import {
		getCalibrationStatus,
		getCharacterStats,
		getCharacterSkills,
		getCharacterProfessions,
		getProfessionOptimizer,
		getProfessionPathOptimizer,
		getHpOptimizer,
		getCharacterProspectOptions,
		getCharacterProspect,
	} from '$lib/api';
	import { invoke } from '@tauri-apps/api/core';
	import { type UnlistenFn } from '@tauri-apps/api/event';
	import { onMount } from 'svelte';
	import { guideState, registerDemoApi, unregisterDemoApi } from '$lib/guide/state.svelte';
	import {
		scanStatus as scanStatusStore,
		hydrate as hydrateScan,
		subscribeScan,
	} from '$lib/stores/scanStore';
	import {
		characterDemoCalibration,
		characterDemoStats,
		characterDemoSkills,
		characterDemoProfessions,
		characterDemoProspectOptions,
		characterDemoProspectProfession,
		characterDemoProspectTargetLevel,
		characterDemoProspectResult,
		characterDemoOptimizerProfession,
		characterDemoOptimizerTargetLevel,
		characterDemoPathOptimizer
	} from '$lib/guide/fixtures/character';

	function openScanOverlay() {
		invoke('show_scan_overlay').catch(() => {});
	}
	import { formatPed, formatDateFull, formatPercent } from '$lib/utils/format';
	import Card from '$lib/components/Card.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import Divider from '$lib/components/Divider.svelte';
	import StatDisplay from '$lib/components/StatDisplay.svelte';
	import Button from '$lib/components/Button.svelte';
	import Tabs from '$lib/components/Tabs.svelte';
	import Input from '$lib/components/Input.svelte';
	import Select from '$lib/components/Select.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import CodexTab from './CodexTab.svelte';
	import ScanInFlightView from './ScanInFlightView.svelte';

	const PAGE_SIZE = 12;

	// ── Data state ──────────────────────────────────────────────────────────────

	let calibration = $state({ calibrated: false, lastCalibration: null as string | null, stale: true });
	let stats = $state({ hp: 80, topProfessions: [] as ProfessionLevel[] });
	let skills = $state([] as SkillLevel[]);
	let professions = $state([] as ProfessionLevel[]);
	let loading = $state(true);

	// ── Tab state ───────────────────────────────────────────────────────────

	let mainTab = $state<'stats' | 'prospect' | 'optimizer' | 'codex'>('stats');
	let statsSubTab = $state<'attributes' | 'skills' | 'professions'>('attributes');

	// Guide-mode fake skill scanner (only consulted when guideState.isActive)
	let demoFakeScannerVisible = $state(false);
	// Guide-mode codex seed flag, passed as a prop to CodexTab.
	let demoCodexSeedActive = $state(false);

	onMount(() => {
		registerDemoApi('character', {
			setMainTab: (tab: string) => {
				mainTab = tab as 'stats' | 'prospect' | 'optimizer' | 'codex';
			},
			setStatsSubTab: (tab: string) => {
				statsSubTab = tab as 'attributes' | 'skills' | 'professions';
			},
			setFakeScannerVisible: (visible: boolean) => {
				demoFakeScannerVisible = visible;
			},
			setProspectSeed: (seed: boolean) => {
				if (seed) {
					selectedProfession = characterDemoProspectProfession;
					prospectTargetInput = characterDemoProspectTargetLevel;
					prospectSliceType = 'global';
					prospectResult = characterDemoProspectResult;
				} else {
					selectedProfession = '';
					prospectTargetInput = '';
					prospectResult = null;
				}
			},
			setOptimizerSeed: (seed: boolean) => {
				if (seed) {
					optimizerMode = 'profession';
					selectedProfession = characterDemoOptimizerProfession;
					pathTargetInput = characterDemoOptimizerTargetLevel;
					pathResult = characterDemoPathOptimizer;
				} else {
					optimizerMode = 'profession';
					selectedProfession = '';
					pathTargetInput = '';
					pathResult = null;
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
	// backend scan frame the relay re-emits, replacing the retired 500ms poll.
	let scanStatus = $derived(guideState.isActive ? null : $scanStatusStore);
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
		void loadCharacterData(guideState.isActive);
	}

	// ── Split attributes from regular skills ────────────────────────────────────

	let attributes = $derived(skills.filter(s => s.isAttribute));
	let regularSkills = $derived(skills.filter(s => !s.isAttribute));

	// ── Optimizer state ─────────────────────────────────────────────────────────

	let optimizerMode = $state<'profession' | 'hp'>('profession');
	let selectedProfession = $state('');
	let optimizerProfLevel = $state(0);

	// HP optimizer state
	let hpSkills = $state([] as HpOptimizerSkill[]);
	let hpAttributes = $state([] as HpOptimizerAttribute[]);
	let hpCurrent = $state(0);
	let hpLoading = $state(false);

	// Path optimizer state
	let pathTargetInput = $state('');
	let pathResult = $state<PathOptimizerResult | null>(null);
	let pathLoading = $state(false);

	// Prospect state
	let prospectOptions = $state<CharacterProspectOptions>({ tags: [], mobs: [], weapons: [] });
	let prospectSliceType = $state<ProspectSliceType>('global');
	let prospectSliceValue = $state('');
	let prospectTargetInput = $state('');
	let prospectMarkupInput = $state('');
	let prospectResult = $state<ProspectResult | null>(null);
	let prospectLoading = $state(false);

	let currentProspectOptions = $derived.by(() => {
		if (prospectSliceType === 'tag') return prospectOptions.tags;
		if (prospectSliceType === 'mob') return prospectOptions.mobs;
		if (prospectSliceType === 'weapon') return prospectOptions.weapons;
		return [] as ProspectOption[];
	});

	$effect(() => {
		if (prospectSliceType === 'global') {
			prospectSliceValue = '';
			return;
		}
		if (!currentProspectOptions.some((option) => option.value === prospectSliceValue)) {
			prospectSliceValue = currentProspectOptions[0]?.value ?? '';
		}
	});

	async function loadOptimizer(profName: string) {
		if (!profName) { optimizerProfLevel = 0; return; }
		try {
			const result = await getProfessionOptimizer(profName);
			optimizerProfLevel = result.currentLevel ?? 0;
		} catch { optimizerProfLevel = 0; }
	}

	async function loadHpOptimizer() {
		hpLoading = true;
		try {
			const result = await getHpOptimizer();
			hpSkills = result.skills || [];
			hpAttributes = result.attributes || [];
			hpCurrent = result.currentHp ?? 0;
		} catch { hpSkills = []; hpAttributes = []; }
		finally { hpLoading = false; }
	}

	async function loadPathOptimizer() {
		if (!selectedProfession) return;
		const target = parseFloat(pathTargetInput);
		if (isNaN(target) || target <= 0) return;
		pathLoading = true;
		pathResult = null;
		try {
			pathResult = await getProfessionPathOptimizer(
				selectedProfession,
				{ targetLevel: target }
			);
		} catch { pathResult = null; }
		finally { pathLoading = false; }
	}

	async function loadProspect() {
		if (!selectedProfession) return;
		const target = parseFloat(prospectTargetInput);
		if (isNaN(target) || target <= 0) return;
		if (prospectSliceType !== 'global' && !prospectSliceValue) return;

		prospectLoading = true;
		prospectResult = null;
		try {
			prospectResult = await getCharacterProspect({
				profession: selectedProfession,
				targetLevel: target,
				sliceType: prospectSliceType,
				sliceValue: prospectSliceType === 'global' ? undefined : prospectSliceValue,
				markupUplift: Math.max(0, (parseFloat(prospectMarkupInput) || 0) / 100),
			});
		} catch {
			prospectResult = null;
		} finally {
			prospectLoading = false;
		}
	}

	// ── Load on mount ───────────────────────────────────────────────────────────

	$effect(() => {
		void loadCharacterData(guideState.isActive);
	});

	// Refresh after the user returns from the scan overlay window.
	$effect(() => {
		if (guideState.isActive) return;
		const onFocus = () => { void loadCharacterData(false); };
		window.addEventListener('focus', onFocus);
		return () => window.removeEventListener('focus', onFocus);
	});

	async function loadCharacterData(guideMode: boolean) {
		if (guideMode) {
			calibration = characterDemoCalibration;
			stats = characterDemoStats;
			skills = characterDemoSkills.map(s => ({ ...s }));
			professions = characterDemoProfessions.map(p => ({ ...p }));
			prospectOptions = characterDemoProspectOptions;
			loading = false;
			return;
		}
		try {
			const [cal, st, sk, pr, po] = await Promise.all([
				getCalibrationStatus(),
				getCharacterStats(),
				getCharacterSkills(),
				getCharacterProfessions(),
				getCharacterProspectOptions(),
			]);
			calibration = cal;
			stats = st;
			skills = sk;
			professions = pr;
			prospectOptions = po;
		} catch {
			// Backend not reachable
		} finally {
			loading = false;
		}
	}

	function formatProspectHours(hours: number): string {
		if (hours <= 0) return '0h';
		if (hours < 1) return `${Math.round(hours * 60)}m`;
		if (hours < 10) return `${hours.toFixed(1)}h`;
		return `${hours.toFixed(0)}h`;
	}

	// ── Skills: filter, sort, paginate ──────────────────────────────────────────

	let skillSearch = $state('');
	let skillCategory = $state<string | null>(null);
	let skillPage = $state(0);
	let skillSortKey = $state<(keyof SkillLevel & string) | undefined>('level');
	let skillSortDir = $state<'asc' | 'desc'>('desc');

	let skillCategories = $derived(
		[...new Set(regularSkills.map(s => s.category))].sort()
	);

	let filteredSkills = $derived.by(() => {
		let result = regularSkills;
		if (skillCategory) result = result.filter(s => s.category === skillCategory);
		const q = skillSearch.toLowerCase();
		if (q) result = result.filter(s => s.name.toLowerCase().includes(q));
		if (skillSortKey) {
			const key = skillSortKey;
			const dir = skillSortDir === 'asc' ? 1 : -1;
			result = [...result].sort((a, b) => {
				const aVal = a[key], bVal = b[key];
				if (aVal == null && bVal == null) return 0;
				if (aVal == null) return 1;   // nulls last
				if (bVal == null) return -1;
				if (typeof aVal === 'number' && typeof bVal === 'number') return dir * (aVal - bVal);
				return dir * String(aVal).localeCompare(String(bVal));
			});
		}
		return result;
	});

	let skillTotalPages = $derived(Math.max(1, Math.ceil(filteredSkills.length / PAGE_SIZE)));
	let skillPageRows = $derived(filteredSkills.slice(skillPage * PAGE_SIZE, (skillPage + 1) * PAGE_SIZE));

	// Reset page when filter changes
	$effect(() => { skillCategory; skillSearch; skillPage = 0; });

	// ── Professions: filter, sort, paginate ─────────────────────────────────────

	let profSearch = $state('');
	let profCategory = $state<string | null>(null);
	let profPage = $state(0);
	let profSortKey = $state<(keyof ProfessionLevel & string) | undefined>('level');
	let profSortDir = $state<'asc' | 'desc'>('desc');

	let profCategories = $derived(
		[...new Set(professions.map(p => p.category))].sort()
	);

	let filteredProfessions = $derived.by(() => {
		let result = professions;
		if (profCategory) result = result.filter(p => p.category === profCategory);
		const pq = profSearch.toLowerCase();
		if (pq) result = result.filter(p => p.name.toLowerCase().includes(pq));
		if (profSortKey) {
			const key = profSortKey;
			const dir = profSortDir === 'asc' ? 1 : -1;
			result = [...result].sort((a, b) => {
				const aVal = a[key], bVal = b[key];
				if (aVal == null && bVal == null) return 0;
				if (aVal == null) return 1;
				if (bVal == null) return -1;
				if (typeof aVal === 'number' && typeof bVal === 'number') return dir * (aVal - bVal);
				return dir * String(aVal).localeCompare(String(bVal));
			});
		}
		return result;
	});

	let profTotalPages = $derived(Math.max(1, Math.ceil(filteredProfessions.length / PAGE_SIZE)));
	let profPageRows = $derived(filteredProfessions.slice(profPage * PAGE_SIZE, (profPage + 1) * PAGE_SIZE));

	$effect(() => { profCategory; profSearch; profPage = 0; });

	// ── Column sort handler ─────────────────────────────────────────────────────

	function handleSkillSort(key: keyof SkillLevel & string) {
		if (skillSortKey === key) { skillSortDir = skillSortDir === 'asc' ? 'desc' : 'asc'; }
		else { skillSortKey = key; skillSortDir = 'asc'; }
		skillPage = 0;
	}

	function handleProfSort(key: keyof ProfessionLevel & string) {
		if (profSortKey === key) { profSortDir = profSortDir === 'asc' ? 'desc' : 'asc'; }
		else { profSortKey = key; profSortDir = 'asc'; }
		profPage = 0;
	}

	// Formatters for the anchor / gain columns (Stats tab progression breakdown).
	// Gain is shown with sign + 2dp; near-zero collapses to '0.00' so it doesn't
	// flicker between '+0.00' and '−0.00'. Null = no anchor on record.
	function formatGain(gain: number | null): string {
		if (gain === null) return '—';
		if (Math.abs(gain) < 0.005) return '0.00';
		return (gain > 0 ? '+' : '') + gain.toFixed(2);
	}

	function gainColorClass(gain: number | null): string {
		if (gain === null || Math.abs(gain) < 0.005) return 'text-text-tertiary';
		return gain > 0 ? 'text-success' : 'text-warning';
	}

	function formatProfLevel(level: number | null): string {
		if (level === null) return '—';
		return `${Math.floor(level)} (${((level % 1) * 100).toFixed(1)}%)`;
	}
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
			{ id: 'codex', label: 'Codex' }
		]}
		active={mainTab}
		onchange={(id) => (mainTab = id as 'stats' | 'prospect' | 'optimizer' | 'codex')}
	/>

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
					<span class="h-2 w-2 rounded-full {calibration.calibrated ? 'bg-success' : 'bg-warning'}"></span>
					<span>Last scanned</span>
					<span class="text-text">
						{calibration.calibrated && calibration.lastCalibration
							? formatDateFull(calibration.lastCalibration)
							: 'never'}
					</span>
				</div>
				{#if scanInFlight}
					<span class="rounded-md bg-surface px-3 py-1.5 text-xs font-medium uppercase tracking-wide text-text-secondary whitespace-nowrap">
						{scanStatus?.phase === 'capturing' ? 'Capturing' : scanStatus?.phase === 'processing' ? 'Processing' : 'Awaiting review'}
					</span>
				{:else}
					<Button size="sm" variant="secondary" onclick={openScanOverlay}>
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
		<div>
			<div class="overflow-x-auto">
				<table data-guide-anchor="character-attributes-table" class="w-full text-sm">
					<thead>
						<tr class="border-b border-border">
							<th class="py-2 px-3 text-left eyebrow">Attribute</th>
							<th class="py-2 px-3 text-right eyebrow">Anchor</th>
							<th class="py-2 px-3 text-right eyebrow">Gain</th>
							<th class="py-2 px-3 text-right eyebrow">Level</th>
						</tr>
					</thead>
					<tbody>
						{#if attributes.length === 0}
							<tr><td colspan="4" class="py-8 text-center text-text-tertiary">{loading ? 'Loading...' : 'No attributes calibrated yet'}</td></tr>
						{:else}
							{#each attributes as attr}
								<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
									<td class="py-2.5 px-3 text-text">{attr.name}</td>
									<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{attr.anchorLevel === null ? '—' : attr.anchorLevel.toFixed(2)}</td>
									<td class="py-2.5 px-3 text-right tabular-nums {gainColorClass(attr.gainSinceAnchor)}">{formatGain(attr.gainSinceAnchor)}</td>
									<td class="py-2.5 px-3 text-right tabular-nums">{attr.level.toFixed(2)}</td>
								</tr>
							{/each}
						{/if}
					</tbody>
				</table>
			</div>
		</div>
	{/if}

	<!-- Skills sub-tab -->
	{#if statsSubTab === 'skills'}
		<!-- Search -->
		<SearchInput bind:value={skillSearch} placeholder="Search skills..." />

		<!-- Category filter pills + table -->
		{#if skillCategories.length > 1}
			<div class="flex flex-wrap gap-1">
				<button
					type="button"
					class="filter-chip {skillCategory === null ? 'is-active' : ''}"
					onclick={() => (skillCategory = null)}
				>All</button>
				{#each skillCategories as cat}
					<button
						type="button"
						class="filter-chip {skillCategory === cat ? 'is-active' : ''}"
						onclick={() => (skillCategory = cat)}
					>{cat}</button>
				{/each}
			</div>
		{/if}

		<div>
				<div class="overflow-x-auto">
					<table class="w-full text-sm" data-guide-anchor="character-skills-table">
						<thead>
							<tr class="border-b border-border">
								<th class="py-2 px-3 text-left eyebrow cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => handleSkillSort('name')}>
									<span class="inline-flex items-center gap-1">Skill {#if skillSortKey === 'name'}<span class="text-accent">{skillSortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</span>
								</th>
								<th class="py-2 px-3 text-right eyebrow cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => handleSkillSort('anchorLevel')}>
									<span class="inline-flex items-center gap-1">Anchor {#if skillSortKey === 'anchorLevel'}<span class="text-accent">{skillSortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</span>
								</th>
								<th class="py-2 px-3 text-right eyebrow cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => handleSkillSort('gainSinceAnchor')}>
									<span class="inline-flex items-center gap-1">Gain {#if skillSortKey === 'gainSinceAnchor'}<span class="text-accent">{skillSortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</span>
								</th>
								<th class="py-2 px-3 text-right eyebrow cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => handleSkillSort('level')}>
									<span class="inline-flex items-center gap-1">Level {#if skillSortKey === 'level'}<span class="text-accent">{skillSortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</span>
								</th>
								<th class="py-2 px-3 text-left eyebrow">Rank</th>
								<th class="py-2 px-3 text-right eyebrow cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => handleSkillSort('ttValue')}>
									<span class="inline-flex items-center gap-1">PES {#if skillSortKey === 'ttValue'}<span class="text-accent">{skillSortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</span>
								</th>
							</tr>
						</thead>
						<tbody>
							{#if skillPageRows.length === 0}
								<tr><td colspan="6" class="py-8 text-center text-text-tertiary">{loading ? 'Loading...' : 'No skills calibrated yet'}</td></tr>
							{:else}
								{#each skillPageRows as skill}
									<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
										<td class="py-2.5 px-3 text-text">{skill.name}</td>
										<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{skill.anchorLevel === null ? '\u2014' : skill.anchorLevel.toFixed(2)}</td>
										<td class="py-2.5 px-3 text-right tabular-nums {gainColorClass(skill.gainSinceAnchor)}">{formatGain(skill.gainSinceAnchor)}</td>
										<td class="py-2.5 px-3 text-right tabular-nums">{skill.level.toFixed(2)}</td>
										<td class="py-2.5 px-3"><Badge variant="neutral">{skill.rankName}</Badge></td>
										<td class="py-2.5 px-3 text-right tabular-nums">{formatPed(skill.ttValue)}</td>
									</tr>
								{/each}
							{/if}
						</tbody>
					</table>
				</div>

				<!-- Pagination -->
				{#if skillTotalPages > 1}
					<div class="flex items-center justify-between mt-3 px-1">
						<span class="text-xs text-text-tertiary">
							{skillPage * PAGE_SIZE + 1}–{Math.min((skillPage + 1) * PAGE_SIZE, filteredSkills.length)} of {filteredSkills.length}
						</span>
						<div class="flex items-center gap-1">
							<Button size="sm" variant="ghost" disabled={skillPage === 0} onclick={() => skillPage--}>
								{#snippet children()}&lsaquo; Prev{/snippet}
							</Button>
							<span class="text-xs text-text-secondary tabular-nums px-2">
								{skillPage + 1} / {skillTotalPages}
							</span>
							<Button size="sm" variant="ghost" disabled={skillPage >= skillTotalPages - 1} onclick={() => skillPage++}>
								{#snippet children()}Next &rsaquo;{/snippet}
							</Button>
						</div>
					</div>
				{/if}
			</div>

	{/if}

	<!-- 4. Professions sub-tab -->
	{#if statsSubTab === 'professions'}
		<!-- Search -->
		<SearchInput bind:value={profSearch} placeholder="Search professions..." />

		<div>
				<div class="overflow-x-auto">
					<table class="w-full text-sm">
						<thead>
							<tr class="border-b border-border">
								<th class="py-2 px-3 text-left eyebrow cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => handleProfSort('name')}>
									<span class="inline-flex items-center gap-1">Profession {#if profSortKey === 'name'}<span class="text-accent">{profSortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</span>
								</th>
								<th class="py-2 px-3 text-right eyebrow cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => handleProfSort('anchorLevel')}>
									<span class="inline-flex items-center gap-1">Anchor {#if profSortKey === 'anchorLevel'}<span class="text-accent">{profSortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</span>
								</th>
								<th class="py-2 px-3 text-right eyebrow cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => handleProfSort('gainSinceAnchor')}>
									<span class="inline-flex items-center gap-1">Gain {#if profSortKey === 'gainSinceAnchor'}<span class="text-accent">{profSortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</span>
								</th>
								<th class="py-2 px-3 text-right eyebrow cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => handleProfSort('level')}>
									<span class="inline-flex items-center gap-1">Level {#if profSortKey === 'level'}<span class="text-accent">{profSortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</span>
								</th>
							</tr>
						</thead>
						<tbody>
							{#if profPageRows.length === 0}
								<tr><td colspan="4" class="py-8 text-center text-text-tertiary">{loading ? 'Loading...' : 'No profession data'}</td></tr>
							{:else}
								{#each profPageRows as prof}
									<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
										<td class="py-2.5 px-3 text-text">{prof.name}</td>
										<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{formatProfLevel(prof.anchorLevel)}</td>
										<td class="py-2.5 px-3 text-right tabular-nums {gainColorClass(prof.gainSinceAnchor)}">{formatGain(prof.gainSinceAnchor)}</td>
										<td class="py-2.5 px-3 text-right tabular-nums">{formatProfLevel(prof.level)}</td>
									</tr>
								{/each}
							{/if}
						</tbody>
					</table>
				</div>

				<!-- Pagination -->
				{#if profTotalPages > 1}
					<div class="flex items-center justify-between mt-3 px-1">
						<span class="text-xs text-text-tertiary">
							{profPage * PAGE_SIZE + 1}–{Math.min((profPage + 1) * PAGE_SIZE, filteredProfessions.length)} of {filteredProfessions.length}
						</span>
						<div class="flex items-center gap-1">
							<Button size="sm" variant="ghost" disabled={profPage === 0} onclick={() => profPage--}>
								{#snippet children()}&lsaquo; Prev{/snippet}
							</Button>
							<span class="text-xs text-text-secondary tabular-nums px-2">
								{profPage + 1} / {profTotalPages}
							</span>
							<Button size="sm" variant="ghost" disabled={profPage >= profTotalPages - 1} onclick={() => profPage++}>
								{#snippet children()}Next &rsaquo;{/snippet}
							</Button>
						</div>
					</div>
				{/if}
			</div>
	{/if}

	{/if}

	{/if}

	{#if mainTab === 'prospect'}
		<div class="space-y-4">
			<div class="flex items-center gap-3" data-guide-anchor="character-prospect-knob-first">
				<label for="prospect-prof-select" class="text-sm text-text-secondary whitespace-nowrap">Profession</label>
				<Select
					id="prospect-prof-select"
					class="flex-1"
					bind:value={selectedProfession}
					onchange={() => { loadOptimizer(selectedProfession); pathResult = null; prospectResult = null; }}
				>
					<option value="">Select a profession...</option>
					{#each professions as prof}
						<option value={prof.name}>{prof.name} (Lv {prof.level.toFixed(2)})</option>
					{/each}
				</Select>
			</div>

			<SegmentedControl
				options={[
					{ id: 'global', label: 'Global' },
					{ id: 'tag', label: 'Tag' },
					{ id: 'mob', label: 'Mob' },
					{ id: 'weapon', label: 'Weapon' }
				]}
				active={prospectSliceType}
				onchange={(id) => {
					prospectSliceType = id as ProspectSliceType;
					prospectResult = null;
				}}
			/>

			<div class="grid gap-3 md:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_minmax(0,0.8fr)_auto]" data-guide-anchor="character-prospect-knob-last">
				{#if prospectSliceType !== 'global'}
					<Select
						bind:value={prospectSliceValue}
						onchange={() => (prospectResult = null)}
					>
						<option value="" disabled selected={prospectSliceValue === ''}>
							Select a {prospectSliceType} sample...
						</option>
						{#each currentProspectOptions as option}
							<option value={option.value}>
								{option.label} ({option.sessions}s · {formatPed(option.cycledPed)} PED)
							</option>
						{/each}
					</Select>
				{:else}
					<div class="flex items-center rounded-md border border-border bg-surface px-3 py-2 text-sm text-text-secondary">
						All eligible tracked sessions
					</div>
				{/if}

				<Input
					type="number"
					min="1"
					step="0.01"
					placeholder={optimizerProfLevel > 0 ? `Target level (current ${optimizerProfLevel.toFixed(2)})` : 'Target level'}
					bind:value={prospectTargetInput}
					oninput={() => (prospectResult = null)}
					onkeydown={(e) => { if (e.key === 'Enter') loadProspect(); }}
				/>

				<Input
					type="number"
					min="0"
					step="0.1"
					placeholder="Markup uplift %"
					bind:value={prospectMarkupInput}
					oninput={() => (prospectResult = null)}
					onkeydown={(e) => { if (e.key === 'Enter') loadProspect(); }}
				/>

				<Button
					onclick={loadProspect}
					disabled={
						prospectLoading
						|| !selectedProfession
						|| !prospectTargetInput
						|| parseFloat(prospectTargetInput) <= 0
						|| (prospectSliceType !== 'global' && !prospectSliceValue)
					}
				>
					{#snippet children()}Calculate{/snippet}
				</Button>
			</div>

			{#if prospectSliceType !== 'global' && currentProspectOptions.length === 0}
				<p class="text-sm text-text-tertiary">No dominant {prospectSliceType} samples are available yet.</p>
			{/if}

			{#if !selectedProfession || !prospectTargetInput}
				<p class="text-sm text-text-tertiary py-4 text-center">Select a profession, choose a sample, and enter a target level to forecast the path.</p>
			{:else if prospectLoading}
				<p class="text-sm text-text-tertiary py-4 text-center">Calculating forecast...</p>
			{:else if prospectResult}
				{#if prospectResult.error}
					<Card class="p-4 border border-warning/30">
						<p class="text-sm font-medium text-warning">{prospectResult.error}</p>
					</Card>
				{:else}
					<div class="flex items-baseline gap-3 text-sm">
						<span class="text-text-secondary">Level</span>
						<span class="text-text tabular-nums font-medium">{prospectResult.currentLevel.toFixed(2)}</span>
						<span class="text-text-tertiary">→</span>
						<span class="text-accent tabular-nums font-medium">{prospectResult.targetLevel.toFixed(2)}</span>
						<span class="text-text-tertiary text-xs">
							{prospectResult.sliceType === 'global' ? 'Global aggregate' : `${prospectResult.sliceType}: ${prospectResult.sliceValue}`}
						</span>
					</div>

					<div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4" data-guide-anchor="character-prospect-result-tiles">
						<StatDisplay label="Projected Cycled" value={formatPed(prospectResult.projectedCycledPed)} unit="PED" />
						<StatDisplay label="Projected Time" value={formatProspectHours(prospectResult.projectedHours)} />
						<StatDisplay label="Expected Loot TT" value={formatPed(prospectResult.expectedLootTt)} unit="PED" />
						<StatDisplay label="Baseline Net Burn" value={formatPed(prospectResult.expectedNetTtBurn)} unit="PED" />
					</div>

					{#if prospectResult.speculativeNetTtBurn !== null}
						<div class="grid gap-4 sm:grid-cols-2">
							<StatDisplay
								label="Speculative Loot"
								value={formatPed(prospectResult.speculativeLootTt ?? 0)}
								unit="PED"
								comparison={`with +${prospectMarkupInput || '0'}% uplift`}
							/>
							<StatDisplay
								label="Speculative Net Burn"
								value={formatPed(prospectResult.speculativeNetTtBurn ?? 0)}
								unit="PED"
								comparison="manual markup uplift applied"
							/>
						</div>
					{/if}

					<Card class="p-4">
						<div class="flex flex-wrap gap-x-5 gap-y-2 text-sm">
							<div class="text-text-secondary">
								Sample: <span class="tabular-nums text-text">{prospectResult.sample.sessions}</span> sessions,
								<span class="tabular-nums text-text"> {prospectResult.sample.hours.toFixed(1)}h</span>,
								<span class="tabular-nums text-text"> {formatPed(prospectResult.sample.cycledPed)}</span> PED cycled
							</div>
							<div class="text-text-secondary">
								Loot rate: <span class="tabular-nums text-text">{formatPercent(prospectResult.sample.returnRate)}</span>
							</div>
							<div class="text-text-secondary">
								PES per 100 cycled: <span class="tabular-nums text-text">{(prospectResult.sample.pesPerPed * 100).toFixed(2)}</span>
							</div>
						</div>
					</Card>

					{#if prospectResult.warnings.length > 0}
						<Card class="p-4 border border-warning/30">
							<div class="space-y-1">
								{#each prospectResult.warnings as warning}
									<p class="text-sm text-warning">{warning}</p>
								{/each}
							</div>
						</Card>
					{/if}

					{#if prospectResult.rows.length > 0}
						<div class="overflow-x-auto">
							<table class="w-full text-sm">
								<thead>
									<tr class="border-b border-border">
										<th class="py-2 px-3 text-left eyebrow">Skill</th>
										<th class="py-2 px-3 text-right eyebrow">Weight</th>
										<th class="py-2 px-3 text-right eyebrow">Current</th>
										<th class="py-2 px-3 text-right eyebrow">Observed</th>
										<th class="py-2 px-3 text-right eyebrow">Projected Gain</th>
										<th class="py-2 px-3 text-right eyebrow">End Level</th>
										<th class="py-2 px-3 text-right eyebrow">Prof +Lv</th>
									</tr>
								</thead>
								<tbody>
									{#each prospectResult.rows as row}
										<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
											<td class="py-2.5 px-3 text-text">
												<div class="flex items-center gap-2">
													<span>{row.name}</span>
													{#if row.isAttribute}
														<Badge variant="neutral">Attribute</Badge>
													{/if}
													{#if !row.relevant}
														<Badge variant="neutral">Off-path</Badge>
													{/if}
												</div>
											</td>
											<td class="py-2.5 px-3 text-right tabular-nums">{row.weight > 0 ? row.weight : '—'}</td>
											<td class="py-2.5 px-3 text-right tabular-nums">{row.currentLevel.toFixed(2)}</td>
											<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">
												{#if row.isAttribute}
													{row.observedRate.toFixed(4)} lvl/PED
												{:else}
													{formatPercent(row.observedShare)}
												{/if}
											</td>
											<td class="py-2.5 px-3 text-right tabular-nums">{row.projectedGain.toFixed(2)}</td>
											<td class="py-2.5 px-3 text-right tabular-nums">{row.projectedEndLevel.toFixed(2)}</td>
											<td class="py-2.5 px-3 text-right tabular-nums {row.relevant ? 'text-accent font-medium' : 'text-text-tertiary'}">
												{row.professionContribution.toFixed(3)}
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
						<p class="text-xs text-text-tertiary">
							Baseline forecast uses tracked cycling versus loot TT only. Markup uplift is shown separately as a speculative adjustment.
						</p>
					{/if}
				{/if}
			{/if}
		</div>
	{/if}

	<!-- 5. Optimizer sub-tab -->
	{#if mainTab === 'optimizer'}
		<div class="space-y-4" data-guide-anchor="character-optimizer-area">
			<!-- Mode toggle: Profession / HP -->
			<SegmentedControl
				options={[
					{ id: 'profession', label: 'Profession' },
					{ id: 'hp', label: 'HP' }
				]}
				active={optimizerMode}
				onchange={(id) => {
					optimizerMode = id as 'profession' | 'hp';
					if (id === 'hp' && hpSkills.length === 0 && !hpLoading) loadHpOptimizer();
				}}
			/>

			{#if optimizerMode === 'profession'}
				<!-- Profession selector -->
				<div class="flex items-center gap-3">
					<label for="prof-select" class="text-sm text-text-secondary whitespace-nowrap">Profession</label>
					<Select
						id="prof-select"
						class="flex-1"
						bind:value={selectedProfession}
						onchange={() => { loadOptimizer(selectedProfession); pathResult = null; prospectResult = null; }}
					>
						<option value="">Select a profession...</option>
						{#each professions as prof}
							<option value={prof.name}>{prof.name} (Lv {prof.level.toFixed(2)})</option>
						{/each}
					</Select>
				</div>

				<!-- ── Path view ── -->
				<div class="flex items-center gap-3">
					<label for="path-target" class="text-sm text-text-secondary whitespace-nowrap">Target Level</label>
					<Input
						id="path-target"
						type="number"
						min="1"
						step="1"
						placeholder={optimizerProfLevel > 0 ? `Current: ${optimizerProfLevel.toFixed(2)}` : 'e.g. 50'}
						class="flex-1"
						bind:value={pathTargetInput}
						onkeydown={(e) => { if (e.key === 'Enter' && selectedProfession && pathTargetInput) loadPathOptimizer(); }}
					/>
					<Button
						onclick={loadPathOptimizer}
						disabled={pathLoading || !selectedProfession || !pathTargetInput || parseFloat(pathTargetInput) <= 0 || parseFloat(pathTargetInput) <= optimizerProfLevel}
					>
						{#snippet children()}Calculate{/snippet}
					</Button>
				</div>

				{#if !selectedProfession || !pathTargetInput}
					<p class="text-sm text-text-tertiary py-4 text-center">Select a profession and target level to see the cheapest skills to level.</p>
				{:else if pathLoading}
						<p class="text-sm text-text-tertiary py-4 text-center">Calculating optimal path...</p>
					{:else if pathResult}
						{#if pathResult.professionLevelsGained === 0}
							<p class="text-sm text-text-tertiary py-4 text-center">Already at or above target level.</p>
						{:else}
							<div class="flex items-baseline gap-3 text-sm">
								<span class="text-text-secondary">Level</span>
								<span class="text-text tabular-nums font-medium">{pathResult.currentLevel.toFixed(2)}</span>
								<span class="text-text-tertiary">→</span>
								<span class="text-accent tabular-nums font-medium">{pathResult.endLevel.toFixed(2)}</span>
								<span class="text-text-tertiary text-xs">(+{pathResult.professionLevelsGained.toFixed(2)} levels for {formatPed(pathResult.totalPed)} PED)</span>
							</div>

							{@const allocated = pathResult.allocations.filter(a => a.levelsToGain > 0)}
							{@const unallocated = pathResult.allocations.filter(a => a.levelsToGain === 0)}
							{#if allocated.length > 0}
								<div class="overflow-x-auto">
									<table class="w-full text-sm">
										<thead>
											<tr class="border-b border-border">
												<th class="py-2 px-3 text-left eyebrow">#</th>
												<th class="py-2 px-3 text-left eyebrow">Skill</th>
												<th class="py-2 px-3 text-right eyebrow">Weight</th>
												<th class="py-2 px-3 text-right eyebrow">Level</th>
												<th class="py-2 px-3 text-right eyebrow">+Levels</th>
												<th class="py-2 px-3 text-right eyebrow">New Level</th>
												<th class="py-2 px-3 text-right eyebrow">PES Cost</th>
												<th class="py-2 px-3 text-right eyebrow">%</th>
											</tr>
										</thead>
										<tbody>
											{#each allocated as alloc, i}
												<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
													<td class="py-2.5 px-3 text-text-tertiary tabular-nums">{i + 1}</td>
													<td class="py-2.5 px-3 text-text">{alloc.name}</td>
													<td class="py-2.5 px-3 text-right tabular-nums">{alloc.weight}</td>
													<td class="py-2.5 px-3 text-right tabular-nums">
														{#if alloc.currentLevel > 0}
															{alloc.currentLevel.toLocaleString()}
														{:else}
															<span class="text-text-tertiary">—</span>
														{/if}
													</td>
													<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">+{alloc.levelsToGain.toLocaleString()}</td>
													<td class="py-2.5 px-3 text-right tabular-nums">{alloc.newLevel.toLocaleString()}</td>
													<td class="py-2.5 px-3 text-right tabular-nums font-medium
														{i === 0 ? 'text-success' : i < 3 ? 'text-accent' : 'text-text'}">
														{formatPed(alloc.pedCost)}
													</td>
													<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">
														{pathResult.totalPed > 0 ? (alloc.pedCost / pathResult.totalPed * 100).toFixed(1) : '0.0'}%
													</td>
												</tr>
											{/each}
										</tbody>
									</table>
								</div>

								<p class="text-xs text-text-tertiary">Optimal allocation distributes skill gains to minimise total PED. Skills ranked by investment size.</p>
							{/if}

							{#if unallocated.length > 0 || pathResult.excluded.length > 0}
								<div class="pt-2">
									<p class="eyebrow mb-2">Not included in path</p>
									<div class="flex flex-wrap gap-2">
										{#each unallocated as skill}
											<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-xs">
												<span class="text-text">{skill.name}</span>
												<span class="text-text-tertiary">Lv {skill.currentLevel.toLocaleString()}</span>
												<span class="text-text-tertiary">wt {skill.weight}</span>
											</div>
										{/each}
										{#each pathResult.excluded as skill}
											<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-xs opacity-60">
												<span class="text-text">{skill.name}</span>
												<span class="text-text-tertiary">wt {skill.weight}</span>
												<span class="text-warning text-[10px]">{skill.reason}</span>
											</div>
										{/each}
									</div>
								</div>
							{/if}

							{#if pathResult.attributes.length > 0}
								<div class="pt-2">
									<p class="eyebrow mb-2">Attributes (if offered as a reward)</p>
									<div class="flex flex-wrap gap-2">
										{#each pathResult.attributes as attr}
											<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-xs">
												<span class="text-text">{attr.name}</span>
												<span class="text-text-tertiary">Lv {attr.currentLevel}</span>
												<span class="text-accent tabular-nums font-medium">×{attr.contributionFactor}</span>
											</div>
										{/each}
									</div>
									<p class="text-xs text-text-tertiary mt-1.5">Contribution factor = weight × 20. Pick the highest when choosing an attribute reward.</p>
								</div>
							{/if}
						{/if}
					{/if}

			{:else}
				<!-- HP optimizer mode -->
				{#if hpLoading}
					<p class="text-sm text-text-tertiary py-4 text-center">Loading...</p>
				{:else if hpSkills.length > 0}
					<div class="flex items-baseline gap-3 text-sm">
						<span class="text-text-secondary">Current HP</span>
						<span class="text-text tabular-nums font-medium">{hpCurrent.toFixed(1)}</span>
						<span class="text-text-tertiary text-xs">({hpSkills.length} contributing skills)</span>
					</div>

					<div class="overflow-x-auto">
						<table class="w-full text-sm">
							<thead>
								<tr class="border-b border-border">
									<th class="py-2 px-3 text-left eyebrow">#</th>
									<th class="py-2 px-3 text-left eyebrow">Skill</th>
									<th class="py-2 px-3 text-right eyebrow">Level</th>
									<th class="py-2 px-3 text-right eyebrow">Lvl / HP</th>
									<th class="py-2 px-3 text-right eyebrow">PES / HP</th>
									<th class="py-2 px-3 text-right eyebrow">HP / PES</th>
								</tr>
							</thead>
							<tbody>
								{#each hpSkills as skill, i}
									<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
										<td class="py-2.5 px-3 text-text-tertiary tabular-nums">{i + 1}</td>
										<td class="py-2.5 px-3 text-text">{skill.name}</td>
										<td class="py-2.5 px-3 text-right tabular-nums">
											{#if skill.currentLevel > 0}
												{skill.currentLevel.toLocaleString()}
											{:else}
												<span class="text-text-tertiary">—</span>
											{/if}
										</td>
										<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{skill.levelsPerHp.toLocaleString()}</td>
										<td class="py-2.5 px-3 text-right tabular-nums font-medium
											{i === 0 ? 'text-success' : i < 3 ? 'text-accent' : 'text-text'}">
											{formatPed(skill.pedPerHp)}
										</td>
										<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{skill.hpPerPed.toFixed(4)}</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>

					<p class="text-xs text-text-tertiary"><strong>PES / HP</strong> = cost of gaining +1 HP by levelling this skill alone from your current level. <strong>HP / PES</strong> = HP gained per 1 PES of skill. Lower cost is better.</p>

					<!-- Attributes -->
					{#if hpAttributes.length > 0}
						<div class="pt-2">
							<p class="eyebrow mb-2">Attributes (if offered as a reward)</p>
							<div class="flex flex-wrap gap-2">
								{#each hpAttributes as attr}
									<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-xs">
										<span class="text-text">{attr.name}</span>
										<span class="text-text-tertiary">Lv {attr.currentLevel}</span>
										<span class="text-accent tabular-nums font-medium">{attr.levelsPerHp} lvl/HP</span>
									</div>
								{/each}
							</div>
							<p class="text-xs text-text-tertiary mt-1.5">Levels per HP accounts for the ×20 attribute multiplier. Pick the lowest when choosing an attribute reward for HP.</p>
						</div>
					{/if}
				{:else}
					<p class="text-sm text-text-tertiary py-4 text-center">No skill data available. Scan your skills first.</p>
				{/if}
			{/if}
		</div>
	{/if}

	{#if mainTab === 'codex'}
		<CodexTab seedActive={demoCodexSeedActive} />
	{/if}
</div>
