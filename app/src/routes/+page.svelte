<script lang="ts">
	import { onMount } from 'svelte';
	import DashboardWidgets from '$lib/components/dashboard/DashboardWidgets.svelte';
	import { createGuideDemoModel } from '$lib/features/dashboard/guideDemoModel.svelte';
	import GuideOverlayDemo from '$lib/features/dashboard/GuideOverlayDemo.svelte';
	import SessionIsland from '$lib/features/dashboard/SessionIsland.svelte';
	import DosesPanel from '$lib/features/consumables/DosesPanel.svelte';
	import { createStatsGridModel } from '$lib/features/dashboard/statsGridModel.svelte';
	import SessionStage from '$lib/features/sessions/SessionStage.svelte';
	import { createLiveDefinitionsModel } from '$lib/features/sessions/definitionsModel.svelte';
	import { createLiveReviewModel } from '$lib/features/sessions/reviewModel.svelte';
	import { createQuestsModel } from '$lib/features/quests/questsModel.svelte';
	import { getCooldownRemaining } from '$lib/features/quests/cooldown';
	import { getActivityOptions, type ActivityOptionsResult } from '$lib/api';
	import { Skeleton } from '$lib/components';
	import { closeGuide, openGuide } from '$lib/guide/engine';
	import { guideState, registerDemoApi, unregisterDemoApi } from '$lib/guide/state.svelte';
	import { dashboardSurface } from '$lib/guide/surfaces/dashboard';
	import { getPreference } from '$lib/preferences';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import { hydrate, subscribeTracking, trackingSnapshot } from '$lib/stores/trackingStore.svelte';

	// The consolidated tracking readout, sourced from the store: the dashboard's
	// single source of live-session render shape. Every tracking read on this
	// route flows through this one derived (the island, widgets, and events list
	// take it as input), so the store has a single consumption point here.
	let status = $derived(trackingSnapshot.current);
	let recentEvents = $derived(trackingSnapshot.current?.recentEvents ?? []);

	// Quest data + lifecycle handlers come from the shared quests feature model;
	// the dashboard reads the selected or running session's integrated roster. The stats
	// grid and the guide-demo plumbing are dashboard feature models.
	const questsModel = createQuestsModel();
	const statsGrid = createStatsGridModel(() => status?.lifetime != null);
	const guideDemo = createGuideDemoModel(statsGrid);

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
	// route changes. Paused during a guide tour: the fixture load below owns
	// the data then, and the live refresh would overwrite it.
	$effect(() => {
		if (guideState.isActive) return;
		const pollMs = status?.status === 'active' ? 3000 : 5000;
		return useVisiblePoll(refreshQuestState, { intervalMs: pollMs });
	});


	// Guide
	let guideSeen = $state(true);
	function toggleSurfaceGuide(): void {
		if (guideState.isActive) {
			closeGuide();
		} else {
			guideSeen = true;
			void openGuide(dashboardSurface);
		}
	}

	onMount(() => {
		void (async () => {
			guideSeen = await getPreference<boolean>('guide_seen_dashboard', false);
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
		registerDemoApi('dashboard', guideDemo.demoApi());
		return () => {
			unmounted = true;
			unregisterDemoApi('dashboard');
			unsubscribeTracking?.();
		};
	});

	// Re-fetch tracking + quest data when guide-mode flips so the dashboard
	// switches between real and demo endpoints immediately instead of waiting
	// on the next 3-5s poll tick. Each dependency on guideState.isActive is a
	// void-read for reactivity tracking; the fetches inherit the demo routing
	// from $lib/api and the quests model's guide-mode load.
	$effect(() => {
		const active = guideState.isActive;
		// Stats: snapshot the live config on guide-open + apply the preselected
		// demo configuration; restore on close. Owned by the stats-grid model.
		statsGrid.syncGuideStats(active);
		// Widgets tab: snapshot on open + force 'pulse'; restore on close.
		guideDemo.syncWidgetsTab(active);
		// Re-read the consolidated snapshot through the guide-aware client so the
		// switch between real and demo data is immediate. This is also the
		// dashboard's initial load on mount.
		void hydrate();
		void guideDemo.refreshDemoTracking(active);
		void (async () => {
			const [, options] = await Promise.all([questsModel.loadData(active), getActivityOptions()]);
			activityOptions = options;
			if (questsModel.error) return;
		})();
	});
</script>

<SessionStage
	model={definitions}
	{review}
	class="px-6 pb-6 flex flex-col gap-4 h-full"
	data-guide-anchor="dashboard-area"
>

	<!-- Page header -->
	<div class="flex items-center justify-between flex-shrink-0">
		<header class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold text-text tracking-tight">Dashboard</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="text-sm text-text-secondary mt-0.5">Track sessions, monitor events, follow session quests</p>
		</header>
		<div class="flex items-center gap-2">
			<button
				type="button"
				onclick={toggleSurfaceGuide}
				title={guideState.isActive ? 'Exit guide' : 'Open guide'}
				aria-label={guideState.isActive ? 'Exit guide' : 'Open guide for this page'}
				class="relative h-8 w-8 rounded-full border border-border bg-surface hover:bg-surface-hover text-text-secondary hover:text-text transition-colors flex items-center justify-center text-sm font-semibold {guideState.isActive ? 'z-[9100]' : ''}"
			>
				{#if guideState.isActive}
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="w-3.5 h-3.5" aria-hidden="true">
						<path d="M5.28 4.22a.75.75 0 00-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 101.06 1.06L8 9.06l2.72 2.72a.75.75 0 101.06-1.06L9.06 8l2.72-2.72a.75.75 0 00-1.06-1.06L8 6.94 5.28 4.22z" />
					</svg>
				{:else}
					?
				{/if}
				{#if !guideSeen}
					<span class="absolute -top-0.5 -right-0.5 h-2 w-2 rounded-full bg-accent"></span>
				{/if}
			</button>
		</div>
	</div>

	<SessionIsland
		{status}
		{statsGrid}
		{definitions}
		onReview={(definitionId) => void review.openReview(definitionId)}
	/>

	{#if !guideState.isActive}
		<DosesPanel />
	{/if}

	{#if !(guideState.isActive && guideDemo.demoOverlayVisible)}
		<!-- ═══ Island: Recent Events ═══ -->
		<section
			class="panel p-4 flex-shrink-0"
			data-guide-anchor="dashboard-recent-events"
			aria-busy={status === null}
		>
			<h3 class="eyebrow mb-3">Recent events</h3>

			{#if status === null}
				<!-- Not read yet: an empty list here would claim there are none. -->
				<ul class="space-y-2" data-testid="recent-events-pending">
					{#each ['w-3/5', 'w-2/5', 'w-1/2'] as width}
						<li class="flex items-center h-5"><Skeleton class="h-3 {width}" /></li>
					{/each}
				</ul>
			{:else if recentEvents.length > 0}
				<ul class="relative space-y-2">
					{#each recentEvents.slice(0, 3) as event}
						<li class="flex items-center gap-2.5 text-sm">
							<span class="w-1.5 h-1.5 rounded-full shrink-0
								{event.type === 'hof'
									? 'bg-warning [box-shadow:0_0_8px_color-mix(in_oklab,var(--color-warning)_60%,transparent)]'
									: event.type === 'quest'
										? 'bg-positive [box-shadow:0_0_8px_color-mix(in_oklab,var(--color-positive)_60%,transparent)]'
										: event.type === 'warning'
											? 'bg-negative [box-shadow:0_0_8px_color-mix(in_oklab,var(--color-negative)_60%,transparent)]'
											: 'bg-accent [box-shadow:0_0_8px_color-mix(in_oklab,var(--color-accent)_60%,transparent)]'}"></span>
							<span class="text-text-secondary truncate">{event.description}</span>
							{#if event.value}
								<span class="ml-auto text-xs text-text-tertiary font-medium tabular-nums tracking-wider">{event.value}</span>
							{/if}
						</li>
					{/each}
				</ul>
			{:else}
				<div class="relative py-4 text-center">
					<p class="text-text-tertiary text-sm">No recent events.</p>
				</div>
			{/if}
		</section>

			<DashboardWidgets
				trackingPending={status === null}
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
	{/if}

	<GuideOverlayDemo
		demoTrackingLive={guideDemo.demoTrackingLive}
		{status}
		overlayStripPhase={guideDemo.overlayStripPhase}
		demoOverlayVisible={guideDemo.demoOverlayVisible}
		demoArmourPopupVisible={guideDemo.demoArmourPopupVisible}
		demoArmourPopupRecorded={guideDemo.demoArmourPopupRecorded}
		armourPopupTop={guideDemo.armourPopupTop}
		armourPopupLeft={guideDemo.armourPopupLeft}
	/>
</SessionStage>
