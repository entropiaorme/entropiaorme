<script lang="ts">
	import type { SkillingForecastSource } from '$lib/api/commands.gen';
	import Menu from '$lib/components/Menu.svelte';
	import DefinitionCataloguePanel from '$lib/features/sessions/DefinitionCataloguePanel.svelte';
	import { formatPed } from '$lib/utils/format';
	import { formatHours, statusLabel } from './skillingFormat';

	// Choose which named session the forecast reads: each row carries its
	// own answer (cycling, time, and cost to the goal), so the choice is
	// made on the figures rather than by trial.
	let {
		sources,
		selected,
		onselect,
	}: {
		sources: SkillingForecastSource[];
		selected: SkillingForecastSource | null;
		onselect: (definitionId: number) => void;
	} = $props();

	type SortKey = 'ttCost' | 'cycledPed' | 'hours';

	let filter = $state('');
	// Cheapest first by default (the backend's order); time-first players
	// re-sort. Sessions that cannot answer always trail.
	let sortKey = $state<SortKey>('ttCost');
	const matches = $derived(
		sources
			.filter((source) =>
				source.name.toLocaleLowerCase().includes(filter.trim().toLocaleLowerCase()),
			)
			.map((source, index) => ({ source, index }))
			.sort((a, b) => {
				const aReady = a.source.status === 'ready';
				const bReady = b.source.status === 'ready';
				if (aReady !== bReady) return aReady ? -1 : 1;
				if (!aReady) return a.index - b.index;
				return a.source[sortKey] - b.source[sortKey] || a.index - b.index;
			})
			.map(({ source }) => source),
	);

	const COL_NAME = 'min-w-0 flex-[1_1_9rem]';
	const COL_FIGURE = 'min-w-0 flex-[0_1_4.75rem] text-right';

</script>

<Menu
	ariaLabel="Choose the named session to forecast from"
	overlay
	align="right"
	initialFocus="first-input"
	overlayOverflow="hidden"
	panelClass="w-[min(36rem,calc(100vw-1rem))] p-0"
	class="min-w-0"
>
	{#snippet trigger({ open, toggle, keydown })}
		<button
			type="button"
			class="group inline-flex max-w-full items-center gap-1.5 rounded-md px-2 py-1 text-left
				transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover
				focus:outline-none focus:bg-surface-hover focus:[box-shadow:var(--shadow-glow)] cursor-pointer"
			aria-haspopup="menu"
			aria-expanded={open}
			aria-label={`Choose the named session (currently ${selected?.name ?? 'none'})`}
			onclick={() => {
				if (!open) filter = '';
				toggle();
			}}
			onkeydown={keydown}
		>
			<span class="eyebrow mr-1">From</span>
			<span class="truncate text-sm font-semibold tracking-tight text-text" title={selected?.name}>
				{selected?.name ?? 'Choose a session'}
			</span>
			<svg
				class="h-4 w-4 shrink-0 text-text-secondary transition-transform group-hover:text-text {open ? 'rotate-180' : ''}"
				viewBox="0 0 20 20"
				fill="currentColor"
				aria-hidden="true"
			>
				<path
					fill-rule="evenodd"
					d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
					clip-rule="evenodd"
				/>
			</svg>
		</button>
	{/snippet}

	{#snippet children({ close })}
		<DefinitionCataloguePanel
			title="Forecast from"
			count={sources.length}
			bind:filter
			hasMatches={matches.length > 0}
			filterLabel="Filter named sessions"
			resultsTestId="skilling-source-results"
		>
			{#snippet results()}
				<div
					class="sticky top-0 z-10 flex items-center gap-2 border-b border-border/50 bg-surface-raised px-2.5 py-2 text-text-tertiary"
				>
					<span class="eyebrow {COL_NAME}">Session</span>
					{#each [['cycledPed', 'Cycled'], ['hours', 'Time'], ['ttCost', 'TT cost']] as [key, label] (key)}
						<button
							type="button"
							class="eyebrow {COL_FIGURE} flex cursor-pointer items-center justify-end gap-1 hover:text-text
								{sortKey === key ? 'text-text-secondary' : ''}"
							aria-label={sortKey === key ? `Sorted by ${label}, least first` : `Sort by ${label}, least first`}
							aria-pressed={sortKey === key}
							onclick={() => (sortKey = key as SortKey)}
						>
							{label}
							{#if sortKey === key}<span class="text-accent" aria-hidden="true">&uarr;</span>{/if}
						</button>
					{/each}
				</div>
				<div class="flex flex-col gap-0.5 p-1">
					{#each matches as source (source.definitionId)}
						{@const current = source.definitionId === selected?.definitionId}
						{@const ready = source.status === 'ready'}
						<button
							type="button"
							role="menuitem"
							aria-current={current ? 'true' : undefined}
							class="flex w-full items-center gap-2 rounded-md border px-2.5 py-2.5 text-left
								transition-[background-color,border-color] duration-[var(--duration-fast)]
								{current
								? 'border-accent/35 bg-accent/[0.09]'
								: 'border-transparent hover:border-border/40 hover:bg-surface-hover'}"
							onclick={() => {
								if (!current) onselect(source.definitionId);
								close();
							}}
						>
							<span
								class="{COL_NAME} flex items-center gap-1.5 text-sm font-medium tracking-tight
									{!ready ? 'text-text-tertiary' : current ? 'text-accent' : 'text-text'}"
							>
								<span class="min-w-0 truncate" title={source.name}>{source.name}</span>
								{#if source.archived}
									<span class="shrink-0 text-[0.625rem] font-medium uppercase tracking-wide text-text-tertiary">
										Archived
									</span>
								{/if}
							</span>
							{#if ready}
								<span class="{COL_FIGURE} truncate text-xs tabular-nums text-text">{formatPed(source.cycledPed)}</span>
								<span class="{COL_FIGURE} truncate text-xs tabular-nums text-text">{formatHours(source.hours)}</span>
								<span class="{COL_FIGURE} truncate text-xs tabular-nums font-medium text-text">{formatPed(source.ttCost)}</span>
							{:else}
								<span class="min-w-0 flex-[0_1_14.25rem] truncate text-right text-xs text-text-tertiary">
									{statusLabel(source.status)}
								</span>
							{/if}
						</button>
					{/each}
				</div>
			{/snippet}
		</DefinitionCataloguePanel>
	{/snippet}
</Menu>
