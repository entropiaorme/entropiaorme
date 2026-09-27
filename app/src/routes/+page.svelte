<script lang="ts">
	import { onMount } from 'svelte';
	import DashboardWidgets from '$lib/components/dashboard/DashboardWidgets.svelte';
	import SessionIsland from '$lib/features/dashboard/SessionIsland.svelte';
	import { createStatsGridModel } from '$lib/features/dashboard/statsGridModel.svelte';
	import SessionStage from '$lib/features/sessions/SessionStage.svelte';
	import { createLiveDefinitionsModel } from '$lib/features/sessions/definitionsModel.svelte';
	import { createLiveReviewModel } from '$lib/features/sessions/reviewModel.svelte';
	import { createQuestsModel } from '$lib/features/quests/questsModel.svelte';
	import { getCooldownRemaining } from '$lib/features/quests/cooldown';
	import { getActivityOptions, type ActivityOptionsResult } from '$lib/api';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import { hydrate, subscribeTracking, trackingSnapshot } from '$lib/stores/trackingStore.svelte';

	// The consolidated tracking readout, sourced from the store: the dashboard's
	// single source of live-session render shape. Every tracking read on this
	// route flows through this one derived (the island and widgets take it as
	// input), so the store has a single consumption point here.
	let status = $derived(trackingSnapshot.current);

	// Quest data + lifecycle handlers come from the shared quests feature model;
	// the dashboard reads the selected or running session's integrated roster. The stats
	// grid is a dashboard feature model.
	const questsModel = createQuestsModel();
	const statsGrid = createStatsGridModel(() => status?.lifetime != null);

	// The sessions surface: the island renders the picker and the review
	// entry; the stage hosts the two full-screen surfaces (authoring and
	// review) that replace the dashboard while either is open.
	const definitions = createLiveDefinitionsModel();
	const review = createLiveReviewModel(definitions.loadDefinitions);
	let activityOptions = $state<ActivityOptionsResult | null>(null);

	async function refreshQuestState() {
		const [, options] = await Promise.all([questsModel.refresh(), getActivityOptions()]);
		activityOptions = options;
	}

	function openSessionAuthoring(definitionId: number | null) {
		const editing = definitions.definitions.find(
			(definition) => definition.id === String(definitionId),
		);
		if (editing) definitions.openEdit(editing);
		else definitions.openCreate();
	}

	// Poll quest state so chat.log auto-start/complete is reflected without
	// route changes.
	$effect(() => {
		const pollMs = status?.status === 'active' ? 3000 : 5000;
		return useVisiblePoll(refreshQuestState, { intervalMs: pollMs });
	});

	onMount(() => {
		// The dashboard's initial load: the consolidated snapshot, then the
		// quest roster and activity options.
		void hydrate();
		void (async () => {
			const [, options] = await Promise.all([questsModel.loadData(), getActivityOptions()]);
			activityOptions = options;
		})();
		// Keep the consolidated snapshot current by subscribing to the bridged
		// backend tracking events: each one re-reads the snapshot, so the session
		// island and stats grid update by subscription rather than by polling.
		let unsubscribeTracking: (() => void) | undefined;
		let unmounted = false;
		void subscribeTracking().then((unlisten) => {
			// Guard the unmount-before-resolve race: if teardown already ran,
			// detach immediately rather than leaking the listener.
			if (unmounted) unlisten();
			else unsubscribeTracking = unlisten;
		});
		return () => {
			unmounted = true;
			unsubscribeTracking?.();
		};
	});
</script>

<SessionStage
	model={definitions}
	{review}
	class="px-6 pb-6 flex flex-col gap-4 h-full"
	data-testid="dashboard-area"
>

	<!-- Page header -->
	<div class="flex items-center justify-between flex-shrink-0">
		<header class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold text-text tracking-tight">Dashboard</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="text-sm text-text-secondary mt-0.5">Track sessions, monitor events, follow session quests</p>
		</header>
	</div>

	<SessionIsland
		{status}
		{statsGrid}
		{definitions}
		onReview={(definitionId) => void review.openReview(definitionId)}
	/>

	<DashboardWidgets
		trackingPending={status === null}
		recentEvents={status === null ? null : (status.recentEvents ?? [])}
		sessionId={status?.session_id ?? null}
		multiplierHistory={status?.multiplierHistory ?? null}
		cumulativeNetHistory={status?.cumulativeNetHistory ?? null}
		{activityOptions}
		quests={questsModel.quests}
		pendingCancelChoiceQuestId={questsModel.pendingCancelChoiceQuestId}
		copiedWp={questsModel.copiedWp}
		onQuestStart={questsModel.handleStart}
		onQuestComplete={questsModel.handleComplete}
		onQuestCancel={questsModel.handleCancel}
		onToggleCancelChoice={questsModel.toggleCancelChoice}
		onCopyWaypoint={questsModel.copyWaypoint}
		onEditSession={openSessionAuthoring}
		getCooldownRemaining={(quest) => getCooldownRemaining(quest, Date.now())}
	/>
</SessionStage>
