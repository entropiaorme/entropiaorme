<script lang="ts">
	import { Button, Divider, ErrorNotice, Input, Modal } from '$lib/components';
	import type { ProtectionCostWindow } from '$lib/api';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import { formatPed } from '$lib/utils/format';
	import { formatDay, formatSessionStart } from './armourRecording';
	import type { ProtectionModel } from './protectionModel.svelte';

	let { model }: { model: ProtectionModel } = $props();

	const unrecorded = $derived(model.overview.unrecorded);

	function windowTitle(window: ProtectionCostWindow): string {
		if (window.kind === 'repair') return 'Unlimited repair';
		return window.setName ?? 'Limited set';
	}

	function windowDetail(window: ProtectionCostWindow): string {
		if (!window.costKnown) return 'A baseline reset left this reading unmeasured';
		if (window.allocations.length === 0) return 'Not attributed to any session';
		const sessions = window.allocations.length;
		const hits = window.allocations.reduce((sum, allocation) => sum + allocation.hitCount, 0);
		return `${sessions} ${sessions === 1 ? 'session' : 'sessions'}, ${hits} ${hits === 1 ? 'hit' : 'hits'}`;
	}
</script>

<div class="space-y-7">
	<div class="flex items-start justify-between gap-6">
		<div>
			<div class="flex items-center gap-2.5">
				<h2 class="text-lg font-semibold text-text">Armour</h2>
				<InDevelopmentMark id="limited-protection" />
			</div>
			<p class="mt-1 max-w-xl text-sm text-text-secondary">
				Record repairs and limited-set readings from the overlay's Cost button. Each cost is spread over the sessions you choose, by hits taken.
			</p>
			{#if !model.loading && unrecorded.sessions > 0}
				<p class="mt-2 text-xs tabular-nums text-warning">
					{unrecorded.sessions} {unrecorded.sessions === 1 ? 'session' : 'sessions'} with {unrecorded.hits.toLocaleString()} {unrecorded.hits === 1 ? 'hit' : 'hits'} {unrecorded.sessions === 1 ? 'has' : 'have'} no armour cost recorded yet.
				</p>
			{/if}
		</div>
		<div class="flex shrink-0 gap-2">
			<Button variant="secondary" size="sm" onclick={() => model.openSet('armour')}>Add armour set</Button>
			<Button variant="secondary" size="sm" onclick={() => model.openSet('plates')}>Add plate set</Button>
		</div>
	</div>

	{#if model.error}
		<ErrorNotice message={model.error} onDismiss={() => (model.error = null)} />
	{/if}

	{#if model.loading}
		<div class="py-14 text-center text-sm text-text-tertiary animate-pulse">Loading armour...</div>
	{:else}
		<section aria-labelledby="protection-sets-heading">
			<h3 id="protection-sets-heading" class="text-sm font-semibold text-text">Limited sets</h3>
			<p class="mt-0.5 mb-3 text-xs text-text-tertiary">
				Limited armour and plates carry their own markup, so each is recorded on its own. Unlimited protection needs no setup: its repairs are recorded as one.
			</p>
			<div class="grid grid-cols-2 gap-10">
				{#each [{ title: 'Armour', kind: 'armour' as const, sets: model.armourSets }, { title: 'Plates', kind: 'plates' as const, sets: model.plateSets }] as group (group.kind)}
					<section aria-label={`Limited ${group.title.toLowerCase()}`}>
						<h4 class="eyebrow mb-2">{group.title}</h4>
						{#if group.sets.length === 0}
							<p class="border-t border-border/70 py-5 text-xs text-text-tertiary">None yet.</p>
						{:else}
							<div class="divide-y divide-border/60 border-y border-border/70">
								{#each group.sets as set (set.id)}
									<div class="group flex items-center gap-3 py-3">
										<div class="min-w-0 flex-1">
											<span class="block truncate text-sm font-medium text-text">{set.name}</span>
											<span class="text-xs tabular-nums text-text-tertiary">{set.markupPercent.toFixed(2)}% average MU</span>
										</div>
										<div class="shrink-0 text-right">
											{#if set.latestObservation}
												<div class="text-sm tabular-nums text-text">{formatPed(set.latestObservation.ttValuePed)} PED</div>
												<div class="text-[10px] text-text-tertiary">{formatSessionStart(set.latestObservation.observedAt)}</div>
											{:else}
												<span class="text-[10px] text-text-tertiary">No reading yet</span>
											{/if}
										</div>
										<div class="flex items-center gap-1 opacity-70 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
											<Button variant="ghost" size="sm" onclick={() => model.editSet(set)}>Edit</Button>
											<Button variant="ghost" size="sm" class="text-error" onclick={() => model.askRemoveSet(set)}>Remove</Button>
										</div>
									</div>
								{/each}
							</div>
						{/if}
					</section>
				{/each}
			</div>
		</section>

		{#if model.overview.recentCostWindows.length > 0}
			<Divider />
			<section aria-labelledby="protection-history-heading">
				<h3 id="protection-history-heading" class="mb-3 text-sm font-semibold text-text">Recent armour costs</h3>
				<div class="divide-y divide-border/60 border-y border-border/70">
					{#each model.overview.recentCostWindows as window (window.id)}
						<div class="grid grid-cols-[minmax(0,1fr)_110px_100px_70px] items-center gap-4 py-2.5 text-xs">
							<div class="min-w-0">
								<span class="font-medium text-text">{windowTitle(window)}</span>
								<div class="truncate text-text-tertiary">{windowDetail(window)}</div>
							</div>
							<div class="tabular-nums text-text-secondary">
								{#if window.kind === 'limitedDecay' && window.costKnown && window.consumedTtPed !== null}
									{window.consumedTtPed.toFixed(2)} TT lost
								{/if}
							</div>
							<div class="text-right tabular-nums font-medium text-text">{window.costKnown ? `${formatPed(window.costPed)} PED` : 'Unknown'}</div>
							<div class="text-right tabular-nums text-text-tertiary">{formatDay(window.createdAt)}</div>
						</div>
					{/each}
				</div>
			</section>
		{/if}
	{/if}
</div>

<Modal bind:open={model.setModalOpen} title={`${model.editingSet ? 'Edit' : 'Add'} limited ${model.setKind === 'armour' ? 'armour' : 'plate'} set`}>
	<div class="space-y-5">
		<div>
			<label for="protection-set-name" class="block eyebrow mb-1.5">Set name</label>
			<Input id="protection-set-name" bind:value={model.setName} placeholder={model.setKind === 'armour' ? 'Viceroy' : '5B plates'} />
		</div>
		<div>
			<label for="protection-set-markup" class="block eyebrow mb-1.5">Average acquisition markup</label>
			<div class="flex items-center gap-2">
				<Input id="protection-set-markup" bind:value={model.setMarkup} type="number" min={100} step="0.01" disabled={model.editingSet?.basisLocked === true} class="max-w-32" />
				<span class="text-sm text-text-tertiary">%</span>
			</div>
			<p class="mt-1.5 text-xs text-text-tertiary">
				{model.editingSet?.basisLocked
					? 'Fixed after the first reading, since every reading is priced with it. A new acquisition is a new set.'
					: 'The TT-weighted average paid across the whole set. It is intentionally approximate.'}
			</p>
		</div>
		<div class="flex justify-end gap-2">
			<Button variant="secondary" onclick={() => (model.setModalOpen = false)}>Cancel</Button>
			<Button onclick={model.saveSet} disabled={model.setSaveDisabled} loading={model.saving}>{model.editingSet ? 'Save changes' : 'Add set'}</Button>
		</div>
	</div>
</Modal>

<Modal bind:open={model.removalModalOpen} title="Remove limited set">
	{#if model.removalTarget}
		<div class="space-y-5">
			<p class="text-sm text-text-secondary">
				Remove <span class="font-medium text-text">{model.removalTarget.name}</span> from the Cost popup? The costs it recorded keep its name.
			</p>
			<div class="flex justify-end gap-2">
				<Button variant="secondary" onclick={() => (model.removalModalOpen = false)} disabled={model.saving}>Cancel</Button>
				<Button variant="danger" onclick={model.confirmRemoval} loading={model.saving}>Remove</Button>
			</div>
		</div>
	{/if}
</Modal>
