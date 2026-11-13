<script lang="ts">
	// The shell of an overlay control that shows one thing quietly and
	// expands into a panel saying more: the creature control and the tool in
	// hand both use it. The owner lays out the control (its label, what it
	// shows, any action beside it) and places the toggle this shell gives it;
	// the shell owns the toggle's wiring, the hover hold, Escape, and the
	// panel. The wrapper holds the panel too, so the pointer crossing from
	// the control into the panel stays inside it. The panel is a drawn
	// overlay surface only while it is open, so clicks reach the game
	// everywhere else.
	import type { Snippet } from 'svelte';
	import type { HoverPanel } from './hoverPanel.svelte';

	let {
		panel,
		label,
		panelLabel = undefined,
		panelId,
		toggleTitle,
		toggleTestId,
		toggleData = {},
		panelTestId,
		testId,
		class: className = '',
		control,
		content,
	}: {
		panel: Pick<HoverPanel, 'expanded' | 'toggle' | 'close' | 'pointerEnter' | 'pointerLeave'>;
		/** The control's accessible name, and the panel's unless it has its own. */
		label: string;
		panelLabel?: string;
		panelId: string;
		toggleTitle: string;
		toggleTestId?: string;
		/** `data-*` attributes for the toggle, keyed without the prefix. */
		toggleData?: Record<string, string>;
		panelTestId?: string;
		testId?: string;
		class?: string;
		/** The control, given the toggle to place: render it with what the
		 * toggle shows. */
		control: Snippet<[Snippet<[Snippet]>]>;
		/** What the open panel holds. */
		content: Snippet;
	} = $props();

	let toggleEl = $state<HTMLButtonElement | null>(null);
	let panelEl = $state<HTMLDivElement | null>(null);
	const dataAttributes = $derived(
		Object.fromEntries(Object.entries(toggleData).map(([key, value]) => [`data-${key}`, value])),
	);

	/** Escape closes the expanded control; focus that was inside it goes back
	 * to the toggle, so the keyboard is not left nowhere. */
	function escape(event: KeyboardEvent) {
		if (event.key !== 'Escape' || !panel.expanded) return;
		const inside = panelEl?.contains(document.activeElement) ?? false;
		panel.close();
		if (inside) toggleEl?.focus();
	}
</script>

<!-- Escape closes the expanded control wherever focus is in this window. -->
<svelte:window onkeydown={escape} />

{#snippet toggle(shows: Snippet)}
	<button
		bind:this={toggleEl}
		type="button"
		class="facet-toggle"
		aria-expanded={panel.expanded}
		aria-controls={panelId}
		title={toggleTitle}
		data-testid={toggleTestId}
		{...dataAttributes}
		onclick={panel.toggle}
	>
		{@render shows()}
		<svg viewBox="0 0 16 16" fill="currentColor" class="h-3 w-3 shrink-0 text-white/35" aria-hidden="true">
			<path d="M4.22 6.22a.75.75 0 0 1 1.06 0L8 8.94l2.72-2.72a.75.75 0 1 1 1.06 1.06l-3.25 3.25a.75.75 0 0 1-1.06 0L4.22 7.28a.75.75 0 0 1 0-1.06Z" />
		</svg>
	</button>
{/snippet}

<div
	class="relative {className}"
	data-testid={testId}
	role="group"
	aria-label={label}
	onpointerenter={panel.pointerEnter}
	onpointerleave={panel.pointerLeave}
>
	{@render control(toggle)}

	{#if panel.expanded}
		<!-- Drawn only while open, and marked as a surface so it takes
			 clicks; closed, its area is the game's again. -->
		<div
			bind:this={panelEl}
			id={panelId}
			class="facet-panel"
			data-overlay-surface
			data-testid={panelTestId}
			role="region"
			aria-label={panelLabel ?? label}
			tabindex="-1"
		>
			{@render content()}
		</div>
	{/if}
</div>

<style>
	.facet-toggle {
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
	.facet-toggle:hover,
	.facet-toggle:focus-visible {
		background: rgba(255, 255, 255, 0.06);
		outline: none;
	}

	/* Hung under the control, flush with it, so the pointer crosses
	   straight in. An edge, not a shadow: a shadow would fall outside the
	   surface. */
	.facet-panel {
		position: absolute;
		top: 100%;
		left: 0;
		z-index: 10;
		width: 15rem;
		margin-top: 8px;
		padding: 8px 10px;
		border-radius: 8px;
		background: rgba(10, 14, 23, 0.92);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}
</style>
