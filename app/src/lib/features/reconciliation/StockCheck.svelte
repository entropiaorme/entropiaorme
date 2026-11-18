<script lang="ts">
	/**
	 * Checking the stock against the account: choose where the comparison
	 * comes from, review each difference, apply. Moving from one step to the
	 * next takes the focus to the step's heading, so a keyboard or a screen
	 * reader follows.
	 */
	import { tick } from 'svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import CheckResult from './CheckResult.svelte';
	import CheckReview from './CheckReview.svelte';
	import CheckStart from './CheckStart.svelte';
	import type { CheckStage, ReconciliationModel } from './reconciliationModel.svelte';

	let { model, ondone }: { model: ReconciliationModel; ondone: () => void } = $props();

	const HEADINGS: Record<CheckStage, string> = {
		start: 'Compare with your account',
		review: 'What differs',
		done: 'Applied',
	};

	let heading = $state<HTMLHeadingElement | null>(null);
	let shown: CheckStage | null = null;
	$effect(() => {
		const stage = model.stage;
		if (model.loading) return;
		// The first step shown keeps the focus where the page put it.
		if (shown !== null && shown !== stage) void tick().then(() => heading?.focus());
		shown = stage;
	});
</script>

<div class="flex flex-col gap-6">
	<ErrorNotice message={model.error} />

	{#if model.loading}
		<p class="py-16 text-center text-sm text-text-tertiary" role="status">Reading your stock...</p>
	{:else}
		<h2 bind:this={heading} tabindex="-1" class="text-base font-medium text-text focus:outline-none">
			{model.stage === 'done' && model.undone ? 'Check undone' : HEADINGS[model.stage]}
		</h2>
		{#if model.stage === 'review' && model.draft}
			<CheckReview {model} />
		{:else if model.stage === 'done'}
			<CheckResult {model} {ondone} />
		{:else}
			<CheckStart {model} />
		{/if}
	{/if}
</div>
