<script lang="ts">
	import type { Snippet } from 'svelte';
	import { fade, scale } from 'svelte/transition';
	import { quintOut } from 'svelte/easing';
	import { portal } from '$lib/utils/portal';

	let {
		open = $bindable(false),
		title,
		children,
		class: className = ''
	}: {
		open?: boolean;
		title?: string;
		children: Snippet;
		class?: string;
	} = $props();

	let panelEl = $state<HTMLDivElement | null>(null);

	// The default panel width applies only when the caller supplies no
	// max-width of its own: two max-w-* utilities on one element resolve by
	// stylesheet order, not class order, so appending a caller width after
	// a hardcoded default would not reliably override it.
	const sizeClass = $derived(/(^|\s)max-w-/.test(className) ? '' : 'max-w-md');

	const FOCUSABLE_SELECTOR =
		'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

	function handleBackdropClick(e: MouseEvent) {
		if (e.target === e.currentTarget) {
			open = false;
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			open = false;
		} else if (e.key === 'Tab') {
			trapTab(e);
		}
	}

	// Keep Tab cycling within the panel while the dialog is open.
	function trapTab(e: KeyboardEvent) {
		if (!panelEl || e.defaultPrevented) return;
		const focusables = Array.from(panelEl.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
		if (focusables.length === 0) {
			e.preventDefault();
			panelEl.focus();
			return;
		}
		const first = focusables[0];
		const last = focusables[focusables.length - 1];
		const active = document.activeElement;
		const inside = active instanceof HTMLElement && panelEl.contains(active);
		if (e.shiftKey) {
			if (!inside || active === first || active === panelEl) {
				e.preventDefault();
				last.focus();
			}
		} else if (!inside || active === last) {
			e.preventDefault();
			first.focus();
		}
	}

	// On open: remember the opener and move focus into the panel; on close
	// (or unmount) hand focus back to the previously focused element.
	$effect(() => {
		if (!open) return;
		const previouslyFocused =
			document.activeElement instanceof HTMLElement ? document.activeElement : null;
		panelEl?.focus();
		return () => {
			previouslyFocused?.focus();
		};
	});
</script>

<svelte:window onkeydown={open ? handleKeydown : undefined} />

{#if open}
	<!-- Kept: backdrop click-to-dismiss is a pointer convenience; Escape (window keydown above) and the Close button are the keyboard paths. -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<!-- Portalled: a dialog covers the window, not its owner. An owner inside
		 a dashboard panel (whose backdrop-filter re-anchors `fixed`) would
		 otherwise trap the dialog within the panel's box. -->
	<div
		use:portal
		class="fixed inset-0 z-50 flex items-center justify-center p-6"
		onclick={handleBackdropClick}
		onkeydown={handleKeydown}
	>
		<div
			class="absolute inset-0 bg-base/70 backdrop-blur-sm"
			transition:fade={{ duration: 180 }}
		></div>

		<div
			bind:this={panelEl}
			class="relative z-10 w-full {sizeClass} rounded-lg border border-border-bright/60
				bg-surface-raised/95 shadow-lg backdrop-blur-md
				focus:outline-hidden
				before:pointer-events-none before:absolute before:inset-0 before:rounded-[inherit]
				before:[box-shadow:inset_0_1px_0_0_rgba(255,255,255,0.05)]
				p-6 {className}"
			role="dialog"
			aria-modal="true"
			aria-label={title}
			tabindex="-1"
			transition:scale={{ duration: 220, start: 0.96, easing: quintOut }}
		>
			{#if title}
				<div class="relative flex items-center justify-between mb-5 pb-4 border-b border-border/50">
					<h2 class="text-base font-semibold text-text tracking-tight">{title}</h2>
					<button
						class="h-7 w-7 flex items-center justify-center rounded-md
							text-text-tertiary cursor-pointer
							transition-colors duration-[var(--duration-fast)]
							hover:text-text hover:bg-surface-hover"
						onclick={() => (open = false)}
						aria-label="Close"
					>
						<svg
							xmlns="http://www.w3.org/2000/svg"
							viewBox="0 0 20 20"
							fill="currentColor"
							class="h-4 w-4"
						>
							<path
								d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"
							/>
						</svg>
					</button>
				</div>
			{/if}

			<div class="relative">
				{@render children()}
			</div>
		</div>
	</div>
{/if}
