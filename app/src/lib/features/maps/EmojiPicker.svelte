<script lang="ts">
	import emojiData from 'emojibase-data/en/compact.json';
	import { createSearchIndex, isSearchQuery } from '$lib/view/search';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import { pinGlyph } from './pinIcons';

	type EmojiEntry = {
		label: string;
		unicode: string;
		tags?: string[];
		shortcodes?: string[];
		skins?: EmojiEntry[];
	};

	let {
		value,
		label,
		onselect,
	}: { value: string; label: string; onselect: (emoji: string) => void } = $props();
	let panel = $state<HTMLDivElement | null>(null);
	let query = $state('');
	let left = $state(0);
	let top = $state(0);

	const choices = (emojiData as EmojiEntry[]).flatMap((emoji) => [emoji, ...(emoji.skins ?? [])]);
	const defaults = ['📍', '🚩', '⭐️', '⛏️', '👾', '🌳', '🌀', '🏠️', '💧'];
	const defaultChoices = defaults
		.map((unicode) => choices.find((emoji) => emoji.unicode === unicode))
		.filter((emoji): emoji is EmojiEntry => emoji !== undefined);
	const index = createSearchIndex(choices, (emoji) => [
		emoji.label,
		...(emoji.tags ?? []),
		...(emoji.shortcodes ?? []),
	]);
	const results = $derived(isSearchQuery(query) ? index.rank(query, 9) : defaultChoices);

	function openPicker(event: MouseEvent) {
		const trigger = event.currentTarget as HTMLElement;
		const rect = trigger.getBoundingClientRect();
		const panelWidth = 176;
		const panelHeight = 210;
		const gap = 8;
		left = Math.min(Math.max(gap, rect.left), window.innerWidth - panelWidth - gap);
		top =
			rect.bottom + gap + panelHeight <= window.innerHeight
				? rect.bottom + gap
				: Math.max(gap, rect.top - panelHeight - gap);
		query = '';
		panel?.showPopover?.();
		requestAnimationFrame(() => panel?.querySelector<HTMLInputElement>('input')?.focus());
	}

	function choose(emoji: string) {
		onselect(emoji);
		panel?.hidePopover?.();
	}
</script>

<div class="min-w-0 space-y-1">
	<span class="block text-xs text-text-secondary">Emoji</span>
	<Button
		type="button"
		variant="secondary"
		class="h-9! w-9! px-0! text-xl"
		aria-label={label}
		aria-haspopup="dialog"
		onclick={openPicker}
	>
		{pinGlyph(value)}
	</Button>
</div>

<div
	bind:this={panel}
	popover="auto"
	role="dialog"
	aria-label="Choose emoji"
	class="fixed m-0 w-44 rounded-md border border-border bg-surface-raised p-2 text-text shadow-lg backdrop:bg-transparent"
	style:left="{left}px"
	style:top="{top}px"
>
	<Input bind:value={query} placeholder="Search emoji" aria-label="Search emoji" />
	<div class="mt-2 grid grid-cols-3 gap-1">
		{#each results as emoji (emoji.unicode)}
			<button
				type="button"
				aria-label={emoji.label}
				class="flex aspect-square items-center justify-center rounded-md border border-transparent text-xl hover:border-border-bright hover:bg-surface-hover focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent"
				onclick={() => choose(emoji.unicode)}
			>
				{emoji.unicode}
			</button>
		{/each}
	</div>
	{#if results.length === 0}
		<p class="py-3 text-center text-xs text-text-secondary">No matching emoji</p>
	{/if}
</div>
