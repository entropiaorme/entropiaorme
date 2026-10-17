<script lang="ts">
	import { onMount } from 'svelte';
	import DashboardWidgets from '$lib/components/dashboard/DashboardWidgets.svelte';
	import SessionIsland from '$lib/features/dashboard/SessionIsland.svelte';
	import { createStatsGridModel } from '$lib/features/dashboard/statsGridModel.svelte';
	import SessionStage from '$lib/features/sessions/SessionStage.svelte';
	import { createLiveDefinitionsModel } from '$lib/features/sessions/definitionsModel.svelte';
	import { createLiveReviewModel } from '$lib/features/sessions/reviewModel.svelte';
	import { createQuestsModel } from '$lib/features/quests/questsModel.svelte';
	import QuestFormModal from '$lib/features/quests/QuestFormModal.svelte';
	import { createQuestBoardModel } from '$lib/features/quests/board/questBoardModel.svelte';
	import type { BoardRow } from '$lib/features/quests/board/questBoard';
	import {
		abandonQuest,
		cancelQuest,
		completeQuest,
		getActivityOptions,
		getQuestCatalogue,
		getUnresolvedQuestRewards,
		reviewQuestReward,
		startQuest,
		updateSessionDefinition,
		type ActivityOptionsResult,
	} from '$lib/api';
	import { getPreference, setPreference } from '$lib/preferences';
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

	// The Quests widget's board: the curated catalogue laid out per planet, over
	// the same quest rows and Activities picture this route already polls.
	const board = createQuestBoardModel({
		quests: () => questsModel.quests,
		families: () => questsModel.families,
		activityOptions: () => activityOptions,
		definition: () =>
			definitions.definitions.find(
				(definition) => definition.id === String(activityOptions?.definitionId),
			) ?? null,
		getCatalogue: getQuestCatalogue,
		getUnresolved: getUnresolvedQuestRewards,
		startQuest,
		completeQuest,
		abandonQuest,
		cancelQuest,
		reviewReward: reviewQuestReward,
		updateDefinition: updateSessionDefinition,
		refresh: refreshQuestState,
		refreshDefinitions: definitions.loadDefinitions,
		getPreference,
		setPreference,
	});

	// A completion (from the chat log or the board) may leave a reward to
	// settle, so the list follows the recorded completions.
	$effect(() => {
		void board.completionSignature;
		void board.loadUnresolved();
	});

	function createCustomQuest(planet: string | null) {
		questsModel.openNewQuest();
		if (planet) questsModel.questForm.planet = planet;
	}

	function editCustomQuest(row: BoardRow) {
		if (row.quest) questsModel.openEditQuest(row.quest);
	}

	async function deleteCustomQuest(row: BoardRow) {
		if (row.quest) await questsModel.handleDeleteQuest(row.quest.id);
	}

	// Poll quest state so chat.log auto-start/complete is reflected without
	// route changes.
	$effect(() => {
		const pollMs = status?.status === 'active' ? 3000 : 5000;
		return useVisiblePoll(refreshQuestState, { intervalMs: pollMs });
	});

	onMount(() => {
		// The dashboard's initial load: the consolidated snapshot, then the
		// quest rows, the board's catalogue, and the activity options.
		void hydrate();
		void board.init();
		const stopTick = useVisiblePoll(() => { board.now = Date.now(); }, { intervalMs: 1000 });
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
			stopTick();
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
			<p class="text-sm text-text-secondary mt-0.5">Track sessions, monitor events, play your quests</p>
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
		{board}
		copiedWp={questsModel.copiedWp}
		onCopyWaypoint={questsModel.copyWaypoint}
		onCreateCustomQuest={createCustomQuest}
		onEditCustomQuest={editCustomQuest}
		onDeleteCustomQuest={deleteCustomQuest}
	/>
</SessionStage>

<QuestFormModal model={questsModel} compact />
