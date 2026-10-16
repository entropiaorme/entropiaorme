<script lang="ts">
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import type { CharacterModel } from './characterModel.svelte';
	import SkillingActivities from './SkillingActivities.svelte';
	import SkillingPath from './SkillingPath.svelte';
	import SkillingRoutes from './SkillingRoutes.svelte';
	import SkillingSessions from './SkillingSessions.svelte';
	import type { SkillingView } from './skillingModel.svelte';
	import SkillingTargetHeader from './SkillingTargetHeader.svelte';

	// The skilling hub: "I want to skill up X; what do I need to know?"
	// answered in one place. The target and goal lead and stay put; under
	// them one view at a time answers against them: the routes (what each
	// way there costs), your sessions as recorded, the activities that
	// train it, and the optimiser.
	let { model }: { model: CharacterModel } = $props();
	const hub = $derived(model.skilling);

	// Restore the last target once the profession list its current level
	// reads has arrived.
	$effect(() => {
		if (!model.loading) void hub.restore();
	});

	$effect(() => () => hub.dispose());

	const quickPicks = $derived(model.stats.topProfessions.slice(0, 5));

	const VIEWS: { id: SkillingView; label: string }[] = [
		{ id: 'routes', label: 'Routes' },
		{ id: 'sessions', label: 'Sessions' },
		{ id: 'activities', label: 'Activities' },
		{ id: 'optimiser', label: 'Optimiser' },
	];
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
		<div>
			<div class="border-b border-border/40 pb-4">
				<SegmentedControl
					size="md"
					options={VIEWS}
					active={hub.view}
					onchange={(id) => hub.setView(id as SkillingView)}
				/>
			</div>
			<div class="pt-6">
				{#if hub.view === 'routes'}
					<SkillingRoutes {hub} professions={model.professions} />
				{:else if hub.view === 'sessions'}
					<SkillingSessions {hub} />
				{:else if hub.view === 'activities'}
					<SkillingActivities {hub} />
				{:else}
					<SkillingPath {hub} />
				{/if}
			</div>
		</div>
	{/if}
</div>
