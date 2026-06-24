<script lang="ts">
	import { goto } from '$app/navigation';
	import { fly, fade } from 'svelte/transition';
	import { quintOut } from 'svelte/easing';
	import Button from '$lib/components/Button.svelte';
	import { setOnboardingComplete } from '$lib/onboarding';
	import { CURRENT_TOS_VERSION, setAcceptedTosVersion } from '$lib/tos';
	import { theme, setTheme, type Theme } from '$lib/theme';
	import { markNewsOptInSeen, setNewsOptIn } from '$lib/news';
	import { setAutoUpdateEnabled } from '$lib/updater';
	import { refreshNews } from '$lib/newsFetch';
	import NetworkingStep from './NetworkingStep.svelte';
	import TermsStep from './TermsStep.svelte';
	import { externalLinks } from '$lib/utils/openExternal';

	let step = $state(1);
	let tosAccepted = $state(false);
	// Networking features default ON (opt-out posture): the user unchecks to
	// opt out on the networking step, not in.
	let newsOptedIn = $state(true);
	let autoUpdateOptedIn = $state(true);
	let exiting = $state(false);
	const totalSteps = 6;

	async function handleThemeChange(id: Theme) {
		await setTheme(id);
	}

	let canAdvance = $derived(step !== totalSteps || tosAccepted);

	function next() {
		if (!canAdvance || exiting) return;
		if (step < totalSteps) step += 1;
		else complete();
	}
	function back() {
		if (step > 1 && !exiting) step -= 1;
	}
	async function complete() {
		if (exiting) return;
		await markNewsOptInSeen();
		await setNewsOptIn(newsOptedIn);
		await setAutoUpdateEnabled(autoUpdateOptedIn);
		await setAcceptedTosVersion(CURRENT_TOS_VERSION);
		await setOnboardingComplete(true);
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
		if (e.key === 'Enter' || e.key === 'ArrowRight') {
			e.preventDefault();
			next();
		} else if (e.key === 'ArrowLeft') {
			e.preventDefault();
			back();
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
		{#key step}
			<section
				class="step"
				class:wide={step === 2}
				in:fly={{ y: 14, duration: 520, easing: quintOut, delay: 110 }}
				out:fade={{ duration: 160 }}
			>
				{#if step === 1}
					<div class="hero">
						<span class="eyebrow">Welcome</span>
						<img
							src={$theme === 'light' ? '/wordmark-on-light.png' : '/wordmark-on-dark.png'}
							alt="EntropiaOrme"
							class="hero-wordmark"
						/>
						<p class="hero-tagline">
							An analytical desktop tool for Entropia&nbsp;Universe.
						</p>
					</div>
				{:else if step === 2}
					<div class="block display-setup">
						<header class="step-header">
							<span class="eyebrow">Recommended layout</span>
							<h2 class="step-title">Designed as a half-screen companion.</h2>
						</header>
						<p class="layout-body">
							EntropiaOrme is optimised for the half of a second 16:9 monitor closest to your main display. Other sizes and single-monitor setups still work, and an overlay is available too.
						</p>
						<svg
							class="layout-visual"
							viewBox="0 0 520 240"
							role="img"
							aria-labelledby="layout-title layout-desc"
						>
							<title id="layout-title">EntropiaOrme snapping beside the game</title>
							<desc id="layout-desc">
								A window moves from the right monitor to the right side of the left monitor, where it snaps to half-screen width.
							</desc>
							<defs>
								<linearGradient id="monitor-sheen" x1="0" x2="1" y1="0" y2="1">
									<stop offset="0%" stop-color="currentColor" stop-opacity="0.12" />
									<stop offset="100%" stop-color="currentColor" stop-opacity="0" />
								</linearGradient>
							</defs>

							<g class="monitor monitor-secondary">
								<rect class="monitor-frame" x="28" y="46" width="210" height="118" rx="8" />
								<rect class="monitor-screen" x="36" y="54" width="194" height="102" rx="4" />
								<rect class="screen-sheen" x="36" y="54" width="194" height="102" rx="4" />
								<rect class="snap-zone" x="133" y="54" width="97" height="102" rx="4">
									<animate
										attributeName="opacity"
										dur="5.8s"
										repeatCount="indefinite"
										keyTimes="0;0.38;0.48;0.76;0.88;1"
										values="0;0;0.78;0.46;0;0"
									/>
								</rect>
								<line class="snap-edge" x1="230" y1="56" x2="230" y2="154">
									<animate
										attributeName="opacity"
										dur="5.8s"
										repeatCount="indefinite"
										keyTimes="0;0.42;0.52;0.72;0.88;1"
										values="0;0;1;0.65;0;0"
									/>
								</line>
								<path class="monitor-stand" d="M117 164h32l5 18h-42z" />
								<rect class="monitor-foot" x="92" y="181" width="82" height="6" rx="3" />
							</g>

							<g class="monitor monitor-main">
								<rect class="monitor-frame" x="282" y="46" width="210" height="118" rx="8" />
								<rect class="monitor-screen" x="290" y="54" width="194" height="102" rx="4" />
								<rect class="screen-sheen" x="290" y="54" width="194" height="102" rx="4" />
								<path class="game-horizon" d="M304 119c28-24 53-22 76 2 22 22 49 20 88-13" />
								<circle class="game-reticle" cx="387" cy="104" r="18" />
								<path class="game-reticle-lines" d="M387 78v12M387 118v12M361 104h12M401 104h12" />
								<path class="monitor-stand" d="M371 164h32l5 18h-42z" />
								<rect class="monitor-foot" x="346" y="181" width="82" height="6" rx="3" />
							</g>

							<g class="layout-window-position">
								<animateTransform
									attributeName="transform"
									type="translate"
									dur="5.8s"
									repeatCount="indefinite"
									calcMode="spline"
									keyTimes="0;0.14;0.38;0.54;0.76;0.9;1"
									keySplines="0.2 0 0.1 1;0.2 0 0.1 1;0.2 0 0.1 1;0 0 0.2 1;0.4 0 1 1;0.4 0 1 1"
									values="340 78;340 78;222 70;133 54;133 54;340 78;340 78"
								/>
								<g class="layout-window-scale">
									<animateTransform
										attributeName="transform"
										type="scale"
										dur="5.8s"
										repeatCount="indefinite"
										calcMode="spline"
										keyTimes="0;0.42;0.54;0.76;0.9;1"
										keySplines="0.2 0 0.1 1;0.1 0 0.1 1;0 0 0.2 1;0.4 0 1 1;0.4 0 1 1"
										values="1 1;1 1;1.155 1.821;1.155 1.821;1 1;1 1"
									/>
									<rect class="app-window-shadow" x="2" y="4" width="84" height="56" rx="5" />
									<rect class="app-window" x="0" y="0" width="84" height="56" rx="5" />
									<rect class="app-titlebar" x="0" y="0" width="84" height="11" rx="5" />
									<circle class="app-dot" cx="8" cy="5.5" r="1.4" />
									<circle class="app-dot" cx="14" cy="5.5" r="1.4" />
									<circle class="app-dot" cx="20" cy="5.5" r="1.4" />
									<rect class="app-sidebar" x="7" y="17" width="17" height="31" rx="2" />
									<rect class="app-line strong" x="31" y="19" width="40" height="4" rx="2" />
									<rect class="app-line" x="31" y="29" width="32" height="3" rx="1.5" />
									<rect class="app-line" x="31" y="38" width="45" height="3" rx="1.5" />
								</g>
							</g>

							<g class="layout-cursor">
								<animateTransform
									attributeName="transform"
									type="translate"
									dur="5.8s"
									repeatCount="indefinite"
									calcMode="spline"
									keyTimes="0;0.16;0.38;0.5;0.72;0.9;1"
									keySplines="0.2 0 0.1 1;0.2 0 0.1 1;0.2 0 0.1 1;0 0 0.2 1;0.4 0 1 1;0.4 0 1 1"
									values="408 99;408 99;290 90;230 68;230 68;408 99;408 99"
								/>
								<path class="cursor-shadow" d="M2 4v25l7-6 5 13 7-3-6-12h10z" />
								<path class="cursor" d="M0 0v25l7-6 5 13 7-3-6-12h10z" />
							</g>
						</svg>
					</div>
				{:else if step === 3}
					<div class="block">
						<header class="step-header">
							<span class="eyebrow">Appearance</span>
							<h2 class="step-title">Light or dark?</h2>
						</header>
						<div class="theme-cards" role="radiogroup" aria-label="Appearance">
							<button
								type="button"
								class="theme-card recommended"
								class:selected={$theme === 'dark'}
								role="radio"
								aria-checked={$theme === 'dark'}
								onclick={() => handleThemeChange('dark')}
							>
								<span class="theme-card-badge">Recommended</span>
								<div class="theme-card-preview" data-theme-preview="dark" aria-hidden="true"></div>
								<span class="theme-card-label">Dark</span>
							</button>
							<button
								type="button"
								class="theme-card"
								class:selected={$theme === 'light'}
								role="radio"
								aria-checked={$theme === 'light'}
								onclick={() => handleThemeChange('light')}
							>
								<div class="theme-card-preview" data-theme-preview="light" aria-hidden="true"></div>
								<span class="theme-card-label">Light</span>
							</button>
						</div>
						<span class="theme-footnote">Change any time in Settings.</span>
					</div>
				{:else if step === 4}
					<div class="block">
						<header class="step-header">
							<span class="eyebrow">Game data</span>
							<h2 class="step-title">Game data in this app comes from Entropia&nbsp;Nexus.</h2>
						</header>
						<p class="attribution-body">
							Weapon stats, mob names, and other game data in EntropiaOrme come from Entropia&nbsp;Nexus, a great Entropia&nbsp;Universe wiki. Go check them out.
						</p>
						<a
							class="external"
							href="https://entropianexus.com/"
							target="_blank"
							rel="noopener noreferrer"
							use:externalLinks
						>
							<span>Visit Entropia Nexus</span>
							<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
								<path
									fill-rule="evenodd"
									d="M5.22 14.78a.75.75 0 001.06 0l7.22-7.22v5.69a.75.75 0 001.5 0v-7.5a.75.75 0 00-.75-.75h-7.5a.75.75 0 000 1.5h5.69l-7.22 7.22a.75.75 0 000 1.06z"
									clip-rule="evenodd"
								/>
							</svg>
						</a>
					</div>
				{:else if step === 5}
					<NetworkingStep bind:news={newsOptedIn} bind:autoUpdate={autoUpdateOptedIn} />
				{:else if step === 6}
					<TermsStep bind:accepted={tosAccepted} />
				{/if}
			</section>
		{/key}
	</main>

	<footer class="bottom-bar">
		<div
			class="progress"
			role="progressbar"
			aria-valuemin="1"
			aria-valuemax={totalSteps}
			aria-valuenow={step}
			aria-label="Onboarding progress"
		>
			{#each Array(totalSteps) as _, i}
				<span class="node" class:active={step === i + 1} class:passed={step > i + 1}></span>
				{#if i < totalSteps - 1}
					<span class="track" class:passed={step > i + 1}></span>
				{/if}
			{/each}
		</div>

		<div class="status-line" aria-hidden="true">
			<span>EntropiaOrme</span>
			<span class="dot">·</span>
			<span>STEP {String(step).padStart(2, '0')} / {String(totalSteps).padStart(2, '0')}</span>
		</div>

		<div class="controls">
			{#if step > 1}
				<Button variant="ghost" onclick={back}>Back</Button>
			{/if}
			<Button variant="primary" onclick={next} disabled={!canAdvance}>
				<span>{step === totalSteps ? 'Get started' : 'Continue'}</span>
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

	/* === Atmospheric layers === */
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

	/* === Stage === */
	.stage {
		position: relative;
		z-index: 1;
		display: grid;
		place-items: center;
		padding: 4rem 2rem 2rem;
	}
	.step {
		grid-area: 1 / 1;
		display: grid;
		gap: 1.75rem;
		width: 100%;
		max-width: 440px;
		min-height: 280px;
	}
	.step.wide {
		max-width: 540px;
		min-height: 340px;
	}

	/* === Typography primitives === */
	.eyebrow {
		font-size: 0.6875rem;
		letter-spacing: 0.22em;
		text-transform: uppercase;
		color: var(--color-text-tertiary);
		font-weight: 500;
	}
	.step-header {
		display: flex;
		flex-direction: column;
		gap: 0.625rem;
	}
	.step-title {
		font-size: 1.5rem;
		line-height: 1.2;
		font-weight: 500;
		letter-spacing: -0.012em;
		color: var(--color-text);
		margin: 0;
	}

	/* === Hero (step 1) === */
	.hero {
		display: grid;
		justify-items: center;
		gap: 1.25rem;
		text-align: center;
	}
	.hero-wordmark {
		height: 3.5rem;
		width: auto;
		max-width: 100%;
		filter: drop-shadow(0 6px 32px color-mix(in oklab, var(--color-accent) 30%, transparent));
		user-select: none;
	}
	.hero-tagline {
		max-width: 30ch;
		margin: 0;
		font-size: 1rem;
		line-height: 1.55;
		color: var(--color-text-secondary);
	}

	/* === Display setup (step 2) === */
	.block {
		display: grid;
		gap: 1.5rem;
	}
	.display-setup {
		gap: 1.125rem;
	}
	.layout-body {
		margin: 0;
		font-size: 0.95rem;
		line-height: 1.55;
		color: var(--color-text-secondary);
	}
	.layout-visual {
		width: 100%;
		height: auto;
		color: var(--color-accent);
		overflow: visible;
	}
	.monitor-frame {
		fill: color-mix(in oklab, var(--color-surface-raised) 72%, transparent);
		stroke: color-mix(in oklab, var(--color-border-bright) 85%, transparent);
		stroke-width: 1.25;
	}
	.monitor-screen {
		fill: color-mix(in oklab, var(--color-base) 86%, black);
		stroke: color-mix(in oklab, var(--color-border) 70%, transparent);
		stroke-width: 1;
	}
	.screen-sheen {
		fill: url(#monitor-sheen);
	}
	.monitor-stand,
	.monitor-foot {
		fill: color-mix(in oklab, var(--color-surface-raised) 62%, transparent);
		stroke: color-mix(in oklab, var(--color-border-bright) 65%, transparent);
		stroke-width: 1;
	}
	.monitor-main {
		opacity: 0.82;
	}
	.snap-zone {
		fill: color-mix(in oklab, var(--color-accent) 16%, transparent);
		stroke: color-mix(in oklab, var(--color-accent) 80%, transparent);
		stroke-width: 1.5;
		opacity: 0;
		vector-effect: non-scaling-stroke;
	}
	.snap-edge {
		stroke: var(--color-accent);
		stroke-width: 2;
		stroke-linecap: round;
		opacity: 0;
		filter: drop-shadow(0 0 10px color-mix(in oklab, var(--color-accent) 75%, transparent));
	}
	.game-horizon,
	.game-reticle,
	.game-reticle-lines {
		fill: none;
		stroke: color-mix(in oklab, var(--color-text-tertiary) 45%, transparent);
		stroke-width: 1.2;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	.game-reticle {
		stroke: color-mix(in oklab, var(--color-accent) 38%, transparent);
	}
	.layout-window-position {
		filter: drop-shadow(0 12px 22px rgba(0, 0, 0, 0.34));
	}
	.app-window-shadow {
		fill: rgba(0, 0, 0, 0.28);
	}
	.app-window {
		fill: color-mix(in oklab, var(--color-surface) 92%, black);
		stroke: color-mix(in oklab, var(--color-accent) 46%, var(--color-border));
		stroke-width: 1.25;
		vector-effect: non-scaling-stroke;
	}
	.app-titlebar {
		fill: color-mix(in oklab, var(--color-accent) 16%, var(--color-surface));
	}
	.app-dot {
		fill: color-mix(in oklab, var(--color-accent) 58%, var(--color-text-secondary));
		opacity: 0.9;
	}
	.app-sidebar {
		fill: color-mix(in oklab, var(--color-accent-muted) 60%, transparent);
	}
	.app-line {
		fill: color-mix(in oklab, var(--color-text-secondary) 60%, transparent);
	}
	.app-line.strong {
		fill: color-mix(in oklab, var(--color-accent) 72%, transparent);
	}
	.cursor-shadow {
		fill: rgba(0, 0, 0, 0.32);
	}
	.cursor {
		fill: var(--color-text);
		stroke: color-mix(in oklab, var(--color-base) 72%, black);
		stroke-linejoin: round;
		stroke-width: 1;
	}

	/* === Theme pick (step 3) === */
	.theme-cards {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 0.875rem;
		width: 100%;
	}
	.theme-card {
		position: relative;
		display: flex;
		flex-direction: column;
		align-items: stretch;
		gap: 0.625rem;
		padding: 0.875rem;
		border: 1px solid color-mix(in oklab, var(--color-border) 90%, transparent);
		background: color-mix(in oklab, var(--color-surface) 50%, transparent);
		border-radius: var(--radius-md);
		cursor: pointer;
		text-align: left;
		font: inherit;
		color: var(--color-text);
		backdrop-filter: blur(2px);
		transition:
			border-color var(--duration-base) var(--ease-out),
			background var(--duration-base) var(--ease-out),
			box-shadow var(--duration-base) var(--ease-out);
	}
	.theme-card:hover:not(.selected) {
		border-color: color-mix(in oklab, var(--color-accent) 35%, var(--color-border));
		background: color-mix(in oklab, var(--color-surface-hover) 70%, transparent);
	}
	.theme-card.selected {
		border-color: var(--color-accent);
		background: color-mix(in oklab, var(--color-accent) 8%, transparent);
		box-shadow: var(--shadow-glow);
	}
	.theme-card-badge {
		position: absolute;
		top: 0.5rem;
		right: 0.5rem;
		z-index: 1;
		font-size: 0.5625rem;
		letter-spacing: 0.16em;
		text-transform: uppercase;
		color: var(--color-accent);
		background: color-mix(in oklab, var(--color-accent) 18%, transparent);
		padding: 0.1875rem 0.5rem;
		border-radius: var(--radius-full);
		font-weight: 600;
		pointer-events: none;
	}
	/* Theme-invariant previews: each card always renders its own theme,
	   regardless of the currently-active theme. The hardcoded color
	   literals here are intentional — they ARE the visualisation, not
	   a token-pivot candidate. */
	.theme-card-preview {
		aspect-ratio: 16 / 10;
		border-radius: var(--radius-sm);
		position: relative;
		overflow: hidden;
	}
	.theme-card-preview[data-theme-preview="dark"] {
		background: linear-gradient(180deg, #131926 0 22%, #0a0e17 22%);
		border: 1px solid rgba(56, 189, 248, 0.15);
	}
	.theme-card-preview[data-theme-preview="dark"]::after {
		content: "";
		position: absolute;
		inset: 38% 24% 28% 16%;
		background: repeating-linear-gradient(
			180deg,
			rgba(56, 189, 248, 0.18) 0 2px,
			transparent 2px 8px
		);
		border-radius: 2px;
	}
	.theme-card-preview[data-theme-preview="light"] {
		background: linear-gradient(180deg, #ffffff 0 22%, #eef2f7 22%);
		border: 1px solid rgba(15, 23, 42, 0.10);
	}
	.theme-card-preview[data-theme-preview="light"]::after {
		content: "";
		position: absolute;
		inset: 38% 24% 28% 16%;
		background: repeating-linear-gradient(
			180deg,
			rgba(2, 132, 199, 0.20) 0 2px,
			transparent 2px 8px
		);
		border-radius: 2px;
	}
	.theme-card-label {
		font-size: 0.875rem;
		font-weight: 600;
		letter-spacing: 0.02em;
		color: var(--color-text);
	}
	.theme-footnote {
		font-size: 0.75rem;
		letter-spacing: 0.04em;
		color: var(--color-text-tertiary);
	}

	/* === Attribution (step 4) === */
	.attribution-body {
		margin: 0;
		font-size: 0.95rem;
		line-height: 1.6;
		color: var(--color-text-secondary);
	}
	.external {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--color-accent);
		text-decoration: none;
		align-self: start;
		padding: 0.5rem 0.875rem 0.5rem 1rem;
		border: 1px solid color-mix(in oklab, var(--color-accent) 35%, transparent);
		border-radius: var(--radius-md);
		background: color-mix(in oklab, var(--color-accent) 6%, transparent);
		transition: all var(--duration-base) var(--ease-out);
	}
	.external:hover {
		color: var(--color-accent-hover);
		border-color: var(--color-accent);
		background: color-mix(in oklab, var(--color-accent) 14%, transparent);
		box-shadow: var(--shadow-glow);
	}
	.external svg {
		width: 0.85rem;
		height: 0.85rem;
		transition: transform var(--duration-base) var(--ease-out);
	}
	.external:hover svg {
		transform: translate(2px, -2px);
	}

	/* === Bottom bar === */
	.bottom-bar {
		position: relative;
		z-index: 1;
		display: grid;
		grid-template-columns: auto 1fr auto;
		align-items: center;
		gap: 1.5rem;
		padding: 1.125rem 2rem 1.5rem;
		border-top: 1px solid color-mix(in oklab, var(--color-border) 70%, transparent);
		background: color-mix(in oklab, var(--color-surface) 35%, transparent);
		backdrop-filter: blur(8px);
	}

	.progress {
		display: flex;
		align-items: center;
		gap: 0;
	}
	.node {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: color-mix(in oklab, var(--color-text-tertiary) 55%, transparent);
		transition: all var(--duration-slow) var(--ease-out);
		flex-shrink: 0;
	}
	.node.passed {
		background: color-mix(in oklab, var(--color-accent) 55%, transparent);
	}
	.node.active {
		background: var(--color-accent);
		box-shadow: 0 0 14px color-mix(in oklab, var(--color-accent) 75%, transparent);
		transform: scale(1.2);
	}
	.track {
		width: 1.75rem;
		height: 1px;
		background: var(--color-border);
		transition: background var(--duration-slow) var(--ease-out);
		flex-shrink: 0;
	}
	.track.passed {
		background: color-mix(in oklab, var(--color-accent) 45%, transparent);
	}

	.status-line {
		display: flex;
		justify-content: center;
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

	/* === Narrow / split-screen friendly === */
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
		.progress {
			justify-content: center;
		}
		.status-line {
			display: none;
		}
		.controls {
			justify-content: space-between;
		}
	}
</style>
