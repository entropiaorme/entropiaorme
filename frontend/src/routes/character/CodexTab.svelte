<script lang="ts">
	import type {
		CodexSpecies,
		CodexRankBreakdown,
		CodexRankItem,
		CodexSkillOption,
		CodexMetaAttribute,
		ProfessionLevel,
	} from '$lib/types/analytics';
	import {
		getCodexSpecies,
		getCodexSpeciesRanks,
		claimCodexRank,
		unclaimCodexRank,
		calibrateCodex,
		getCodexRecommendation,
		getCharacterProfessions,
		getCodexMetaAttributes,
		claimCodexMeta,
	} from '$lib/api';
	import { formatPed } from '$lib/utils/format';
	import Card from '$lib/components/Card.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import Select from '$lib/components/Select.svelte';
	import { guideState } from '$lib/guide/state.svelte';
	import {
		characterDemoProfessions,
		characterDemoCodexSpecies,
		characterDemoCodexRankBreakdown,
		characterDemoCodexSkillOptions,
		characterDemoCodexSelectedSpecies,
		characterDemoCodexSelectedProfession,
	} from '$lib/guide/fixtures/character';

	let { seedActive = false } = $props<{ seedActive?: boolean }>();

	const PAGE_SIZE = 20;
	const HP_GAIN_OPTION = '__hp__';

	// ── Top-level mode ──────────────────────────────────────────────────────────

	let codexMode = $state<'mobs' | 'meta'>('mobs');

	// ── Data state ──────────────────────────────────────────────────────────────

	let species = $state([] as CodexSpecies[]);
	let professions = $state([] as ProfessionLevel[]);
	let loading = $state(true);

	// ── Meta state ──────────────────────────────────────────────────────────────

	let metaAttributes = $state([] as CodexMetaAttribute[]);
	let metaLoading = $state(false);
	let metaClaimMessage = $state<string | null>(null);

	// ── Controls ────────────────────────────────────────────────────────────────

	let search = $state('');
	let selectedProfession = $state('');
	let calibrateMode = $state(false);
	let page = $state(0);

	// ── Selected species (right panel) ──────────────────────────────────────────

	let selectedSpecies = $state<string | null>(null);
	let rankBreakdown = $state<CodexRankBreakdown | null>(null);
	let skillOptions = $state([] as CodexSkillOption[]);
	let panelLoading = $state(false);
	let claimMessage = $state<string | null>(null);

	// ── Derived: next rank data for the selected species ────────────────────────

	let nextRankData = $derived.by(() => {
		if (!rankBreakdown) return null;
		const next = rankBreakdown.ranks.find(r => r.isNext);
		return next ?? null;
	});

	let isHpMode = $derived(selectedProfession === HP_GAIN_OPTION);

	// ── Load on mount ───────────────────────────────────────────────────────────

	$effect(() => {
		loadData();
	});

	async function loadData() {
		if (guideState.isActive) {
			species = characterDemoCodexSpecies;
			professions = characterDemoProfessions;
			loading = false;
			return;
		}
		try {
			const [sp, pr] = await Promise.all([
				getCodexSpecies(),
				getCharacterProfessions(),
			]);
			species = sp;
			professions = pr;
		} catch {
			// Backend not reachable
		} finally {
			loading = false;
		}
	}

	// Seed-active reactive effect: when the parent flips the codex-seed flag on,
	// pre-select the demo species + profession + rank breakdown so the recommendation
	// panel is fully populated for the guide card.
	$effect(() => {
		if (seedActive) {
			selectedSpecies = characterDemoCodexSelectedSpecies;
			selectedProfession = characterDemoCodexSelectedProfession;
			rankBreakdown = characterDemoCodexRankBreakdown;
			skillOptions = characterDemoCodexSkillOptions;
			panelLoading = false;
			loading = false;
		} else if (selectedSpecies === characterDemoCodexSelectedSpecies) {
			selectedSpecies = null;
			selectedProfession = '';
			rankBreakdown = null;
			skillOptions = [];
		}
	});

	// ── Meta functions ──────────────────────────────────────────────────────────

	async function loadMeta() {
		metaLoading = true;
		metaClaimMessage = null;
		try {
			metaAttributes = await getCodexMetaAttributes();
		} catch {
			metaAttributes = [];
		} finally {
			metaLoading = false;
		}
	}

	async function handleMetaClaim(attributeName: string) {
		try {
			const result = await claimCodexMeta(attributeName);
			metaClaimMessage = `Claimed! ${result.attributeName} +${formatPed(result.pedValue)} PES`;
			metaAttributes = await getCodexMetaAttributes();
		} catch (err: any) {
			metaClaimMessage = `Error: ${err.message}`;
		}
	}

	$effect(() => {
		if (codexMode === 'meta' && metaAttributes.length === 0) {
			loadMeta();
		}
	});

	// ── Filtering & pagination ──────────────────────────────────────────────────

	let filtered = $derived.by(() => {
		const q = search.toLowerCase();
		if (!q) return species;
		return species.filter(s => s.name.toLowerCase().includes(q));
	});

	let totalPages = $derived(Math.max(1, Math.ceil(filtered.length / PAGE_SIZE)));
	let pageRows = $derived(filtered.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE));

	$effect(() => { search; page = 0; });

	// ── Category display helpers ────────────────────────────────────────────────

	const categoryLabel: Record<string, string> = {
		cat1: 'Cat 1',
		cat2: 'Cat 2',
		cat3: 'Cat 3',
		cat4: 'Cat 4',
	};

	const categoryVariant: Record<string, 'accent' | 'positive' | 'warning' | 'negative' | 'neutral'> = {
		cat1: 'accent',
		cat2: 'positive',
		cat3: 'warning',
		cat4: 'negative',
	};

	function getRecommendationRequest() {
		if (selectedProfession === HP_GAIN_OPTION) {
			return { target: 'hp' as const };
		}
		if (selectedProfession) {
			return {
				target: 'profession' as const,
				profession: selectedProfession,
			};
		}
		return undefined;
	}

	async function loadRecommendations(speciesName: string, rank: number) {
		if (guideState.isActive) return;
		skillOptions = await getCodexRecommendation(
			speciesName,
			rank,
			getRecommendationRequest(),
		);
	}

	// ── Select species → load detail + auto-select next rank ────────────────────

	async function selectSpecies(name: string) {
		if (guideState.isActive) return;
		if (selectedSpecies === name) {
			selectedSpecies = null;
			rankBreakdown = null;
			skillOptions = [];
			claimMessage = null;
			return;
		}
		selectedSpecies = name;
		panelLoading = true;
		skillOptions = [];
		claimMessage = null;
		try {
			rankBreakdown = await getCodexSpeciesRanks(name);
			// Auto-load skill options for the next rank
			const nextRank = rankBreakdown?.ranks.find(r => r.isNext);
			if (nextRank) {
				await loadRecommendations(name, nextRank.rank);
			}
		} catch {
			rankBreakdown = null;
		} finally {
			panelLoading = false;
		}
	}

	// ── Claim ───────────────────────────────────────────────────────────────────

	async function handleClaim(skillName: string) {
		if (guideState.isActive) return;
		if (!selectedSpecies || !nextRankData) return;
		try {
			const result = await claimCodexRank(selectedSpecies, nextRankData.rank, skillName);
			claimMessage = `Claimed! ${result.skillName} +${formatPed(result.pedValue)} PES`;
			// Refresh
			rankBreakdown = await getCodexSpeciesRanks(selectedSpecies);
			species = await getCodexSpecies();
			// Load new next rank options
			const newNext = rankBreakdown?.ranks.find(r => r.isNext);
			if (newNext) {
				await loadRecommendations(selectedSpecies, newNext.rank);
			} else {
				skillOptions = [];
			}
		} catch (err: any) {
			claimMessage = `Error: ${err.message}`;
		}
	}

	// ── Unclaim (undo the most recent claim) ─────────────────────────────────────

	async function handleUnclaim() {
		if (guideState.isActive) return;
		if (!selectedSpecies) return;
		try {
			const result = await unclaimCodexRank(selectedSpecies);
			claimMessage = `Undid rank ${result.rank}: ${result.skillName}`;
			// Refresh
			rankBreakdown = await getCodexSpeciesRanks(selectedSpecies);
			species = await getCodexSpecies();
			const newNext = rankBreakdown?.ranks.find(r => r.isNext);
			if (newNext) {
				await loadRecommendations(selectedSpecies, newNext.rank);
			} else {
				skillOptions = [];
			}
		} catch (err: any) {
			claimMessage = `Error: ${err.message}`;
		}
	}

	// ── Calibrate ───────────────────────────────────────────────────────────────

	async function handleCalibrate(speciesName: string, delta: number) {
		if (guideState.isActive) return;
		const sp = species.find(s => s.name === speciesName);
		if (!sp) return;
		const newRank = Math.max(0, Math.min(25, sp.currentRank + delta));
		await calibrateCodex(speciesName, newRank);
		species = await getCodexSpecies();
		if (selectedSpecies === speciesName) {
			rankBreakdown = await getCodexSpeciesRanks(speciesName);
			const nextRank = rankBreakdown?.ranks.find(r => r.isNext);
			if (nextRank) {
				await loadRecommendations(speciesName, nextRank.rank);
			} else {
				skillOptions = [];
			}
		}
	}

	// ── Reload skill options when profession changes ────────────────────────────

	async function onProfessionChange() {
		if (selectedSpecies && nextRankData) {
			await loadRecommendations(selectedSpecies, nextRankData.rank);
		}
	}
</script>

<div class="space-y-3">
	<!-- Top bar: Mode toggle + Search + Profession + Calibrate -->
	<div class="flex items-center gap-3">
		<div class="flex items-center gap-1 bg-surface rounded-md p-0.5 shrink-0">
			<button
				class="px-3 py-1.5 text-xs font-medium rounded transition-colors cursor-pointer
					{codexMode === 'mobs' ? 'bg-surface-hover text-text' : 'text-text-secondary hover:text-text'}"
				onclick={() => codexMode = 'mobs'}
			>Mobs</button>
			<button
				class="px-3 py-1.5 text-xs font-medium rounded transition-colors cursor-pointer
					{codexMode === 'meta' ? 'bg-surface-hover text-text' : 'text-text-secondary hover:text-text'}"
				onclick={() => codexMode = 'meta'}
			>Meta</button>
		</div>

		{#if codexMode === 'mobs'}
			<SearchInput bind:value={search} placeholder="Search species..." class="flex-1" />

			<Select
				bind:value={selectedProfession}
				onchange={onProfessionChange}
				data-guide-anchor="character-codex-profession-select"
			>
				<option value="">No profession</option>
				<option value={HP_GAIN_OPTION}>HP gain</option>
				{#each professions as prof}
					<option value={prof.name}>{prof.name}</option>
			{/each}
			</Select>

			<button
				class="px-3 py-1.5 text-xs font-medium rounded-md transition-colors cursor-pointer whitespace-nowrap
					{calibrateMode ? 'bg-warning/20 text-warning' : 'text-text-secondary hover:text-text bg-surface-hover'}"
				onclick={() => calibrateMode = !calibrateMode}
			>
				{calibrateMode ? 'Done' : 'Calibrate'}
			</button>
		{/if}
	</div>

	{#if codexMode === 'meta'}
		<!-- ═══ Meta codex view ═══ -->
		<Card class="p-4 space-y-4">
			<div>
				<h3 class="text-sm font-medium text-text">Meta Codex Reward</h3>
			</div>

			{#if metaLoading}
				<p class="text-sm text-text-tertiary py-4 text-center">Loading...</p>
			{:else}
				<div class="space-y-1">
					{#each metaAttributes as attr}
						<div class="flex items-center justify-between py-2 px-3 rounded hover:bg-surface-hover/50 transition-colors group">
							<div class="flex items-center gap-3">
								<span class="text-sm font-medium text-text w-24">{attr.name}</span>
								{#if attr.currentLevel != null}
									<span class="text-xs text-text-secondary tabular-nums">Lv {attr.currentLevel.toFixed(1)}</span>
								{:else}
									<span class="text-xs text-text-tertiary">Not scanned</span>
								{/if}
							</div>
							<button
								class="px-3 py-1 text-xs font-medium text-accent bg-accent/10 hover:bg-accent/20 rounded transition-colors cursor-pointer opacity-0 group-hover:opacity-100"
								onclick={() => handleMetaClaim(attr.name)}
							>Claim 1 PES</button>
						</div>
					{/each}
				</div>
			{/if}

			{#if metaClaimMessage}
				<div class="text-sm text-center {metaClaimMessage.startsWith('Error') ? 'text-negative' : 'text-positive'}">
					{metaClaimMessage}
				</div>
			{/if}
		</Card>

	{:else}
		<!-- ═══ Mobs codex view ═══ -->

		<!-- Side-by-side layout: species list | detail panel -->
	<div class="flex gap-4 items-stretch">

		<!-- Left: Species list (sizes naturally to content) -->
		<div class="w-64 shrink-0 flex flex-col">
			<div class="border border-border rounded-md">
				{#if loading}
					<div class="py-8 text-center text-text-tertiary text-sm">Loading...</div>
				{:else if pageRows.length === 0}
					<div class="py-8 text-center text-text-tertiary text-sm">No species found</div>
				{:else}
					{#each pageRows as sp}
						<div
							class="flex items-center justify-between px-3 py-2 border-b border-border/30 transition-colors
								{selectedSpecies === sp.name ? 'bg-accent/10 text-accent' : 'hover:bg-surface-hover/50 text-text'}
								{calibrateMode ? '' : 'cursor-pointer'}"
							role="button"
							tabindex="0"
							onclick={() => { if (!calibrateMode) selectSpecies(sp.name); }}
							onkeydown={(e) => { if (!calibrateMode && (e.key === 'Enter' || e.key === ' ')) selectSpecies(sp.name); }}
						>
							<span class="text-sm truncate mr-2">{sp.name}</span>
							<div class="flex items-center gap-1 shrink-0">
								{#if calibrateMode}
									<button
										class="w-5 h-5 flex items-center justify-center rounded text-text-secondary hover:text-text hover:bg-surface-hover transition-colors cursor-pointer disabled:opacity-30 disabled:cursor-default text-xs"
										disabled={sp.currentRank <= 0}
										onclick={(e) => { e.stopPropagation(); handleCalibrate(sp.name, -1); }}
									>&minus;</button>
									<span class="text-xs tabular-nums text-text-secondary w-5 text-center">{sp.currentRank}</span>
									<button
										class="w-5 h-5 flex items-center justify-center rounded text-text-secondary hover:text-text hover:bg-surface-hover transition-colors cursor-pointer disabled:opacity-30 disabled:cursor-default text-xs"
										disabled={sp.currentRank >= 25}
										onclick={(e) => { e.stopPropagation(); handleCalibrate(sp.name, 1); }}
									>+</button>
								{:else}
									<span class="text-xs tabular-nums {sp.currentRank >= 25 ? 'text-positive' : sp.currentRank > 0 ? 'text-text-secondary' : 'text-text-tertiary/50'}">
										{sp.currentRank}/25
									</span>
								{/if}
							</div>
						</div>
					{/each}
				{/if}
			</div>
			<div class="h-0 w-full" data-guide-anchor="character-codex-mobs-list-placement"></div>

			<!-- Pagination -->
			{#if totalPages > 1}
				<div class="flex items-center justify-between mt-2 px-1">
					<span class="text-xs text-text-tertiary tabular-nums">
						{page * PAGE_SIZE + 1}–{Math.min((page + 1) * PAGE_SIZE, filtered.length)} / {filtered.length}
					</span>
					<div class="flex items-center gap-1">
						<button
							class="px-1.5 py-0.5 text-xs rounded transition-colors cursor-pointer
								{page > 0 ? 'text-text-secondary hover:text-text hover:bg-surface-hover' : 'text-text-tertiary/50 cursor-default'}"
							disabled={page === 0}
							onclick={() => page--}
						>&lsaquo;</button>
						<button
							class="px-1.5 py-0.5 text-xs rounded transition-colors cursor-pointer
								{page < totalPages - 1 ? 'text-text-secondary hover:text-text hover:bg-surface-hover' : 'text-text-tertiary/50 cursor-default'}"
							disabled={page >= totalPages - 1}
							onclick={() => page++}
						>&rsaquo;</button>
					</div>
				</div>
			{/if}
		</div>

		<!-- Right: Detail panel -->
		<div class="flex-1 min-w-0 overflow-y-auto border border-border rounded-md" data-guide-anchor="character-codex-recommendation">
			{#if !selectedSpecies}
				<div class="h-full flex items-center justify-center">
					<p class="text-sm text-text-tertiary">Select a species to view codex details</p>
				</div>
			{:else if panelLoading}
				<div class="h-full flex items-center justify-center">
					<p class="text-sm text-text-tertiary">Loading...</p>
				</div>
			{:else if rankBreakdown}
				<div class="p-4 space-y-4">
					<!-- Species header -->
					<div class="flex items-baseline justify-between">
						<h3 class="text-base font-semibold text-text">{rankBreakdown.speciesName}</h3>
						<span class="text-sm text-text-secondary tabular-nums">
							Rank {rankBreakdown.currentRank} / 25
						</span>
					</div>

					{#if rankBreakdown.currentRank >= 25}
						<Card class="p-4">
							<p class="text-sm text-positive font-medium">Codex complete</p>
						</Card>
					{:else if nextRankData}
						<!-- Next rank claim card -->
						<Card class="p-4 space-y-3">
							<div class="flex items-center justify-between">
								<div class="flex items-center gap-2">
									<span class="text-sm font-medium text-text">Rank {nextRankData.rank}</span>
									<Badge variant={categoryVariant[nextRankData.category] ?? 'neutral'}>
										{categoryLabel[nextRankData.category] ?? nextRankData.category}
									</Badge>
									{#if nextRankData.cat4Bonus}
										<Badge variant="negative">+ Cat 4</Badge>
									{/if}
								</div>
								<span class="text-sm tabular-nums text-text font-medium">
									{formatPed(nextRankData.rewardPed)} PES
								</span>
							</div>

							{#if nextRankData.claimed}
								<div class="text-sm text-positive">
									Claimed: {nextRankData.claimedSkill} ({formatPed(nextRankData.claimedPed ?? 0)} PES)
								</div>
							{:else}
								<!-- Skill options list -->
								<div class="space-y-0.5">
									{#each skillOptions as opt}
										{@const rank = opt.recommendRank}
										<div class="flex items-center justify-between py-1.5 px-2 rounded hover:bg-surface-hover/50 transition-colors group">
											<div class="flex items-center gap-2 min-w-0">
												{#if rank != null}
													<span class="text-xs font-medium tabular-nums w-5 text-center shrink-0
														{rank === 1 ? 'text-success' : rank <= 3 ? 'text-accent' : 'text-text-tertiary'}">
														#{rank}
													</span>
												{:else}
													<span class="w-5 shrink-0"></span>
												{/if}
												<span class="text-sm text-text truncate">{opt.skillName}</span>
											</div>
											<div class="flex items-center gap-3 shrink-0 ml-2">
												{#if isHpMode}
													<div class="text-right text-xs tabular-nums">
														{#if opt.hpIncrease != null}
															<span class="text-text-secondary">+{opt.levelsGained.toFixed(1)} lvl</span>
															<span class="text-text-tertiary mx-0.5">/</span>
															<span class="text-text-secondary">{opt.hpIncrease.toFixed(0)} lvl/HP</span>
															<span class="text-accent font-medium ml-1">= +{opt.hpGain.toFixed(3)} HP</span>
														{:else}
															<span class="text-text-tertiary">No HP gain</span>
														{/if}
													</div>
												{:else if opt.professionWeight > 0}
													<div class="text-right text-xs tabular-nums">
														<span class="text-text-secondary">+{opt.levelsGained.toFixed(1)} lvl</span>
														<span class="text-text-tertiary mx-0.5">&times;</span>
														<span class="text-text-secondary">w{opt.professionWeight}</span>
														{#if opt.profContribution > 0}
															<span class="text-accent font-medium ml-1">= +{(opt.profContribution * 100).toFixed(3)}%</span>
														{/if}
													</div>
												{:else if opt.currentLevel != null}
													<span class="text-xs text-text-tertiary tabular-nums">Lv {opt.currentLevel.toFixed(0)}, +{opt.levelsGained.toFixed(1)}</span>
												{/if}
												<button
													class="px-2 py-1 text-xs font-medium text-accent bg-accent/10 hover:bg-accent/20 rounded transition-colors cursor-pointer opacity-0 group-hover:opacity-100"
													onclick={() => handleClaim(opt.skillName)}
												>Claim</button>
											</div>
										</div>
									{/each}
								</div>

								<p class="text-xs text-text-tertiary">
									{#if isHpMode && skillOptions.some(o => o.recommendRank === 1)}
										Ranked by expected HP gain from this codex reward at your current level. HP gain uses the skill's HP increase stat: every N skill levels adds 1 HP.
									{:else if selectedProfession && skillOptions.some(o => o.recommendRank === 1)}
										Ranked by profession contribution: +levels &times; weight. Accounts for diminishing returns at higher skill levels.
									{/if}
								</p>
							{/if}
						</Card>

						<!-- Claim message -->
						{#if claimMessage}
							<div class="text-sm text-center {claimMessage.startsWith('Error') ? 'text-negative' : 'text-positive'}">
								{claimMessage}
							</div>
						{/if}
					{/if}

					<!-- Rank history (compact) -->
					{#if rankBreakdown.currentRank > 0}
						<div>
							<p class="text-xs text-text-tertiary font-medium uppercase tracking-wide mb-2">Claimed ranks</p>
							<div class="flex flex-wrap gap-1">
								{#each rankBreakdown.ranks.filter(r => r.claimed) as r}
									<div class="flex items-center gap-1.5 bg-surface-hover/50 rounded px-2 py-1 text-xs">
										<span class="text-text-secondary tabular-nums">{r.rank}.</span>
										<span class="text-text">{r.claimedSkill}</span>
										<span class="text-text-tertiary tabular-nums">{formatPed(r.claimedPed ?? 0)}</span>
										{#if r.rank === rankBreakdown.currentRank}
											<button
												class="ml-0.5 leading-none text-text-tertiary hover:text-negative transition-colors cursor-pointer"
												title="Undo this claim"
												aria-label="Undo rank {r.rank} claim"
												onclick={handleUnclaim}
											>&times;</button>
										{/if}
									</div>
								{/each}
							</div>
						</div>
					{/if}
				</div>
			{/if}
		</div>
	</div>
	{/if}
</div>
