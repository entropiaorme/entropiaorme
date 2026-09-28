<script lang="ts">
	import { onMount, tick } from 'svelte';
	import Badge from '$lib/components/Badge.svelte';
	import PickerInput from '$lib/components/PickerInput.svelte';
	import { IconStar } from '$lib/icons';
	import { createTypeahead } from '$lib/view/typeahead.svelte';
	import {
		filterRows,
		loadFavouriteProfessions,
		rowToTarget,
		saveFavouriteProfessions,
		targetLabel,
		type CodexRankingTarget,
		type PickerRow,
	} from './codexRankingTarget';

	let {
		professions,
		target,
		onchange,
		align = 'right',
		variant = 'field',
		includeNone = true,
		levels,
		hp,
		class: className = '',
	}: {
		/** Profession names available as ranking targets. */
		professions: string[];
		target: CodexRankingTarget;
		onchange: (target: CodexRankingTarget) => void;
		/** Which edge the dropdown panel hangs from: match the picker's
		 *  side of the page so the panel never leaves the window. */
		align?: 'left' | 'right';
		/** `field` is a form control; `headline` makes the chosen target the
		 *  surface's title, the way a session view names its session. */
		variant?: 'field' | 'headline';
		/** Offer the "No profession" row (a surface where no target is not a
		 *  choice leaves it out). */
		includeNone?: boolean;
		/** Current level per profession name, shown beside each row. */
		levels?: Record<string, number>;
		/** Current HP, shown beside the HP row. */
		hp?: number;
		class?: string;
	} = $props();

	let open = $state(false);
	let root = $state<HTMLDivElement | null>(null);
	let favourites = $state<string[]>([]);

	onMount(() => {
		loadFavouriteProfessions().then(list => (favourites = list));
	});

	const model = createTypeahead<PickerRow>({
		search: async query => filterRows(professions, favourites, query, { includeNone }),
		debounceMs: 50,
		minLength: 0,
	});

	// A selection in the embedded picker IS the choice: hand the target
	// up and close (the picker's own selection chip never shows).
	$effect(() => {
		const row = model.selected;
		if (row) {
			onchange(rowToTarget(row));
			close();
		}
	});

	async function openPanel() {
		open = true;
		model.clear();
		model.query = '';
		model.refresh();
		await tick();
		root?.querySelector('input')?.focus();
	}

	function close() {
		open = false;
		model.cancel();
		model.clear();
	}

	// While open: dismiss on any click outside the component.
	$effect(() => {
		if (!open) return;
		const onDocumentClick = (e: MouseEvent) => {
			if (root && e.target instanceof Node && !root.contains(e.target)) {
				close();
			}
		};
		document.addEventListener('click', onDocumentClick, true);
		return () => document.removeEventListener('click', onDocumentClick, true);
	});

	function handleRootKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && open) {
			e.preventDefault();
			e.stopPropagation();
			close();
		}
	}

	const isFavourite = $derived(target.kind === 'profession' && favourites.includes(target.name));

	async function toggleFavourite() {
		if (target.kind !== 'profession') return;
		const name = target.name;
		favourites = favourites.includes(name)
			? favourites.filter(entry => entry !== name)
			: [...favourites, name];
		await saveFavouriteProfessions(favourites);
	}
</script>

<div
	class="relative flex items-center gap-1 {className}"
	bind:this={root}
	onkeydown={handleRootKeydown}
	role="presentation"
>
	{#if variant === 'headline'}
		<button
			class="group -ml-1.5 inline-flex max-w-full min-w-0 items-center gap-2 rounded-md px-1.5 py-1 text-left
				transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover
				focus:outline-none focus:bg-surface-hover focus:[box-shadow:var(--shadow-glow)] cursor-pointer"
			aria-haspopup="listbox"
			aria-expanded={open}
			aria-label={`Choose what to skill up (currently ${targetLabel(target)})`}
			onclick={() => (open ? close() : openPanel())}
		>
			<span
				class="truncate text-3xl font-bold leading-tight tracking-tight
					{target.kind === 'none' ? 'text-text-secondary' : 'text-text'}"
				title={targetLabel(target)}
			>
				{target.kind === 'none' ? 'Choose a target' : targetLabel(target)}
			</span>
			<svg
				class="h-5 w-5 shrink-0 text-text-secondary transition-[transform,color] group-hover:text-text {open ? 'rotate-180' : ''}"
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
	{:else}
		<button
			class="h-9 pl-3 pr-8 text-sm bg-surface/70 text-text rounded-md border border-border cursor-pointer text-left truncate min-w-40 relative
				transition-colors hover:border-border-bright focus:outline-none focus:border-accent/60
				{target.kind === 'none' ? 'text-text-secondary' : ''}"
			aria-haspopup="listbox"
			aria-expanded={open}
			onclick={() => (open ? close() : openPanel())}
		>
			{targetLabel(target)}
			<svg
				class="absolute right-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-text-tertiary pointer-events-none"
				xmlns="http://www.w3.org/2000/svg"
				viewBox="0 0 20 20"
				fill="currentColor"
				aria-hidden="true"
			>
				<path
					fill-rule="evenodd"
					d="M5.23 7.21a.75.75 0 011.06.02L10 11.06l3.71-3.83a.75.75 0 111.08 1.04l-4.25 4.39a.75.75 0 01-1.08 0L5.21 8.27a.75.75 0 01.02-1.06z"
					clip-rule="evenodd"
				/>
			</svg>
		</button>
	{/if}

	{#if target.kind === 'profession'}
		<button
			class="w-7 {variant === 'headline' ? 'h-10' : 'h-9'} flex shrink-0 items-center justify-center rounded-md transition-colors cursor-pointer
				{isFavourite ? 'text-warning' : 'text-text-tertiary hover:text-text'}"
			title={isFavourite ? 'Remove from favourites' : 'Add to favourites'}
			aria-label={isFavourite ? 'Remove from favourites' : 'Add to favourites'}
			aria-pressed={isFavourite}
			onclick={toggleFavourite}
		>
			<IconStar />
		</button>
	{/if}

	{#if open}
		<!-- The panel hangs from the edge named by `align` and expands
		     toward the page's centre to stay on screen (right for the
		     codex picker at the window's right edge, left for the
		     recommender picker at the left). -->
		<div
			class="absolute {align === 'right' ? 'right-0' : 'left-0'} top-full mt-1 {levels ? 'w-80' : 'w-72'} z-20 bg-surface border border-border rounded-md shadow-lg p-2"
		>
			<PickerInput
				id="codex-profession-picker"
				placeholder="Search professions..."
				dropdownClass={variant === 'headline' ? 'max-h-80' : ''}
				{model}
			>
				{#snippet result({ item }: { item: PickerRow })}
					<span class="flex items-center gap-2 min-w-0">
						{#if item.kind === 'profession' && item.favourite}
							<span class="text-warning shrink-0"><IconStar /></span>
						{/if}
						<span class="truncate">
							{item.kind === 'profession' ? item.name : item.label}
						</span>
					</span>
					{#if item.kind === 'family'}
						<Badge variant="accent">Family</Badge>
					{:else if item.kind === 'hp'}
						{#if hp !== undefined}
							<span class="shrink-0 pl-2 text-xs tabular-nums text-text-tertiary">{hp} HP</span>
						{:else}
							<Badge variant="positive">HP</Badge>
						{/if}
					{:else if item.kind === 'profession' && levels?.[item.name] !== undefined}
						<span class="shrink-0 pl-2 text-xs tabular-nums text-text-tertiary">
							Lv {levels[item.name].toFixed(2)}
						</span>
					{/if}
				{/snippet}
				{#snippet selection({ item }: { item: PickerRow })}
					<!-- At most a one-frame flash: a selection immediately
					     hands the target up and closes the panel. -->
					<span>{targetLabel(rowToTarget(item))}</span>
				{/snippet}
			</PickerInput>
		</div>
	{/if}
</div>
