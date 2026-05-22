<script lang="ts">
	import { ApiError } from '$lib/api';
	import {
		activateLootItem,
		deactivateLootItem,
		getSessionDetail,
		renameSessionMob,
		restoreSessionMob,
	} from '$lib/api';
	import type {
		SessionDetail,
		LootItem,
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

	// ── Loot breakdown (aggregate-by-item with wholesale archive) ────
	// The canonical user-facing view is the item-name aggregate. The
	// wholesale archive affordance operates on the aggregate row:
	// "Deactivate Nanocube" flips every Nanocube capture in this session
	// in one atomic backend transaction. The greyed parallel aggregate
	// below carries the inverse Activate affordance per item name.
	const activeLootRows = $derived(detail.lootBreakdown ?? []);
	const deactivatedLootRows = $derived(detail.deactivatedLootBreakdown ?? []);

	let lootConfirmName = $state<string | null>(null);
	// `confirm-active` opens the deactivate prompt on an active-aggregate
	// row; `confirm-deactivated` opens the activate prompt on a
	// deactivated-aggregate row. The same item name can in principle
	// appear in both arrays (partial-state cohort), so the source array
	// has to be tracked alongside the name.
	let lootConfirmSource = $state<'active' | 'deactivated' | null>(null);
	// Per-row pending tracking. A single shared-string busy marker would
	// let a click on a second row overwrite the marker, leaving the first
	// request's finally to clear the busy state while the second is still
	// in flight. The Set allows concurrent different-row mutations while
	// blocking same-row re-entry.
	let pendingLootNames = $state(new Set<string>());
	let lootError = $state<string | null>(null);

	function markLootPending(name: string) {
		pendingLootNames = new Set(pendingLootNames).add(name);
	}

	function clearLootPending(name: string) {
		const next = new Set(pendingLootNames);
		next.delete(name);
		pendingLootNames = next;
	}

	function openLootConfirm(itemName: string, source: 'active' | 'deactivated') {
		lootError = null;
		lootConfirmName = itemName;
		lootConfirmSource = source;
	}

	function cancelLootConfirm() {
		lootConfirmName = null;
		lootConfirmSource = null;
	}

	async function onLootDeactivate(row: LootItem) {
		if (pendingLootNames.has(row.name)) return;
		lootError = null;
		markLootPending(row.name);
		try {
			const resp = await deactivateLootItem(detail.sessionId, row.name);
			moveLootRow(row.name, 'active->deactivated');
			applySessionTotals(resp.sessionTotalReturns);
		} catch (e) {
			lootError = errorMessage(e, 'Failed to deactivate loot.');
		} finally {
			clearLootPending(row.name);
			cancelLootConfirm();
		}
	}

	async function onLootActivate(row: LootItem) {
		if (pendingLootNames.has(row.name)) return;
		lootError = null;
		markLootPending(row.name);
		try {
			const resp = await activateLootItem(detail.sessionId, row.name);
			moveLootRow(row.name, 'deactivated->active');
			applySessionTotals(resp.sessionTotalReturns);
		} catch (e) {
			lootError = errorMessage(e, 'Failed to activate loot.');
		} finally {
			clearLootPending(row.name);
			cancelLootConfirm();
		}
	}

	function moveLootRow(itemName: string, direction: 'active->deactivated' | 'deactivated->active') {
		// Wholesale flip means every matching row moves between the two
		// aggregates. Partial-state cohorts merge: if Nanocube already
		// exists on the destination side, the moved row's quantity +
		// ttValue fold into the existing destination row. The next
		// session refetch is the source of truth either way.
		const srcKey = direction === 'active->deactivated' ? 'lootBreakdown' : 'deactivatedLootBreakdown';
		const destKey = direction === 'active->deactivated' ? 'deactivatedLootBreakdown' : 'lootBreakdown';
		const moving = detail[srcKey].find((r) => r.name === itemName);
		if (!moving) return;
		const existingDest = detail[destKey].find((r) => r.name === itemName);
		const nextDest = existingDest
			? detail[destKey].map((r) =>
				r.name === itemName
					? { ...r, quantity: r.quantity + moving.quantity, ttValue: r.ttValue + moving.ttValue }
					: r,
			)
			: [...detail[destKey], { ...moving }];
		// Re-sort by ttValue desc to keep aggregate ordering stable
		// with the backend.
		nextDest.sort((a, b) => b.ttValue - a.ttValue);
		const nextSrc = detail[srcKey].filter((r) => r.name !== itemName);
		detail = {
			...detail,
			[srcKey]: nextSrc,
			[destKey]: nextDest,
		};
	}

	// ── Mob attribution edit ──────────────────────────────────────────
	const mobBreakdown = $derived(detail.mobBreakdown ?? []);
	const isTagMode = $derived(detail.mobEntryMode === 'tag');
	const attributionHeading = $derived(isTagMode ? 'Tag Attribution' : 'Mob Attribution');
	const attributionInputLabel = $derived(isTagMode ? 'New tag' : 'New mob name');
	const attributionRenameVerb = $derived(isTagMode ? 'Retag' : 'Rename');
	const attributionRestoreTitle = $derived(
		isTagMode ? 'Restore original tag' : 'Restore original name',
	);
	const attributionOriginallyLabel = $derived(
		isTagMode ? 'originally tagged' : 'originally',
	);

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
			mobError = isTagMode ? 'Tag cannot be blank.' : 'Name cannot be blank.';
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
			await refetchSessionDetail();
			cancelMobEdit();
		} catch (e) {
			mobError = errorMessage(e, isTagMode ? 'Retag failed.' : 'Rename failed.');
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
			await restoreSessionMob(detail.sessionId, current);
			await refetchSessionDetail();
			cancelMobEdit();
		} catch (e) {
			mobError = errorMessage(e, 'Restore failed.');
		} finally {
			mobBusy = false;
		}
	}

	// Backend groups mobBreakdown by (mob_name, original_mob_name), so a
	// rename can leave two rows sharing currentName but carrying distinct
	// originalName (the renamed cohort + the pre-existing cohort at the
	// destination). Client-side merge-by-currentName loses that
	// distinction and the restore affordance; refetch is the source of
	// truth.
	async function refetchSessionDetail() {
		try {
			detail = await getSessionDetail(detail.sessionId);
		} catch {
			// Soft-fail: the next expand cycle will reconcile from the
			// backend regardless. The mutation itself succeeded.
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
			<h3 class="eyebrow mb-3">{attributionHeading}</h3>
			<div class="rounded-md border border-border/60 divide-y divide-border/40">
				{#each mobBreakdown as row (`${row.currentName}|${row.originalName ?? ''}`)}
					{@const isEditing = mobEditMode === 'edit' && mobEditTarget === row.currentName}
					{@const isRestoring = mobEditMode === 'restore' && mobEditTarget === row.currentName}
					<div class="px-3 py-2 flex flex-wrap items-center gap-3 text-sm">
						{#if isEditing}
							<div class="flex flex-1 items-center gap-2 min-w-[200px]">
								<input
									type="text"
									class="flex-1 min-w-0 bg-surface border border-border rounded-sm px-2 py-1 text-sm text-text focus:outline-none focus:border-accent"
									bind:value={mobEditDraft}
									aria-label={attributionInputLabel}
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
										{attributionOriginallyLabel} {row.originalName}
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
									aria-label="{attributionRenameVerb} {row.currentName}"
									title={attributionRenameVerb}
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
										title={attributionRestoreTitle}
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

	<!-- 3. Loot breakdown (aggregate-by-item with wholesale archive) -->
	<Divider />
	<div>
		<h3 class="eyebrow mb-3">Loot Breakdown</h3>
		{#if activeLootRows.length === 0 && deactivatedLootRows.length === 0}
			<p class="text-xs text-text-tertiary">No loot recorded.</p>
		{:else}
			<DataTable
				columns={[
					{ key: 'name', label: 'Item' },
					{ key: 'quantity', label: 'Qty', align: 'right' },
					{ key: 'ttValue', label: 'TT Value', align: 'right' },
					{ key: '__action', label: '', align: 'right', widthClass: 'w-[10%]' }
				]}
				rows={activeLootRows}
				rowKeyFn={(row) => row.name}
				overlayKey={lootConfirmSource === 'active' ? lootConfirmName : null}
			>
				{#snippet cell({ row, column, value })}
					{#if column.key === 'ttValue'}
						{formatPed(row.ttValue)}
					{:else if column.key === '__action'}
						<button
							type="button"
							class="text-text-tertiary hover:text-text transition-colors duration-[var(--duration-fast)] cursor-pointer p-1 disabled:opacity-50 disabled:cursor-not-allowed"
							disabled={pendingLootNames.has(row.name)}
							onclick={() => openLootConfirm(row.name, 'active')}
							aria-label="Archive all {row.name} entries"
							title="Archive all {row.name} entries"
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
						<span class="text-xs text-text-secondary">
							Archive all {row.quantity} "{row.name}" {row.quantity === 1 ? 'entry' : 'entries'}?
						</span>
						<button
							type="button"
							class="text-xs text-text-secondary hover:text-text px-2 py-0.5 rounded-sm cursor-pointer border border-border/60 hover:border-border-bright"
							onclick={cancelLootConfirm}
						>
							Cancel
						</button>
						<button
							type="button"
							class="text-xs text-accent hover:text-accent-hover px-2 py-0.5 rounded-sm cursor-pointer border border-accent/40 hover:border-accent font-medium disabled:opacity-50 disabled:cursor-not-allowed"
							disabled={pendingLootNames.has(row.name)}
							onclick={() => onLootDeactivate(row)}
						>
							Yes
						</button>
					</div>
				{/snippet}
			</DataTable>
		{/if}

		{#if deactivatedLootRows.length > 0}
			<div class="mt-4">
				<p class="eyebrow mb-2 text-text-tertiary">Archived</p>
				<DataTable
					columns={[
						{ key: 'name', label: 'Item' },
						{ key: 'quantity', label: 'Qty', align: 'right' },
						{ key: 'ttValue', label: 'TT Value', align: 'right' },
						{ key: '__action', label: '', align: 'right', widthClass: 'w-[10%]' }
					]}
					rows={deactivatedLootRows}
					rowKeyFn={(row) => row.name}
					overlayKey={lootConfirmSource === 'deactivated' ? lootConfirmName : null}
					class="opacity-60"
				>
					{#snippet cell({ row, column, value })}
						{#if column.key === 'ttValue'}
							{formatPed(row.ttValue)}
						{:else if column.key === '__action'}
							<button
								type="button"
								class="text-text-tertiary hover:text-text transition-colors duration-[var(--duration-fast)] cursor-pointer p-1 disabled:opacity-50 disabled:cursor-not-allowed"
								disabled={pendingLootNames.has(row.name)}
								onclick={() => openLootConfirm(row.name, 'deactivated')}
								aria-label="Restore all {row.name} entries"
								title="Restore all {row.name} entries"
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
							<span class="text-xs text-text-secondary">
								Restore all {row.quantity} "{row.name}" {row.quantity === 1 ? 'entry' : 'entries'}?
							</span>
							<button
								type="button"
								class="text-xs text-text-secondary hover:text-text px-2 py-0.5 rounded-sm cursor-pointer border border-border/60 hover:border-border-bright"
								onclick={cancelLootConfirm}
							>
								Cancel
							</button>
							<button
								type="button"
								class="text-xs text-accent hover:text-accent-hover px-2 py-0.5 rounded-sm cursor-pointer border border-accent/40 hover:border-accent font-medium disabled:opacity-50 disabled:cursor-not-allowed"
								disabled={pendingLootNames.has(row.name)}
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
