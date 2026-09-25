<script lang="ts">
	import { Skeleton, Tabs } from '$lib/components';
	import QuestingWidget from './QuestingWidget.svelte';
	import CustomiseStatsWidget from './CustomiseStatsWidget.svelte';
	import LootCompositionWidget from './LootCompositionWidget.svelte';
	import LootPulseWidget from './LootPulseWidget.svelte';
	import type { ActivityOptionsResult } from '$lib/api';
	import type { Quest } from '$lib/types/quests';
	import { onMount } from 'svelte';
	import { registerDemoApi, unregisterDemoApi } from '$lib/guide/state.svelte';

	let {
		trackingPending,
		sessionId,
		multiplierHistory,
		cumulativeNetHistory,
		activityOptions,
		quests,
		pendingCancelChoiceQuestId,
		copiedWp,
		onQuestStart,
		onQuestComplete,
		onQuestCancel,
		onToggleCancelChoice,
		onCopyWaypoint,
		onEditSession,
		getCooldownRemaining,
	}: {
		/** No tracking snapshot has been read yet, so whether a session is
		 * running is still unknown. */
		trackingPending: boolean;
		sessionId: string | null;
		multiplierHistory: number[] | null;
		cumulativeNetHistory: number[] | null;
		activityOptions: ActivityOptionsResult | null;
		quests: Quest[];
		pendingCancelChoiceQuestId: string | null;
		copiedWp: string | null;
		onQuestStart: (questId: string) => void;
		onQuestComplete: (questId: string) => void;
		onQuestCancel: (questId: string, undoReward: boolean) => void;
		onToggleCancelChoice: (questId: string) => void;
		onCopyWaypoint: (questId: string, waypoint: string) => void;
		onEditSession: (definitionId: number | null) => void;
		getCooldownRemaining: (quest: import('$lib/types/quests').Quest) => string | null;
	} = $props();

	const tabs = [
		{ id: 'pulse', label: 'Loot Pulse' },
		{ id: 'loot', label: 'Loot Composition' },
		{ id: 'quests', label: 'Quests' },
		{ id: 'customise', label: 'Customise Stats' },
	];

	let activeTab = $state<string>('pulse');

	onMount(() => {
		// Sub-API composition. Surface module's dashboard-widgets card cycles
		// tabs via setTab during its looped play(): the cursor's click ripple
		// is visual; setTab is the authoritative state mutation (DnD
		// imperative-shim shape generalised to tab switching).
		registerDemoApi('dashboard-widgets', {
			setTab: (id: string) => {
				activeTab = id;
			},
			getTab: () => activeTab
		});
		return () => unregisterDemoApi('dashboard-widgets');
	});
</script>

<section
	class="panel p-4 flex-1 min-h-[480px] flex flex-col"
	data-guide-anchor="dashboard-widgets-area"
>
	<Tabs {tabs} active={activeTab} onchange={(id) => (activeTab = id)} class="mb-3" />

	{#if activeTab !== 'customise' && (activeTab === 'quests' ? activityOptions === null : trackingPending)}
		<!-- The tab's data has not been read yet: its empty states ("No active
			 session", "Choose a session type") would be claims, not facts. -->
		<div class="flex-1 flex flex-col" aria-busy="true" data-testid="dashboard-widget-pending">
			<Skeleton class="flex-1 w-full rounded-md" />
		</div>
	{:else if activeTab === 'pulse'}
		<LootPulseWidget history={multiplierHistory} netHistory={cumulativeNetHistory} />
	{:else if activeTab === 'loot'}
		<LootCompositionWidget {sessionId} />
	{:else if activeTab === 'quests'}
		<QuestingWidget
			{activityOptions}
			{quests}
			{pendingCancelChoiceQuestId}
			{copiedWp}
			{onQuestStart}
			{onQuestComplete}
			{onQuestCancel}
			{onToggleCancelChoice}
			{onCopyWaypoint}
			{onEditSession}
			{getCooldownRemaining}
		/>
	{:else if activeTab === 'customise'}
		<CustomiseStatsWidget />
	{/if}
</section>
