<script lang="ts">
	// The networking & privacy onboarding panel. Both features are ON by
	// default; the user opts out here. The bold promise is about USER DATA
	// (which never leaves the machine), worded so it stays true even though the
	// app does make two narrow outbound calls (news + update checks) that send
	// nothing about the user. Copy is placeholder-grade pending a community-voice
	// pass; the structure and defaults are the load-bearing part.
	let { news = $bindable(true), autoUpdate = $bindable(true) } = $props<{
		news?: boolean;
		autoUpdate?: boolean;
	}>();
</script>

<div class="block">
	<header class="step-header">
		<span class="eyebrow">Privacy &amp; networking</span>
		<h2 class="step-title">Your data never leaves your machine.</h2>
	</header>
	<p class="body">
		EntropiaOrme keeps everything about your gameplay on your computer. Your chat logs, loot,
		skills, and analytics are never uploaded, shared, or sent anywhere.
	</p>
	<p class="body">
		The app reaches out to <code>entropiaorme.com</code> for two things only: project news and app
		updates. Neither sends any of your data; they only fetch. Both are on by default, and you can turn
		either off here or later in Settings.
	</p>
	<label class="toggle">
		<input type="checkbox" bind:checked={news} />
		<span class="toggle-copy">
			<span class="toggle-title">News &amp; Updates</span>
			<span class="toggle-sub">Pull project articles and release notices from entropiaorme.com.</span>
		</span>
	</label>
	<label class="toggle">
		<input type="checkbox" bind:checked={autoUpdate} />
		<span class="toggle-copy">
			<span class="toggle-title">Automatic updates</span>
			<span class="toggle-sub">Check for new versions of EntropiaOrme and install them with your go-ahead.</span>
		</span>
	</label>
	<span class="footnote">Change either of these any time from Settings.</span>
</div>

<style>
	.block {
		display: grid;
		gap: 1.125rem;
	}
	.step-header {
		display: flex;
		flex-direction: column;
		gap: 0.625rem;
	}
	.eyebrow {
		font-size: 0.6875rem;
		letter-spacing: 0.22em;
		text-transform: uppercase;
		color: var(--color-text-tertiary);
		font-weight: 500;
	}
	.step-title {
		font-size: 1.5rem;
		line-height: 1.2;
		font-weight: 500;
		letter-spacing: -0.012em;
		color: var(--color-text);
		margin: 0;
	}
	.body {
		margin: 0;
		font-size: 0.9rem;
		line-height: 1.6;
		color: var(--color-text-secondary);
		max-width: 48ch;
	}
	.body :global(code) {
		font-family: var(--font-mono, ui-monospace, 'Cascadia Code', monospace);
		font-size: 0.85em;
		padding: 0.05rem 0.3rem;
		border-radius: var(--radius-sm, 4px);
		background: color-mix(in oklab, var(--color-surface) 60%, transparent);
		border: 1px solid color-mix(in oklab, var(--color-border) 60%, transparent);
		color: var(--color-text);
	}
	.toggle {
		display: flex;
		align-items: flex-start;
		gap: 0.75rem;
		padding: 0.875rem 1rem;
		border: 1px solid color-mix(in oklab, var(--color-border) 90%, transparent);
		border-radius: var(--radius-md);
		background: color-mix(in oklab, var(--color-surface) 50%, transparent);
		cursor: pointer;
		transition:
			border-color var(--duration-base) var(--ease-out),
			background var(--duration-base) var(--ease-out);
	}
	.toggle:hover {
		border-color: color-mix(in oklab, var(--color-accent) 35%, var(--color-border));
		background: color-mix(in oklab, var(--color-surface-hover) 70%, transparent);
	}
	.toggle input[type='checkbox'] {
		flex-shrink: 0;
		margin-top: 0.125rem;
		width: 0.9375rem;
		height: 0.9375rem;
		accent-color: var(--color-accent);
		cursor: pointer;
	}
	.toggle-copy {
		display: grid;
		gap: 0.25rem;
	}
	.toggle-title {
		font-size: 0.8125rem;
		font-weight: 600;
		color: var(--color-text);
	}
	.toggle-sub {
		font-size: 0.75rem;
		line-height: 1.5;
		color: var(--color-text-secondary);
	}
	.footnote {
		font-size: 0.75rem;
		letter-spacing: 0.04em;
		color: var(--color-text-tertiary);
	}
</style>
