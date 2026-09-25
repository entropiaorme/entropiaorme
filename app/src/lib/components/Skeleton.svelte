<script lang="ts">
	/**
	 * The one placeholder for a value or region whose data has not arrived yet
	 * (the first read of a page, or the backend still starting). It stands in
	 * the slot the real content will occupy, so the layout does not jump when
	 * the data lands, and it never shows a figure or an empty-state message
	 * that could be mistaken for the answer. It fades in after a short delay,
	 * so a read that answers quickly never flashes it. Size it with `class`;
	 * mark the enclosing region `aria-busy` while it shows.
	 */
	let { class: className = '' }: { class?: string } = $props();
</script>

<span class="skeleton {className}" aria-hidden="true"></span>

<style>
	.skeleton {
		display: block;
		border-radius: 4px;
		background: linear-gradient(
			90deg,
			color-mix(in oklab, var(--color-text) 5%, transparent) 0%,
			color-mix(in oklab, var(--color-text) 10%, transparent) 50%,
			color-mix(in oklab, var(--color-text) 5%, transparent) 100%
		);
		background-size: 200% 100%;
		animation:
			skeleton-in 240ms var(--ease-out) 150ms both,
			skeleton-shimmer 1.6s ease-in-out 150ms infinite;
	}

	@keyframes skeleton-in {
		from {
			opacity: 0;
		}
		to {
			opacity: 1;
		}
	}

	@keyframes skeleton-shimmer {
		from {
			background-position: 100% 0;
		}
		to {
			background-position: -100% 0;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.skeleton {
			animation: skeleton-in 1ms 150ms both;
		}
	}
</style>
