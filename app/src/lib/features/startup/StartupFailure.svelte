<script lang="ts">
	/**
	 * What the main window shows in place of its pages when the backend did
	 * not start: what went wrong in one plain sentence, what to do about it,
	 * and the two actions that help (a restart, and the logged detail for a
	 * report). A failed start is terminal for the session, so this never
	 * degrades into an endless loading state.
	 */
	import { getVersion } from '@tauri-apps/api/app';
	import { Button } from '$lib/components';
	import { restartApp } from '$lib/api/shell';
	import type { SubstrateFailure } from '$lib/api/readiness.svelte';
	import { failureCopy, failureReport } from './failureCopy';

	let { failure }: { failure: SubstrateFailure } = $props();

	const copy = $derived(failureCopy(failure.reason));
	let restarting = $state(false);
	let copyState = $state<'idle' | 'copied' | 'failed'>('idle');

	async function restart(): Promise<void> {
		restarting = true;
		try {
			await restartApp();
		} catch {
			restarting = false;
		}
	}

	async function copyDetails(): Promise<void> {
		let version: string | null = null;
		try {
			version = await getVersion();
		} catch {
			// The report is still useful without the version.
		}
		try {
			await navigator.clipboard.writeText(failureReport(failure, version));
			copyState = 'copied';
		} catch {
			copyState = 'failed';
		}
	}
</script>

<section class="h-full flex items-center justify-center px-6 pb-16" role="alert" data-testid="startup-failure">
	<div class="max-w-md flex flex-col gap-1.5">
		<h1 class="text-xl font-semibold text-text tracking-tight">EntropiaOrme couldn't start</h1>
		<span class="block h-px w-12 bg-gradient-to-r from-negative/60 to-transparent"></span>
		<p class="text-sm text-text-secondary mt-2">{copy.summary}</p>
		<p class="text-sm text-text-tertiary">{copy.remedy}</p>
		<div class="flex items-center gap-2 mt-4">
			<Button size="sm" variant="primary" loading={restarting} disabled={restarting} onclick={restart}>
				{#snippet children()}Restart{/snippet}
			</Button>
			<Button size="sm" variant="secondary" onclick={copyDetails}>
				{#snippet children()}{copyState === 'copied' ? 'Copied' : 'Copy details'}{/snippet}
			</Button>
			{#if copyState === 'failed'}
				<span class="text-xs text-text-tertiary">The clipboard is unavailable.</span>
			{/if}
		</div>
	</div>
</section>
