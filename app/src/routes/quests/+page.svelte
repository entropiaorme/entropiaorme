<script lang="ts">
	import { onMount } from 'svelte';
	import { Button, ErrorNotice, Tabs } from '$lib/components';
	import FamilyFormModal from '$lib/features/quests/FamilyFormModal.svelte';
	import FamilyListView from '$lib/features/quests/FamilyListView.svelte';
	import { createFamilyModel } from '$lib/features/quests/familyModel.svelte';
	import QuestAnalyticsView from '$lib/features/quests/QuestAnalyticsView.svelte';
	import QuestFormModal from '$lib/features/quests/QuestFormModal.svelte';
	import QuestListView from '$lib/features/quests/QuestListView.svelte';
	import QuestRewardReview from '$lib/features/quests/QuestRewardReview.svelte';
	import { createQuestsModel } from '$lib/features/quests/questsModel.svelte';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import { hydrate, subscribeTracking, trackingSnapshot } from '$lib/stores/trackingStore.svelte';

	const model = createQuestsModel();
	const familyModel = createFamilyModel({
		get families() {
			return model.families;
		},
		set families(value) {
			model.families = value;
		},
		get error() {
			return model.error;
		},
		set error(value) {
			model.error = value;
		},
		get deleteConfirmId() {
			return model.deleteConfirmId;
		},
		set deleteConfirmId(value) {
			model.deleteConfirmId = value;
		},
		refreshQuests: () => model.refresh(),
	});

	// View toggle
	let view: 'quests' | 'families' | 'review' | 'analytics' = $state('quests');

	// Cooldown tick
	let now = $state(Date.now());

	let trackingActive = $derived(trackingSnapshot.current?.status === 'active');

	onMount(() => {
		void model.loadData();
		return useVisiblePoll(() => { now = Date.now(); }, { intervalMs: 1000 });
	});

	// Quest data refreshes every 10s while tracking is active (below) to pick up
	// chat.log mission-completion lines. The active/idle signal that gates it is
	// event-driven: hydrate the tracking snapshot once, then keep it current from
	// pushed session frames rather than polling for session start/stop.
	$effect(() => {
		// Subscribe-then-hydrate (the canonical consumer discipline): attach the
		// tracking listener first so a frame landing during the initial read is
		// re-announced rather than lost. The hydrate stays independent of the
		// listen() promise (which never resolves in the e2e shell), so the first
		// read always runs; a frame arriving before the listener attaches is the
		// only residual gap, far smaller than reading before subscribing at all.
		let unsubscribe: (() => void) | undefined;
		let stopped = false;
		void subscribeTracking().then((un) => {
			// Guard the teardown-before-resolve race: detach immediately if the
			// effect already cleaned up rather than leaking the listener.
			if (stopped) un();
			else unsubscribe = un;
		});
		void hydrate();
		return () => {
			stopped = true;
			unsubscribe?.();
		};
	});

	$effect(() => {
		if (!trackingActive) return;
		return useVisiblePoll(() => model.refresh(), { intervalMs: 10000, immediate: false });
	});

	// Lazy-load analytics on first entry to the analytics tab.
	$effect(() => {
		if (view === 'analytics' && !model.analyticsLoaded && !model.analyticsLoading) {
			model.loadAnalytics();
		}
	});
</script>

<div class="px-6 pb-6 space-y-4">
	<!-- Header -->
	<div class="flex items-center justify-between">
		<header class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold text-text tracking-tight">Quests</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="text-sm text-text-secondary mt-0.5">Track missions, manage cooldowns, review rewards</p>
		</header>
		<div class="flex items-center gap-2">
			<Button size="sm" variant="secondary" onclick={() => model.openNewQuest()}>
				{#snippet children()}+ Quest{/snippet}
			</Button>
			<Button size="sm" variant="secondary" onclick={() => familyModel.openNewFamily()}>
				{#snippet children()}+ Family{/snippet}
			</Button>
		</div>
	</div>

	<ErrorNotice message={model.error} />

	<!-- Main tab toggle -->
	<Tabs
		tabs={[
			{ id: 'quests', label: 'Quests' },
			{ id: 'families', label: 'Families' },
			{ id: 'review', label: 'Reward Review' },
			{ id: 'analytics', label: 'Analytics' }
		]}
		active={view}
		onchange={(id) => (view = id as 'quests' | 'families' | 'review' | 'analytics')}
	/>

	{#if model.loading}
		<div class="text-sm text-text-tertiary py-8 text-center">Loading quests...</div>
	{:else if view === 'quests'}
		<QuestListView {model} {now} />
	{:else if view === 'families'}
		<FamilyListView model={familyModel} questsModel={model} {now} />
	{:else if view === 'review'}
		<QuestRewardReview />
	{:else if view === 'analytics'}
		<QuestAnalyticsView {model} />
	{/if}
</div>

<QuestFormModal {model} />
<FamilyFormModal model={familyModel} />
