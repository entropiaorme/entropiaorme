<script lang="ts">
	import { onMount } from 'svelte';
	import { Badge, Button, Card, DataTable, Input, Modal, SearchInput, SegmentedControl, Select, Tabs } from '$lib/components';
	import type {
		Quest,
		QuestPlaylist,
		QuestCreateData,
		QuestAnalyticsRow,
		PlaylistAnalyticsRow,
		PlaylistItemGroup
	} from '$lib/types';
	import type { CooldownStatus } from '$lib/types/common';
	import { formatPed, formatPercent } from '$lib/utils/format';
	import { getPreference } from '$lib/preferences';
	import { guideState, registerDemoApi, unregisterDemoApi } from '$lib/guide/state.svelte';
	import { closeGuide, openGuide } from '$lib/guide/engine';
	import { questsSurface } from '$lib/guide/surfaces/quests';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import { hydrate, subscribeTracking, trackingSnapshot } from '$lib/stores/trackingStore';
	import {
		questsDemoQuests,
		questsDemoPlaylists,
		questsDemoQuestAnalytics,
		questsDemoPlaylistAnalytics,
		questsDemoGlobalLiquidReturnRate,
		questsDemoGlobalSkillProgressionRate
	} from '$lib/guide/fixtures/quests';
	import {
		getQuests,
		getPlaylists,
		createQuest,
		updateQuest,
		deleteQuest,
		startQuest,
		completeQuest,
		cancelQuest,
		createPlaylist,
		updatePlaylist,
		deletePlaylist,
		getQuestAnalytics,
		getPlaylistAnalytics,
		getAnalyticsOverview
	} from '$lib/api';

	// ── State ──
	let quests: Quest[] = $state([]);
	let playlists: QuestPlaylist[] = $state([]);
	let loading = $state(true);
	let error: string | null = $state(null);

	// View toggle
	let view: 'quests' | 'playlists' | 'analytics' = $state('quests');

	// Quest view state
	let searchQuery = $state('');
	let selectedPlanet: string | null = $state(null);
	let selectedMob: string | null = $state(null);
	let collapsedCategories: Set<string> = $state(new Set());
	let categoriesInitialised = false;

	// Playlist view state
	let expandedPlaylistId: string | null = $state(null);

	// Quest modal
	let showQuestModal = $state(false);
	let editingQuest: Quest | null = $state(null);
	let questForm = $state(defaultQuestForm());
	let mobInput = $state('');

	// Playlist modal
	let showPlaylistModal = $state(false);
	let editingPlaylist: QuestPlaylist | null = $state(null);
	let playlistForm = $state(defaultPlaylistForm());

	// Delete confirmation
	let confirmDeleteQuest: Quest | null = $state(null);
	let confirmDeletePlaylist: QuestPlaylist | null = $state(null);

	// Three-dot menus
	let openMenuId: string | null = $state(null);
	let deleteConfirmId: string | null = $state(null);
	let pendingCancelChoiceQuestId: string | null = $state(null);

	// Cooldown tick
	let now = $state(Date.now());

	// Waypoint copy
	let copiedWp: string | null = $state(null);

	// Cooldown unit for the form
	let cooldownUnit: 'hours' | 'days' = $state('hours');
	let cooldownInput: number | null = $state(null);

	interface PlaylistFormItem {
		quest_id: string;
		description: string | null;
		group_type: PlaylistItemGroup;
	}

	function defaultQuestForm() {
		return {
			name: '',
			planet: 'Calypso',
			category: '',
			waypoint: '',
			cooldown_hours: null as number | null,
			reward_ped: null as number | null,
			reward_is_skill: false,
			expected_reward_markup_percent: null as number | null,
			reward_description: '',
			notes: '',
			chain_name: '',
			chain_position: null as number | null,
			chain_total: null as number | null,
			mobs: [] as string[]
		};
	}

	function defaultPlaylistForm() {
		return {
			name: '',
			planet: 'Calypso',
			estimated_minutes: 30,
			immediate_items: [] as PlaylistFormItem[],
			long_horizon_items: [] as PlaylistFormItem[]
		};
	}

	// ── Load data ──
	let trackingActive = $derived($trackingSnapshot?.status === 'active');

	// Guide
	let guideSeen = $state(true);
	function toggleSurfaceGuide(): void {
		if (guideState.isActive) {
			closeGuide();
		} else {
			guideSeen = true;
			void openGuide(questsSurface);
		}
	}

	onMount(() => {
		void (async () => {
			guideSeen = await getPreference<boolean>('guide_seen_quests', false);
		})();
		const stopClock = useVisiblePoll(() => { now = Date.now(); }, { intervalMs: 1000 });
		registerDemoApi('quests', {
			setView: (v: string) => {
				view = v as 'quests' | 'playlists' | 'analytics';
			},
			openNewQuestModal: () => {
				openNewQuest();
			},
			closeNewQuestModal: () => {
				showQuestModal = false;
				editingQuest = null;
			},
			closePlaylistModal: () => {
				showPlaylistModal = false;
				editingPlaylist = null;
			}
		});
		return () => {
			stopClock();
			unregisterDemoApi('quests');
		};
	});

	// Quest data refreshes every 10s while tracking is active (below) to pick up
	// chat.log mission-completion lines. The active/idle signal that gates it is
	// event-driven: hydrate the tracking snapshot once, then keep it current from
	// pushed session frames rather than polling for session start/stop.
	$effect(() => {
		if (guideState.isActive) return;
		let unlisten: (() => void) | undefined;
		let disposed = false;
		// Attach the listener BEFORE the first hydrate: a session frame landing
		// between the hydrate GET and the listener attaching would otherwise be
		// lost (the subscribe-then-hydrate ordering the scan and character views
		// follow).
		void subscribeTracking().then((fn) => {
			if (disposed) {
				fn();
				return;
			}
			unlisten = fn;
			void hydrate();
		});
		return () => {
			disposed = true;
			unlisten?.();
		};
	});

	$effect(() => {
		if (guideState.isActive) return;
		if (!trackingActive) return;
		const refreshQuests = async () => {
			try {
				const [q, p] = await Promise.all([getQuests(), getPlaylists()]);
				quests = q;
				playlists = p;
			} catch { /* ignore */ }
		};
		return useVisiblePoll(refreshQuests, { intervalMs: 10000, immediate: false });
	});

	async function loadData(guideMode: boolean) {
		loading = true;
		error = null;
		try {
			if (guideMode) {
				quests = questsDemoQuests.map((q) => ({ ...q }));
				playlists = questsDemoPlaylists.map((p) => ({ ...p }));
				analyticsData = questsDemoQuestAnalytics.map((a) => ({ ...a }));
				playlistAnalyticsData = questsDemoPlaylistAnalytics.map((a) => ({ ...a }));
				globalLiquidReturnRate = questsDemoGlobalLiquidReturnRate;
				globalSkillProgressionRate = questsDemoGlobalSkillProgressionRate;
				analyticsLoaded = true;
				analyticsError = null;
				if (!categoriesInitialised) {
					const cats = new Set<string>();
					for (const quest of quests) {
						if (quest.category) cats.add(quest.category);
					}
					collapsedCategories = cats;
					categoriesInitialised = true;
				}
				return;
			}
			const [q, p] = await Promise.all([getQuests(), getPlaylists()]);
			quests = q;
			playlists = p;
			// Start with all categories collapsed
			if (!categoriesInitialised) {
				const cats = new Set<string>();
				for (const quest of q) {
					if (quest.category) cats.add(quest.category);
				}
				collapsedCategories = cats;
				categoriesInitialised = true;
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load quests';
		} finally {
			loading = false;
		}
	}

	// Reload data on initial mount and whenever guide-mode toggles.
	$effect(() => {
		void loadData(guideState.isActive);
	});

	// ── Analytics ──
	let analyticsData: QuestAnalyticsRow[] = $state([]);
	let playlistAnalyticsData: PlaylistAnalyticsRow[] = $state([]);
	let analyticsLoading = $state(false);
	let analyticsError: string | null = $state(null);
	let analyticsLoaded = false;
	// Liquid PED returns per cycled PED — drives Net/Rate forecasts and the
	// "PES burn-saved" translation. Skill TT, codex PES, and quest PES are
	// progression and stay out of this rate.
	let globalLiquidReturnRate = $state(0);
	// Combined PES throughput per cycled PED (skill TT + codex PES + quest PES).
	// Used to translate a PES reward into the equivalent cycle it would replace.
	let globalSkillProgressionRate = $state(0);
	let analyticsRewardMode = $state<'tt' | 'markup'>('tt');

	$effect(() => {
		if (guideState.isActive) return;
		if (view === 'analytics' && !analyticsLoaded && !analyticsLoading) {
			loadAnalytics();
		}
	});

	async function loadAnalytics() {
		analyticsLoading = true;
		analyticsError = null;
		try {
			const [qAnalytics, plAnalytics, overview] = await Promise.all([
				getQuestAnalytics(),
				getPlaylistAnalytics(),
				getAnalyticsOverview('all')
			]);
			analyticsData = qAnalytics;
			playlistAnalyticsData = plAnalytics;
			// Liquid PED returns: TT loot plus liquid ledger gains (convert).
			// Quest reward markup is per-quest in `total_expected_reward_ped`
			// so it's intentionally not folded into the global rate here.
			const convertGains = overview.returnsBreakdown.ledger.convert ?? 0;
			const liquidReturns = overview.returnsBreakdown.lootTt + convertGains;
			// PES throughput across all progression channels.
			const skillProgressionReturns = overview.returnsBreakdown.pes
				+ (overview.returnsBreakdown.codexPes ?? 0)
				+ (overview.returnsBreakdown.questPes ?? 0);
			const rawCycled = overview.lossesBreakdown.trackingCost;
			globalLiquidReturnRate = rawCycled > 0 ? liquidReturns / rawCycled : 0;
			globalSkillProgressionRate = rawCycled > 0 ? skillProgressionReturns / rawCycled : 0;
			analyticsLoaded = true;
		} catch (e) {
			analyticsError = e instanceof Error ? e.message : 'Failed to load quest analytics';
		} finally {
			analyticsLoading = false;
		}
	}

	interface QuestAnalyticsComputed {
		questId: string;
		questName: string;
		planet: string;
		category: string | null;
		rewardPed: number;
		rewardIsSkill: boolean;
		expectedRewardMarkupPercent: number | null;
		linkedSessions: number;
		totalCycled: number;
		avgRawReturns: number;
		avgCycled: number;
		// Liquid PED reward shown in the Reward column. Toggle-aware: TT mode
		// = face value, Markup mode = with expected markup applied. 0 for
		// skill quests since they have no liquid contribution.
		displayLiquidReward: number;
		// PES face value of the reward, invariant to toggle. 0 for liquid quests.
		avgRewardPes: number;
		rewardMarkupPercent: number | null;
		// Liquid Net for the run: liquid cycle returns + liquid reward − cycled.
		avgNet: number;
		// Cycle PES baseline + explicit PES reward. Always at face value.
		avgPesNet: number;
		returnRate: number;
	}

	let computedAnalytics: QuestAnalyticsComputed[] = $derived.by(() => {
		return analyticsData.map((row) => {
			const totalCycled = row.totalWeaponCost + row.totalHealCost + row.totalEnhancerCost + row.totalArmourCost;
			const totalReward = row.rewardPed * row.linkedSessions;
			const sessions = row.linkedSessions || 1;
			const avgCycled = totalCycled / sessions;
			// Liquid reward: face value or with markup, depending on toggle.
			// Skill quests contribute 0 to the liquid side regardless.
			const avgRewardLiquidFace = row.rewardIsSkill ? 0 : totalReward / sessions;
			const avgRewardLiquidMarkup = row.rewardIsSkill
				? 0
				: row.totalExpectedRewardPed / sessions;
			const displayLiquidReward = analyticsRewardMode === 'markup'
				? avgRewardLiquidMarkup
				: avgRewardLiquidFace;
			// PES reward stays at face value across both modes.
			const avgRewardPes = row.rewardIsSkill ? row.rewardPed : 0;
			// Liquid cycle projection (PES sources excluded — denomination-pure).
			const avgRawReturns = avgCycled * globalLiquidReturnRate;
			const avgNet = avgRawReturns + displayLiquidReward - avgCycled;
			const returnRate = avgCycled > 0
				? (avgRawReturns + displayLiquidReward) / avgCycled
				: 0;
			// PES cycle baseline + explicit PES reward (face value).
			const avgPesNet = avgCycled * globalSkillProgressionRate + avgRewardPes;
			return {
				questId: row.questId,
				questName: row.questName,
				planet: row.planet,
				category: row.category,
				rewardPed: row.rewardPed,
				rewardIsSkill: row.rewardIsSkill,
				expectedRewardMarkupPercent: row.expectedRewardMarkupPercent,
				linkedSessions: row.linkedSessions,
				totalCycled,
				displayLiquidReward,
				avgRewardPes,
				rewardMarkupPercent: row.expectedRewardMarkupPercent,
				avgRawReturns,
				avgCycled,
				avgNet,
				avgPesNet,
				returnRate,
			};
		});
	});

	let analyticsSortKey = $state<(keyof QuestAnalyticsComputed & string) | undefined>(undefined);
	let analyticsSortDir = $state<'asc' | 'desc'>('asc');

	let sortedAnalytics = $derived.by(() => {
		if (!analyticsSortKey) return computedAnalytics;
		const key = analyticsSortKey;
		return [...computedAnalytics].sort((a, b) => {
			const aVal = a[key];
			const bVal = b[key];
			if (typeof aVal === 'number' && typeof bVal === 'number') {
				return analyticsSortDir === 'asc' ? aVal - bVal : bVal - aVal;
			}
			return analyticsSortDir === 'asc'
				? String(aVal).localeCompare(String(bVal))
				: String(bVal).localeCompare(String(aVal));
		});
	});

	const analyticsColumns = $derived.by((): {
		key: keyof QuestAnalyticsComputed & string;
		label: string;
		align?: 'left' | 'right' | 'center';
		sortable?: boolean;
	}[] => {
		const columns: {
			key: keyof QuestAnalyticsComputed & string;
			label: string;
			align?: 'left' | 'right' | 'center';
			sortable?: boolean;
		}[] = [
			{ key: 'questName', label: 'Quest', sortable: true },
			{ key: 'linkedSessions', label: 'Sessions', align: 'right', sortable: true },
			{ key: 'displayLiquidReward', label: 'Reward', align: 'right', sortable: true },
			{ key: 'avgCycled', label: 'Avg Cycled', align: 'right', sortable: true },
		];
		if (analyticsRewardMode === 'markup') {
			columns.push({ key: 'rewardMarkupPercent', label: 'Markup', align: 'right', sortable: true });
		}
		columns.push(
			{ key: 'avgNet', label: 'Avg Net', align: 'right', sortable: true },
			{ key: 'returnRate', label: 'Rate', align: 'right', sortable: true },
		);
		return columns;
	});

	// ── Playlist analytics computed ──
	interface PlaylistAnalyticsComputed {
		playlistName: string;
		questCount: number;
		longHorizonQuestCount: number;
		// Toggle-aware liquid display (face value or markup-applied).
		displayImmediateReward: number;
		displayBonusReward: number;
		// PES face-value sub-line totals, invariant to toggle.
		avgImmediateSkillReward: number;
		avgBonusSkillReward: number;
		rewardMarkupPercent: number | null;
		avgCycled: number;
		avgRawReturns: number;
		// Liquid Net + cycle-PES Net (face value).
		avgNet: number;
		avgPesNet: number;
		returnRate: number;
	}

	let computedPlaylistAnalytics: PlaylistAnalyticsComputed[] = $derived.by(() => {
		return playlistAnalyticsData.map((row) => {
			const totalCycled = row.totalWeaponCost + row.totalHealCost + row.totalEnhancerCost + row.totalArmourCost;
			const sessions = row.matchedSessions || 1;
			const avgImmediateReward = row.totalImmediateRewardPed / sessions;
			const avgBonusReward = row.totalBonusRewardPed / sessions;
			const avgImmediateSkillReward = row.totalImmediatePesReward / sessions;
			const avgBonusSkillReward = row.totalBonusPesReward / sessions;
			// Liquid portions (face value).
			const avgImmediateLiquidFace = avgImmediateReward - avgImmediateSkillReward;
			const avgBonusLiquidFace = avgBonusReward - avgBonusSkillReward;
			// Liquid portions with expected markup applied. Backend already
			// emits face value for skill quests in the expected totals, so
			// subtracting the PES sum yields the liquid-with-markup amount.
			const avgImmediateLiquidMarkup = (row.totalExpectedImmediateRewardPed
				- row.totalImmediatePesReward) / sessions;
			const avgBonusLiquidMarkup = (row.totalExpectedBonusRewardPed
				- row.totalBonusPesReward) / sessions;
			const displayImmediateReward = analyticsRewardMode === 'markup'
				? avgImmediateLiquidMarkup
				: avgImmediateLiquidFace;
			const displayBonusReward = analyticsRewardMode === 'markup'
				? avgBonusLiquidMarkup
				: avgBonusLiquidFace;
			const liquidFaceTotal = avgImmediateLiquidFace + avgBonusLiquidFace;
			const liquidMarkupTotal = avgImmediateLiquidMarkup + avgBonusLiquidMarkup;
			const rewardMarkupPercentValue = liquidFaceTotal > 0
				? (liquidMarkupTotal / liquidFaceTotal) * 100
				: null;
			const avgCycled = totalCycled / sessions;
			const avgRawReturns = avgCycled * globalLiquidReturnRate;
			const avgNet = avgRawReturns + displayImmediateReward + displayBonusReward - avgCycled;
			const returnRate = avgCycled > 0
				? (avgRawReturns + displayImmediateReward + displayBonusReward) / avgCycled
				: 0;
			const avgPesNet = avgCycled * globalSkillProgressionRate
				+ avgImmediateSkillReward + avgBonusSkillReward;
			return {
				playlistName: row.playlistName,
				questCount: row.questCount,
				longHorizonQuestCount: row.longHorizonQuestCount,
				displayImmediateReward,
				displayBonusReward,
				avgImmediateSkillReward,
				avgBonusSkillReward,
				rewardMarkupPercent: rewardMarkupPercentValue,
				avgCycled,
				avgRawReturns,
				avgNet,
				avgPesNet,
				returnRate,
			};
		});
	});

	const playlistAnalyticsColumns = $derived.by((): {
		key: keyof PlaylistAnalyticsComputed & string;
		label: string;
		align?: 'left' | 'right' | 'center';
		sortable?: boolean;
	}[] => {
		const columns: {
			key: keyof PlaylistAnalyticsComputed & string;
			label: string;
			align?: 'left' | 'right' | 'center';
			sortable?: boolean;
		}[] = [
			{ key: 'playlistName', label: 'Playlist', sortable: true },
			{ key: 'displayImmediateReward', label: 'Base Reward', align: 'right', sortable: true },
			{ key: 'displayBonusReward', label: 'Bonus/Run', align: 'right', sortable: true },
			{ key: 'avgCycled', label: 'Avg Cycled', align: 'right', sortable: true },
		];
		if (analyticsRewardMode === 'markup') {
			columns.push({ key: 'rewardMarkupPercent', label: 'Markup', align: 'right', sortable: true });
		}
		columns.push(
			{ key: 'avgNet', label: 'Avg Net', align: 'right', sortable: true },
			{ key: 'returnRate', label: 'Rate', align: 'right', sortable: true },
		);
		return columns;
	});

	// ── Cooldown helpers ──
	function getCooldownStatus(quest: Quest): CooldownStatus {
		if (!quest.cooldownDurationHours) return 'no_cooldown';
		if (!quest.cooldownExpiresAt) return 'ready';
		return now >= new Date(quest.cooldownExpiresAt).getTime() ? 'ready' : 'cooling';
	}

	function getCooldownRemaining(quest: Quest): string | null {
		if (!quest.cooldownExpiresAt) return null;
		const remainMs = new Date(quest.cooldownExpiresAt).getTime() - now;
		if (remainMs <= 0) return null;
		const totalSec = Math.floor(remainMs / 1000);
		const d = Math.floor(totalSec / 86400);
		const h = Math.floor((totalSec % 86400) / 3600);
		const m = Math.floor((totalSec % 3600) / 60);
		const s = totalSec % 60;
		if (d > 0) return `${d}d ${h}h`;
		if (h > 0) return `${h}h ${m.toString().padStart(2, '0')}m`;
		return `${m}m ${s.toString().padStart(2, '0')}s`;
	}

	function formatCooldownHours(h: number): string {
		if (h >= 24 && h % 24 === 0) return `${h / 24}d`;
		return `${h}h`;
	}

	// ── Computed: planets from data ──
	let planets = $derived([...new Set(quests.map((q) => q.planet))].sort());

	// ── Computed: mobs available on current planet filter ──
	let planetQuests = $derived(selectedPlanet ? quests.filter((q) => q.planet === selectedPlanet) : quests);
	let mobs = $derived(
		[...new Set(planetQuests.flatMap((q) => q.targetMobs))].sort()
	);

	// ── Computed: quest filtering (planet + mob + search) ──
	let filteredQuests = $derived.by(() => {
		let result = planetQuests;
		if (selectedMob) {
			result = result.filter((q) => q.targetMobs.includes(selectedMob!));
		}
		if (searchQuery) {
			const s = searchQuery.toLowerCase();
			result = result.filter(
				(q) =>
					q.name.toLowerCase().includes(s) ||
					q.targetMobs.some((m) => m.toLowerCase().includes(s)) ||
					q.planet.toLowerCase().includes(s) ||
					(q.category?.toLowerCase().includes(s) ?? false)
			);
		}
		return result;
	});

	let questsByCategory = $derived.by(() => {
		const groups: { category: string; quests: Quest[] }[] = [];
		const catMap = new Map<string, Quest[]>();
		const uncategorised: Quest[] = [];

		for (const q of filteredQuests) {
			if (q.category) {
				if (!catMap.has(q.category)) catMap.set(q.category, []);
				catMap.get(q.category)!.push(q);
			} else {
				uncategorised.push(q);
			}
		}

		if (uncategorised.length > 0) {
			groups.push({ category: '', quests: uncategorised });
		}
		for (const [cat, qs] of catMap) {
			groups.push({ category: cat, quests: qs });
		}
		return groups;
	});

	// Category status summary
	function categoryStatusCounts(qs: Quest[]): { ready: number; started: number; cooling: number } {
		let ready = 0, started = 0, cooling = 0;
		for (const q of qs) {
			if (q.startedAt) started++;
			else if (getCooldownStatus(q) === 'cooling') cooling++;
			else ready++;
		}
		return { ready, started, cooling };
	}

	// ── Computed: playlist with quest data ──
	function playlistQuestItems(pl: QuestPlaylist, groupType?: PlaylistItemGroup) {
		return pl.items
			.filter((item) => !groupType || item.groupType === groupType)
			.map((item) => {
				const quest = quests.find((q) => q.id === item.questId);
				return quest ? { quest, description: item.description, groupType: item.groupType } : null;
			})
			.filter((x): x is { quest: Quest; description: string | null; groupType: PlaylistItemGroup } => x !== null);
	}

	function playlistAllReady(pl: QuestPlaylist): boolean {
		const immediateItems = playlistQuestItems(pl, 'immediate');
		if (immediateItems.length === 0) return false;
		return immediateItems.every((item) => {
			const s = getCooldownStatus(item.quest);
			return s === 'ready' || s === 'no_cooldown';
		});
	}

	function formatMinutes(m: number): string {
		if (m < 60) return `${m}m`;
		const h = Math.floor(m / 60);
		const rem = m % 60;
		return rem > 0 ? `${h}h ${rem}m` : `${h}h`;
	}

	// ── Quest actions ──
	async function handleStart(questId: string) {
		try {
			const updated = await startQuest(questId);
			quests = quests.map((q) => (q.id === updated.id ? updated : q));
			if (pendingCancelChoiceQuestId === questId) pendingCancelChoiceQuestId = null;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to start quest';
		}
	}

	async function handleComplete(questId: string) {
		try {
			const updated = await completeQuest(questId);
			quests = quests.map((q) => (q.id === updated.id ? updated : q));
			if (pendingCancelChoiceQuestId === questId) pendingCancelChoiceQuestId = null;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to complete quest';
		}
	}

	async function handleCancel(questId: string, undoReward = false) {
		try {
			const updated = await cancelQuest(questId, undoReward);
			quests = quests.map((q) => (q.id === updated.id ? updated : q));
			if (pendingCancelChoiceQuestId === questId) pendingCancelChoiceQuestId = null;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to cancel quest';
		}
	}

	function toggleCancelChoice(questId: string) {
		pendingCancelChoiceQuestId = pendingCancelChoiceQuestId === questId ? null : questId;
	}

	function copyWaypoint(questId: string, waypoint: string) {
		navigator.clipboard.writeText(waypoint);
		copiedWp = questId;
		setTimeout(() => { if (copiedWp === questId) copiedWp = null; }, 1500);
	}

	// ── Quest CRUD ──
	function openNewQuest() {
		editingQuest = null;
		questForm = defaultQuestForm();
		cooldownUnit = 'hours';
		cooldownInput = null;
		mobInput = '';
		showQuestModal = true;
	}

	function openEditQuest(quest: Quest) {
		editingQuest = quest;
		const h = quest.cooldownDurationHours;
		if (h != null && h >= 24 && h % 24 === 0) {
			cooldownUnit = 'days';
			cooldownInput = h / 24;
		} else {
			cooldownUnit = 'hours';
			cooldownInput = h;
		}
		questForm = {
			name: quest.name,
			planet: quest.planet,
			category: quest.category ?? '',
			waypoint: quest.waypoint ?? '',
			cooldown_hours: h,
			reward_ped: quest.reward,
			reward_is_skill: quest.rewardIsSkill,
			expected_reward_markup_percent: quest.expectedRewardMarkupPercent,
			reward_description: quest.rewardDescription,
			notes: quest.notes,
			chain_name: quest.chainName ?? '',
			chain_position: quest.chainPosition,
			chain_total: quest.chainTotal,
			mobs: [...quest.targetMobs]
		};
		mobInput = '';
		openMenuId = null;
		showQuestModal = true;
	}

	async function saveQuest() {
		const cdHours = cooldownInput != null
			? (cooldownUnit === 'days' ? cooldownInput * 24 : cooldownInput)
			: null;
		const data: QuestCreateData = {
			name: questForm.name,
			planet: questForm.planet,
			category: questForm.category || null,
			waypoint: questForm.waypoint || null,
			cooldown_hours: cdHours,
			reward_ped: questForm.reward_ped,
			reward_is_skill: questForm.reward_is_skill,
			expected_reward_markup_percent: (!questForm.reward_is_skill && (questForm.reward_ped ?? 0) > 0)
				? questForm.expected_reward_markup_percent
				: null,
			reward_description: questForm.reward_description || null,
			notes: questForm.notes || null,
			chain_name: questForm.chain_name || null,
			chain_position: questForm.chain_position,
			chain_total: questForm.chain_total,
			mobs: questForm.mobs
		};
		try {
			if (editingQuest) {
				const updated = await updateQuest(editingQuest.id, data);
				quests = quests.map((q) => (q.id === updated.id ? updated : q));
			} else {
				const created = await createQuest(data);
				quests = [...quests, created];
			}
			showQuestModal = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to save quest';
		}
	}

	async function handleDeleteQuest(questId: string) {
		try {
			await deleteQuest(questId);
			quests = quests.filter((q) => q.id !== questId);
			deleteConfirmId = null;
			openMenuId = null;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete quest';
		}
	}

	function addMob() {
		const mob = mobInput.trim();
		if (mob && !questForm.mobs.includes(mob)) {
			questForm.mobs = [...questForm.mobs, mob];
		}
		mobInput = '';
	}

	function removeMob(mob: string) {
		questForm.mobs = questForm.mobs.filter((m) => m !== mob);
	}

	function rewardMarkupInputDisabled() {
		return (questForm.reward_ped ?? 0) <= 0;
	}

	// ── Playlist CRUD ──
	function openNewPlaylist() {
		editingPlaylist = null;
		playlistForm = defaultPlaylistForm();
		showPlaylistModal = true;
	}

	function openEditPlaylist(playlist: QuestPlaylist) {
		editingPlaylist = playlist;
		playlistForm = {
			name: playlist.name,
			planet: playlist.planet,
			estimated_minutes: playlist.estimatedMinutes,
			immediate_items: playlist.items
				.filter((item) => item.groupType === 'immediate')
				.map((item) => ({
					quest_id: item.questId,
					description: item.description,
					group_type: 'immediate' as const,
				})),
			long_horizon_items: playlist.items
				.filter((item) => item.groupType === 'long_horizon')
				.map((item) => ({
					quest_id: item.questId,
					description: item.description,
					group_type: 'long_horizon' as const,
				})),
		};
		openMenuId = null;
		showPlaylistModal = true;
	}

	async function savePlaylist() {
		const data = {
			name: playlistForm.name,
			planet: playlistForm.planet,
			estimated_minutes: playlistForm.estimated_minutes,
			items: [
				...playlistForm.immediate_items.map((item) => ({
					quest_id: parseInt(item.quest_id),
					description: item.description,
					group_type: 'immediate' as const
				})),
				...playlistForm.long_horizon_items.map((item) => ({
					quest_id: parseInt(item.quest_id),
					description: item.description,
					group_type: 'long_horizon' as const
				}))
			]
		};
		try {
			if (editingPlaylist) {
				const updated = await updatePlaylist(editingPlaylist.id, data);
				playlists = playlists.map((p) => (p.id === updated.id ? updated : p));
			} else {
				const created = await createPlaylist(data);
				playlists = [...playlists, created];
			}
			showPlaylistModal = false;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to save playlist';
		}
	}

	async function handleDeletePlaylist(playlistId: string) {
		try {
			await deletePlaylist(playlistId);
			playlists = playlists.filter((p) => p.id !== playlistId);
			deleteConfirmId = null;
			openMenuId = null;
			if (expandedPlaylistId === playlistId) expandedPlaylistId = null;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to delete playlist';
		}
	}

	let availableForPlaylist = $derived(
		quests.filter((q) => !playlistForm.immediate_items.some((item) => item.quest_id === q.id)
			&& !playlistForm.long_horizon_items.some((item) => item.quest_id === q.id))
	);

	function questName(id: string): string {
		return quests.find((q) => q.id === id)?.name ?? `Quest #${id}`;
	}

	function moveQuestUp(groupType: PlaylistItemGroup, index: number) {
		if (index === 0) return;
		const items = groupType === 'immediate'
			? [...playlistForm.immediate_items]
			: [...playlistForm.long_horizon_items];
		[items[index - 1], items[index]] = [items[index], items[index - 1]];
		if (groupType === 'immediate') playlistForm.immediate_items = items;
		else playlistForm.long_horizon_items = items;
	}

	function moveQuestDown(groupType: PlaylistItemGroup, index: number) {
		const items = groupType === 'immediate'
			? [...playlistForm.immediate_items]
			: [...playlistForm.long_horizon_items];
		if (index >= items.length - 1) return;
		[items[index], items[index + 1]] = [items[index + 1], items[index]];
		if (groupType === 'immediate') playlistForm.immediate_items = items;
		else playlistForm.long_horizon_items = items;
	}

	function addQuestToPlaylist(questId: string, groupType: PlaylistItemGroup) {
		const item = { quest_id: questId, description: null, group_type: groupType };
		if (groupType === 'immediate') {
			playlistForm.immediate_items = [...playlistForm.immediate_items, item];
		} else {
			playlistForm.long_horizon_items = [...playlistForm.long_horizon_items, item];
		}
	}

	function removeQuestFromPlaylist(questId: string, groupType: PlaylistItemGroup) {
		if (groupType === 'immediate') {
			playlistForm.immediate_items = playlistForm.immediate_items.filter((item) => item.quest_id !== questId);
		} else {
			playlistForm.long_horizon_items = playlistForm.long_horizon_items.filter((item) => item.quest_id !== questId);
		}
	}

	function moveQuestBetweenGroups(questId: string, sourceGroup: PlaylistItemGroup) {
		const targetGroup = sourceGroup === 'immediate' ? 'long_horizon' : 'immediate';
		const sourceItems = sourceGroup === 'immediate' ? playlistForm.immediate_items : playlistForm.long_horizon_items;
		const item = sourceItems.find((entry) => entry.quest_id === questId);
		if (!item) return;
		if (sourceGroup === 'immediate') {
			playlistForm.immediate_items = playlistForm.immediate_items.filter((entry) => entry.quest_id !== questId);
			playlistForm.long_horizon_items = [
				...playlistForm.long_horizon_items,
				{ ...item, group_type: targetGroup }
			];
		} else {
			playlistForm.long_horizon_items = playlistForm.long_horizon_items.filter((entry) => entry.quest_id !== questId);
			playlistForm.immediate_items = [
				...playlistForm.immediate_items,
				{ ...item, group_type: targetGroup }
			];
		}
	}

	// Close menus on outside click
	function handleWindowClick() {
		if (openMenuId) openMenuId = null;
	}
</script>

<svelte:window onclick={handleWindowClick} />

<div class="px-6 pb-6 space-y-4">
	<!-- Header -->
	<div class="flex items-center justify-between">
		<header class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold text-text tracking-tight">Quests</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="text-sm text-text-secondary mt-0.5">Track missions, manage cooldowns, build hunt playlists</p>
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
			<Button size="sm" variant="secondary" onclick={openNewQuest}>
				{#snippet children()}+ Quest{/snippet}
			</Button>
			<Button size="sm" variant="secondary" onclick={openNewPlaylist}>
				{#snippet children()}+ Playlist{/snippet}
			</Button>
		</div>
	</div>

	{#if error}
		<div class="text-sm text-negative bg-negative/10 rounded-md px-3 py-2">{error}</div>
	{/if}

	<!-- Main tab toggle -->
	<Tabs
		tabs={[
			{ id: 'quests', label: 'Quests' },
			{ id: 'playlists', label: 'Playlists' },
			{ id: 'analytics', label: 'Analytics' }
		]}
		active={view}
		onchange={(id) => (view = id as 'quests' | 'playlists' | 'analytics')}
	/>

	{#if loading}
		<div class="text-sm text-text-tertiary py-8 text-center">Loading quests...</div>

	{:else if view === 'quests'}
		<!-- ═══ QUEST VIEW ═══ -->

		<!-- Filters row -->
		<div class="flex flex-wrap items-center gap-4">
			<!-- Planet filter -->
			{#if planets.length > 1}
				<div class="flex items-center gap-2">
					<label for="planet-select" class="text-xs text-text-tertiary uppercase tracking-wide">Planet</label>
					<Select
						id="planet-select"
						class="min-w-[120px]"
						bind:value={selectedPlanet}
						onchange={() => (selectedMob = null)}
					>
						<option value={null}>All Planets</option>
						{#each planets as planet}
							<option value={planet}>{planet}</option>
						{/each}
					</Select>
				</div>
			{/if}

			<!-- Mob filter -->
			{#if mobs.length > 0}
				<div class="flex items-center gap-2">
					<label for="mob-select" class="text-xs text-text-tertiary uppercase tracking-wide">Mob</label>
					<Select
						id="mob-select"
						class="min-w-[120px]"
						bind:value={selectedMob}
					>
						<option value={null}>All Mobs</option>
						{#each mobs as mob}
							<option value={mob}>{mob}</option>
						{/each}
					</Select>
				</div>
			{/if}

			<!-- Search -->
			<div class="flex-1 min-w-[200px]">
				<SearchInput bind:value={searchQuery} placeholder="Search by name, mob, category..." />
			</div>
		</div>

		{#if filteredQuests.length === 0}
			<div class="text-center py-8 text-sm text-text-tertiary">
				{searchQuery ? `No quests match "${searchQuery}"` : 'No quests yet. Add your first quest to get started.'}
			</div>
		{:else}
			<div class="space-y-4">
				{#each questsByCategory as group (group.category)}
					{@const isCollapsed = collapsedCategories.has(group.category)}
					{@const counts = categoryStatusCounts(group.quests)}

					{#if group.category}
						<!-- Category section -->
						<div class="rounded-lg border border-border/50 overflow-hidden">
							<!-- Category header -->
							<button
								class="w-full flex items-center gap-2.5 py-2.5 px-4 text-left cursor-pointer
									bg-surface-raised/60 hover:bg-surface-raised/80 transition-colors"
								onclick={() => {
									const next = new Set(collapsedCategories);
									if (isCollapsed) next.delete(group.category);
									else next.add(group.category);
									collapsedCategories = next;
								}}
							>
								<svg
									class="w-3.5 h-3.5 text-text-tertiary transition-transform shrink-0 {isCollapsed ? '-rotate-90' : ''}"
									fill="none" stroke="currentColor" viewBox="0 0 24 24"
								>
									<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
								</svg>
								<span class="text-sm font-semibold text-text">{group.category}</span>
								<span class="text-xs text-text-tertiary">{group.quests.length}</span>
								<div class="flex items-center gap-1.5 ml-auto">
									{#if counts.ready > 0}
										<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-success/10 text-success border border-success/20">
											{counts.ready} ready
										</span>
									{/if}
									{#if counts.started > 0}
										<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-accent/10 text-accent border border-accent/20">
											{counts.started} started
										</span>
									{/if}
									{#if counts.cooling > 0}
										<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-warning/10 text-warning border border-warning/20">
											{counts.cooling} on cd
										</span>
									{/if}
								</div>
							</button>

							<!-- Category quests -->
							{#if !isCollapsed}
								<div class="space-y-1.5 p-2">
									{#each group.quests as quest (quest.id)}
										{@const status = getCooldownStatus(quest)}
										{@const remaining = getCooldownRemaining(quest)}
										{@render questRow(quest, status, remaining)}
									{/each}
								</div>
							{/if}
						</div>
					{:else}
						<!-- Uncategorised quests (no wrapper) -->
						<div class="space-y-1.5">
							{#each group.quests as quest (quest.id)}
								{@const status = getCooldownStatus(quest)}
								{@const remaining = getCooldownRemaining(quest)}
								{@render questRow(quest, status, remaining)}
							{/each}
						</div>
					{/if}
				{/each}
			</div>
		{/if}

	{:else if view === 'playlists'}
		<!-- ═══ PLAYLIST VIEW ═══ -->
		<div data-guide-anchor="quests-playlists-view">
		{#if playlists.length === 0}
			<div class="text-center py-8 text-sm text-text-tertiary">
				No playlists yet. Create one to organise your quest rotation.
			</div>
		{:else}
			<div class="space-y-2">
				{#each playlists as pl (pl.id)}
					{@const isExpanded = expandedPlaylistId === pl.id}
					{@const allReady = playlistAllReady(pl)}
					{@const immediateItems = playlistQuestItems(pl, 'immediate')}
					{@const longHorizonItems = playlistQuestItems(pl, 'long_horizon')}
					<div class="bg-surface-raised/50 rounded-lg border border-border/50 hover:bg-surface-raised/70 transition-colors">
						<!-- Playlist header -->
						<div class="flex items-center px-4 py-3">
							<button
								class="flex-1 flex items-center gap-2.5 text-left cursor-pointer min-w-0"
								onclick={() => (expandedPlaylistId = isExpanded ? null : pl.id)}
							>
								<!-- Time badge -->
								<span class="text-[10px] font-medium px-1.5 py-0.5 rounded-full border shrink-0
									{pl.estimatedMinutes <= 10 ? 'bg-success/10 text-success border-success/20' :
									 pl.estimatedMinutes <= 30 ? 'bg-warning/10 text-warning border-warning/20' :
									 'bg-negative/10 text-negative border-negative/20'}">
									{formatMinutes(pl.estimatedMinutes)}
								</span>
								<span class="text-sm font-medium text-text truncate">{pl.name}</span>
								<span class="text-xs text-text-tertiary shrink-0">{immediateItems.length} immediate</span>
								{#if longHorizonItems.length > 0}
									<span class="text-xs text-text-tertiary shrink-0">+ {longHorizonItems.length} long</span>
								{/if}
								{#if allReady && immediateItems.length > 0}
									<Badge variant="positive">{#snippet children()}Ready{/snippet}</Badge>
								{/if}
								<svg
									class="w-3.5 h-3.5 text-text-tertiary transition-transform ml-auto shrink-0 {isExpanded ? 'rotate-180' : ''}"
									fill="none" stroke="currentColor" viewBox="0 0 24 24"
								>
									<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
								</svg>
							</button>
							<!-- Three-dot menu -->
							<div class="relative ml-2 shrink-0">
								<button
									aria-label="Playlist actions"
									class="w-7 h-7 flex items-center justify-center rounded-md border border-border/50
										text-text-tertiary hover:text-text hover:bg-surface-hover transition-colors cursor-pointer"
									onclick={(e) => { e.stopPropagation(); openMenuId = openMenuId === `pl-${pl.id}` ? null : `pl-${pl.id}`; }}
								>
									<svg class="w-3.5 h-3.5" fill="currentColor" viewBox="0 0 16 16">
										<circle cx="8" cy="3" r="1.5"/><circle cx="8" cy="8" r="1.5"/><circle cx="8" cy="13" r="1.5"/>
									</svg>
								</button>
								{#if openMenuId === `pl-${pl.id}`}
									<!-- svelte-ignore a11y_no_static_element_interactions -->
									<!-- svelte-ignore a11y_click_events_have_key_events -->
									<div class="absolute right-0 top-8 z-20 bg-surface-raised border border-border rounded-md shadow-lg py-1 min-w-[100px]"
										onclick={(e) => e.stopPropagation()}>
										<button class="w-full px-3 py-1.5 text-xs text-left text-text-secondary hover:bg-surface-hover hover:text-text cursor-pointer"
											onclick={() => openEditPlaylist(pl)}>Edit</button>
										{#if deleteConfirmId === `pl-${pl.id}`}
											<div class="flex gap-1 px-2 py-1">
												<Button class="flex-1" size="sm" variant="danger" onclick={() => handleDeletePlaylist(pl.id)}>
													{#snippet children()}Confirm{/snippet}
												</Button>
												<Button class="flex-1" size="sm" variant="ghost" onclick={() => (deleteConfirmId = null)}>
													{#snippet children()}Cancel{/snippet}
												</Button>
											</div>
										{:else}
											<button class="w-full px-3 py-1.5 text-xs text-left text-text-secondary hover:bg-surface-hover hover:text-negative cursor-pointer"
												onclick={() => (deleteConfirmId = `pl-${pl.id}`)}>Delete</button>
										{/if}
									</div>
								{/if}
							</div>
						</div>

						<!-- Expanded playlist items -->
						{#if isExpanded}
							<div class="border-t border-border/50 px-3 pb-3 pt-2 space-y-1.5">
								{#if immediateItems.length > 0}
									<div class="space-y-1.5">
										<div class="px-1 pt-1 eyebrow">Immediate Quests</div>
										{#each immediateItems as item (item.quest.id)}
											{@const status = getCooldownStatus(item.quest)}
											{@const remaining = getCooldownRemaining(item.quest)}
											{#if item.description}
												<div class="text-xs text-text-secondary ml-7 px-1 pt-1">{item.description}</div>
											{/if}
											<div class="flex items-center gap-2.5 bg-surface/50 rounded-md px-3 py-2">
												<span class="text-xs text-text-tertiary font-mono w-4 text-right shrink-0">{@html '&bull;'}</span>
												<div class="shrink-0">
													{#if item.quest.startedAt}
														<div class="w-2 h-2 rounded-full bg-accent animate-pulse"></div>
													{:else if status === 'ready' || status === 'no_cooldown'}
														<div class="w-2 h-2 rounded-full bg-success"></div>
													{:else}
														<div class="w-2 h-2 rounded-full bg-text-tertiary"></div>
													{/if}
												</div>
												<span class="text-sm text-text truncate flex-1">{item.quest.name}</span>
												{#if item.quest.waypoint}
													<button
														class="text-[10px] text-accent hover:text-accent/80 transition-colors cursor-pointer shrink-0"
														onclick={() => copyWaypoint(item.quest.id, item.quest.waypoint!)}
													>{copiedWp === item.quest.id ? 'Copied!' : 'WP'}</button>
												{/if}
												<div class="shrink-0 flex items-center gap-1">
													{#if status === 'cooling' && remaining}
														<span class="text-xs text-warning tabular-nums font-mono">{remaining}</span>
														{#if pendingCancelChoiceQuestId === item.quest.id}
															<Button size="sm" variant="secondary" onclick={() => handleCancel(item.quest.id, false)}>
																{#snippet children()}Keep Reward{/snippet}
															</Button>
															<Button size="sm" variant="danger" onclick={() => handleCancel(item.quest.id, true)}>
																{#snippet children()}Undo Reward{/snippet}
															</Button>
														{:else}
															<Button size="sm" variant="ghost" onclick={() => toggleCancelChoice(item.quest.id)}>
																{#snippet children()}Cancel{/snippet}
															</Button>
														{/if}
													{:else if item.quest.startedAt}
														<Button size="sm" onclick={() => handleComplete(item.quest.id)}>
															{#snippet children()}Complete{/snippet}
														</Button>
														<Button size="sm" variant="ghost" onclick={() => handleCancel(item.quest.id, false)}>
															{#snippet children()}Cancel{/snippet}
														</Button>
													{:else}
														<Button size="sm" variant="secondary" onclick={() => handleStart(item.quest.id)}>
															{#snippet children()}Start{/snippet}
														</Button>
													{/if}
												</div>
											</div>
										{/each}
									</div>
								{/if}
								{#if longHorizonItems.length > 0}
									<div class="space-y-1.5 pt-2">
										<div class="px-1 pt-1 eyebrow">Long-Horizon Quests</div>
										{#each longHorizonItems as item (item.quest.id)}
											{@const status = getCooldownStatus(item.quest)}
											{@const remaining = getCooldownRemaining(item.quest)}
											{#if item.description}
												<div class="text-xs text-text-secondary ml-7 px-1 pt-1">{item.description}</div>
											{/if}
											<div class="flex items-center gap-2.5 bg-surface/35 rounded-md px-3 py-2">
												<span class="text-xs text-text-tertiary font-mono w-4 text-right shrink-0">{@html '&bull;'}</span>
												<div class="shrink-0">
													{#if item.quest.startedAt}
														<div class="w-2 h-2 rounded-full bg-accent animate-pulse"></div>
													{:else if status === 'ready' || status === 'no_cooldown'}
														<div class="w-2 h-2 rounded-full bg-success"></div>
													{:else}
														<div class="w-2 h-2 rounded-full bg-text-tertiary"></div>
													{/if}
												</div>
												<span class="text-sm text-text truncate flex-1">{item.quest.name}</span>
												<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-surface-hover text-text-tertiary border border-border/50 shrink-0">Optional</span>
												{#if item.quest.waypoint}
													<button
														class="text-[10px] text-accent hover:text-accent/80 transition-colors cursor-pointer shrink-0"
														onclick={() => copyWaypoint(item.quest.id, item.quest.waypoint!)}
													>{copiedWp === item.quest.id ? 'Copied!' : 'WP'}</button>
												{/if}
												<div class="shrink-0 flex items-center gap-1">
													{#if status === 'cooling' && remaining}
														<span class="text-xs text-warning tabular-nums font-mono">{remaining}</span>
														{#if pendingCancelChoiceQuestId === item.quest.id}
															<Button size="sm" variant="secondary" onclick={() => handleCancel(item.quest.id, false)}>
																{#snippet children()}Keep Reward{/snippet}
															</Button>
															<Button size="sm" variant="danger" onclick={() => handleCancel(item.quest.id, true)}>
																{#snippet children()}Undo Reward{/snippet}
															</Button>
														{:else}
															<Button size="sm" variant="ghost" onclick={() => toggleCancelChoice(item.quest.id)}>
																{#snippet children()}Cancel{/snippet}
															</Button>
														{/if}
													{:else if item.quest.startedAt}
														<Button size="sm" onclick={() => handleComplete(item.quest.id)}>
															{#snippet children()}Complete{/snippet}
														</Button>
														<Button size="sm" variant="ghost" onclick={() => handleCancel(item.quest.id, false)}>
															{#snippet children()}Cancel{/snippet}
														</Button>
													{:else}
														<Button size="sm" variant="secondary" onclick={() => handleStart(item.quest.id)}>
															{#snippet children()}Start{/snippet}
														</Button>
													{/if}
												</div>
											</div>
										{/each}
									</div>
								{/if}
								{#if immediateItems.length === 0 && longHorizonItems.length === 0}
									<p class="text-xs text-text-tertiary py-2 text-center">No quests in this playlist.</p>
								{/if}
							</div>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
		</div>

	{:else if view === 'analytics'}
		<!-- ═══ ANALYTICS VIEW ═══ -->
		{#if analyticsLoading}
			<div class="text-sm text-text-tertiary py-8 text-center">Loading quest analytics...</div>
		{:else if analyticsError}
			<div class="text-sm text-negative bg-negative/10 rounded-md px-3 py-2">{analyticsError}</div>
		{:else if computedAnalytics.length === 0}
			<Card class="p-6">
				<p class="text-sm text-text-tertiary text-center">
					No curated quest analytics yet. Quest tracking continues in the background, but analytics only include sessions you explicitly link after a clean tracked run.
				</p>
			</Card>
		{:else}
			<div class="space-y-3">
					<div class="flex flex-wrap items-center justify-between gap-2">
					<h3 class="text-sm font-medium text-text-secondary">Single Quest Analytics</h3>
					<SegmentedControl
						options={[
							{ id: 'tt', label: 'TT Only' },
							{ id: 'markup', label: 'With Reward Markup' }
						]}
						active={analyticsRewardMode}
						onchange={(id) => (analyticsRewardMode = id as 'tt' | 'markup')}
					/>
				</div>
				{#snippet analyticsCell({ column, value, row }: { column: { key: string }; value: unknown; row: QuestAnalyticsComputed })}
					{#if column.key === 'questName'}
						<span class="font-medium">{value}</span>
					{:else if column.key === 'displayLiquidReward'}
						<div class="flex flex-col items-end leading-tight">
							<span class="tabular-nums">{formatPed(Number(value))}</span>
							{#if row.avgRewardPes > 0}
								<span class="text-[11px] text-accent">+{formatPed(row.avgRewardPes)} PES</span>
							{/if}
						</div>
					{:else if column.key === 'avgCycled'}
						<span class="tabular-nums">{formatPed(Number(value))}</span>
					{:else if column.key === 'avgNet'}
						<div class="flex flex-col items-end leading-tight">
							<span class="tabular-nums {Number(value) >= 0 ? 'text-positive' : 'text-negative'}">
								{Number(value) >= 0 ? '+' : ''}{formatPed(Number(value))}
							</span>
							{#if row.avgPesNet > 0}
								<span class="text-[11px] text-accent">+{formatPed(row.avgPesNet)} PES</span>
							{/if}
						</div>
					{:else if column.key === 'rewardMarkupPercent'}
						<span class="tabular-nums text-text-secondary">
							{value == null ? '—' : `${Number(value).toFixed(0)}%`}
						</span>
					{:else if column.key === 'returnRate'}
						<span class="tabular-nums">{formatPercent(Number(value))}</span>
					{:else}
						{value}
					{/if}
				{/snippet}
				<DataTable
					columns={analyticsColumns}
					rows={sortedAnalytics}
					bind:sortKey={analyticsSortKey}
					bind:sortDir={analyticsSortDir}
					cell={analyticsCell}
					emptyMessage="No curated quest runs"
				/>

				<!-- Playlist Analytics -->
				{#if computedPlaylistAnalytics.length > 0}
					<h3 class="text-sm font-medium text-text-secondary mt-6 mb-2">Playlist Analytics</h3>
					{#snippet playlistCell({ column, value, row }: { column: { key: string }; value: unknown; row: PlaylistAnalyticsComputed })}
						{#if column.key === 'playlistName'}
							<span class="font-medium">{value}</span>
						{:else if column.key === 'displayImmediateReward' || column.key === 'displayBonusReward'}
							{@const pesPortion = column.key === 'displayImmediateReward'
								? row.avgImmediateSkillReward
								: row.avgBonusSkillReward}
							<div class="flex flex-col items-end leading-tight">
								<span class="tabular-nums">{formatPed(Number(value))}</span>
								{#if pesPortion > 0}
									<span class="text-[11px] text-accent">+{formatPed(pesPortion)} PES</span>
								{/if}
							</div>
						{:else if column.key === 'avgCycled'}
							<span class="tabular-nums">{formatPed(Number(value))}</span>
						{:else if column.key === 'avgNet'}
							<div class="flex flex-col items-end leading-tight">
								<span class="tabular-nums {Number(value) >= 0 ? 'text-positive' : 'text-negative'}">
									{Number(value) >= 0 ? '+' : ''}{formatPed(Number(value))}
								</span>
								{#if row.avgPesNet > 0}
									<span class="text-[11px] text-accent">+{formatPed(row.avgPesNet)} PES</span>
								{/if}
							</div>
						{:else if column.key === 'rewardMarkupPercent'}
							<span class="tabular-nums text-text-secondary">
								{value == null ? '—' : `${Number(value).toFixed(0)}%`}
							</span>
						{:else if column.key === 'returnRate'}
							<span class="tabular-nums">{formatPercent(Number(value))}</span>
						{:else}
							{value}
						{/if}
					{/snippet}
					<DataTable
						columns={playlistAnalyticsColumns}
						rows={computedPlaylistAnalytics}
						cell={playlistCell}
						emptyMessage="No curated playlist runs"
					/>
				{/if}

				<div class="text-[11px] text-text-tertiary tabular-nums pt-2 text-right">
					Liquid baseline: {formatPercent(globalLiquidReturnRate)}
				</div>
			</div>
		{/if}
	{/if}
</div>

<!-- ═══ Quest Row Snippet ═══ -->
{#snippet questRow(quest: Quest, status: CooldownStatus, remaining: string | null)}
	<div class="bg-surface-raised/50 rounded-lg border border-border/50 hover:bg-surface-raised/70 transition-colors px-4 py-2.5">
		<!-- Top row -->
		<div class="flex items-center gap-2.5">
			<!-- Status dot -->
			<div class="shrink-0">
				{#if quest.startedAt}
					<div class="w-2.5 h-2.5 rounded-full bg-accent animate-pulse"></div>
				{:else if status === 'ready' || status === 'no_cooldown'}
					<div class="w-2.5 h-2.5 rounded-full bg-success"></div>
				{:else}
					<div class="w-2.5 h-2.5 rounded-full bg-text-tertiary"></div>
				{/if}
			</div>

			<!-- Quest info -->
			<div class="flex-1 min-w-0">
				<!-- Title line -->
				<div class="flex items-center gap-2 flex-wrap">
					<span class="text-sm font-medium text-text truncate">{quest.name}</span>
					{#if quest.rewardDescription}
						<span class="text-xs text-text-tertiary truncate hidden sm:inline">{quest.rewardDescription}</span>
					{/if}
					{#if quest.waypoint}
						<button
							class="text-[10px] text-accent hover:text-accent/80 transition-colors cursor-pointer shrink-0"
							onclick={() => copyWaypoint(quest.id, quest.waypoint!)}
						>{copiedWp === quest.id ? 'Copied!' : 'WP'}</button>
					{/if}
					{#each quest.targetMobs as mob}
						<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-accent/10 text-accent/70 border border-accent/20">{mob}</span>
					{/each}
				</div>
				<!-- Stats line -->
				<div class="flex items-center gap-1.5 mt-0.5 text-xs text-text-tertiary">
					{#if quest.reward}
						<span class="font-mono {quest.rewardIsSkill ? 'text-accent' : 'text-success'}">
							{quest.reward.toFixed(2)}
						</span>
						<span>{quest.rewardIsSkill ? 'PES' : 'PED'}</span>
					{/if}
					{#if quest.reward && quest.cooldownDurationHours}
						<span class="text-text-tertiary/50">|</span>
					{/if}
					{#if quest.cooldownDurationHours}
						<span>CD: {formatCooldownHours(quest.cooldownDurationHours)}</span>
					{/if}
				</div>
			</div>

			<!-- Action area -->
			<div class="shrink-0 flex items-center gap-1.5">
				{#if status === 'cooling' && remaining}
					<div class="text-right">
						<div class="text-xs text-warning tabular-nums font-mono">{remaining}</div>
						<div class="text-[10px] text-text-tertiary">remaining</div>
					</div>
					{#if pendingCancelChoiceQuestId === quest.id}
						<Button size="sm" variant="secondary" onclick={() => handleCancel(quest.id, false)}>
							{#snippet children()}Keep Reward{/snippet}
						</Button>
						<Button size="sm" variant="danger" onclick={() => handleCancel(quest.id, true)}>
							{#snippet children()}Undo Reward{/snippet}
						</Button>
					{:else}
						<Button size="sm" variant="ghost" onclick={() => toggleCancelChoice(quest.id)}>
							{#snippet children()}Cancel{/snippet}
						</Button>
					{/if}
				{:else if quest.startedAt}
					<Button size="sm" onclick={() => handleComplete(quest.id)}>
						{#snippet children()}Complete{/snippet}
					</Button>
					<Button size="sm" variant="ghost" onclick={() => handleCancel(quest.id, false)}>
						{#snippet children()}Cancel{/snippet}
					</Button>
				{:else}
					<Button size="sm" variant="secondary" onclick={() => handleStart(quest.id)}>
						{#snippet children()}Start{/snippet}
					</Button>
				{/if}

				<!-- Three-dot menu -->
				<div class="relative">
					<button
						aria-label="Quest actions"
						class="w-7 h-7 flex items-center justify-center rounded-md border border-border/50
							text-text-tertiary hover:text-text hover:bg-surface-hover transition-colors cursor-pointer"
						onclick={(e) => { e.stopPropagation(); openMenuId = openMenuId === quest.id ? null : quest.id; }}
					>
						<svg class="w-3.5 h-3.5" fill="currentColor" viewBox="0 0 16 16">
							<circle cx="8" cy="3" r="1.5"/><circle cx="8" cy="8" r="1.5"/><circle cx="8" cy="13" r="1.5"/>
						</svg>
					</button>
					{#if openMenuId === quest.id}
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<div class="absolute right-0 top-8 z-20 bg-surface-raised border border-border rounded-md shadow-lg py-1 min-w-[100px]"
							onclick={(e) => e.stopPropagation()}>
							<button class="w-full px-3 py-1.5 text-xs text-left text-text-secondary hover:bg-surface-hover hover:text-text cursor-pointer"
								onclick={() => openEditQuest(quest)}>Edit</button>
							{#if deleteConfirmId === quest.id}
								<div class="flex gap-1 px-2 py-1">
									<Button class="flex-1" size="sm" variant="danger" onclick={() => handleDeleteQuest(quest.id)}>
										{#snippet children()}Confirm{/snippet}
									</Button>
									<Button class="flex-1" size="sm" variant="ghost" onclick={() => (deleteConfirmId = null)}>
										{#snippet children()}Cancel{/snippet}
									</Button>
								</div>
							{:else}
								<button class="w-full px-3 py-1.5 text-xs text-left text-text-secondary hover:bg-surface-hover hover:text-negative cursor-pointer"
									onclick={() => (deleteConfirmId = quest.id)}>Delete</button>
							{/if}
						</div>
					{/if}
				</div>
			</div>
		</div>

		<!-- Notes (if present) -->
		{#if quest.notes}
			<div class="ml-[1.3rem] pl-3 mt-1.5 border-l-2 border-border/50">
				<p class="text-xs text-text-tertiary whitespace-pre-wrap">{quest.notes}</p>
			</div>
		{/if}
	</div>
{/snippet}

<!-- ═══ Quest Create/Edit Modal ═══ -->
<Modal bind:open={showQuestModal} title={editingQuest ? 'Edit Quest' : 'New Quest'} class="max-w-lg">
	{#snippet children()}
		<form class="space-y-3" onsubmit={(e) => { e.preventDefault(); saveQuest(); }}>
			<div class="grid grid-cols-2 gap-3">
				<div class="col-span-2">
					<label class="block text-xs text-text-secondary mb-1" for="q-name">Name</label>
					<Input id="q-name" type="text" required bind:value={questForm.name}
						placeholder="e.g., Atlas Haven Imperium Ranger Hunt!" />
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-planet">Planet</label>
					<Select id="q-planet" bind:value={questForm.planet}>
						<option value="Calypso">Calypso</option>
						<option value="Arkadia">Arkadia</option>
						<option value="Cyrene">Cyrene</option>
						<option value="Monria">Monria</option>
						<option value="Toulan">Toulan</option>
						<option value="Rocktropia">Rocktropia</option>
						<option value="Next Island">Next Island</option>
						<option value="Ancient Greece">Ancient Greece</option>
					</Select>
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-cat">Category</label>
					<Input id="q-cat" type="text" bind:value={questForm.category}
						placeholder="e.g., A.R.C. Faction" />
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-reward">Reward ({questForm.reward_is_skill ? 'PES' : 'PED'})</label>
					<Input id="q-reward" type="number" step="0.01" min="0" bind:value={questForm.reward_ped} />
				</div>
				<div>
					<div class="block text-xs text-text-secondary mb-1" aria-hidden="true">&nbsp;</div>
					<label class="flex items-center gap-1.5 h-[38px] text-xs text-text-secondary cursor-pointer">
						<input type="checkbox" bind:checked={questForm.reward_is_skill} class="accent-accent" />
						Reward is PES (skills)
					</label>
				</div>
				{#if !questForm.reward_is_skill}
					<div>
						<label class="block text-xs text-text-secondary mb-1" for="q-rmarkup">Expected Reward Markup %</label>
						<Input
							id="q-rmarkup"
							type="number"
							step="0.1"
							min="0"
							bind:value={questForm.expected_reward_markup_percent}
							disabled={rewardMarkupInputDisabled()}
							placeholder="e.g. 130"
						/>
					</div>
				{/if}
				<div class="col-span-2">
					<label class="block text-xs text-text-secondary mb-1" for="q-rdesc">Reward Note</label>
					<Input id="q-rdesc" type="text" bind:value={questForm.reward_description}
						placeholder="e.g., 3x A.R.C. Faction Badge" />
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-cd">Cooldown</label>
					<div class="flex items-stretch gap-2">
						<Input id="q-cd" type="number" step="1" min="0" bind:value={cooldownInput}
							class="flex-1 min-w-0"
							placeholder={cooldownUnit === 'hours' ? '21' : '7'} />
						<SegmentedControl
							size="md"
							options={[
								{ id: 'hours', label: 'Hours' },
								{ id: 'days', label: 'Days' }
							]}
							active={cooldownUnit}
							onchange={(id) => {
								if (id === 'hours' && cooldownUnit === 'days' && cooldownInput != null) {
									cooldownInput = cooldownInput * 24;
								} else if (id === 'days' && cooldownUnit === 'hours' && cooldownInput != null) {
									cooldownInput = Math.round((cooldownInput / 24) * 10) / 10;
								}
								cooldownUnit = id as 'hours' | 'days';
							}}
						/>
					</div>
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-wp">Waypoint</label>
					<Input id="q-wp" type="text" bind:value={questForm.waypoint}
						class="font-mono"
						placeholder="/wp [Planet, Lon, Lat, Alt]" />
				</div>
			</div>

			<!-- Target Mobs -->
			<div>
				<div class="block text-xs text-text-secondary mb-1">Target Mobs</div>
				{#if questForm.mobs.length > 0}
					<div class="flex flex-wrap gap-1 mb-1.5">
						{#each questForm.mobs as mob}
							<span class="text-xs px-2 py-0.5 rounded-full bg-accent/10 text-accent border border-accent/20 flex items-center gap-1">
								{mob}
								<button class="hover:text-text cursor-pointer" onclick={() => removeMob(mob)}>×</button>
							</span>
						{/each}
					</div>
				{/if}
				<div class="flex gap-2">
					<Input type="text" bind:value={mobInput} placeholder="Type mob name, press Enter"
						class="flex-1"
						onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addMob(); } }} />
					<Button size="sm" variant="secondary" onclick={addMob}>{#snippet children()}Add{/snippet}</Button>
				</div>
			</div>


			<div class="flex justify-end gap-2 pt-1">
				<Button variant="ghost" onclick={() => (showQuestModal = false)}>{#snippet children()}Cancel{/snippet}</Button>
				<Button type="submit">{#snippet children()}{editingQuest ? 'Save' : 'Create'}{/snippet}</Button>
			</div>
		</form>
	{/snippet}
</Modal>

<!-- ═══ Playlist Create/Edit Modal ═══ -->
<Modal bind:open={showPlaylistModal} title={editingPlaylist ? 'Edit Playlist' : 'New Playlist'} class="max-w-lg">
	{#snippet children()}
		<form class="space-y-3" onsubmit={(e) => { e.preventDefault(); savePlaylist(); }}>
			<div class="grid grid-cols-2 gap-3">
				<div class="col-span-2">
					<label class="block text-xs text-text-secondary mb-1" for="pl-name">Name</label>
					<Input id="pl-name" type="text" required bind:value={playlistForm.name}
						placeholder="e.g., Quick Calypso Run" />
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="pl-planet">Planet</label>
					<Select id="pl-planet" bind:value={playlistForm.planet}>
						<option value="Calypso">Calypso</option><option value="Arkadia">Arkadia</option>
						<option value="Cyrene">Cyrene</option><option value="Monria">Monria</option>
						<option value="Toulan">Toulan</option><option value="Rocktropia">Rocktropia</option>
						<option value="Next Island">Next Island</option><option value="Ancient Greece">Ancient Greece</option>
					</Select>
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="pl-time">Est. Time (min)</label>
					<Input id="pl-time" type="number" min="1" bind:value={playlistForm.estimated_minutes} />
				</div>
			</div>

			<!-- Immediate quests -->
			<div>
				<div class="block text-xs text-text-secondary mb-1.5">Immediate Quests</div>
				<p class="text-[11px] text-text-tertiary mb-2">These define the daily run and the playlist match requirement.</p>
				{#if playlistForm.immediate_items.length > 0}
					<div class="flex flex-col gap-1 mb-2">
						{#each playlistForm.immediate_items as item, i (item.quest_id)}
							<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-sm">
								<span class="text-text-tertiary text-xs font-mono w-4 text-right">{i + 1}</span>
								<span class="flex-1 text-text truncate">{questName(item.quest_id)}</span>
								<button type="button" class="text-[10px] text-text-tertiary hover:text-accent cursor-pointer" onclick={() => moveQuestBetweenGroups(item.quest_id, 'immediate')}>Long</button>
								<button type="button" class="text-text-tertiary hover:text-text cursor-pointer disabled:opacity-30" disabled={i === 0} onclick={() => moveQuestUp('immediate', i)}>&#x25B2;</button>
								<button type="button" class="text-text-tertiary hover:text-text cursor-pointer disabled:opacity-30" disabled={i >= playlistForm.immediate_items.length - 1} onclick={() => moveQuestDown('immediate', i)}>&#x25BC;</button>
								<button type="button" class="text-text-tertiary hover:text-negative cursor-pointer" onclick={() => removeQuestFromPlaylist(item.quest_id, 'immediate')}>×</button>
							</div>
						{/each}
					</div>
				{/if}
			</div>

			<!-- Long-horizon quests -->
			<div>
				<div class="block text-xs text-text-secondary mb-1.5">Long-Horizon Quests</div>
				<p class="text-[11px] text-text-tertiary mb-2">These may complete during the run, but they are optional for playlist matching.</p>
				{#if playlistForm.long_horizon_items.length > 0}
					<div class="flex flex-col gap-1 mb-2">
						{#each playlistForm.long_horizon_items as item, i (item.quest_id)}
							<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-sm">
								<span class="text-text-tertiary text-xs font-mono w-4 text-right">{i + 1}</span>
								<span class="flex-1 text-text truncate">{questName(item.quest_id)}</span>
								<button type="button" class="text-[10px] text-text-tertiary hover:text-accent cursor-pointer" onclick={() => moveQuestBetweenGroups(item.quest_id, 'long_horizon')}>Immediate</button>
								<button type="button" class="text-text-tertiary hover:text-text cursor-pointer disabled:opacity-30" disabled={i === 0} onclick={() => moveQuestUp('long_horizon', i)}>&#x25B2;</button>
								<button type="button" class="text-text-tertiary hover:text-text cursor-pointer disabled:opacity-30" disabled={i >= playlistForm.long_horizon_items.length - 1} onclick={() => moveQuestDown('long_horizon', i)}>&#x25BC;</button>
								<button type="button" class="text-text-tertiary hover:text-negative cursor-pointer" onclick={() => removeQuestFromPlaylist(item.quest_id, 'long_horizon')}>×</button>
							</div>
						{/each}
					</div>
				{/if}
			</div>

			<!-- Available quests -->
			<div>
				<div class="block text-xs text-text-secondary mb-1.5">Add Quests</div>
				{#if availableForPlaylist.length > 0}
					<div class="border border-border rounded-md max-h-48 overflow-y-auto">
						{#each availableForPlaylist as quest (quest.id)}
							<div class="w-full flex items-center gap-2 px-3 py-1.5 text-sm text-left text-text-secondary border-b border-border/30 last:border-b-0">
								<span class="truncate flex-1">{quest.name}</span>
								<span class="text-xs text-text-tertiary shrink-0">{quest.planet}</span>
								<button
									type="button"
									class="text-[10px] px-2 py-1 rounded border border-accent/25 text-accent hover:bg-accent/10 transition-colors cursor-pointer"
									onclick={() => addQuestToPlaylist(quest.id, 'immediate')}
								>+ Immediate</button>
								<button
									type="button"
									class="text-[10px] px-2 py-1 rounded border border-border/60 text-text-tertiary hover:text-text hover:bg-surface-hover transition-colors cursor-pointer"
									onclick={() => addQuestToPlaylist(quest.id, 'long_horizon')}
								>+ Long</button>
							</div>
						{/each}
					</div>
				{:else}
					<div class="text-xs text-text-tertiary rounded-md border border-border/50 px-3 py-2">
						All active quests are already in this playlist.
					</div>
				{/if}
			</div>

			<div class="flex justify-end gap-2 pt-1">
				<Button variant="ghost" onclick={() => (showPlaylistModal = false)}>{#snippet children()}Cancel{/snippet}</Button>
				<Button type="submit">{#snippet children()}{editingPlaylist ? 'Save' : 'Create'}{/snippet}</Button>
			</div>
		</form>
	{/snippet}
</Modal>
