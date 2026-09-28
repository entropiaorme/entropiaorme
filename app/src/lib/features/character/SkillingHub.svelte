<script lang="ts">
	import type { CharacterModel } from './characterModel.svelte';
	import SkillingActivities from './SkillingActivities.svelte';
	import SkillingAnswers from './SkillingAnswers.svelte';
	import SkillingForecast from './SkillingForecast.svelte';
	import SkillingPath from './SkillingPath.svelte';
	import SkillingTargetHeader from './SkillingTargetHeader.svelte';

	// The skilling hub: "I want to skill up X; what do I need to know?"
	// answered in one place. The target and goal lead; the answers at a
	// glance follow; each facet then explains its answer.
	let { model }: { model: CharacterModel } = $props();
	const hub = $derived(model.skilling);

	// Restore the last target once the profession list its current level
	// reads has arrived.
	$effect(() => {
		if (!model.loading) void hub.restore();
	});

	$effect(() => () => hub.dispose());

	const quickPicks = $derived(model.stats.topProfessions.slice(0, 5));
</script>

<div class="space-y-8 pb-4">
	<SkillingTargetHeader {model} />

	{#if hub.target.kind === 'none'}
		<div class="border-t border-border/50 pt-7">
			<p class="max-w-xl text-sm leading-relaxed text-text-secondary">
				Pick what you want to skill up: a profession, a profession family, or HP. You will see where
				to train it, what your own named sessions say reaching a goal takes, and the cheapest skill
				path for codex rewards and chips.
			</p>
			<div class="mt-5 flex flex-wrap items-center gap-2">
				{#each quickPicks as prof (prof.name)}
					<button
						type="button"
						class="filter-chip border border-border/60"
						onclick={() => hub.setTarget({ kind: 'profession', name: prof.name })}
					>
						{prof.name}
						<span class="ml-1 tabular-nums text-text-tertiary">{prof.level.toFixed(2)}</span>
					</button>
				{/each}
				<button type="button" class="filter-chip border border-border/60" onclick={() => hub.setTarget({ kind: 'hp' })}>
					HP
					<span class="ml-1 tabular-nums text-text-tertiary">{model.stats.hp}</span>
				</button>
			</div>
		</div>
	{:else}
		<SkillingAnswers {hub} />
		<SkillingActivities {hub} />
		<SkillingForecast {hub} />
		<SkillingPath {hub} />
	{/if}
</div>
