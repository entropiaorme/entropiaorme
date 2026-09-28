<script lang="ts">
	import type { CharacterModel } from './characterModel.svelte';
	import SkillingActivities from './SkillingActivities.svelte';
	import SkillingForecast from './SkillingForecast.svelte';
	import SkillingPath from './SkillingPath.svelte';
	import SkillingTargetHeader from './SkillingTargetHeader.svelte';

	// The skilling hub: "I want to skill up X; what do I need to know?"
	// answered in one place. The target and goal lead; the session
	// forecast, the activity recommender, and the optimiser each answer
	// against them.
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
		<div class="flex flex-wrap items-center gap-2">
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
	{:else}
		<SkillingForecast {hub} />
		<SkillingActivities {hub} />
		<SkillingPath {hub} />
	{/if}
</div>
