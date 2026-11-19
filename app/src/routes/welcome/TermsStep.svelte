<script lang="ts">
	import MarkdownBody from '$lib/components/MarkdownBody.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import { PRIVACY_POLICY, TERMS_OF_SERVICE, type LegalDocument } from '$lib/legal';

	let { accepted = $bindable(false) } = $props<{ accepted?: boolean }>();

	// The documents ship with the app and open in place.
	let reading = $state<LegalDocument | null>(null);
	let readerOpen = $state(false);

	function read(doc: LegalDocument) {
		reading = doc;
		readerOpen = true;
	}
</script>

<div class="block">
	<header class="step-header">
		<span class="eyebrow">Before you continue</span>
		<h2 class="step-title">Terms and privacy.</h2>
	</header>
	<p class="body">
		Please review the Terms of Service and Privacy Policy.
	</p>
	<div class="links">
		<button type="button" class="document" onclick={() => read(TERMS_OF_SERVICE)}>
			{TERMS_OF_SERVICE.title}
		</button>
		<button type="button" class="document" onclick={() => read(PRIVACY_POLICY)}>
			{PRIVACY_POLICY.title}
		</button>
	</div>
	<label class="accept">
		<input type="checkbox" bind:checked={accepted} />
		<span class="accept-text">
			I have read and agree to the Terms of Service and Privacy Policy.
		</span>
	</label>
</div>

{#if reading}
	<Modal bind:open={readerOpen} title={reading.title} class="max-w-2xl">
		<MarkdownBody markdown={reading.body} />
	</Modal>
{/if}

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
	.document {
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
	.document:hover {
		color: var(--color-accent-hover);
		border-color: var(--color-accent);
		background: color-mix(in oklab, var(--color-accent) 14%, transparent);
		box-shadow: var(--shadow-glow);
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
