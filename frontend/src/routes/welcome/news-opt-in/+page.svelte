<script lang="ts">
	import { goto } from '$app/navigation';
	import { fly, fade } from 'svelte/transition';
	import { quintOut } from 'svelte/easing';
	import { onMount } from 'svelte';
	import Button from '$lib/components/Button.svelte';
	import { getPreference } from '$lib/preferences';
	import { markNewsOptInSeen, NEWS_PREFERENCE_KEYS, setNewsOptIn } from '$lib/news';
	import { AUTO_UPDATE_PREFERENCE_KEY, setAutoUpdateEnabled } from '$lib/updater';
	import { refreshNews } from '$lib/newsFetch';
	import NetworkingStep from '../NetworkingStep.svelte';

	// Re-prompt for users who onboarded before these features existed. Default ON
	// (opt-out, matching the first-run step) only when a preference is genuinely
	// unset; hydrate any saved choice first so re-opening this page never flips an
	// existing opt-out back on.
	let newsOptedIn = $state(true);
	let autoUpdateOptedIn = $state(true);

	onMount(async () => {
		const [savedNews, savedAuto] = await Promise.all([
			getPreference<boolean | null>(NEWS_PREFERENCE_KEYS.optIn, null),
			getPreference<boolean | null>(AUTO_UPDATE_PREFERENCE_KEY, null),
		]);
		newsOptedIn = savedNews ?? true;
		autoUpdateOptedIn = savedAuto ?? true;
	});
	let exiting = $state(false);

	async function complete() {
		if (exiting) return;
		await markNewsOptInSeen();
		await setNewsOptIn(newsOptedIn);
		await setAutoUpdateEnabled(autoUpdateOptedIn);
		if (newsOptedIn) {
			void refreshNews();
		}
		if (typeof sessionStorage !== 'undefined') {
			sessionStorage.setItem('welcome_just_finished', '1');
		}
		exiting = true;
		setTimeout(() => goto('/'), 380);
	}

	function handleKey(e: KeyboardEvent) {
		const t = e.target;
		if (t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement) return;
		if (e.key === 'Enter') {
			e.preventDefault();
			complete();
		}
	}
</script>

<svelte:window onkeydown={handleKey} />

<div class="shell" class:exiting>
	<div class="bg-grid" aria-hidden="true"></div>
	<div class="bg-glow" aria-hidden="true"></div>
	<img src="/watermark.png" alt="" aria-hidden="true" class="bg-mascot" />
	<div class="top-rule" aria-hidden="true"></div>

	<main class="stage">
		<section
			class="step"
			in:fly={{ y: 14, duration: 520, easing: quintOut, delay: 110 }}
			out:fade={{ duration: 160 }}
		>
			<div class="reprompt-eyebrow eyebrow">News &amp; updates</div>
			<NetworkingStep bind:news={newsOptedIn} bind:autoUpdate={autoUpdateOptedIn} />
		</section>
	</main>

	<footer class="bottom-bar">
		<div class="status-line" aria-hidden="true">
			<span>EntropiaOrme</span>
			<span class="dot">·</span>
			<span>NEWS &amp; UPDATES</span>
		</div>

		<div class="controls">
			<Button variant="primary" onclick={complete}>
				<span>Continue</span>
				<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="h-3.5 w-3.5">
					<path
						fill-rule="evenodd"
						d="M3 10a.75.75 0 01.75-.75h10.638L10.23 5.29a.75.75 0 111.04-1.08l5.5 5.25a.75.75 0 010 1.08l-5.5 5.25a.75.75 0 11-1.04-1.08l4.158-3.96H3.75A.75.75 0 013 10z"
						clip-rule="evenodd"
					/>
				</svg>
			</Button>
		</div>
	</footer>
</div>

<style>
	.shell {
		position: relative;
		display: grid;
		grid-template-rows: 1fr auto;
		height: 100%;
		width: 100%;
		background: var(--color-base);
		overflow: hidden;
		color: var(--color-text);
		transition:
			opacity 380ms var(--ease-out),
			transform 380ms var(--ease-out);
	}
	.shell.exiting {
		opacity: 0;
		transform: scale(1.025);
		pointer-events: none;
	}
	.bg-grid {
		position: absolute;
		inset: 0;
		background-image: radial-gradient(
			circle at 1px 1px,
			color-mix(in oklab, var(--color-border-bright) 65%, transparent) 1px,
			transparent 0
		);
		background-size: 28px 28px;
		opacity: 0.55;
		mask-image: radial-gradient(ellipse 70% 80% at 50% 45%, black 30%, transparent 85%);
		-webkit-mask-image: radial-gradient(ellipse 70% 80% at 50% 45%, black 30%, transparent 85%);
		pointer-events: none;
	}
	.bg-glow {
		position: absolute;
		inset: 0;
		background:
			radial-gradient(
				ellipse 55% 45% at 78% 18%,
				color-mix(in oklab, var(--color-accent) 22%, transparent),
				transparent 60%
			),
			radial-gradient(
				ellipse 60% 50% at 12% 92%,
				color-mix(in oklab, var(--color-accent-muted) 70%, transparent),
				transparent 55%
			);
		pointer-events: none;
	}
	.bg-mascot {
		position: absolute;
		right: -7%;
		bottom: -10%;
		width: min(56vh, 540px);
		height: auto;
		opacity: 0.05;
		filter: drop-shadow(0 0 80px color-mix(in oklab, var(--color-accent) 35%, transparent));
		transform: scaleX(-1);
		pointer-events: none;
		user-select: none;
	}
	.top-rule {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		height: 1px;
		background: linear-gradient(
			90deg,
			transparent 0%,
			color-mix(in oklab, var(--color-accent) 55%, transparent) 50%,
			transparent 100%
		);
		opacity: 0.55;
		pointer-events: none;
	}
	.stage {
		position: relative;
		z-index: 1;
		display: grid;
		place-items: center;
		padding: 4rem 2rem 2rem;
	}
	.step {
		display: grid;
		gap: 1.25rem;
		width: 100%;
		max-width: 460px;
	}
	.eyebrow {
		font-size: 0.6875rem;
		letter-spacing: 0.22em;
		text-transform: uppercase;
		color: var(--color-text-tertiary);
		font-weight: 500;
	}
	.reprompt-eyebrow {
		color: var(--color-accent);
	}
	.bottom-bar {
		position: relative;
		z-index: 1;
		display: grid;
		grid-template-columns: 1fr auto;
		align-items: center;
		gap: 1.5rem;
		padding: 1.125rem 2rem 1.5rem;
		border-top: 1px solid color-mix(in oklab, var(--color-border) 70%, transparent);
		background: color-mix(in oklab, var(--color-surface) 35%, transparent);
		backdrop-filter: blur(8px);
	}
	.status-line {
		display: flex;
		align-items: center;
		gap: 0.625rem;
		font-size: 0.6875rem;
		letter-spacing: 0.18em;
		text-transform: uppercase;
		color: var(--color-text-tertiary);
		font-variant-numeric: tabular-nums;
	}
	.status-line .dot {
		opacity: 0.5;
	}
	.controls {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
	@media (max-width: 720px) {
		.stage {
			padding: 3rem 1.5rem 1.25rem;
		}
		.step {
			max-width: 100%;
		}
		.bottom-bar {
			grid-template-columns: 1fr;
			grid-template-rows: auto auto;
			gap: 0.875rem;
			padding: 1rem 1.25rem 1.25rem;
		}
		.status-line {
			justify-content: center;
		}
		.controls {
			justify-content: flex-end;
		}
	}
</style>
