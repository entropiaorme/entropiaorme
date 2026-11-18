<script lang="ts">
	import Menu from '$lib/components/Menu.svelte';
	import DefinitionCataloguePanel from './DefinitionCataloguePanel.svelte';
	import { filterDefinitions } from './definitionCatalogue';
	import type { DefinitionsModel } from './definitionsModel.svelte';

	let {
		model,
		selectedId = null,
		onOpenAuthoring
	}: {
		model: DefinitionsModel;
		/** The session selected for the next run (the snapshot's
		 * `sessionDefinitionId`), carried in configuration so it survives
		 * app restarts and reads as the last session played. */
		selectedId?: string | null;
		/** Open the authoring environment; an id means edit-in-place,
		 * null means create. */
		onOpenAuthoring: (definitionId: string | null) => void;
	} = $props();

	let filter = $state('');
	const selected = $derived(
		model.definitions.find((definition) => definition.id === selectedId) ?? null
	);
	const matchingDefinitions = $derived(filterDefinitions(model.definitions, filter));
</script>

<div class="flex items-center gap-1.5 min-w-0">
	<h2 class="text-[15px] font-semibold text-text tracking-tight shrink-0">Session:</h2>

	{#if model.definitions.length > 0}
		<Menu
			ariaLabel="Switch session"
			overlay
			align="left"
			initialFocus="first-input"
			overlayOverflow="hidden"
			panelClass="w-[min(24rem,calc(100vw-1rem))] p-0"
		>
			{#snippet trigger({ open, toggle, keydown })}
				<button
					type="button"
					class="inline-flex max-w-[18rem] items-center gap-1 rounded-md px-1.5 py-0.5 cursor-pointer
						text-[15px] font-semibold tracking-tight
						transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover
						disabled:cursor-not-allowed disabled:opacity-60
						{selected ? 'text-accent' : 'text-text-tertiary hover:text-text'}"
					aria-haspopup="menu"
					aria-expanded={open}
					aria-label={selected ? `Switch session (currently ${selected.name})` : 'Choose a session'}
					title="Switch session"
					disabled={model.selecting}
					onclick={() => {
						if (!open) filter = '';
						toggle();
					}}
					onkeydown={(event) => {
						if (!open && event.key === 'ArrowDown') filter = '';
						keydown(event);
					}}
				>
					<span class="truncate">{selected ? selected.name : 'Choose'}</span>
					<span class="text-text-secondary" aria-hidden="true">&#x2304;</span>
				</button>
			{/snippet}

			{#snippet children({ close })}
				<DefinitionCataloguePanel
					title="Choose session"
					count={model.definitions.length}
					bind:filter
					hasMatches={matchingDefinitions.length > 0}
					filterLabel="Filter sessions"
					resultsTestId="definition-results"
				>
					{#snippet results()}
						{#if matchingDefinitions.length > 0}
							{#each matchingDefinitions as definition (definition.id)}
								{@const current = selectedId === definition.id}
								<button
									type="button"
									role="menuitem"
									aria-current={current ? 'true' : undefined}
									class="mt-0.5 flex w-full items-center gap-2 rounded px-2.5 py-2 text-left
										text-sm cursor-pointer transition-colors duration-[var(--duration-fast)]
										{current
										? 'bg-accent/10 text-accent'
										: 'text-text-secondary hover:bg-surface-hover hover:text-text'}"
									onclick={() => {
										if (!current) void model.select(definition.id);
										close();
									}}
								>
									<span class="min-w-0 flex-1 truncate">{definition.name}</span>
									{#if current}
										<svg
											class="h-4 w-4 shrink-0"
											viewBox="0 0 20 20"
											fill="currentColor"
											aria-hidden="true"
										>
											<path fill-rule="evenodd" d="M16.704 5.29a1 1 0 010 1.414l-8 8a1 1 0 01-1.414 0l-4-4a1 1 0 011.414-1.414L8 12.586l7.296-7.296a1 1 0 011.408 0z" clip-rule="evenodd" />
										</svg>
									{/if}
								</button>
							{/each}
						{/if}
					{/snippet}

					{#snippet footer()}
						<button
							type="button"
							role="menuitem"
							class="rounded px-2 py-1.5 text-xs text-text-secondary transition-colors
								hover:bg-surface-hover hover:text-text disabled:cursor-not-allowed disabled:opacity-40"
							disabled={!selected}
							onclick={() => {
								if (!selected) return;
								close();
								onOpenAuthoring(selected.id);
							}}
						>
							Edit current
						</button>
						<button
							type="button"
							role="menuitem"
							class="rounded px-2 py-1.5 text-xs font-medium text-accent transition-colors
								hover:bg-accent/10 hover:text-accent-hover"
							onclick={() => {
								close();
								onOpenAuthoring(null);
							}}
						>
							+ New session
						</button>
					{/snippet}
				</DefinitionCataloguePanel>
			{/snippet}
		</Menu>
	{:else}
		<button
			type="button"
			class="filter-chip shrink-0"
			title="Create a session"
			disabled={model.selecting}
			onclick={() => onOpenAuthoring(null)}
		>
			+ New
		</button>
	{/if}
</div>
