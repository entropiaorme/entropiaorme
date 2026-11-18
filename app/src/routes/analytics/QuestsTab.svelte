<script lang="ts">
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import QuestAnalyticsView from '$lib/features/quests/QuestAnalyticsView.svelte';
	import { createQuestsModel } from '$lib/features/quests/questsModel.svelte';

	const model = createQuestsModel();
	let planet = $state<string | null>(null);

	$effect(() => {
		void model.loadAnalytics();
	});

	const planets = $derived(
		[...new Set(model.analyticsData.map((row) => row.planet))].sort((a, b) => a.localeCompare(b))
	);
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<p class="text-sm text-text-tertiary">What each quest has paid, from its recorded completions.</p>
		{#if planets.length > 1}
			<SegmentedControl
				options={[{ id: '', label: 'All planets' }, ...planets.map((name) => ({ id: name, label: name }))]}
				active={planet ?? ''}
				onchange={(id) => (planet = id === '' ? null : id)}
			/>
		{/if}
	</div>
	<QuestAnalyticsView {model} {planet} />
</div>
