<script lang="ts">
	// A line of the overlay that opens a sub-window saying more about it:
	// what the line shows, as a button, with a mark that it opens. A click
	// opens the sub-window and a second closes it; the rest of how it
	// behaves is the sub-windows' own (see `lib/windows/subWindow.ts`).
	import type { Snippet } from 'svelte';

	let {
		open,
		title,
		testId = undefined,
		data = {},
		class: className = '',
		onToggle,
		children,
	}: {
		/** Its sub-window is open. */
		open: boolean;
		title: string;
		testId?: string;
		/** `data-*` attributes, keyed without the prefix. */
		data?: Record<string, string>;
		class?: string;
		/** Clicked: the button is what the sub-window hangs from. */
		onToggle: (anchor: HTMLElement) => void | Promise<void>;
		children: Snippet;
	} = $props();

	const dataAttributes = $derived(
		Object.fromEntries(Object.entries(data).map(([key, value]) => [`data-${key}`, value])),
	);
</script>

<button
	type="button"
	class="sub-toggle {className}"
	class:sub-toggle-open={open}
	aria-haspopup="dialog"
	aria-expanded={open}
	{title}
	data-testid={testId}
	{...dataAttributes}
	onclick={(event) => void onToggle(event.currentTarget)}
>
	{@render children()}
	<svg viewBox="0 0 16 16" fill="currentColor" class="chevron" aria-hidden="true">
		<path d="M4.22 6.22a.75.75 0 0 1 1.06 0L8 8.94l2.72-2.72a.75.75 0 1 1 1.06 1.06l-3.25 3.25a.75.75 0 0 1-1.06 0L4.22 7.28a.75.75 0 0 1 0-1.06Z" />
	</svg>
</button>

<style>
	.sub-toggle {
		display: flex;
		align-items: center;
		gap: 4px;
		min-width: 0;
		padding: 2px 4px;
		border: none;
		border-radius: 4px;
		background: transparent;
		font-size: 14px;
		font-weight: 500;
		text-align: left;
		cursor: pointer;
	}
	.sub-toggle:hover,
	.sub-toggle:focus-visible,
	.sub-toggle-open {
		background: rgba(255, 255, 255, 0.06);
		outline: none;
	}
	.chevron {
		width: 12px;
		height: 12px;
		flex-shrink: 0;
		color: rgba(255, 255, 255, 0.35);
		transition: transform 120ms ease;
	}
	.sub-toggle-open .chevron {
		transform: rotate(180deg);
		color: rgba(255, 255, 255, 0.6);
	}
	@media (prefers-reduced-motion: reduce) {
		.chevron {
			transition: none;
		}
	}
</style>
