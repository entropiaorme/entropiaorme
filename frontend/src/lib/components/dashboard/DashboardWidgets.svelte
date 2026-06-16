<script lang="ts">
	import { Tabs } from '$lib/components';
	import QuestingWidget, { type PlaylistQuestItem } from './QuestingWidget.svelte';
	import CustomiseStatsWidget from './CustomiseStatsWidget.svelte';
	import LootCompositionWidget from './LootCompositionWidget.svelte';
	import LootPulseWidget from './LootPulseWidget.svelte';
	import type { Quest, QuestPlaylist } from '$lib/types/quests';
	import { onMount } from 'svelte';
	import { registerDemoApi, unregisterDemoApi } from '$lib/guide/state.svelte';

	let {
		sessionId,
		multiplierHistory,
		cumulativeNetHistory,
		playlists,
		activePlaylistId,
		activePlaylist,
		immediateItems,
		longHorizonItems,
		pendingCancelChoiceQuestId,
		copiedWp,
		onPlaylistChange,
		onQuestStart,
		onQuestComplete,
		onQuestCancel,
		onToggleCancelChoice,
		onCopyWaypoint,
		formatMinutes,
		getCooldownRemaining,
	}: {
		sessionId: string | null;
		multiplierHistory: number[] | null;
		cumulativeNetHistory: number[] | null;
		playlists: QuestPlaylist[];
		activePlaylistId: string | null;
		activePlaylist: QuestPlaylist | null;
		immediateItems: PlaylistQuestItem[];
		longHorizonItems: PlaylistQuestItem[];
		pendingCancelChoiceQuestId: string | null;
		copiedWp: string | null;
		onPlaylistChange: (id: string | null) => void;
		onQuestStart: (questId: string) => void;
		onQuestComplete: (questId: string) => void;
		onQuestCancel: (questId: string, undoReward: boolean) => void;
		onToggleCancelChoice: (questId: string) => void;
		onCopyWaypoint: (questId: string, waypoint: string) => void;
		formatMinutes: (m: number) => string;
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

	{#if activeTab === 'pulse'}
		<LootPulseWidget history={multiplierHistory} netHistory={cumulativeNetHistory} />
	{:else if activeTab === 'loot'}
		<LootCompositionWidget {sessionId} />
	{:else if activeTab === 'quests'}
		<QuestingWidget
			{playlists}
			{activePlaylistId}
			{activePlaylist}
			{immediateItems}
			{longHorizonItems}
			{pendingCancelChoiceQuestId}
			{copiedWp}
			{onPlaylistChange}
			{onQuestStart}
			{onQuestComplete}
			{onQuestCancel}
			{onToggleCancelChoice}
			{onCopyWaypoint}
			{formatMinutes}
			{getCooldownRemaining}
		/>
	{:else if activeTab === 'customise'}
		<CustomiseStatsWidget />
	{/if}
</section>
