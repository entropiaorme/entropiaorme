<script lang="ts">
	// The body of a sub-window that says more about one of the overlay's
	// lines: the glass, its heading, and the type its content is set in. A
	// name wraps here rather than being cut: this is where there is room.
	import type { Snippet } from 'svelte';

	let {
		heading,
		label = heading,
		testId = undefined,
		children,
	}: {
		heading: string;
		/** The panel's accessible name, where the heading is not enough. */
		label?: string;
		testId?: string;
		children: Snippet;
	} = $props();
</script>

<div class="sw-panel" role="region" aria-label={label} data-testid={testId}>
	<span class="sw-head">{heading}</span>
	{@render children()}
</div>

<style>
	.sw-panel {
		width: 17rem;
		padding: 9px 11px 10px;
		border-radius: 8px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		background: rgba(10, 14, 23, 0.96);
		color: rgba(255, 255, 255, 0.95);
		font-size: 13px;
	}
	.sw-head {
		display: block;
		margin-bottom: 6px;
		font-size: 11px;
		font-weight: 500;
		color: #38bdf8;
	}
	.sw-panel :global(.panel-key) {
		font-size: 11px;
		font-weight: 400;
		color: rgba(255, 255, 255, 0.58);
	}
	.sw-panel :global(.panel-value) {
		font-size: 13px;
		font-weight: 500;
		color: rgba(255, 255, 255, 0.95);
		overflow-wrap: anywhere;
	}
	.sw-panel :global(.panel-note) {
		font-size: 11px;
		font-weight: 400;
		line-height: 1.4;
		color: rgba(255, 255, 255, 0.58);
	}
	.sw-panel :global(.panel-faint) {
		color: rgba(255, 255, 255, 0.4);
	}
	.sw-panel :global(.panel-link) {
		padding: 0;
		border: none;
		background: transparent;
		font-size: 11px;
		color: rgba(255, 255, 255, 0.58);
		cursor: pointer;
	}
	.sw-panel :global(.panel-link:hover:not(:disabled)) {
		color: rgba(255, 255, 255, 0.95);
	}
	.sw-panel :global(.panel-section + .panel-section) {
		margin-top: 8px;
		padding-top: 8px;
		border-top: 1px solid rgba(255, 255, 255, 0.075);
	}
	.sw-panel :global(.panel-chip) {
		padding: 3px 8px;
		border: 1px solid rgba(255, 255, 255, 0.14);
		border-radius: 5px;
		background: rgba(255, 255, 255, 0.05);
		font-size: 11px;
		font-weight: 500;
		color: rgba(255, 255, 255, 0.9);
		white-space: nowrap;
		cursor: pointer;
	}
	.sw-panel :global(.panel-chip:hover:not(:disabled)) {
		background: rgba(255, 255, 255, 0.1);
	}
	.sw-panel :global(.panel-chip:disabled),
	.sw-panel :global(.panel-link:disabled) {
		opacity: 0.45;
		cursor: default;
	}
</style>
