<script lang="ts">
	import { untrack } from 'svelte';
	import { Skeleton, Tabs } from '$lib/components';
	import CustomiseStatsWidget from './CustomiseStatsWidget.svelte';
	import LootCompositionWidget from './LootCompositionWidget.svelte';
	import LootPulseWidget from './LootPulseWidget.svelte';
	import RecentEventsWidget from './RecentEventsWidget.svelte';
	import MapsSurface from '$lib/features/maps/MapsSurface.svelte';
	import { createDashboardEvents, dashboardTabs } from '$lib/extension/slots';
	import QuestBoard from '$lib/features/quests/board/QuestBoard.svelte';
	import type { BoardRow } from '$lib/features/quests/board/questBoard';
	import type { QuestBoardModel } from '$lib/features/quests/board/questBoardModel.svelte';
	import {
		acknowledgeHead,
		feedHead,
		hasUnseenEvents,
	} from '$lib/features/dashboard/recentEvents';
	import type { RecentEvent } from '$lib/api';
	import type { LiveLootValuation } from '$lib/features/dashboard/liveLootValuation.svelte';

	let {
		trackingPending,
		recentEvents,
		sessionId,
		valuation = null,
		multiplierHistory,
		cumulativeNetHistory,
		board,
		copiedWp,
		onCopyWaypoint,
		onCreateCustomQuest,
		onEditCustomQuest,
		onDeleteCustomQuest,
	}: {
		/** No tracking snapshot has been read yet, so whether a session is
		 * running is still unknown. */
		trackingPending: boolean;
		/** The live session's notable-event feed, newest first; `null` until
		 * the tracking snapshot has been read. */
		recentEvents: RecentEvent[] | null;
		sessionId: string | null;
		/** The live session's loot valuation, shared with the stat tiles. */
		valuation?: LiveLootValuation | null;
		multiplierHistory: number[] | null;
		cumulativeNetHistory: number[] | null;
		/** The quest board's view model (the Quests widget tab). */
		board: QuestBoardModel;
		copiedWp: string | null;
		onCopyWaypoint: (key: string, waypoint: string) => void;
		onCreateCustomQuest: (planet: string | null) => void;
		onEditCustomQuest: (row: BoardRow) => void;
		onDeleteCustomQuest: (row: BoardRow) => void;
	} = $props();

	let activeTab = $state<string>('pulse');

	// An event arriving while another tab is open marks the events tab until
	// the reader opens it.
	const eventsHead = $derived(recentEvents === null ? undefined : feedHead(recentEvents));
	let seenEventsHead = $state<string | null | undefined>(undefined);
	$effect.pre(() => {
		const head = eventsHead;
		const watching = activeTab === 'events';
		seenEventsHead = acknowledgeHead(untrack(() => seenEventsHead), head, watching);
	});
	const eventsUnseen = $derived(
		hasUnseenEvents(seenEventsHead, eventsHead, activeTab === 'events'),
	);

	// Rows an extension adds to the events tab. They are kept up here, not
	// in the tab body, so an unread one can light the tab while another
	// widget is open.
	const extraEvents = createDashboardEvents();
	$effect(() => extraEvents?.start());
	const extraUnread = $derived((extraEvents?.unread ?? false) && activeTab !== 'events');

	const tabs = $derived([
		{ id: 'pulse', label: 'Loot Pulse' },
		{ id: 'events', label: 'Recent Events', attention: eventsUnseen || extraUnread },
		{ id: 'loot', label: 'Loot Composition' },
		{ id: 'quests', label: 'Quests' },
		{ id: 'map', label: 'Map' },
		...dashboardTabs.map(({ id, label }) => ({ id, label })),
		{ id: 'customise', label: 'Customise Stats' },
	]);
	const extensionTab = $derived(dashboardTabs.find((tab) => tab.id === activeTab));

	// Whether the open tab's data is still unread, so its empty state ("No
	// active session", an empty board) would be a claim, not a fact.
	// The map and the stats customiser load their own data and show their own
	// loading states.
	const tabPending = $derived.by(() => {
		switch (activeTab) {
			case 'events':
			case 'pulse':
			case 'loot':
				return trackingPending;
			case 'quests':
				return !board.catalogueLoaded;
			default:
				return false;
		}
	});

</script>

<section
	class="panel p-4 flex-1 min-h-[480px] flex flex-col"
	data-testid="dashboard-widgets-area"
>
	<Tabs
		{tabs}
		active={activeTab}
		onchange={(id) => (activeTab = id)}
		class="mb-3 {extraUnread ? extraEvents?.unreadClass : ''}"
	/>

	{#if tabPending}
		<div class="flex-1 flex flex-col" aria-busy="true" data-testid="dashboard-widget-pending">
			<Skeleton class="flex-1 w-full rounded-md" />
		</div>
	{:else if activeTab === 'events'}
		{#if extraEvents}<extraEvents.component model={extraEvents.model} />{/if}
		<!-- Extra rows above an empty feed: "No recent events" would read
			 as a claim about the row sitting right over it. -->
		{#if (recentEvents?.length ?? 0) > 0 || !extraEvents?.hasRows}
			<RecentEventsWidget events={recentEvents ?? []} />
		{/if}
	{:else if activeTab === 'pulse'}
		<LootPulseWidget history={multiplierHistory} netHistory={cumulativeNetHistory} />
	{:else if activeTab === 'loot'}
		<LootCompositionWidget {sessionId} {valuation} />
	{:else if activeTab === 'quests'}
		<QuestBoard
			model={board}
			copiedKey={copiedWp}
			{onCopyWaypoint}
			onCreateCustom={onCreateCustomQuest}
			onEditCustom={onEditCustomQuest}
			onDeleteCustom={onDeleteCustomQuest}
		/>
	{:else if activeTab === 'map'}
		<MapsSurface class="flex-1" />
	{:else if activeTab === 'customise'}
		<CustomiseStatsWidget />
	{:else if extensionTab}
		<extensionTab.component />
	{/if}
</section>

