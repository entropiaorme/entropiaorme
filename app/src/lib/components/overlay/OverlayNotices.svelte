<script lang="ts">
	import { fade } from 'svelte/transition';
	import type { OverlayNotice } from '$lib/features/tracking/overlayNotices.svelte';
	import { shouldSettleInstantly } from '$lib/motion/testMotion';

	const noop = () => {};

	let {
		notices,
		onHold = noop,
		onRelease = noop
	}: {
		notices: readonly OverlayNotice[];
		onHold?: () => void;
		onRelease?: () => void;
	} = $props();

	// A reduced-motion preference (and the frozen e2e build) drops the fade.
	const fadeMs = shouldSettleInstantly() ? 0 : 150;
</script>

<!-- Sits under the strip and takes the strip's width without widening
	 it (`width: 0; min-width: 100%` contributes nothing to the frame's
	 intrinsic width), so a long message wraps in full instead of being
	 cut off, and the controls above never shift. Pointing at a notice
	 holds it for as long as the pointer stays. -->
<div
	class="notice-rail flex flex-col gap-1"
	class:notice-rail-open={notices.length > 0}
	role="status"
	aria-live="polite"
	data-testid="overlay-notices"
	onpointerenter={onHold}
	onpointerleave={onRelease}
>
	{#each notices as notice (notice.id)}
		<div
			class="notice rounded-lg px-3 py-1.5 text-[11px] leading-snug"
			class:notice-warning={notice.tone === 'warning'}
			class:notice-error={notice.tone === 'error'}
			transition:fade={{ duration: fadeMs }}
		>
			{notice.text}
		</div>
	{/each}
</div>

<style>
	.notice-rail {
		width: 0;
		min-width: 100%;
	}
	.notice-rail-open {
		margin-top: 6px;
	}

	/* The strip's glass, so a notice stays legible over the game. */
	.notice {
		background: rgba(10, 14, 23, 0.85);
		backdrop-filter: blur(16px) saturate(150%);
		border: 1px solid rgba(255, 255, 255, 0.08);
		overflow-wrap: anywhere;
	}

	.notice-warning {
		border-left: 2px solid rgba(252, 211, 77, 0.6);
		color: rgb(253, 230, 138);
	}

	.notice-error {
		border-left: 2px solid rgba(251, 146, 60, 0.6);
		color: rgba(253, 186, 116, 0.95);
	}
</style>
