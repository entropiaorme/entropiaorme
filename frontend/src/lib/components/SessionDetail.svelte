<script lang="ts">
	import { ApiError } from '$lib/api';
	import {
		activateLootItem,
		deactivateLootItem,
		renameSessionMob,
		restoreSessionMob,
	} from '$lib/api';
	import type {
		SessionDetail,
		LootEntry,
		MobBreakdownRow,
	} from '$lib/types/tracking';
	import { formatPed, formatPercent } from '$lib/utils/format';
	import StatDisplay from '$lib/components/StatDisplay.svelte';
	import Badge from '$lib/components/Badge.svelte';
	import Divider from '$lib/components/Divider.svelte';
	import DataTable from '$lib/components/DataTable.svelte';

	let {
		detail = $bindable(),
	}: { detail: SessionDetail } = $props();

	// ── Weapon cycle (unchanged) ──────────────────────────────────────
	const weaponCycleRows = $derived.by(() => {
		const rows = detail.toolStats.filter((t) => t.costAttributed > 0);
		const total = rows.reduce((s, t) => s + t.costAttributed, 0);
		return rows
			.map((t) => ({
				weaponName: t.weaponName,
				shotsFired: t.shotsFired,
				costAttributed: t.costAttributed,
				sharePct: total > 0 ? ((t.costAttributed / total) * 100).toFixed(1) : '0.0'
			}))
			.sort((a, b) => b.costAttributed - a.costAttributed);
	});

	// ── Notable events (unchanged) ────────────────────────────────────
	const badgeLabels: Record<string, string> = {
		global_kill: 'Global Kill',
		global_item: 'Global Item',
		hof_kill: 'HoF Kill',
		hof_item: 'HoF Item',
		quest_started: 'Quest Started',
		quest_completed: 'Quest Completed',
		quest_completed_pes: 'Quest Completed',
	};

	function badgeVariant(type: SessionDetail['notableEvents'][number]['type']) {
		if (type === 'hof') return 'accent';
		if (type === 'quest') return 'positive';
		return 'warning';
	}

	// ── Per-row loot entries ──────────────────────────────────────────
	// Backend returns one row per kill_loot_items capture (shrapnel rows
	// included for completeness). Filter shrapnel out of the visible
	// list since it's already excluded from the canonical Loot Breakdown
	// aggregate and end users don't reason about it as discrete loot.
	const visibleLootEntries = $derived(
		(detail.lootEntries ?? []).filter((e) => !e.isEnhancerShrapnel),
	);
	const activeLootEntries = $derived(
		visibleLootEntries
			.filter((e) => e.deactivatedAt === null)
			.slice()
			.sort((a, b) => b.valuePed - a.valuePed),
	);
	const deactivatedLootEntries = $derived(
		visibleLootEntries
			.filter((e) => e.deactivatedAt !== null)
			.slice()
			.sort((a, b) => b.valuePed - a.valuePed),
	);

	let lootConfirmId = $state<number | null>(null);
	let lootBusyId = $state<number | null>(null);
	let lootError = $state<string | null>(null);

	async function onLootDeactivate(entry: LootEntry) {
		lootError = null;
		lootBusyId = entry.id;
		try {
			const resp = await deactivateLootItem(detail.sessionId, entry.id);
			// Mutate the entry in-place so the derived splits re-evaluate
			// without a full refetch. Update the per-session returns total
			// so the Summary card's Loot TT + Net reflect the edit live.
			detail.lootEntries = detail.lootEntries.map((e) =>
				e.id === entry.id ? { ...e, deactivatedAt: resp.deactivatedAt } : e,
			);
			applySessionTotals(resp.sessionTotalReturns);
		} catch (e) {
			lootError = errorMessage(e, 'Failed to deactivate loot entry.');
		} finally {
			lootBusyId = null;
			lootConfirmId = null;
		}
	}

	async function onLootActivate(entry: LootEntry) {
		lootError = null;
		lootBusyId = entry.id;
		try {
			const resp = await activateLootItem(detail.sessionId, entry.id);
			detail.lootEntries = detail.lootEntries.map((e) =>
				e.id === entry.id ? { ...e, deactivatedAt: resp.deactivatedAt } : e,
			);
			applySessionTotals(resp.sessionTotalReturns);
		} catch (e) {
			lootError = errorMessage(e, 'Failed to activate loot entry.');
		} finally {
			lootBusyId = null;
			lootConfirmId = null;
		}
	}

	// ── Mob attribution edit ──────────────────────────────────────────
	const mobBreakdown = $derived(detail.mobBreakdown ?? []);

	type MobEditMode = 'idle' | 'edit' | 'restore';
	let mobEditMode = $state<MobEditMode>('idle');
	let mobEditTarget = $state<string | null>(null);
	let mobEditDraft = $state('');
	let mobBusy = $state(false);
	let mobError = $state<string | null>(null);

	function startMobEdit(row: MobBreakdownRow) {
		mobError = null;
		mobEditMode = 'edit';
		mobEditTarget = row.currentName;
		mobEditDraft = row.currentName;
	}

	function startMobRestore(row: MobBreakdownRow) {
		mobError = null;
		mobEditMode = 'restore';
		mobEditTarget = row.currentName;
	}

	function cancelMobEdit() {
		mobEditMode = 'idle';
		mobEditTarget = null;
		mobEditDraft = '';
		mobError = null;
	}

	async function confirmMobRename() {
		if (mobEditMode !== 'edit' || mobEditTarget === null) return;
		const from = mobEditTarget;
		const to = mobEditDraft.trim();
		if (!to) {
			mobError = 'Name cannot be blank.';
			return;
		}
		if (to === from) {
			cancelMobEdit();
			return;
		}
		mobBusy = true;
		mobError = null;
		try {
			await renameSessionMob(detail.sessionId, from, to);
			applyMobRename(from, to);
			cancelMobEdit();
		} catch (e) {
			mobError = errorMessage(e, 'Rename failed.');
		} finally {
			mobBusy = false;
		}
	}

	async function confirmMobRestore() {
		if (mobEditMode !== 'restore' || mobEditTarget === null) return;
		const current = mobEditTarget;
		mobBusy = true;
		mobError = null;
		try {
			const resp = await restoreSessionMob(detail.sessionId, current);
			applyMobRestore(current, resp.mobName);
			cancelMobEdit();
		} catch (e) {
			mobError = errorMessage(e, 'Restore failed.');
		} finally {
			mobBusy = false;
		}
	}

	function applyMobRename(from: string, to: string) {
		// Local merge of the rename's effect on mobBreakdown. If `to`
		// already exists as a row (the rename merges two cohorts), fold
		// the counts together; otherwise rewrite the row's currentName
		// in place. `originalName` follows the COALESCE-on-first-rename
		// rule from the backend (first rename captures the genuine
		// original; subsequent renames preserve it).
		const fromRow = detail.mobBreakdown.find((r) => r.currentName === from);
		if (!fromRow) return;
		const existingTo = detail.mobBreakdown.find((r) => r.currentName === to);
		const preservedOriginal = fromRow.originalName ?? from;
		// Round-trip A->B->A landing back at the original: clear the
		// preservation column so no "originally X" indicator appears.
		const isRoundTripClear = preservedOriginal === to;
		const newOriginal = isRoundTripClear ? null : preservedOriginal;
		if (existingTo) {
			detail.mobBreakdown = detail.mobBreakdown
				.map((r) => {
					if (r.currentName === to) {
						return {
							...r,
							// Merged cohort: preservation column on the merge
							// destination is ambiguous (multiple distinct
							// originals possible). Backend lands the row at
							// its existing originalName; the next refetch is
							// the source of truth for ambiguity surfacing.
							killCount: r.killCount + fromRow.killCount,
						};
					}
					return r;
				})
				.filter((r) => r.currentName !== from);
		} else {
			detail.mobBreakdown = detail.mobBreakdown.map((r) =>
				r.currentName === from
					? { ...r, currentName: to, originalName: newOriginal }
					: r,
			);
		}
		// Rewrite kills.mob_name on every loot-entry-killing kill is not
		// reflected client-side; the next session refetch reconciles. The
		// loot section keeps rendering against its current data.
	}

	function applyMobRestore(currentName: string, restoredTo: string) {
		// Restore reverts the rename: rewrite currentName back to its
		// originalName and null out the preservation column.
		const existing = detail.mobBreakdown.find((r) => r.currentName === restoredTo);
		const restoredRow = detail.mobBreakdown.find((r) => r.currentName === currentName);
		if (!restoredRow) return;
		if (existing) {
			detail.mobBreakdown = detail.mobBreakdown
				.map((r) => {
					if (r.currentName === restoredTo) {
						return { ...r, killCount: r.killCount + restoredRow.killCount };
					}
					return r;
				})
				.filter((r) => r.currentName !== currentName);
		} else {
			detail.mobBreakdown = detail.mobBreakdown.map((r) =>
				r.currentName === currentName
					? { ...r, currentName: restoredTo, originalName: null }
					: r,
			);
		}
	}

	function applySessionTotals(sessionTotalReturns: number) {
		const cost = detail.summary.cost;
		const net = sessionTotalReturns - cost;
		const returnRate = cost > 0 ? sessionTotalReturns / cost : 0;
		detail.summary = {
			...detail.summary,
			returns: sessionTotalReturns,
			net,
			returnRate,
		};
		detail.effectiveLoot = sessionTotalReturns;
	}

	function errorMessage(e: unknown, fallback: string): string {
		if (e instanceof ApiError) {
			// Ambiguous-restore 409 carries a structural marker in the
			// detail string; surface a friendlier message in that case.
			if (e.status === 409 && /Ambiguous restore/i.test(e.message)) {
				return "Can't auto-restore: this name was merged from multiple originals.";
			}
			return e.message || fallback;
		}
		return fallback;
	}
</script>

<div class="bg-surface/50 border border-border/50 rounded-b-md p-5 -mt-1 space-y-5">
	<!-- 1. Summary stats -->
	<div>
		<h3 class="eyebrow mb-3">Summary</h3>
		<div class="grid grid-cols-3 gap-4 sm:grid-cols-6">
			<StatDisplay label="Kills" value={detail.summary.kills} />
			<StatDisplay label="Cycled" value={formatPed(detail.summary.cost)} unit="PED" />
			<StatDisplay label="Loot TT" value={formatPed(detail.summary.returns)} unit="PED" />
			<StatDisplay label="Return" value={formatPercent(detail.summary.returnRate)} />
			<div class="flex flex-col gap-1.5">
				<span class="eyebrow">Net</span>
				<div class="flex items-baseline gap-1.5">
					<span
						class="text-2xl font-semibold tabular-nums leading-none tracking-tight {detail.summary.net >= 0
							? 'text-positive'
							: 'text-negative'}"
					>
						{detail.summary.net >= 0 ? '+' : ''}{formatPed(detail.summary.net)}
					</span>
					<span class="text-xs font-medium text-text-tertiary uppercase tracking-wider">PED</span>
				</div>
			</div>
			<StatDisplay label="PES" value={formatPed(detail.summary.pes ?? 0)} unit="PES" />
		</div>
	</div>

	<!-- 1b. Cost breakdown (shown when non-weapon costs exist) -->
	{#if detail.summary.costBreakdown && (detail.summary.costBreakdown.healCost > 0 || detail.summary.costBreakdown.enhancerCost > 0 || detail.summary.costBreakdown.armourCost > 0)}
		<div class="mt-2 pl-1 flex flex-wrap gap-x-5 gap-y-1 text-xs text-text-secondary">
			<span>Weapon: <span class="text-text tabular-nums">{formatPed(detail.summary.costBreakdown.weaponCost)}</span></span>
			{#if detail.summary.costBreakdown.healCost > 0}
				<span>Healing: <span class="text-text tabular-nums">{formatPed(detail.summary.costBreakdown.healCost)}</span></span>
			{/if}
			{#if detail.summary.costBreakdown.enhancerCost > 0}
				<span>Enhancers: <span class="text-text tabular-nums">{formatPed(detail.summary.costBreakdown.enhancerCost)}</span></span>
			{/if}
			{#if detail.summary.costBreakdown.armourCost > 0}
				<span>Armour: <span class="text-text tabular-nums">{formatPed(detail.summary.costBreakdown.armourCost)}</span></span>
			{/if}
		</div>
	{/if}

	<!-- 1c. Weapon cycle breakdown -->
	{#if weaponCycleRows.length > 0}
		<Divider />
		<div>
			<h3 class="eyebrow mb-3">Weapon Cycle</h3>
			<DataTable
				columns={[
					{ key: 'weaponName', label: 'Weapon' },
					{ key: 'shotsFired', label: 'Shots', align: 'right' },
					{ key: 'costAttributed', label: 'Cycle', align: 'right' },
					{ key: 'sharePct', label: 'Share', align: 'right' }
				]}
				rows={weaponCycleRows}
			>
				{#snippet cell({ row, column, value })}
					{#if column.key === 'costAttributed'}
						{formatPed(row.costAttributed)}
					{:else if column.key === 'sharePct'}
						{row.sharePct}%
					{:else}
						{value}
					{/if}
				{/snippet}
			</DataTable>
		</div>
	{/if}

	<!-- 1d. Mob attribution (edit + restore affordances) -->
	{#if mobBreakdown.length > 0}
		<Divider />
		<div>
			<h3 class="eyebrow mb-3">Mob Attribution</h3>
			<div class="rounded-md border border-border/60 divide-y divide-border/40">
				{#each mobBreakdown as row (row.currentName)}
					{@const isEditing = mobEditMode === 'edit' && mobEditTarget === row.currentName}
					{@const isRestoring = mobEditMode === 'restore' && mobEditTarget === row.currentName}
					<div class="px-3 py-2 flex flex-wrap items-center gap-3 text-sm">
						{#if isEditing}
							<div class="flex flex-1 items-center gap-2 min-w-[200px]">
								<input
									type="text"
									class="flex-1 min-w-0 bg-surface border border-border rounded-sm px-2 py-1 text-sm text-text focus:outline-none focus:border-accent"
									bind:value={mobEditDraft}
									aria-label="New mob name"
									disabled={mobBusy}
									onkeydown={(e) => {
										if (e.key === 'Enter') confirmMobRename();
										if (e.key === 'Escape') cancelMobEdit();
									}}
								/>
								<button
									type="button"
									class="text-xs text-accent hover:text-accent-hover px-2 py-0.5 rounded-sm cursor-pointer border border-accent/40 hover:border-accent font-medium disabled:opacity-50 disabled:cursor-not-allowed"
									disabled={mobBusy}
									onclick={confirmMobRename}
								>
									Save
								</button>
								<button
									type="button"
									class="text-xs text-text-secondary hover:text-text px-2 py-0.5 rounded-sm cursor-pointer border border-border/60 hover:border-border-bright disabled:opacity-50 disabled:cursor-not-allowed"
									disabled={mobBusy}
									onclick={cancelMobEdit}
								>
									Cancel
								</button>
							</div>
						{:else if isRestoring}
							<div class="flex flex-1 items-center gap-3 min-w-[200px]">
								<span class="text-xs text-text-secondary">
									Restore to "{row.originalName}"?
								</span>
								<button
									type="button"
									class="text-xs text-accent hover:text-accent-hover px-2 py-0.5 rounded-sm cursor-pointer border border-accent/40 hover:border-accent font-medium disabled:opacity-50 disabled:cursor-not-allowed"
									disabled={mobBusy}
									onclick={confirmMobRestore}
								>
									Yes
								</button>
								<button
									type="button"
									class="text-xs text-text-secondary hover:text-text px-2 py-0.5 rounded-sm cursor-pointer border border-border/60 hover:border-border-bright disabled:opacity-50 disabled:cursor-not-allowed"
									disabled={mobBusy}
									onclick={cancelMobEdit}
								>
									Cancel
								</button>
							</div>
						{:else}
							<div class="flex flex-1 items-center gap-2 min-w-[200px]">
								<span class="text-text">{row.currentName}</span>
								{#if row.originalName}
									<span class="text-xs text-text-tertiary italic">
										originally {row.originalName}
									</span>
								{/if}
							</div>
							<span class="text-xs text-text-tertiary tabular-nums">
								{row.killCount} {row.killCount === 1 ? 'kill' : 'kills'}
							</span>
							<div class="flex items-center gap-1">
								<button
									type="button"
									class="text-text-tertiary hover:text-text transition-colors duration-[var(--duration-fast)] cursor-pointer p-1"
									onclick={() => startMobEdit(row)}
									aria-label="Rename {row.currentName}"
									title="Rename"
								>
									<svg
										xmlns="http://www.w3.org/2000/svg"
										fill="none"
										viewBox="0 0 24 24"
										stroke-width="1.5"
										stroke="currentColor"
										class="w-4 h-4"
									>
										<path
											stroke-linecap="round"
											stroke-linejoin="round"
											d="M16.862 4.487l1.687-1.688a1.875 1.875 0 1 1 2.652 2.652L10.582 16.07a4.5 4.5 0 0 1-1.897 1.13L6 18l.8-2.685a4.5 4.5 0 0 1 1.13-1.897l8.932-8.931Zm0 0L19.5 7.125M18 14v4.75A2.25 2.25 0 0 1 15.75 21H5.25A2.25 2.25 0 0 1 3 18.75V8.25A2.25 2.25 0 0 1 5.25 6H10"
										/>
									</svg>
								</button>
								{#if row.originalName}
									<button
										type="button"
										class="text-text-tertiary hover:text-text transition-colors duration-[var(--duration-fast)] cursor-pointer p-1"
										onclick={() => startMobRestore(row)}
										aria-label="Restore {row.currentName}"
										title="Restore original name"
									>
										<svg
											xmlns="http://www.w3.org/2000/svg"
											fill="none"
											viewBox="0 0 24 24"
											stroke-width="1.5"
											stroke="currentColor"
											class="w-4 h-4"
										>
											<path
												stroke-linecap="round"
												stroke-linejoin="round"
												d="M9 15 3 9m0 0 6-6M3 9h12a6 6 0 0 1 0 12h-3"
											/>
										</svg>
									</button>
								{/if}
							</div>
						{/if}
					</div>
				{/each}
			</div>
			{#if mobError}
				<p class="mt-2 text-xs text-negative">{mobError}</p>
			{/if}
		</div>
	{/if}

	<!-- 2. Notable events -->
	{#if detail.notableEvents.length > 0}
		<Divider />
		<div>
			<h3 class="eyebrow mb-3">Notable Events</h3>
			<div class="space-y-2">
				{#each detail.notableEvents as event}
					<div class="flex items-center justify-between bg-surface-hover/50 rounded-md px-3 py-2">
						<div class="flex items-center gap-2">
							<Badge variant={badgeVariant(event.type)}>
								{badgeLabels[event.eventType] ?? 'Event'}
							</Badge>
							<span class="text-sm text-text">{event.target}</span>
							{#if event.item && event.item !== event.target}
								<span class="text-xs text-text-tertiary">&mdash;</span>
								<span class="text-sm text-text-secondary">{event.item}</span>
							{/if}
						</div>
						<span class="text-sm font-medium text-positive tabular-nums">
							{formatPed(event.value)} {event.eventType === 'quest_completed_pes' ? 'PES' : 'PED'}
						</span>
					</div>
				{/each}
			</div>
		</div>
	{/if}

	<!-- 3. Loot breakdown (per-row with deactivate / activate) -->
	<Divider />
	<div>
		<h3 class="eyebrow mb-3">Loot Breakdown</h3>
		{#if activeLootEntries.length === 0 && deactivatedLootEntries.length === 0}
			<p class="text-xs text-text-tertiary">No loot recorded.</p>
		{:else}
			<DataTable
				columns={[
					{ key: 'itemName', label: 'Item' },
					{ key: 'quantity', label: 'Qty', align: 'right' },
					{ key: 'valuePed', label: 'TT Value', align: 'right' },
					{ key: '__action', label: '', align: 'right', widthClass: 'w-[10%]' }
				]}
				rows={activeLootEntries}
				rowKeyFn={(row) => String(row.id)}
				overlayKey={lootConfirmId !== null ? String(lootConfirmId) : null}
			>
				{#snippet cell({ row, column, value })}
					{#if column.key === 'valuePed'}
						{formatPed(row.valuePed)}
					{:else if column.key === '__action'}
						<button
							type="button"
							class="text-text-tertiary hover:text-text transition-colors duration-[var(--duration-fast)] cursor-pointer p-1 disabled:opacity-50 disabled:cursor-not-allowed"
							disabled={lootBusyId === row.id}
							onclick={() => (lootConfirmId = row.id)}
							aria-label="Deactivate {row.itemName}"
							title="Deactivate this loot entry"
						>
							<svg
								xmlns="http://www.w3.org/2000/svg"
								fill="none"
								viewBox="0 0 24 24"
								stroke-width="1.5"
								stroke="currentColor"
								class="w-4 h-4"
							>
								<path
									stroke-linecap="round"
									stroke-linejoin="round"
									d="M20.25 7.5l-.625 10.632a2.25 2.25 0 0 1-2.247 2.118H6.622a2.25 2.25 0 0 1-2.247-2.118L3.75 7.5M10 11.25h4M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125z"
								/>
							</svg>
						</button>
					{:else}
						{value}
					{/if}
				{/snippet}
				{#snippet rowOverlay({ row })}
					<div class="inline-flex items-center gap-3">
						<span class="text-xs text-text-secondary">Deactivate this loot entry?</span>
						<button
							type="button"
							class="text-xs text-text-secondary hover:text-text px-2 py-0.5 rounded-sm cursor-pointer border border-border/60 hover:border-border-bright"
							onclick={() => (lootConfirmId = null)}
						>
							Cancel
						</button>
						<button
							type="button"
							class="text-xs text-accent hover:text-accent-hover px-2 py-0.5 rounded-sm cursor-pointer border border-accent/40 hover:border-accent font-medium"
							onclick={() => onLootDeactivate(row)}
						>
							Yes
						</button>
					</div>
				{/snippet}
			</DataTable>
		{/if}

		{#if deactivatedLootEntries.length > 0}
			<div class="mt-4">
				<p class="eyebrow mb-2 text-text-tertiary">Deactivated</p>
				<DataTable
					columns={[
						{ key: 'itemName', label: 'Item' },
						{ key: 'quantity', label: 'Qty', align: 'right' },
						{ key: 'valuePed', label: 'TT Value', align: 'right' },
						{ key: '__action', label: '', align: 'right', widthClass: 'w-[10%]' }
					]}
					rows={deactivatedLootEntries}
					rowKeyFn={(row) => String(row.id)}
					overlayKey={lootConfirmId !== null ? String(lootConfirmId) : null}
					class="opacity-60"
				>
					{#snippet cell({ row, column, value })}
						{#if column.key === 'valuePed'}
							{formatPed(row.valuePed)}
						{:else if column.key === '__action'}
							<button
								type="button"
								class="text-text-tertiary hover:text-text transition-colors duration-[var(--duration-fast)] cursor-pointer p-1 disabled:opacity-50 disabled:cursor-not-allowed"
								disabled={lootBusyId === row.id}
								onclick={() => (lootConfirmId = row.id)}
								aria-label="Activate {row.itemName}"
								title="Activate this loot entry"
							>
								<svg
									xmlns="http://www.w3.org/2000/svg"
									fill="none"
									viewBox="0 0 24 24"
									stroke-width="1.5"
									stroke="currentColor"
									class="w-4 h-4"
								>
									<path
										stroke-linecap="round"
										stroke-linejoin="round"
										d="M20.25 7.5l-.625 10.632a2.25 2.25 0 0 1-2.247 2.118H6.622a2.25 2.25 0 0 1-2.247-2.118L3.75 7.5m8.25 3.75l2.25 2.25m0-2.25l-2.25 2.25M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125z"
									/>
								</svg>
							</button>
						{:else}
							{value}
						{/if}
					{/snippet}
					{#snippet rowOverlay({ row })}
						<div class="inline-flex items-center gap-3">
							<span class="text-xs text-text-secondary">Activate this loot entry?</span>
							<button
								type="button"
								class="text-xs text-text-secondary hover:text-text px-2 py-0.5 rounded-sm cursor-pointer border border-border/60 hover:border-border-bright"
								onclick={() => (lootConfirmId = null)}
							>
								Cancel
							</button>
							<button
								type="button"
								class="text-xs text-accent hover:text-accent-hover px-2 py-0.5 rounded-sm cursor-pointer border border-accent/40 hover:border-accent font-medium"
								onclick={() => onLootActivate(row)}
							>
								Yes
							</button>
						</div>
					{/snippet}
				</DataTable>
			</div>
		{/if}

		{#if lootError}
			<p class="mt-2 text-xs text-negative">{lootError}</p>
		{/if}
	</div>

	<!-- 4. Skill gains -->
	{#if detail.skillGains.length > 0}
		<Divider />
		<div>
			<h3 class="eyebrow mb-3">Skill Gains</h3>
			<div class="space-y-1.5">
				{#each detail.skillGains as skill}
					<div class="flex items-center justify-between text-sm">
						<div class="flex items-center gap-2">
							<span class="text-text">{skill.skillName}</span>
							<span class="text-xs text-text-tertiary">Lv {skill.level}</span>
						</div>
						<span class="text-positive tabular-nums font-medium">
							+{formatPed(skill.ttValueGained)} PES
						</span>
					</div>
				{/each}
			</div>
		</div>
	{/if}
</div>
