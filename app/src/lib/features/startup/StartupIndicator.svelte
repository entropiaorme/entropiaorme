<script lang="ts">
	/**
	 * The app-wide cue that the backend is still starting: a hairline sweeping
	 * along the foot of the title bar, joined by a quiet "Starting up" label
	 * only if the wait runs past a couple of seconds. It sits over the title
	 * bar without taking layout space or pointer events (the bar stays
	 * draggable), and fades in after a short delay so a fast start never shows
	 * it. The page regions waiting on data carry their own skeletons; this is
	 * only the single "the app is still coming up" signal.
	 *
	 * Render it inside a positioned container whose top edge is the title
	 * bar's, and only while startup is in progress.
	 */
	import { fade } from 'svelte/transition';
</script>

<div class="startup-indicator" role="status" out:fade={{ duration: 240 }}>
	<span class="label">Starting up</span>
	<span class="track" aria-hidden="true"><span class="sweep"></span></span>
</div>

<style>
	.startup-indicator {
		position: absolute;
		inset: 0 0 auto 0;
		height: 32px;
		pointer-events: none;
		animation: indicator-in 240ms var(--ease-out) 150ms both;
	}

	.label {
		position: absolute;
		inset: 0 auto 0 16px;
		display: flex;
		align-items: center;
		font-size: 11px;
		letter-spacing: 0.06em;
		color: var(--color-text-tertiary);
		animation: indicator-in 320ms var(--ease-out) 2s both;
	}

	.track {
		position: absolute;
		inset: auto 0 0 0;
		height: 1px;
		overflow: hidden;
	}

	.sweep {
		position: absolute;
		inset: 0 auto 0 0;
		width: 40%;
		background: linear-gradient(90deg, transparent, var(--color-accent), transparent);
		animation: sweep 1.6s ease-in-out infinite;
	}

	@keyframes indicator-in {
		from {
			opacity: 0;
		}
		to {
			opacity: 1;
		}
	}

	@keyframes sweep {
		from {
			transform: translateX(-100%);
		}
		to {
			transform: translateX(250%);
		}
	}

	/* Without motion the line holds still across the full width. */
	@media (prefers-reduced-motion: reduce) {
		.sweep {
			width: 100%;
			opacity: 0.5;
			animation: none;
		}
	}
</style>
