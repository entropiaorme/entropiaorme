<script lang="ts">
	import Menu from '$lib/components/Menu.svelte';
	import { matchesQuery } from '$lib/view/search';
	import DefinitionCataloguePanel from '$lib/features/sessions/DefinitionCataloguePanel.svelte';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';
	import type { TableModel } from '$lib/view/tableModel.svelte';
	import type {
		HuntingOverallLine,
		HuntingSessionSection,
		HuntingSessionSortKey,
	} from './huntingModel.svelte';

	let {
		table,
		selected,
		overall,
		totalCount,
		onselect,
	}: {
		table: TableModel<HuntingSessionSection>;
		selected: HuntingSessionSection | null;
		overall: HuntingOverallLine;
		totalCount: number;
		onselect: (key: string | null) => void;
	} = $props();

	const displaySections = $derived([
		...table.filtered.filter((section) => !section.isUnassigned),
		...table.filtered.filter((section) => section.isUnassigned),
	]);
	const COL_NAME = 'min-w-0 flex-[1_1_8rem]';
	const COL_CYCLED = 'min-w-0 flex-[0_1_4rem]';
	const COL_LOOT_MU = 'min-w-0 flex-[0_1_4.5rem]';
	const COL_MU = 'min-w-0 flex-[0_1_4.5rem]';
	const COL_REALISED = 'min-w-0 flex-[0_1_7.5rem]';
	const rateTone = (value: number) => (value >= 1 ? 'text-positive' : 'text-negative');
	const selectedName = $derived(selected?.name ?? 'Overall');
	const showOverall = $derived(matchesQuery('Overall', table.search));
	const sortArrow = (key: HuntingSessionSortKey) =>
		table.sortKey === key ? (table.sortDir === 'asc' ? '↑' : '↓') : '';
	const sortDescription = (key: HuntingSessionSortKey, label: string) => {
		if (table.sortKey !== key) return `Sort by ${label}`;
		return `Sort by ${label}, currently ${table.sortDir === 'asc' ? 'ascending' : 'descending'}`;
	};
</script>

<Menu
	ariaLabel="Switch hunting view"
	overlay
	align="left"
	initialFocus="first-input"
	overlayOverflow="hidden"
	panelClass="w-[min(44rem,calc(100vw-1rem))] p-0"
	class="min-w-0"
>
	{#snippet trigger({ open, toggle, keydown })}
		<button
			type="button"
			class="group -ml-1.5 inline-flex max-w-full items-center gap-1.5 rounded-md px-1.5 py-1
				text-left transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover
				focus:outline-none focus:bg-surface-hover focus:[box-shadow:var(--shadow-glow)]"
			aria-haspopup="menu"
			aria-expanded={open}
			aria-label={`Switch hunting view (currently ${selectedName})`}
			onclick={() => {
				if (!open) table.search = '';
				toggle();
			}}
			onkeydown={(event) => {
				if (!open && event.key === 'ArrowDown') table.search = '';
				keydown(event);
			}}
		>
			<span class="flex min-w-0 items-center gap-2">
				<span class="truncate text-3xl font-bold leading-none tracking-tight text-text" title={selectedName}>
					{selectedName}
				</span>
				{#if selected?.isArchived}
					<span class="shrink-0 text-[0.625rem] font-medium uppercase tracking-wide text-text-tertiary">
						Archived
					</span>
				{/if}
			</span>
			<span class="shrink-0 text-text-secondary transition-colors group-hover:text-text" aria-hidden="true">
				<svg class="h-4 w-4 transition-transform {open ? 'rotate-180' : ''}" viewBox="0 0 20 20" fill="currentColor">
					<path fill-rule="evenodd" d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z" clip-rule="evenodd" />
				</svg>
			</span>
		</button>
	{/snippet}

	{#snippet children({ close })}
		<DefinitionCataloguePanel
			title="Choose session"
			count={totalCount + 1}
			bind:filter={table.search}
			hasMatches={showOverall || displaySections.length > 0}
			filterLabel="Filter analytics sessions"
			resultsTestId="hunting-session-results"
		>
			{#snippet results()}
				<div class="sticky top-0 z-10 flex items-center gap-2 border-b border-border/50 bg-surface-raised px-2.5 py-2 text-text-tertiary">
						<button type="button" class="eyebrow {COL_NAME} flex cursor-pointer items-center gap-1 text-left hover:text-text" aria-label={sortDescription('name', 'Session')} onclick={() => table.setSort('name')}>
							View {#if table.sortKey === 'name'}<span class="text-accent">{sortArrow('name')}</span>{/if}
						</button>
						<button type="button" class="eyebrow {COL_CYCLED} flex cursor-pointer items-center justify-end gap-1 text-right hover:text-text" aria-label={sortDescription('cycled', 'Cycled')} onclick={() => table.setSort('cycled')}>
							Cycled {#if table.sortKey === 'cycled'}<span class="text-accent">{sortArrow('cycled')}</span>{/if}
						</button>
						<span class="eyebrow {COL_LOOT_MU} text-right">Loot MU</span>
						<button type="button" class="eyebrow {COL_MU} flex cursor-pointer items-center justify-end gap-1 text-right hover:text-text" aria-label={sortDescription('muRate', 'MU Rate')} onclick={() => table.setSort('muRate')}>
							MU Rate {#if table.sortKey === 'muRate'}<span class="text-accent">{sortArrow('muRate')}</span>{/if}
						</button>
						<button type="button" class="eyebrow {COL_REALISED} flex cursor-pointer items-center justify-end gap-1 text-right hover:text-text" aria-label={sortDescription('realisedRate', 'Realised Rate')} onclick={() => table.setSort('realisedRate')}>
							Realised Rate {#if table.sortKey === 'realisedRate'}<span class="text-accent">{sortArrow('realisedRate')}</span>{/if}
						</button>
				</div>
				<div class="flex flex-col gap-0.5 p-1">
						{#if showOverall}<button
							type="button"
							role="menuitem"
							aria-current={selected === null ? 'true' : undefined}
							class="flex w-full items-center gap-2 rounded-md border px-2.5 py-2.5 text-left
								transition-[background-color,border-color] duration-[var(--duration-fast)]
								{selected === null
									? 'border-accent/35 bg-accent/[0.09]'
									: 'border-transparent hover:border-border/40 hover:bg-surface-hover'}"
							onclick={() => {
								if (selected !== null) onselect(null);
								close();
							}}
						>
							<span class="{COL_NAME} min-w-0 truncate text-sm font-medium tracking-tight {selected === null ? 'text-accent' : 'text-text'}">Overall</span>
							<span class="{COL_CYCLED} truncate text-right text-xs tabular-nums text-text">{formatPed(overall.cycled)}</span>
							<span class="{COL_LOOT_MU} truncate text-right text-xs tabular-nums text-text">{overall.lootMarkupFactor !== null ? formatPercent(overall.lootMarkupFactor) : NO_DATA}</span>
							<span class="{COL_MU} truncate text-right text-xs tabular-nums text-text">{overall.muRate !== null ? formatPercent(overall.muRate) : NO_DATA}</span>
							<span class="{COL_REALISED} truncate text-right text-xs tabular-nums font-medium {rateTone(overall.realisedRate)}">{formatPercent(overall.realisedRate)}</span>
						</button>{/if}
						{#each displaySections as section (section.key)}
							{@const current = section.key === selected?.key}
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
									if (!current) onselect(section.key);
									close();
								}}
							>
								<span class="{COL_NAME} flex min-w-0 items-center gap-1.5 text-sm font-medium tracking-tight {section.isUnassigned || section.isArchived ? 'text-text-tertiary' : current ? 'text-accent' : 'text-text'}">
									<span class="min-w-0 truncate" title={section.name}>{section.name}</span>
									{#if section.isArchived}
										<span class="shrink-0 text-[0.625rem] font-medium uppercase tracking-wide text-text-tertiary">Archived</span>
									{/if}
								</span>
								{#if section.isUnassigned}
									<span class="sr-only">Session metrics not applicable</span>
									<span class={COL_CYCLED} aria-hidden="true"></span>
									<span class={COL_LOOT_MU} aria-hidden="true"></span>
									<span class={COL_MU} aria-hidden="true"></span>
									<span class={COL_REALISED} aria-hidden="true"></span>
								{:else}
									<span class="{COL_CYCLED} truncate text-right text-xs tabular-nums text-text">{formatPed(section.cycled)}</span>
									<span class="{COL_LOOT_MU} truncate text-right text-xs tabular-nums text-text">{section.lootMarkupFactor !== null ? formatPercent(section.lootMarkupFactor) : NO_DATA}</span>
									<span class="{COL_MU} truncate text-right text-xs tabular-nums text-text">{section.muRate !== null ? formatPercent(section.muRate) : NO_DATA}</span>
									<span class="{COL_REALISED} truncate text-right text-xs tabular-nums font-medium {rateTone(section.realisedRate)}">{formatPercent(section.realisedRate)}</span>
								{/if}
							</button>
						{/each}
				</div>
			{/snippet}
		</DefinitionCataloguePanel>
	{/snippet}
</Menu>
