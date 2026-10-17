<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import QuestBoardRow from './QuestBoardRow.svelte';
	import type { QuestBoardModel } from './questBoardModel.svelte';
	import type { BoardRow } from './questBoard';

	let {
		model,
		copiedKey,
		onCopyWaypoint,
		onCreateCustom,
		onEditCustom,
		onDeleteCustom
	}: {
		model: QuestBoardModel;
		copiedKey: string | null;
		onCopyWaypoint: (key: string, waypoint: string) => void;
		onCreateCustom: (planet: string | null) => void;
		onEditCustom: (row: BoardRow) => void;
		onDeleteCustom: (row: BoardRow) => void;
	} = $props();

	const current = $derived(model.current);
	const offersHere = $derived(model.planet !== null && model.offeredPlanet === model.planet);
</script>

<div class="flex-1 min-h-0 flex flex-col" data-testid="quest-board">
	<!-- The planet is the board's one choice; everything under it is laid out
		 by the catalogue. The overlay note says what the running session
		 offers there, and lets it offer this planet instead in one tap. -->
	<div class="flex items-end justify-between gap-4 border-b border-border/60">
		<div class="flex items-end gap-5" role="group" aria-label="Planet">
			{#each model.planets as planet (planet.name)}
				{@const active = planet.name === model.planet}
				<button
					type="button"
					aria-pressed={active}
					class="planet-tab {active ? 'planet-tab-active' : ''}"
					onclick={() => model.selectPlanet(planet.name)}
				>
					{planet.name}
					{#if planet.name === model.offeredPlanet}
						<span class="offered-dot" title="Offered in the overlay" aria-label="offered in the overlay"></span>
					{/if}
				</button>
			{/each}
		</div>
		<div class="flex items-center gap-3 pb-2">
			{#if model.definitionName && model.planet}
				{#if offersHere}
					<span class="text-xs text-text-tertiary">
						Offered in the overlay by {model.definitionName}
					</span>
				{:else}
					<button
						type="button"
						class="header-action"
						title="Make {model.definitionName} offer {model.planet} quests in the overlay"
						onclick={() => model.offerInOverlay(model.planet as string)}
					>Offer in the overlay</button>
				{/if}
			{/if}
			<button
				type="button"
				class="header-action"
				onclick={() => onCreateCustom(model.planet)}
			>+ Custom quest</button>
		</div>
	</div>

	{#if model.error}
		<div class="pt-3">
			<ErrorNotice message={model.error} />
		</div>
	{/if}

	{#if current}
		<div class="flex-1 min-h-0 overflow-y-auto pt-3 pr-1">
			{#each current.sections as section (section.key)}
				<section class="mb-5 last:mb-1" aria-label={section.name}>
					<div class="flex items-baseline justify-between gap-3 pb-1">
						<div class="flex items-baseline gap-2 min-w-0">
							<h4 class="eyebrow-strong truncate">{section.name}</h4>
							{#if section.rewardNote}
								<span class="truncate text-xs text-text-tertiary">{section.rewardNote} each</span>
							{/if}
						</div>
						<span class="shrink-0 text-xs tabular-nums text-text-tertiary">
							{section.done} of {section.rows.length} done
						</span>
					</div>
					<ul>
						{#each section.rows as row (row.key)}
							<QuestBoardRow
								{row}
								now={model.now}
								busy={model.busy === row.key}
								settling={model.settling === row.key}
								copied={copiedKey === row.key}
								onCopyWaypoint={() => row.waypoint && onCopyWaypoint(row.key, row.waypoint)}
								onStart={() => model.start(row)}
								onComplete={() => model.complete(row)}
								onAbandon={() => model.abandon(row)}
								onResetCooldown={() => model.resetCooldown(row)}
								onUndoReward={() => model.undoReward(row)}
								onToggleSettle={() => model.toggleSettle(row)}
								onSettle={(input) => model.settle(row, input)}
								onEdit={() => onEditCustom(row)}
								onDelete={() => onDeleteCustom(row)}
							/>
						{/each}
					</ul>
				</section>
			{/each}
		</div>
	{:else}
		<div class="flex-1 flex items-center justify-center text-center">
			<p class="text-sm text-text-tertiary">No quests to show yet.</p>
		</div>
	{/if}
</div>

<style>
	.planet-tab {
		position: relative;
		display: inline-flex;
		align-items: center;
		gap: 0.375rem;
		padding: 0 0 0.5rem;
		font-size: 0.875rem;
		font-weight: 500;
		color: var(--color-text-tertiary);
		cursor: pointer;
		transition: color var(--duration-base) var(--ease-out);
	}
	.planet-tab:hover {
		color: var(--color-text-secondary);
	}
	.planet-tab-active {
		color: var(--color-text);
	}
	.planet-tab-active::after {
		content: '';
		position: absolute;
		left: 0;
		right: 0;
		bottom: -1px;
		height: 2px;
		border-radius: 1px;
		background: var(--color-accent);
	}
	.offered-dot {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--color-accent);
		box-shadow: 0 0 6px color-mix(in oklab, var(--color-accent) 70%, transparent);
	}
	.header-action {
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--color-accent);
		cursor: pointer;
		transition: color var(--duration-base) var(--ease-out);
	}
	.header-action:hover {
		color: var(--color-accent-hover);
	}
</style>
