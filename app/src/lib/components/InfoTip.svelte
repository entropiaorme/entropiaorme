<script lang="ts">
	import type { Snippet } from 'svelte';
	import { portal } from '$lib/utils/portal';

	// A hover/focus text popover (no click). By default the trigger is a
	// small circled "i"; pass a `trigger` snippet to hang the same popover
	// off any glyph instead. `align` picks the edge it grows from so it
	// never spills past a right-hung control.
	//
	// The popover is portalled to the document body and positioned fixed
	// from the trigger's rect (the Menu overlay's idiom): an ancestor with
	// `overflow: auto` would otherwise clip it, and the bounded-scroll
	// panes these tips live in are exactly such ancestors. It flips above
	// the trigger when there is no room below and clamps to the viewport
	// on both axes.
	let {
		children,
		trigger,
		align = 'right',
		width = 'w-72',
		label = 'More information',
	}: {
		children: Snippet;
		trigger?: Snippet;
		align?: 'left' | 'right';
		width?: string;
		label?: string;
	} = $props();

	// Each instance needs its own id so the trigger describes its own popover
	// (id references resolve document-wide, so the portal does not break it).
	const tooltipId = `infotip-${crypto.randomUUID()}`;

	let open = $state(false);
	let triggerEl = $state<HTMLElement | null>(null);
	let tipEl = $state<HTMLElement | null>(null);
	let pos = $state({ top: 0, left: 0 });

	const VIEWPORT_MARGIN = 8;
	const TRIGGER_GAP = 8;

	function position() {
		if (!triggerEl || !tipEl) return;
		const anchor = triggerEl.getBoundingClientRect();
		const tipWidth = Math.min(
			tipEl.offsetWidth,
			Math.max(0, window.innerWidth - VIEWPORT_MARGIN * 2),
		);
		const tipHeight = tipEl.offsetHeight;

		let left = align === 'right' ? anchor.right - tipWidth : anchor.left;
		left = Math.max(
			VIEWPORT_MARGIN,
			Math.min(left, window.innerWidth - tipWidth - VIEWPORT_MARGIN),
		);

		let top = anchor.bottom + TRIGGER_GAP;
		if (top + tipHeight > window.innerHeight - VIEWPORT_MARGIN) {
			const above = anchor.top - tipHeight - TRIGGER_GAP;
			// Flip only when there is genuinely room above; otherwise clamp,
			// so a tall tip still starts on screen rather than off its top.
			top = above >= VIEWPORT_MARGIN ? above : VIEWPORT_MARGIN;
		}
		pos = { top, left };
	}

	// While open, track the trigger through container scrolls and window
	// resizes so the tip never drifts away from the glyph it explains.
	$effect(() => {
		if (!open) return;
		position();
		const reposition = () => position();
		window.addEventListener('scroll', reposition, true);
		window.addEventListener('resize', reposition);
		return () => {
			window.removeEventListener('scroll', reposition, true);
			window.removeEventListener('resize', reposition);
		};
	});
</script>

<span class="relative inline-flex items-center">
	<button
		bind:this={triggerEl}
		type="button"
		aria-label={label}
		aria-describedby={tooltipId}
		onmouseenter={() => (open = true)}
		onmouseleave={() => (open = false)}
		onfocus={() => (open = true)}
		onblur={() => (open = false)}
		onkeydown={(event) => {
			if (event.key === 'Escape' && open) {
				event.stopPropagation();
				open = false;
			}
		}}
		class="inline-flex items-center justify-center rounded leading-none cursor-help
			transition-colors duration-[var(--duration-fast)]
			focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent/60
			{trigger
			? ''
			: 'h-4 w-4 rounded-full border border-border/70 text-[10px] font-semibold text-text-tertiary hover:text-text hover:border-border'}"
	>
		{#if trigger}{@render trigger()}{:else}i{/if}
	</button>
</span>

<span
	bind:this={tipEl}
	use:portal
	id={tooltipId}
	role="tooltip"
	style="top: {pos.top}px; left: {pos.left}px;"
	class:invisible={!open}
	class="pointer-events-none fixed z-[60] {width}
		transition-[opacity,transform] duration-[var(--duration-base)] ease-[var(--ease-out)]
		{open ? 'opacity-100 translate-y-0' : 'opacity-0 translate-y-0.5'}"
>
	<span
		class="block rounded-lg border border-border/70 bg-surface-raised p-3 text-left
			shadow-[var(--shadow-lg)]"
	>
		{@render children()}
	</span>
</span>
