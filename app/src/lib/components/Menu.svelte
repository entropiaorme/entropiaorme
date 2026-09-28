<script lang="ts">
	import type { Snippet } from 'svelte';
	import { portal } from '$lib/utils/portal';

	interface MenuItem {
		label: string;
		danger?: boolean;
		onSelect: () => void;
	}

	let {
		ariaLabel = 'Open menu',
		items,
		trigger,
		children,
		class: className = '',
		panelClass = '',
		overlay = false,
		align = 'right',
		initialFocus = 'first-item',
		overlayOverflow = 'auto'
	}: {
		/** Accessible name for the default three-dot trigger. */
		ariaLabel?: string;
		/** Declarative menu items; rendered before any custom panel content. */
		items?: MenuItem[];
		/**
		 * Custom trigger. When provided it replaces the default three-dot
		 * button; the snippet is responsible for wiring `toggle` to a button
		 * and carrying `aria-haspopup="menu"` / `aria-expanded`.
		 */
		trigger?: Snippet<[{ open: boolean; toggle: () => void; keydown: (event: KeyboardEvent) => void }]>;
		/**
		 * Custom panel content, rendered after `items`. Elements marked
		 * `role="menuitem"` participate in the keyboard roving focus.
		 */
		children?: Snippet<[{ close: () => void }]>;
		class?: string;
		panelClass?: string;
		/**
		 * Escape an ancestor's overflow clipping by positioning the panel
		 * against the viewport instead of the trigger.
		 *
		 * The default absolute panel is laid out inside whatever scroll
		 * box contains it. A scrollable ancestor (a table wrapper with
		 * `overflow-x-auto` is the usual one: CSS computes the other axis
		 * to `auto` with it) therefore clips the panel and grows its own
		 * scroll extent to fit, which reads as the list being squashed to
		 * make room rather than the menu floating over it.
		 *
		 * Opt-in because `panelClass` carries the positioning utilities at
		 * every other call site; with this set, pass only sizing there and
		 * let `align` place it.
		 */
		overlay?: boolean;
		/** Which trigger edge an overlay panel aligns to. */
		align?: 'left' | 'right';
		/** Where focus lands when the menu opens. Searchable catalogues start
		 * in their text field; ordinary action menus keep the first item. */
		initialFocus?: 'first-item' | 'first-input' | 'panel';
		/** A catalogue can own an inner scroll region so its search and
		 * footer remain fixed; ordinary overlay menus scroll as one panel. */
		overlayOverflow?: 'auto' | 'hidden';
	} = $props();

	let open = $state(false);
	let rootEl = $state<HTMLDivElement | null>(null);
	let panelEl = $state<HTMLDivElement | null>(null);
	let triggerEl = $state<HTMLButtonElement | null>(null);
	let openedFrom: HTMLElement | null = null;
	/** Viewport coordinates for an overlay panel, measured once it has
	 * rendered (its own size decides the flip). */
	let panelPos = $state<{ top: number; left: number } | null>(null);

	/** Keep the panel wholly on screen: aligned to the trigger, flipped
	 * above it when it would overhang the bottom, and clamped to the
	 * viewport on both axes. */
	const VIEWPORT_MARGIN = 8;
	const TRIGGER_GAP = 4;

	function positionPanel() {
		if (!overlay || !rootEl || !panelEl) return;
		const anchor = rootEl.getBoundingClientRect();
		const width = Math.min(
			panelEl.offsetWidth,
			Math.max(0, window.innerWidth - VIEWPORT_MARGIN * 2),
		);
		const height = Math.min(
			panelEl.offsetHeight,
			Math.max(0, window.innerHeight - VIEWPORT_MARGIN * 2),
		);

		let left = align === 'right' ? anchor.right - width : anchor.left;
		left = Math.max(
			VIEWPORT_MARGIN,
			Math.min(left, window.innerWidth - width - VIEWPORT_MARGIN),
		);

		let top = anchor.bottom + TRIGGER_GAP;
		if (top + height > window.innerHeight - VIEWPORT_MARGIN) {
			const above = anchor.top - height - TRIGGER_GAP;
			// Flip only when there is genuinely more room above; otherwise
			// clamp, so a panel taller than the viewport still starts at
			// the top rather than disappearing off it.
			top = above >= VIEWPORT_MARGIN ? above : VIEWPORT_MARGIN;
		}
		panelPos = { top, left };
	}

	function menuItemEls(): HTMLElement[] {
		return panelEl
			? Array.from(
					panelEl.querySelectorAll<HTMLElement>(
						'[role="menuitem"]:not([disabled]):not([aria-disabled="true"])',
					),
				)
			: [];
	}

	function focusItem(index: number) {
		const els = menuItemEls();
		if (els.length === 0) {
			// Custom panel content without menuitems: park focus on the panel
			// so Escape keeps working from inside the popover.
			panelEl?.focus();
			return;
		}
		const i = ((index % els.length) + els.length) % els.length;
		for (let j = 0; j < els.length; j++) {
			els[j].tabIndex = j === i ? 0 : -1;
		}
		els[i].focus();
	}

	function focusInitialTarget() {
		if (initialFocus === 'panel') {
			panelEl?.focus();
			return;
		}
		if (initialFocus === 'first-input') {
			const input = panelEl?.querySelector<HTMLElement>(
				'input:not([disabled]), textarea:not([disabled])'
			);
			if (input) {
				input.focus();
				return;
			}
		}
		focusItem(0);
	}

	function toggle() {
		if (open) {
			open = false;
		} else {
			openedFrom =
				triggerEl ??
				rootEl?.querySelector<HTMLElement>('[aria-haspopup="menu"]') ??
				(document.activeElement instanceof HTMLElement ? document.activeElement : null);
			open = true;
		}
	}

	function close(returnFocus: boolean) {
		open = false;
		if (returnFocus) {
			openedFrom?.focus();
		}
	}

	function handleTriggerKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown' && !open) {
			e.preventDefault();
			toggle();
		} else if (e.key === 'Escape' && open) {
			e.stopPropagation();
			close(true);
		}
	}

	function handlePanelKeydown(e: KeyboardEvent) {
		const textEntry = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
		if (textEntry) {
			if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
				e.preventDefault();
				focusItem(e.key === 'ArrowDown' ? 0 : -1);
			}
			// Home and End retain their text-entry meaning.
			if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
		}
		const els = menuItemEls();
		const current = els.indexOf(document.activeElement as HTMLElement);
		switch (e.key) {
			case 'ArrowDown':
				e.preventDefault();
				focusItem(current + 1);
				break;
			case 'ArrowUp':
				e.preventDefault();
				focusItem(current - 1);
				break;
			case 'Home':
				e.preventDefault();
				focusItem(0);
				break;
			case 'End':
				e.preventDefault();
				focusItem(els.length - 1);
				break;
			case 'Escape':
				e.preventDefault();
				// The menu is the innermost layer: a surface listening on
				// the window would otherwise close itself on the same
				// press, dismissing far more than was asked for.
				e.stopPropagation();
				close(true);
				break;
			case 'Tab':
				// Menus do not trap focus: let Tab proceed and dismiss the popover.
				close(false);
				break;
		}
	}

	function handleItemClick(item: MenuItem) {
		close(true);
		item.onSelect();
	}

	// While open: focus the first menuitem and dismiss on any click outside
	// the component (listener lives only for the open lifetime).
	$effect(() => {
		if (!open) return;
		focusInitialTarget();
		const onDocumentClick = (e: MouseEvent) => {
			if (
				rootEl &&
				e.target instanceof Node &&
				!rootEl.contains(e.target) &&
				!panelEl?.contains(e.target)
			) {
				close(false);
			}
		};
		document.addEventListener('click', onDocumentClick, true);
		return () => document.removeEventListener('click', onDocumentClick, true);
	});

	// An overlay panel is placed against the viewport, so anything that
	// moves the trigger under it has to move it too. Scroll is captured
	// so an inner scroll box (the one whose clipping this escapes) counts
	// as well as the page.
	$effect(() => {
		if (!open || !overlay) {
			panelPos = null;
			return;
		}
		positionPanel();
		const reposition = () => positionPanel();
		window.addEventListener('scroll', reposition, true);
		window.addEventListener('resize', reposition);
		const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(reposition);
		if (panelEl) observer?.observe(panelEl);
		return () => {
			window.removeEventListener('scroll', reposition, true);
			window.removeEventListener('resize', reposition);
			observer?.disconnect();
		};
	});
</script>

<div class="relative {className}" bind:this={rootEl}>
	{#if trigger}
		{@render trigger({ open, toggle, keydown: handleTriggerKeydown })}
	{:else}
		<button
			bind:this={triggerEl}
			aria-label={ariaLabel}
			aria-haspopup="menu"
			aria-expanded={open}
			class="w-7 h-7 flex items-center justify-center rounded-md border border-border/50
				text-text-tertiary hover:text-text hover:bg-surface-hover transition-colors cursor-pointer"
			onclick={(e) => {
				e.stopPropagation();
				toggle();
			}}
			onkeydown={handleTriggerKeydown}
		>
			<svg class="w-3.5 h-3.5" fill="currentColor" viewBox="0 0 16 16">
				<circle cx="8" cy="3" r="1.5" /><circle cx="8" cy="8" r="1.5" /><circle
					cx="8"
					cy="13"
					r="1.5"
				/>
			</svg>
		</button>
	{/if}

	{#if open}
		<div
			use:portal={overlay}
			bind:this={panelEl}
			role="menu"
			tabindex="-1"
			class="bg-surface-raised border border-border rounded-md shadow-lg py-1
				min-w-[100px] focus:outline-none
				{overlay ? 'fixed z-50' : 'absolute right-0 top-8 z-20'} {panelClass}"
			style={overlay
				? // Hidden until measured: one frame at the unpositioned
					// origin would read as the panel jumping into place.
					`top: ${panelPos?.top ?? 0}px; left: ${panelPos?.left ?? 0}px; ` +
					`max-width: calc(100vw - ${VIEWPORT_MARGIN * 2}px); ` +
					`max-height: calc(100vh - ${VIEWPORT_MARGIN * 2}px); overflow: ${overlayOverflow};` +
					(panelPos ? '' : ' visibility: hidden;')
				: undefined}
			onkeydown={handlePanelKeydown}
		>
			{#each items ?? [] as item (item.label)}
				<button
					role="menuitem"
					tabindex="-1"
					class="w-full px-3 py-1.5 text-xs text-left text-text-secondary hover:bg-surface-hover {item.danger
						? 'hover:text-negative'
						: 'hover:text-text'} cursor-pointer"
					onclick={() => handleItemClick(item)}
				>
					{item.label}
				</button>
			{/each}
			{#if children}
				{@render children({ close: () => close(true) })}
			{/if}
		</div>
	{/if}
</div>
