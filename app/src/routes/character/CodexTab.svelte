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
		getCodexMasteryOptions,
		claimCodexMastery,
		unclaimCodexMastery,
		getCharacterProfessions,
		getCodexMetaAttributes,
		claimCodexMeta,
	} from '$lib/api';
	import { formatPed, formatPedHalfEven } from '$lib/utils/format';
	import Card from '$lib/components/Card.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import { IconStar } from '$lib/icons';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import CodexProfessionPicker from '$lib/features/character/CodexProfessionPicker.svelte';
	import CodexSkillOptionList from '$lib/features/character/CodexSkillOptionList.svelte';
	import {
		targetProfessions,
		type CodexRankingTarget,
	} from '$lib/features/character/codexRankingTarget';
	import { createTableModel } from '$lib/view/tableModel.svelte';

	const PAGE_SIZE = 20;

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

	let rankingTarget = $state<CodexRankingTarget>({ kind: 'none' });
	let calibrateMode = $state(false);

	// ── Selected species (right panel) ──────────────────────────────────────────

	let selectedSpecies = $state<string | null>(null);
	let rankBreakdown = $state<CodexRankBreakdown | null>(null);
	let skillOptions = $state([] as CodexSkillOption[]);
	let masteryOptions = $state([] as CodexSkillOption[]);
	let panelLoading = $state(false);
	let claimMessage = $state<string | null>(null);

	// Transient per-row confirmation for mastery claims: unlike a rank
	// claim (whose whole card advances to the next rank), a mastery
	// claim leaves the same panel on screen, so the clicked row itself
	// must acknowledge the claim.
	let justClaimedSkill = $state<string | null>(null);
	let justClaimedTimer: ReturnType<typeof setTimeout> | undefined;
	function flashClaimed(skillName: string) {
		justClaimedSkill = skillName;
		clearTimeout(justClaimedTimer);
		justClaimedTimer = setTimeout(() => (justClaimedSkill = null), 2000);
	}

	// ── Derived: next rank data for the selected species ────────────────────────

	let nextRankData = $derived.by(() => {
		if (!rankBreakdown) return null;
		const next = rankBreakdown.ranks.find(r => r.isNext);
		return next ?? null;
	});

	let rankedBy = $derived<'hp' | 'profession' | null>(
		rankingTarget.kind === 'hp'
			? 'hp'
			: targetProfessions(rankingTarget).length > 0
				? 'profession'
				: null,
	);
	let familyTarget = $derived(rankingTarget.kind === 'family');

	// ── Load on mount ───────────────────────────────────────────────────────────

	$effect(() => {
		loadData();
	});

	async function loadData() {
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

	const table = createTableModel<CodexSpecies>({
		rows: () => species,
		pageSize: PAGE_SIZE,
		searchText: (s) => [s.name],
	});

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
		if (rankingTarget.kind === 'hp') {
			return { target: 'hp' as const };
		}
		const professions = targetProfessions(rankingTarget);
		if (professions.length > 0) {
			return { target: 'profession' as const, professions };
		}
		return undefined;
	}

	async function loadRecommendations(speciesName: string, rank: number) {
		skillOptions = await getCodexRecommendation(
			speciesName,
			rank,
			getRecommendationRequest(),
		);
	}

	async function loadMasteryOptions() {
		masteryOptions = await getCodexMasteryOptions(getRecommendationRequest());
	}

	/** Refresh the selected species' panel and load whichever option
	 *  list its rank calls for (the next rank's, or mastery past 25). */
	async function refreshPanel(speciesName: string) {
		rankBreakdown = await getCodexSpeciesRanks(speciesName);
		const nextRank = rankBreakdown?.ranks.find(r => r.isNext);
		if (nextRank) {
			await loadRecommendations(speciesName, nextRank.rank);
		} else {
			skillOptions = [];
			if (rankBreakdown && rankBreakdown.currentRank >= 25) {
				await loadMasteryOptions();
			}
		}
	}

	// ── Select species → load detail + auto-select next rank ────────────────────

	async function selectSpecies(name: string) {
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
		masteryOptions = [];
		claimMessage = null;
		try {
			await refreshPanel(name);
		} catch {
			rankBreakdown = null;
		} finally {
			panelLoading = false;
		}
	}

	// ── Claim / unclaim (ranks and mastery) ─────────────────────────────────────

	/** Shared claim-action shell: selection guard, success message,
	 *  species reload, panel refresh, and error feedback. */
	async function runClaimAction(action: (speciesName: string) => Promise<string>) {
		if (!selectedSpecies) return;
		const speciesName = selectedSpecies;
		try {
			claimMessage = await action(speciesName);
			species = await getCodexSpecies();
			await refreshPanel(speciesName);
		} catch (err: any) {
			claimMessage = `Error: ${err.message}`;
		}
	}

	async function handleClaim(skillName: string) {
		if (!nextRankData) return;
		const rank = nextRankData.rank;
		await runClaimAction(async (speciesName) => {
			const result = await claimCodexRank(speciesName, rank, skillName);
			return `Claimed! ${result.skillName} +${formatPed(result.pedValue)} PES`;
		});
	}

	async function handleUnclaim() {
		await runClaimAction(async (speciesName) => {
			const result = await unclaimCodexRank(speciesName);
			return `Undid rank ${result.rank}: ${result.skillName}`;
		});
	}

	async function handleMasteryClaim(skillName: string) {
		await runClaimAction(async (speciesName) => {
			const result = await claimCodexMastery(speciesName, skillName);
			flashClaimed(result.skillName);
			return `Mastery ${result.masteryLevel} claimed! ${result.skillName} +${formatPedHalfEven(result.pedValue)} PES`;
		});
	}

	async function handleMasteryUnclaim() {
		await runClaimAction(async (speciesName) => {
			const result = await unclaimCodexMastery(speciesName);
			return `Undid mastery ${result.masteryLevel}: ${result.skillName}`;
		});
	}

	// ── Calibrate ───────────────────────────────────────────────────────────────

	async function handleCalibrate(speciesName: string, delta: number) {
		const sp = species.find(s => s.name === speciesName);
		if (!sp) return;
		const newRank = Math.max(0, Math.min(25, sp.currentRank + delta));
		try {
			await calibrateCodex(speciesName, newRank);
			species = await getCodexSpecies();
			if (selectedSpecies === speciesName) {
				await refreshPanel(speciesName);
			}
		} catch (err: any) {
			claimMessage = `Error: ${err.message}`;
		}
	}

	// ── Reload skill options when the ranking target changes ───────────────────

	async function onTargetChange(target: CodexRankingTarget) {
		rankingTarget = target;
		if (!selectedSpecies) return;
		try {
			if (nextRankData) {
				await loadRecommendations(selectedSpecies, nextRankData.rank);
			} else if (rankBreakdown && rankBreakdown.currentRank >= 25) {
				await loadMasteryOptions();
			}
		} catch (err: any) {
			claimMessage = `Error: ${err.message}`;
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
			<SearchInput bind:value={table.search} placeholder="Search species..." class="flex-1" />

			<span class="text-xs text-text-secondary whitespace-nowrap shrink-0">Codex Optimiser:</span>
			<CodexProfessionPicker
				professions={professions.map(prof => prof.name)}
				target={rankingTarget}
				onchange={onTargetChange}
			/>

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
								class="px-3 py-1 text-xs font-medium text-accent bg-accent/10 hover:bg-accent/20 rounded transition-colors cursor-pointer opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
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
				{:else if table.pageRows.length === 0}
					<div class="py-8 text-center text-text-tertiary text-sm">No species found</div>
				{:else}
					{#each table.pageRows as sp}
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
								{:else if sp.currentRank >= 25 && sp.masteryLevel > 0}
									<span class="text-xs tabular-nums text-positive" title="Mastery level {sp.masteryLevel}">
										<IconStar /> {sp.masteryLevel}
									</span>
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
			<div class="h-0 w-full"></div>

			<!-- Pagination -->
			{#if table.totalPages > 1}
				<div class="flex items-center justify-between mt-2 px-1">
					<span class="text-xs text-text-tertiary tabular-nums">
						{table.page * PAGE_SIZE + 1}{'\u2013'}{Math.min((table.page + 1) * PAGE_SIZE, table.filtered.length)} / {table.filtered.length}
					</span>
					<div class="flex items-center gap-1">
						<button
							class="px-1.5 py-0.5 text-xs rounded transition-colors cursor-pointer
								{table.page > 0 ? 'text-text-secondary hover:text-text hover:bg-surface-hover' : 'text-text-tertiary/50 cursor-default'}"
							disabled={table.page === 0}
							onclick={() => table.page--}
						>&lsaquo;</button>
						<button
							class="px-1.5 py-0.5 text-xs rounded transition-colors cursor-pointer
								{table.page < table.totalPages - 1 ? 'text-text-secondary hover:text-text hover:bg-surface-hover' : 'text-text-tertiary/50 cursor-default'}"
							disabled={table.page >= table.totalPages - 1}
							onclick={() => table.page++}
						>&rsaquo;</button>
					</div>
				</div>
			{/if}
		</div>

		<!-- Right: Detail panel -->
		<div class="flex-1 min-w-0 overflow-y-auto border border-border rounded-md">
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
						<Card class="p-4 space-y-3">
							<div class="flex items-center gap-2">
								<span class="text-sm font-medium text-text">Mastery</span>
								<span class="text-sm font-medium text-positive tabular-nums">
									<IconStar /> x {rankBreakdown.masteryLevel}
								</span>
							</div>

							<CodexSkillOptionList
								options={masteryOptions}
								rankedBy={rankedBy}
								onClaim={handleMasteryClaim}
								mastery
								family={familyTarget}
								{justClaimedSkill}
							/>

							<p class="text-xs text-text-tertiary/70">
								Mastery choices and values reflect limited in-game observation and may
								vary by creature; if the game offers different options, please report
								it on the project's GitHub issues.
							</p>
						</Card>

						{#if claimMessage}
							<div class="text-sm text-center {claimMessage.startsWith('Error') ? 'text-negative' : 'text-positive'}">
								{claimMessage}
							</div>
						{/if}
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
								<CodexSkillOptionList
									options={skillOptions}
									rankedBy={rankedBy}
									onClaim={handleClaim}
									family={familyTarget}
								/>
							{/if}
						</Card>

						<!-- Claim message -->
						{#if claimMessage}
							<div class="text-sm text-center {claimMessage.startsWith('Error') ? 'text-negative' : 'text-positive'}">
								{claimMessage}
							</div>
						{/if}
					{/if}

					<!-- Mastery history (compact); the latest claim carries the undo,
					     mirroring the rank history below. -->
					{#if rankBreakdown.masteryClaims.length > 0}
						<div>
							<p class="text-xs text-text-tertiary font-medium uppercase tracking-wide mb-2">Mastery claims</p>
							<div class="flex flex-wrap gap-1">
								{#each rankBreakdown.masteryClaims as claim}
									<div class="flex items-center gap-1.5 bg-surface-hover/50 rounded px-2 py-1 text-xs">
										<span class="text-text-secondary tabular-nums"><IconStar /> {claim.masteryLevel}.</span>
										<span class="text-text">{claim.skillName}</span>
										<span class="text-text-tertiary tabular-nums">{formatPedHalfEven(claim.pedValue)}</span>
										{#if claim.masteryLevel === rankBreakdown.masteryLevel}
											<button
												class="ml-0.5 leading-none text-text-tertiary hover:text-negative transition-colors cursor-pointer"
												title="Undo this claim"
												aria-label="Undo mastery {claim.masteryLevel} claim"
												onclick={handleMasteryUnclaim}
											>&times;</button>
										{/if}
									</div>
								{/each}
							</div>
						</div>
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
