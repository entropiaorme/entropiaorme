<script lang="ts" generics="T">
	import type { Snippet } from 'svelte';
	import SearchInput from './SearchInput.svelte';

	/**
	 * Structural contract for the typeahead model this presenter renders.
	 * A `createTypeahead` instance satisfies it; the shape is declared here
	 * so the presenter stays decoupled from any one factory.
	 */
	interface PickerModel {
		query: string;
		results: T[];
		selected: T | null;
		loading: boolean;
		error: string | null;
		/** The trimmed query the current results answer, once a search settles. */
		settledQuery?: string | null;
		select(item: T): void;
		clear(): void;
	}

	let {
		id,
		placeholder,
		model,
		result,
		selection,
		extraRow,
		class: className = '',
		dropdownClass = '',
	}: {
		id: string;
		placeholder?: string;
		model: PickerModel;
		/** Renders one dropdown row for a search hit; `query` is the text it
		 * answers, for `SearchHighlight`. */
		result: Snippet<[{ item: T; query: string }]>;
		/** Renders the confirmed selection inside the search box. */
		selection: Snippet<[{ item: T }]>;
		/** Optional trailing dropdown row (e.g. an "Add custom" affordance). */
		extraRow?: Snippet;
		class?: string;
		dropdownClass?: string;
	} = $props();

	const listboxId = $derived(`${id}-listbox`);

	let activeIndex = $state(-1);
	// Escape dismisses the dropdown for the query it was pressed on; editing
	// the query brings it back. The presenter hides rows rather than mutating
	// the model's result list, which it does not own.
	let dismissedForQuery = $state<string | null>(null);

	// A settled search that found nothing says so rather than leaving the
	// box looking unresponsive.
	const noMatches = $derived(
		!model.loading &&
			!model.error &&
			model.results.length === 0 &&
			model.settledQuery != null &&
			model.settledQuery !== '' &&
			model.settledQuery === model.query.trim(),
	);

	const showDropdown = $derived(
		!model.selected &&
			dismissedForQuery !== model.query &&
			(model.results.length > 0 ||
				noMatches ||
				(!!extraRow && model.query.trim().length > 0)),
	);

	let listbox = $state<HTMLDivElement>();

	// Keep the keyboard-highlighted row in view inside the scrolling list.
	$effect(() => {
		if (activeIndex < 0 || !listbox) return;
		const option = listbox.querySelector<HTMLElement>(`#${CSS.escape(`${id}-option-${activeIndex}`)}`);
		option?.scrollIntoView?.({ block: 'nearest' });
	});

	// A fresh result set restarts the highlight.
	$effect(() => {
		void model.results;
		activeIndex = -1;
	});

	function handleKeydown(e: KeyboardEvent) {
		if (!showDropdown) return;
		const count = model.results.length;
		switch (e.key) {
			case 'ArrowDown':
				e.preventDefault();
				if (count > 0) activeIndex = (activeIndex + 1) % count;
				break;
			case 'ArrowUp':
				e.preventDefault();
				if (count > 0) activeIndex = activeIndex <= 0 ? count - 1 : activeIndex - 1;
				break;
			case 'Enter':
				if (count > 0) {
					e.preventDefault();
					model.select(model.results[activeIndex >= 0 ? activeIndex : 0]);
				}
				break;
			case 'Escape':
				// The innermost layer: a dialog listening on the window would
				// otherwise close on the same press.
				e.preventDefault();
				e.stopPropagation();
				dismissedForQuery = model.query;
				activeIndex = -1;
				break;
		}
	}
</script>

<div class={className}>
	{#if model.selected}
		<!-- The search box itself holds the confirmed selection; the X (or a
		     click anywhere on it) releases it back into a search box. -->
		<button
			{id}
			type="button"
			class="relative w-full h-9 pl-3 pr-8 text-sm bg-surface text-text rounded-md
				border border-border flex items-center gap-3 min-w-0 text-left cursor-pointer
				transition-[border-color] duration-[var(--duration-base)] ease-[var(--ease-out)]
				hover:border-border-bright
				focus:outline-none focus:border-accent/60"
			onclick={() => model.clear()}
		>
			{@render selection({ item: model.selected })}
			<span class="sr-only">, clear selection</span>
			<span
				class="absolute right-2 top-1/2 -translate-y-1/2 h-5 w-5 flex items-center
					justify-center rounded-full text-text-tertiary"
				aria-hidden="true"
			>
				<svg
					xmlns="http://www.w3.org/2000/svg"
					viewBox="0 0 20 20"
					fill="currentColor"
					class="h-3.5 w-3.5"
				>
					<path
						d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"
					/>
				</svg>
			</span>
		</button>
	{:else}
		<SearchInput
			{id}
			{placeholder}
			bind:value={model.query}
			loading={model.loading}
			role="combobox"
			aria-expanded={showDropdown}
			aria-autocomplete="list"
			aria-controls={showDropdown ? listboxId : undefined}
			aria-activedescendant={activeIndex >= 0 ? `${id}-option-${activeIndex}` : undefined}
			onkeydown={handleKeydown}
		/>
	{/if}

	{#if showDropdown}
		<div
			bind:this={listbox}
			id={listboxId}
			role="listbox"
			aria-label="Suggestions"
			class="mt-1 bg-surface border border-border rounded-md overflow-hidden max-h-48 overflow-y-auto {dropdownClass}"
		>
			{#each model.results as item, i}
				<button
					id="{id}-option-{i}"
					role="option"
					aria-selected={i === activeIndex}
					tabindex="-1"
					class="w-full text-left px-3 py-2 text-sm hover:bg-surface-hover
						transition-colors duration-[var(--duration-fast)] cursor-pointer
						flex items-center justify-between {i === activeIndex ? 'bg-surface-hover' : ''}"
					onclick={() => model.select(item)}
				>
					{@render result({ item, query: model.query })}
				</button>
			{/each}
			{#if noMatches}
				<p class="px-3 py-2 text-sm text-text-tertiary">
					No matches for “{model.query.trim()}”
				</p>
			{/if}
			{#if extraRow}
				{@render extraRow()}
			{/if}
		</div>
	{/if}

	{#if model.error}
		<p class="mt-1 text-xs text-negative">{model.error}</p>
	{/if}
</div>
