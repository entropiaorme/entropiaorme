<script lang="ts">
	import { externalLinks } from '$lib/utils/openExternal';

	let { accepted = $bindable(false) } = $props<{ accepted?: boolean }>();

	const TERMS_URL = 'https://entropiaorme.com/terms';
	const PRIVACY_URL = 'https://entropiaorme.com/privacy';
</script>

<div class="block">
	<header class="step-header">
		<span class="eyebrow">Before you continue</span>
		<h2 class="step-title">Terms and privacy.</h2>
	</header>
	<p class="body">
		Please review the linked Terms of Service and Privacy Policy.
	</p>
	<div class="links">
		<a class="external" href={TERMS_URL} target="_blank" rel="noopener noreferrer" use:externalLinks>
			<span>Terms of Service</span>
			<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
				<path
					fill-rule="evenodd"
					d="M5.22 14.78a.75.75 0 001.06 0l7.22-7.22v5.69a.75.75 0 001.5 0v-7.5a.75.75 0 00-.75-.75h-7.5a.75.75 0 000 1.5h5.69l-7.22 7.22a.75.75 0 000 1.06z"
					clip-rule="evenodd"
				/>
			</svg>
		</a>
		<a class="external" href={PRIVACY_URL} target="_blank" rel="noopener noreferrer" use:externalLinks>
			<span>Privacy Policy</span>
			<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
				<path
					fill-rule="evenodd"
					d="M5.22 14.78a.75.75 0 001.06 0l7.22-7.22v5.69a.75.75 0 001.5 0v-7.5a.75.75 0 00-.75-.75h-7.5a.75.75 0 000 1.5h5.69l-7.22 7.22a.75.75 0 000 1.06z"
					clip-rule="evenodd"
				/>
			</svg>
		</a>
	</div>
	<label class="accept">
		<input type="checkbox" bind:checked={accepted} />
		<span class="accept-text">
			I have read and agree to the Terms of Service and Privacy Policy.
		</span>
	</label>
</div>

<style>
	.block {
		display: grid;
		gap: 1.5rem;
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
		font-size: 0.95rem;
		line-height: 1.6;
		color: var(--color-text-secondary);
		max-width: 42ch;
	}
	.links {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 0.5rem;
	}
	.external {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--color-accent);
		text-decoration: none;
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
	.accept {
		display: flex;
		align-items: flex-start;
		gap: 0.625rem;
		padding: 0.875rem 1rem;
		border: 1px solid color-mix(in oklab, var(--color-border) 90%, transparent);
		border-radius: var(--radius-md);
		background: color-mix(in oklab, var(--color-surface) 50%, transparent);
		cursor: pointer;
		transition:
			border-color var(--duration-base) var(--ease-out),
			background var(--duration-base) var(--ease-out);
	}
	.accept:hover {
		border-color: color-mix(in oklab, var(--color-accent) 35%, var(--color-border));
		background: color-mix(in oklab, var(--color-surface-hover) 70%, transparent);
	}
	.accept input[type='checkbox'] {
		flex-shrink: 0;
		margin-top: 0.1875rem;
		width: 0.9375rem;
		height: 0.9375rem;
		accent-color: var(--color-accent);
		cursor: pointer;
	}
	.accept-text {
		font-size: 0.8125rem;
		line-height: 1.55;
		color: var(--color-text);
	}
</style>
