<script lang="ts">
	// The prominent update nudge: a toast that appears when a check finds a newer
	// release, routing to the Updates page on click. The unread dot on the
	// sidebar Updates item is the quiet, persistent affordance; this is the loud
	// one. Dismissal is session-scoped (the next launch check re-nudges).
	import { goto } from '$app/navigation';
	import { fly } from 'svelte/transition';
	import { quintOut } from 'svelte/easing';
	import { availableUpdate, dismissUpdateToast, showUpdateToast } from '$lib/updater';

	function openUpdates() {
		dismissUpdateToast();
		void goto('/updates');
	}

	function dismiss(event: MouseEvent) {
		event.stopPropagation();
		dismissUpdateToast();
	}
</script>

{#if $showUpdateToast}
	<div
		class="toast"
		role="alert"
		transition:fly={{ y: 16, duration: 280, easing: quintOut }}
	>
		<button class="surface" onclick={openUpdates} type="button">
			<span class="spark" aria-hidden="true">
				<svg viewBox="0 0 20 20" fill="currentColor">
					<path
						fill-rule="evenodd"
						d="M10 18a8 8 0 100-16 8 8 0 000 16zm.75-11.25a.75.75 0 00-1.5 0v3.69L7.7 8.89a.75.75 0 10-1.06 1.06l2.83 2.83a.75.75 0 001.06 0l2.83-2.83a.75.75 0 10-1.06-1.06l-1.55 1.55V6.75z"
						clip-rule="evenodd"
					/>
				</svg>
			</span>
			<span class="copy">
				<span class="title">New update for EntropiaOrme available</span>
				<span class="sub">
					{#if $availableUpdate}Version {$availableUpdate.version} is ready to install.{:else}
						A newer version is ready.{/if}
				</span>
			</span>
			<span class="cta" aria-hidden="true">View</span>
		</button>
		<button class="close" onclick={dismiss} type="button" aria-label="Dismiss update notice">
			<svg viewBox="0 0 20 20" fill="currentColor">
				<path
					d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"
				/>
			</svg>
		</button>
	</div>
{/if}

<style>
	.toast {
		position: fixed;
		right: 1.25rem;
		bottom: 1.25rem;
		z-index: 60;
		display: flex;
		align-items: stretch;
		max-width: min(24rem, calc(100vw - 2.5rem));
		border: 1px solid color-mix(in oklab, var(--color-accent) 38%, var(--color-border));
		border-radius: var(--radius-lg);
		background: color-mix(in oklab, var(--color-surface) 92%, transparent);
		box-shadow:
			0 12px 32px rgba(0, 0, 0, 0.35),
			var(--shadow-glow);
		backdrop-filter: blur(10px);
		overflow: hidden;
	}
	.surface {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: 0.875rem 0.5rem 0.875rem 1rem;
		text-align: left;
		font: inherit;
		color: var(--color-text);
		background: transparent;
		border: 0;
		cursor: pointer;
		flex: 1;
		min-width: 0;
	}
	.spark {
		flex-shrink: 0;
		display: grid;
		place-items: center;
		width: 1.75rem;
		height: 1.75rem;
		border-radius: var(--radius-full, 999px);
		color: var(--color-accent);
		background: color-mix(in oklab, var(--color-accent) 16%, transparent);
	}
	.spark svg {
		width: 1.05rem;
		height: 1.05rem;
	}
	.copy {
		display: grid;
		gap: 0.125rem;
		min-width: 0;
	}
	.title {
		font-size: 0.8125rem;
		font-weight: 600;
		line-height: 1.3;
	}
	.sub {
		font-size: 0.75rem;
		color: var(--color-text-secondary);
		line-height: 1.4;
	}
	.cta {
		flex-shrink: 0;
		font-size: 0.75rem;
		font-weight: 600;
		letter-spacing: 0.03em;
		color: var(--color-accent);
		padding-left: 0.25rem;
	}
	.close {
		flex-shrink: 0;
		display: grid;
		place-items: center;
		width: 2rem;
		color: var(--color-text-tertiary);
		background: transparent;
		border: 0;
		border-left: 1px solid color-mix(in oklab, var(--color-border) 70%, transparent);
		cursor: pointer;
		transition: color var(--duration-base) var(--ease-out), background var(--duration-base) var(--ease-out);
	}
	.close:hover {
		color: var(--color-text);
		background: color-mix(in oklab, var(--color-surface-hover) 70%, transparent);
	}
	.close svg {
		width: 1rem;
		height: 1rem;
	}
</style>
