<script lang="ts">
	/**
	 * The strongbox opening view: choose the box, open it in game, review
	 * what came out, and record the run. One box type per run; a sitting with
	 * several types is several runs in a row.
	 */
	import { goto } from '$app/navigation';
	import Button from '$lib/components/Button.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import { formatPed, formatQuantity } from '$lib/utils/format';
	import BoxPicker from './BoxPicker.svelte';
	import DetectedOpenings from './DetectedOpenings.svelte';
	import OpeningIntake from './OpeningIntake.svelte';
	import OpeningReview from './OpeningReview.svelte';
	import OpeningsLog from './OpeningsLog.svelte';
	import type { OpeningModel, OpeningStage } from './openingModel.svelte';

	let { model }: { model: OpeningModel } = $props();

	const steps: { id: OpeningStage; label: string }[] = [
		{ id: 'choose', label: 'Choose' },
		{ id: 'open', label: 'Open' },
		{ id: 'review', label: 'Review' },
	];
	const order: Record<OpeningStage, number> = { choose: 0, open: 1, review: 2, done: 3 };
	const at = $derived(order[model.stage]);

	const recorded = $derived(model.recorded);
	const keptCount = $derived(
		recorded?.contents.filter((item) => item.nanocubes === null).length ?? 0,
	);
	const cubeCount = $derived(
		recorded?.contents.reduce((sum, item) => sum + (item.nanocubes ?? 0), 0) ?? 0,
	);
</script>

<div class="flex flex-col gap-8">
	<ol class="flex items-center gap-6" aria-label="Steps">
		{#each steps as step, i (step.id)}
			{@const state = i < at ? 'done' : i === at ? 'current' : 'next'}
			<li class="flex items-center gap-2.5" aria-current={state === 'current' ? 'step' : undefined}>
				<span
					class="flex h-5 w-5 items-center justify-center rounded-full border text-[10.5px] tabular-nums transition-colors {state ===
					'current'
						? 'border-accent bg-accent/10 text-accent'
						: state === 'done'
							? 'border-accent/40 text-accent/80'
							: 'border-border-bright text-text-tertiary'}"
				>
					{#if state === 'done'}&#10003;{:else}{i + 1}{/if}
				</span>
				<span class="eyebrow {state === 'current' ? 'eyebrow-strong' : ''}">{step.label}</span>
				{#if i < steps.length - 1}
					<span class="ml-3.5 h-px w-10 {i < at ? 'bg-accent/40' : 'bg-border-bright'}" aria-hidden="true"></span>
				{/if}
			</li>
		{/each}
	</ol>

	<ErrorNotice message={model.error} />

	{#if model.loading}
		<p class="py-16 text-center text-sm text-text-tertiary">Reading your strongboxes...</p>
	{:else}
		{#key model.stage}
			<div class="stage">
				{#if model.stage === 'choose'}
					<div class="max-w-2xl space-y-6">
						<DetectedOpenings {model} />
						{#if model.tracking}
							<p class="flex items-start gap-2.5 text-sm text-text-secondary" role="status">
								<span class="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full bg-warning" aria-hidden="true"></span>
								<span>
									<span class="text-text">Stop tracking to open boxes.</span> A running session's loot
									and ammo would land in the same inventory changes as what the boxes held.
								</span>
							</p>
						{/if}
						<BoxPicker holdings={model.holdings} selected={model.boxItem} onchoose={model.choose} />
						<div class="flex justify-end">
							<Button disabled={!model.boxItem || model.tracking} onclick={model.begin}>
								Continue
							</Button>
						</div>
					</div>
				{:else if model.stage === 'open'}
					<div class="space-y-8">
						<p class="text-sm text-text-secondary">
							Opening <span class="text-text">{model.boxItem}</span>. A key pays back its TT in whatever
							the box holds; what a box is worth is the markup on that.
						</p>
						<OpeningIntake {model} />
						<Button variant="ghost" size="sm" onclick={model.back}>Choose another box</Button>
					</div>
				{:else if model.stage === 'review' && model.result}
					<OpeningReview {model} />
				{:else if model.stage === 'done'}
					<section aria-label="Recorded" class="max-w-2xl space-y-5">
						<p class="flex items-center gap-3">
							<span class="flex h-8 w-8 items-center justify-center rounded-full bg-positive/15 text-positive" aria-hidden="true">
								&#10003;
							</span>
							<span class="text-xl font-light text-text">Opening recorded</span>
						</p>
						{#if recorded}
							<p class="text-sm leading-relaxed text-text-secondary">
								{formatQuantity(recorded.boxCount)}
								{recorded.boxItem} came to
								{#if keptCount > 0}{keptCount} {keptCount === 1 ? 'item' : 'items'} kept,{/if}
								{#if cubeCount > 0}{formatQuantity(cubeCount)} Nanocubes,{/if}
								and {formatPed(recorded.ammoPed)} PED of ammo. They are in your loot now, carrying the
								boxes' origin: whatever they sell for above TT credits the play that looted the boxes.
							</p>
						{/if}
						<div class="flex gap-2">
							<Button onclick={model.startOver}>Open another box type</Button>
							<Button variant="secondary" onclick={() => goto('/inventory')}>Back to Inventory</Button>
						</div>
					</section>
				{/if}
			</div>
		{/key}

		{#if (model.stage === 'choose' || model.stage === 'done') && model.openings.length > 0}
			<section aria-label="Past openings" class="max-w-2xl space-y-2 pt-2">
				<span class="eyebrow">Past openings</span>
				<OpeningsLog
					openings={model.openings}
					undoing={model.undoing}
					onundo={(id) => void model.undo(id)}
					highlight={recorded?.id ?? null}
				/>
			</section>
		{/if}
	{/if}
</div>

<style>
	.stage {
		animation: enter 260ms var(--ease-out) both;
	}
	@keyframes enter {
		from {
			opacity: 0;
			transform: translateY(4px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.stage {
			animation: none;
		}
	}
</style>
