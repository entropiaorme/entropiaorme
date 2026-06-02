<script lang="ts">
	import { Button } from '$lib/components';
	import {
		getTrackingLive,
		getQuests,
		getPlaylists,
		startQuest,
		completeQuest,
		cancelQuest,
		type TrackingLive,
	} from '$lib/api';
	import { trackingSnapshot, hydrate, subscribeTracking } from '$lib/stores/trackingStore';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import type { Quest, QuestPlaylist } from '$lib/types/quests';
	import type { CooldownStatus } from '$lib/types/common';
	import { invoke } from '@tauri-apps/api/core';
	import { flip } from 'svelte/animate';
	import { quintOut } from 'svelte/easing';
	import { get } from 'svelte/store';
	import DashboardWidgets from '$lib/components/dashboard/DashboardWidgets.svelte';
	import OverlayStrip from '$lib/components/overlay/OverlayStrip.svelte';
	import { getStatDef } from '$lib/statsRegistry';
	import {
		dashboardStats,
		overlayStats,
		setDashboardStats,
		DEFAULT_STAT_PREFS,
		DEFAULT_OVERLAY_PREFS,
		type StatPref
	} from '$lib/statsCustomisation';
	import { questsDemoQuests, questsDemoPlaylists } from '$lib/guide/fixtures/quests';
	import { onMount } from 'svelte';
	import { getPreference } from '$lib/preferences';
	import { guideState, registerDemoApi, unregisterDemoApi, getDemoApi } from '$lib/guide/state.svelte';
	import { closeGuide, openGuide } from '$lib/guide/engine';
	import { dashboardSurface } from '$lib/guide/surfaces/dashboard';

	// The consolidated tracking readout, sourced from the store: the dashboard's
	// single source of live-session render shape. `status` keeps its name so the
	// session island, stats grid, and widgets read it unchanged (the snapshot is
	// a superset of the old status shape).
	let status = $derived($trackingSnapshot);
	let elapsedSeconds = $state(0);
	// Guide-only: live overlay-strip feed sourced from /demo/tracking/live.
	// Drives the inline <OverlayStrip> mount that replaces the spawn screenshot
	// during the dashboard guide's overlay-spawn step. Fetched in the guide-mode
	// re-fetch $effect alongside the snapshot hydration.
	let demoTrackingLive = $state<TrackingLive | null>(null);
	// Guide-only: lifecycle phase for the demo overlay strip. The overlay-spawn
	// card mounts the strip in 'idle' first, then animates a cursor click on
	// the strip's TRACK button to flip to 'active' (mid-hunt readout). The
	// brief idle dwell shows a new user what the overlay looks like before
	// they start a session: pedagogically the most-frequent first state.
	let overlayStripPhase = $state<'idle' | 'active'>('idle');
	// Guide-only: fake armour-cost popup visibility + position. The real
	// armour-cost UI lives in a separate Tauri webview window
	// (/overlay-armour-cost) which the dashboard's inline strip cannot reach,
	// so the guide renders a styled stand-in that mirrors RepairCostPanel's
	// initial state (label + Record + Enter manually). Position is computed
	// from the Cost button's bounding rect at show time so the stand-in lands
	// directly below the button (matching the real popup's centred placement).
	let demoArmourPopupVisible = $state(false);
	// Two-state popup body: false = initial (label + Record + Enter manually);
	// true = post-record confirmation ("Cost recorded: 1.23 PED"). Auto-resets
	// to false whenever the popup is hidden so the next show starts clean.
	let demoArmourPopupRecorded = $state(false);
	let armourPopupTop = $state(0);
	let armourPopupLeft = $state(0);
	const ARMOUR_POPUP_WIDTH = 220;

	// Quest fixtures for the dashboard's guide-mode mount. getQuests /
	// getPlaylists hit the /quests surface (inline-fixture posture, not
	// demoPath-wrapped); dashboard short-circuits them to those fixtures
	// during guide-mode so the Quests widget shows a populated playlist
	// rather than the live (possibly-empty) data. The pre-guide
	// playlist selection is snapshotted so it survives the tour.
	let snapshotActivePlaylistId: string | null | undefined = undefined;

	// Preselected stat configuration applied to both stores while the guide
	// is open. Cards 1-6 show populated stat content (10 dashboard + 3
	// overlay pills enabled) regardless of the live prefs. Card 7
	// (modular-stats) takes its own snapshot at play() start and switches
	// to a 3-enabled baseline for the demo, then restores this preselected
	// configuration on exit, so back-nav 7→6 lands cleanly. Guide-close
	// reverses the outer snapshot held below to restore the live stats.
	const DASHBOARD_GUIDE_PRESELECTED_IDS = new Set<string>([
		'cycled', 'loot_tt', 'net', 'rate', 'pes',
		'pes_per_100', 'avg_cost_per_kill', 'multiplier_max', 'dpp', 'kills_count'
	]);
	const OVERLAY_GUIDE_PRESELECTED_IDS = new Set<string>([
		'cycled', 'rate', 'kills_count'
	]);
	const DASHBOARD_GUIDE_PRESELECTED: StatPref[] = DEFAULT_STAT_PREFS.map((p) => ({
		id: p.id,
		enabled: DASHBOARD_GUIDE_PRESELECTED_IDS.has(p.id)
	}));
	const OVERLAY_GUIDE_PRESELECTED: StatPref[] = DEFAULT_STAT_PREFS.map((p) => ({
		id: p.id,
		enabled: OVERLAY_GUIDE_PRESELECTED_IDS.has(p.id)
	}));
	let snapshotStatsForGuide:
		| { dashboard: StatPref[]; overlay: StatPref[] }
		| undefined = undefined;

	// DashboardWidgets active-tab snapshot/restore across the guide lifecycle.
	// Guide opens on 'pulse' regardless of where the user left it; their
	// choice returns on close. The widget unmounts during overlay-state cards
	// (gated on !(isActive && demoOverlayVisible)), so its sub-API may be
	// briefly unregistered; snapshot is captured at guide-open while still
	// mounted, and restored on close after the widget remounts.
	let snapshotWidgetsTab: string | undefined = undefined;

	async function loadQuests(): Promise<Quest[]> {
		return guideState.isActive ? questsDemoQuests : getQuests();
	}
	async function loadPlaylists(): Promise<QuestPlaylist[]> {
		return guideState.isActive ? questsDemoPlaylists : getPlaylists();
	}

	function syncArmourPopupPosition() {
		const btn = document.querySelector<HTMLElement>(
			'[data-guide-anchor="overlay-armour-cost-btn"]'
		);
		if (!btn) return;
		const rect = btn.getBoundingClientRect();
		armourPopupTop = rect.bottom + 8;
		armourPopupLeft = rect.left + rect.width / 2 - ARMOUR_POPUP_WIDTH / 2;
	}

	// Quest state
	let quests = $state<Quest[]>([]);
	let playlists = $state<QuestPlaylist[]>([]);
	let activePlaylistId = $state<string | null>(null);
	let now = $state(Date.now());
	let copiedWp = $state<string | null>(null);
	let pendingCancelChoiceQuestId = $state<string | null>(null);
	let recentEvents = $derived($trackingSnapshot?.recentEvents ?? []);

	// Stats grid drag-reorder via pointer events (not HTML5 drag — the latter cedes
	// cursor control to the OS, so we can't keep the grabbing hand stable through
	// the gesture). dragFilteredIndex tracks the dragged cell's position within the
	// enabled-only filtered list; the underlying full $dashboardStats list is
	// mutated via fullIndexOfEnabled() below so disabled stats stay in their slots.
	let dragFilteredIndex = $state<number | null>(null);
	let dragMoved = $state(false);
	let dragStartX = 0;
	let dragStartY = 0;
	// Cooldown after each reorder so cursor jitter at a cell boundary doesn't
	// ping-pong the layout while the flip animation is still settling.
	let lastReorderAt = 0;
	const REORDER_COOLDOWN_MS = 100;
	const DRAG_THRESHOLD_PX = 4;

	function fullIndexOfEnabled(prefs: StatPref[], filteredIndex: number): number {
		let count = 0;
		for (let i = 0; i < prefs.length; i++) {
			if (prefs[i].enabled) {
				if (count === filteredIndex) return i;
				count++;
			}
		}
		return -1;
	}

	function handleStatPointerDown(e: PointerEvent, filteredIndex: number) {
		if (e.button !== 0) return;
		const target = e.currentTarget as HTMLElement;
		target.setPointerCapture(e.pointerId);
		dragFilteredIndex = filteredIndex;
		dragStartX = e.clientX;
		dragStartY = e.clientY;
		dragMoved = false;
		lastReorderAt = 0;
		document.body.classList.add('stat-drag-active');
	}

	function handleStatPointerMove(e: PointerEvent) {
		if (dragFilteredIndex === null) return;
		// Threshold-gate: don't reorder for sub-pixel jitter on a click.
		if (!dragMoved) {
			const dx = e.clientX - dragStartX;
			const dy = e.clientY - dragStartY;
			if (dx * dx + dy * dy < DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX) return;
			dragMoved = true;
		}
		const now = performance.now();
		if (now - lastReorderAt < REORDER_COOLDOWN_MS) return;
		// Hit-test by walking cells' bounding rects directly. elementFromPoint
		// would return the captured (dragged) element because of pointer capture.
		const cells = document.querySelectorAll<HTMLElement>('[data-stat-cell]');
		let targetFilteredIndex = -1;
		for (const cell of cells) {
			const rect = cell.getBoundingClientRect();
			if (
				e.clientX >= rect.left &&
				e.clientX <= rect.right &&
				e.clientY >= rect.top &&
				e.clientY <= rect.bottom
			) {
				const idx = Number(cell.dataset.statCell);
				if (!Number.isNaN(idx)) targetFilteredIndex = idx;
				break;
			}
		}
		if (targetFilteredIndex < 0 || targetFilteredIndex === dragFilteredIndex) return;
		const full = get(dashboardStats);
		const sourceFull = fullIndexOfEnabled(full, dragFilteredIndex);
		const targetFull = fullIndexOfEnabled(full, targetFilteredIndex);
		if (sourceFull < 0 || targetFull < 0) return;
		const next = [...full];
		const [moved] = next.splice(sourceFull, 1);
		next.splice(targetFull, 0, moved);
		dashboardStats.set(next);
		dragFilteredIndex = targetFilteredIndex;
		lastReorderAt = now;
	}

	function handleStatPointerUp(e: PointerEvent) {
		if (dragFilteredIndex === null) return;
		const target = e.currentTarget as HTMLElement;
		if (target?.hasPointerCapture?.(e.pointerId)) {
			target.releasePointerCapture(e.pointerId);
		}
		if (dragMoved) void setDashboardStats(get(dashboardStats));
		dragFilteredIndex = null;
		dragMoved = false;
		lastReorderAt = 0;
		document.body.classList.remove('stat-drag-active');
	}

	function handleStatPointerCancel() {
		if (dragFilteredIndex === null) return;
		if (dragMoved) void setDashboardStats(get(dashboardStats));
		dragFilteredIndex = null;
		dragMoved = false;
		lastReorderAt = 0;
		document.body.classList.remove('stat-drag-active');
	}

	// Poll quest state so chat.log auto-start/complete is reflected without route changes.
	$effect(() => {
		const pollMs = status?.status === 'active' ? 3000 : 5000;
		return useVisiblePoll(refreshQuestState, { intervalMs: pollMs });
	});

	// Cooldown tick (1s)
	$effect(() => {
		return useVisiblePoll(() => { now = Date.now(); }, { intervalMs: 1000 });
	});

	// Elapsed timer when tracking is active
	$effect(() => {
		if (status?.status === 'active' && status.started_at) {
			const startMs = new Date(status.started_at).getTime();
			elapsedSeconds = Math.max(0, Math.floor((Date.now() - startMs) / 1000));
			return useVisiblePoll(() => {
				elapsedSeconds = Math.max(0, Math.floor((Date.now() - startMs) / 1000));
			}, { intervalMs: 1000, immediate: false });
		} else {
			elapsedSeconds = 0;
		}
	});

	async function refreshQuestState() {
		try {
			const [loadedQuests, loadedPlaylists] = await Promise.all([
				loadQuests(),
				loadPlaylists(),
			]);
			quests = loadedQuests;
			playlists = loadedPlaylists;
			syncActivePlaylist(loadedPlaylists);
			if (pendingCancelChoiceQuestId && !loadedQuests.some((quest) => quest.id === pendingCancelChoiceQuestId)) {
				pendingCancelChoiceQuestId = null;
			}
		} catch { /* ignore */ }
	}

	function syncActivePlaylist(loadedPlaylists: QuestPlaylist[]) {
		if (loadedPlaylists.length === 0) {
			activePlaylistId = null;
			return;
		}
		if (guideState.isActive) {
			// Guide-mode: pin to first demo playlist regardless of prior selection
			// so the Quests widget shows populated content on the dashboard-widgets
			// card. snapshotActivePlaylistId holds the pre-guide selection across
			// the guide lifecycle (see the guide-flip $effect below).
			activePlaylistId = loadedPlaylists[0].id;
			return;
		}
		if (activePlaylistId && !loadedPlaylists.some((playlist) => playlist.id === activePlaylistId)) {
			activePlaylistId = null;
		}
	}

	// ── Quest helpers ──
	let activePlaylist = $derived(playlists.find((p) => p.id === activePlaylistId) ?? null);

	function playlistQuestItemsForGroup(playlist: QuestPlaylist | null, groupType?: 'immediate' | 'long_horizon') {
		if (!playlist) return [];
		const out: { quest: Quest; description: string | null; cd: CooldownStatus; inProgress: boolean }[] = [];
		for (const item of playlist.items) {
			if (groupType && item.groupType !== groupType) continue;
			const quest = quests.find((q) => q.id === item.questId);
			if (!quest) continue;
			out.push({
				quest,
				description: item.description,
				cd: getCooldownStatus(quest),
				inProgress: quest.startedAt != null,
			});
		}
		return out;
	}

	let immediatePlaylistQuestItems = $derived.by(() =>
		playlistQuestItemsForGroup(activePlaylist, 'immediate')
	);

	let longHorizonPlaylistQuestItems = $derived.by(() =>
		playlistQuestItemsForGroup(activePlaylist, 'long_horizon')
	);

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

	async function handleQuestStart(questId: string) {
		try {
			const updated = await startQuest(questId);
			quests = quests.map((q) => (q.id === updated.id ? updated : q));
			if (pendingCancelChoiceQuestId === questId) pendingCancelChoiceQuestId = null;
		} catch { /* ignore */ }
	}

	async function handleQuestComplete(questId: string) {
		try {
			const updated = await completeQuest(questId);
			quests = quests.map((q) => (q.id === updated.id ? updated : q));
			if (pendingCancelChoiceQuestId === questId) pendingCancelChoiceQuestId = null;
		} catch { /* ignore */ }
	}

	async function handleQuestCancel(questId: string, undoReward = false) {
		try {
			const updated = await cancelQuest(questId, undoReward);
			quests = quests.map((q) => (q.id === updated.id ? updated : q));
			if (pendingCancelChoiceQuestId === questId) pendingCancelChoiceQuestId = null;
		} catch { /* ignore */ }
	}

	function toggleQuestCancelChoice(questId: string) {
		pendingCancelChoiceQuestId = pendingCancelChoiceQuestId === questId ? null : questId;
	}

	function copyWaypoint(questId: string, waypoint: string) {
		navigator.clipboard.writeText(waypoint);
		copiedWp = questId;
		setTimeout(() => { if (copiedWp === questId) copiedWp = null; }, 1500);
	}

	function formatElapsed(seconds: number): string {
		const h = Math.floor(seconds / 3600);
		const m = Math.floor((seconds % 3600) / 60);
		return h > 0 ? `${h}h ${m}m` : `${m}m`;
	}

	function formatMinutes(m: number): string {
		if (m < 60) return `~${m}m`;
		const h = Math.floor(m / 60);
		const rem = m % 60;
		return rem > 0 ? `~${h}h ${rem}m` : `~${h}h`;
	}

	// Guide
	let guideSeen = $state(true);
	// Guide-only: when true, the Recent Events + DashboardWidgets islands hide
	// and a fake overlay-window screenshot mounts in their place. Driven by the
	// surface module's setOverlayDemoVisible demoApi call. Mirrors the character
	// surface's demoFakeScannerVisible pattern.
	let demoOverlayVisible = $state(false);
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
		// Keep the consolidated snapshot current by subscribing to the relayed
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
		registerDemoApi('dashboard', {
			setOverlayDemoVisible: (visible: boolean) => {
				demoOverlayVisible = visible;
				// Reset the lifecycle phase on (un)mount so each guide opening
				// starts the strip in idle regardless of where the prior session
				// left it. Resetting on mount keeps the looped play()'s "set
				// visible then set tracking-started" sequence working without
				// the surface module having to reset twice.
				overlayStripPhase = 'idle';
			},
			setOverlayDemoTrackingStarted: (started: boolean) => {
				overlayStripPhase = started ? 'active' : 'idle';
			},
			setOverlayArmourPopupVisible: (visible: boolean) => {
				// Sync position before mounting so the stand-in lands under the
				// Cost button on first frame (the popup's rect-bound style only
				// applies once $state has propagated; pre-syncing avoids a
				// one-tick flash at the prior coordinates).
				if (visible) syncArmourPopupPosition();
				else demoArmourPopupRecorded = false; // reset body so next show starts clean
				demoArmourPopupVisible = visible;
			},
			setOverlayArmourPopupRecorded: (recorded: boolean) => {
				demoArmourPopupRecorded = recorded;
			},
			snapshotStats: () => ({
				dashboard: get(dashboardStats),
				overlay: get(overlayStats)
			}),
			restoreStats: (snap: { dashboard: StatPref[]; overlay: StatPref[] }) => {
				dashboardStats.set(snap.dashboard);
				overlayStats.set(snap.overlay);
			},
			setDemoStatsBaseline: (overrides?: Record<string, boolean>) => {
				// Reset both stores to default prefs (transient: no setDashboardStats
				// call, so nothing persists to user preferences). Optional overrides
				// flip specific stat-ids' enabled flags, letting cards bend the
				// baseline (e.g. start the modular-stats card with 3 enabled
				// instead of 4) without forking the constant.
				const base = overrides
					? DEFAULT_STAT_PREFS.map((p) =>
							p.id in overrides ? { ...p, enabled: overrides[p.id] } : p
						)
					: DEFAULT_STAT_PREFS;
				dashboardStats.set(base);
				overlayStats.set(DEFAULT_OVERLAY_PREFS);
			},
			toggleDemoStatPill: (surface: 'dashboard' | 'overlay', statId: string) => {
				// Transient toggle on the named pill. Mirrors handlePillClick's
				// shape (map → flip enabled flag) but bypasses setDashboardStats
				// so the persisted prefs aren't touched.
				const store = surface === 'dashboard' ? dashboardStats : overlayStats;
				const current = get(store);
				const next = current.map((p) =>
					p.id === statId ? { ...p, enabled: !p.enabled } : p
				);
				store.set(next);
			},
			reorderDemoStat: (fromFilteredIdx: number, toFilteredIdx: number) => {
				// Transient reorder using the existing fullIndexOfEnabled logic
				// so the move respects the disabled-stats-stay-put invariant
				// the real drag handler enforces.
				const current = get(dashboardStats);
				const sourceFull = fullIndexOfEnabled(current, fromFilteredIdx);
				const targetFull = fullIndexOfEnabled(current, toFilteredIdx);
				if (sourceFull < 0 || targetFull < 0) return;
				const next = [...current];
				const [moved] = next.splice(sourceFull, 1);
				next.splice(targetFull, 0, moved);
				dashboardStats.set(next);
			},
			setDragVisualIndex: (idx: number | null) => {
				// Sets the existing dragFilteredIndex $state so the real drag
				// visual (opacity-40 + shadow + ring on the cell at the matching
				// filtered index) renders for the guide's virtual drag.
				dragFilteredIndex = idx;
			},
			triggerArmourDrag: () => {
				// CSS transition handles the slide; toggling `.docked` is enough.
				// `style.transition = ''` restores the class-defined transition in
				// case `resetArmourSvg` previously inlined `transition: none` for
				// an instant snap back to the start position.
				const win = document.getElementById('armour-svg-window');
				if (!win) return;
				win.style.transition = '';
				win.classList.add('docked');
			},
			triggerArmourFlash: () => {
				// Set animation via inline style + force-reflow trick so the
				// keyframe runs from frame 0 on every cursor-click iteration.
				// SVG elements lack `offsetWidth`; `getBoundingClientRect()` is
				// the SVG-compatible force-reflow primitive.
				const flash = document.getElementById('armour-svg-flash');
				if (!flash) return;
				flash.style.animation = 'none';
				flash.getBoundingClientRect();
				flash.style.animation = 'armourFlash 500ms ease-out';
			},
			resetArmourSvg: () => {
				// Snap window back to start (no animation) and clear any in-flight
				// flash. Done in the loop's gap phase so the next iteration's drag
				// reads as a fresh "place the terminal" beat, not a slow revert.
				const win = document.getElementById('armour-svg-window');
				if (win) {
					win.style.transition = 'none';
					win.classList.remove('docked');
					win.getBoundingClientRect();
					win.style.transition = '';
				}
				const flash = document.getElementById('armour-svg-flash');
				if (flash) flash.style.animation = 'none';
			}
		});
		return () => {
			unmounted = true;
			unregisterDemoApi('dashboard');
			unsubscribeTracking?.();
		};
	});

	// Re-fetch tracking + quest data when guide-mode flips so the dashboard
	// switches between real and /demo/ endpoints immediately instead of
	// waiting on the next 3-5s poll tick. Each dependency on guideState.isActive
	// is a void-read for reactivity tracking; the fetches inherit the demoPath
	// routing from $lib/api.ts and (for quests) loadQuests/loadPlaylists.
	$effect(() => {
		const active = guideState.isActive;
		// Snapshot the active playlist selection on guide-open so the
		// post-tour restore returns to the pre-guide state. Undefined
		// sentinel means "no snapshot held"; null is a valid snapshot value.
		if (active && snapshotActivePlaylistId === undefined) {
			snapshotActivePlaylistId = activePlaylistId;
		}
		// Stats: snapshot the live config on guide-open + apply the preselected
		// demo configuration so cards 1-6 render populated stats grids. Card 7
		// takes its own snapshot at play() start (which captures this preselected
		// config) and runs its own 3-enabled baseline demo on top, restoring
		// the preselected on its exit so back-nav 7→6 is clean. Guide-close
		// reverses this outer snapshot to restore the live config.
		if (active && snapshotStatsForGuide === undefined) {
			snapshotStatsForGuide = {
				dashboard: get(dashboardStats),
				overlay: get(overlayStats)
			};
			dashboardStats.set(DASHBOARD_GUIDE_PRESELECTED);
			overlayStats.set(OVERLAY_GUIDE_PRESELECTED);
		} else if (!active && snapshotStatsForGuide !== undefined) {
			dashboardStats.set(snapshotStatsForGuide.dashboard);
			overlayStats.set(snapshotStatsForGuide.overlay);
			snapshotStatsForGuide = undefined;
		}
		// Widgets tab: snapshot on open + force 'pulse' so the demo always
		// starts on Loot Pulse. Restore the pre-guide tab on close. Microtask
		// defer for the restore so the widget has a tick to remount before
		// receiving setTab (gate flips when isActive→false).
		if (active && snapshotWidgetsTab === undefined) {
			const wapi = getDemoApi('dashboard-widgets') as {
				setTab?: (id: string) => void;
				getTab?: () => string;
			};
			snapshotWidgetsTab = wapi.getTab?.() ?? 'pulse';
			wapi.setTab?.('pulse');
		} else if (!active && snapshotWidgetsTab !== undefined) {
			const restored = snapshotWidgetsTab;
			snapshotWidgetsTab = undefined;
			queueMicrotask(() => {
				const wapi = getDemoApi('dashboard-widgets') as {
					setTab?: (id: string) => void;
				};
				wapi.setTab?.(restored);
			});
		}
		// Re-read the consolidated snapshot through the guide-aware client so the
		// switch between real and demo data is immediate (the snapshot GET routes
		// to the demo namespace automatically while the guide is active). This is
		// also the dashboard's initial load on mount.
		void hydrate();
		void (async () => {
			try {
				const [live, loadedQuests, loadedPlaylists] = await Promise.all([
					active ? getTrackingLive() : Promise.resolve(null),
					loadQuests(),
					loadPlaylists(),
				]);
				demoTrackingLive = live;
				quests = loadedQuests;
				playlists = loadedPlaylists;
				if (!active && snapshotActivePlaylistId !== undefined) {
					// Restore on guide-close. syncActivePlaylist then validates
					// the restored id against the freshly-loaded real playlists
					// (and nulls it if the user deleted that playlist mid-tour).
					activePlaylistId = snapshotActivePlaylistId;
					snapshotActivePlaylistId = undefined;
				}
				syncActivePlaylist(loadedPlaylists);
			} catch { /* ignore */ }
		})();
	});
</script>

<div class="px-6 pb-6 flex flex-col gap-4 h-full" data-guide-anchor="dashboard-area">

	<!-- Page header -->
	<div class="flex items-center justify-between">
		<header class="flex flex-col gap-1.5">
			<h1 class="text-xl font-semibold text-text tracking-tight">Dashboard</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="text-sm text-text-secondary mt-0.5">Track sessions, monitor events, run quest playlists</p>
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

	<!-- ═══ Island: Session ═══ -->
	<section class="panel p-4 flex flex-col gap-3 flex-shrink-0">
		<!-- Session strip -->
		<div class="relative flex items-center justify-between">
			{#if status?.status === 'active'}
				<div class="flex items-center gap-3">
					<span class="signal-dot positive animate-pulse"></span>
					<span class="text-sm font-medium text-text tracking-tight">Tracking active</span>
					<span class="text-xs text-text-tertiary tabular-nums tracking-wider">
						{formatElapsed(elapsedSeconds)}
					</span>
				</div>
			{:else}
				<div class="flex items-center gap-3">
					<span class="signal-dot idle"></span>
					<span class="text-sm text-text-secondary">No active session</span>
				</div>
			{/if}

			<div class="flex items-center gap-2">
				<span class="inline-flex" data-guide-anchor="dashboard-overlay-btn">
					<Button size="sm" onclick={() => invoke('toggle_overlay').catch(() => {})}>
						{#snippet children()}Overlay{/snippet}
					</Button>
				</span>
			</div>
		</div>

		<!-- Session stats -->
		<div
			class="dashboard-stat-grid relative grid gap-2"
			data-guide-anchor="dashboard-stats-grid"
		>
			{#each $dashboardStats.filter((p) => p.enabled) as pref, i (pref.id)}
				{@const def = getStatDef(pref.id)}
				{@const r = def ? def.render(status) : { value: '—', color: 'text-text-tertiary' }}
				{@const isDragged = dragFilteredIndex === i}
				<div
					animate:flip={{ duration: 240, easing: quintOut }}
					data-stat-cell={i}
					role="group"
					aria-label={def?.label ?? pref.id}
					class="relative rounded-md border border-border/60 bg-base/40 px-3 py-2.5 flex flex-col gap-1
						min-w-0 cursor-grab select-none touch-none
						transition-[opacity,box-shadow,border-color] duration-[var(--duration-base)] ease-[var(--ease-out)]
						before:pointer-events-none before:absolute before:inset-0 before:rounded-[inherit]
						before:[box-shadow:inset_0_1px_0_0_rgba(255,255,255,0.03)]
						{isDragged ? 'opacity-40 shadow-lg ring-1 ring-accent/60 z-10' : ''}"
					onpointerdown={(e) => handleStatPointerDown(e, i)}
					onpointermove={handleStatPointerMove}
					onpointerup={handleStatPointerUp}
					onpointercancel={handleStatPointerCancel}
				>
					<span class="eyebrow truncate">{def?.label ?? pref.id}</span>
					<span class="truncate text-[17px] font-semibold tabular-nums leading-none tracking-tight
						{r.value === '—' ? 'text-text-tertiary' : r.color}">
						{r.value}
					</span>
				</div>
			{/each}
		</div>

		{#if status?.status === 'active'}
			{#if status.weaponAttribution === 'hotbar' && status.hotbarListenerActive === false}
				<div class="relative flex items-start gap-3 px-3.5 py-3 rounded-md border border-warning/30 bg-warning/[0.06]">
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" class="h-4.5 w-4.5 mt-0.5 text-warning shrink-0">
						<path fill-rule="evenodd" d="M8.485 2.495c.673-1.167 2.357-1.167 3.03 0l6.28 10.875c.673 1.167-.17 2.625-1.516 2.625H3.72c-1.347 0-2.189-1.458-1.515-2.625L8.485 2.495zM10 5a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 0110 5zm0 9a1 1 0 100-2 1 1 0 000 2z" clip-rule="evenodd" />
					</svg>
					<div class="flex flex-col gap-0.5">
						<p class="text-sm font-medium text-warning tracking-tight">Hotbar key listener not active</p>
						<p class="text-xs text-text-secondary leading-relaxed">
							Cost attribution is using the hotbar but the listener isn't running. Check that the hotbar key listener is enabled in Settings.
						</p>
					</div>
				</div>
			{/if}
		{/if}
	</section>

	{#if !(guideState.isActive && demoOverlayVisible)}
		<!-- ═══ Island: Recent Events ═══ -->
		<section class="panel p-4 flex-shrink-0" data-guide-anchor="dashboard-recent-events">
			<h3 class="eyebrow mb-3">Recent events</h3>

			{#if recentEvents.length > 0}
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
				sessionId={status?.session_id ?? null}
				multiplierHistory={status?.multiplierHistory ?? null}
				cumulativeNetHistory={status?.cumulativeNetHistory ?? null}
				{playlists}
				{activePlaylistId}
				{activePlaylist}
				immediateItems={immediatePlaylistQuestItems}
				longHorizonItems={longHorizonPlaylistQuestItems}
				{pendingCancelChoiceQuestId}
				{copiedWp}
				onPlaylistChange={(id) => (activePlaylistId = id)}
				onQuestStart={handleQuestStart}
				onQuestComplete={handleQuestComplete}
				onQuestCancel={handleQuestCancel}
				onToggleCancelChoice={toggleQuestCancelChoice}
				onCopyWaypoint={copyWaypoint}
				{formatMinutes}
				{getCooldownRemaining}
			/>
	{/if}

	<!--
		Guide-only: inline overlay spawn. Mounts the real OverlayStrip with demo
		data routed through /demo/tracking/* so the actual overlay affordances
		render (TRACK button, mob row, stat pills, weapon, COST) instead of a
		static screenshot. Same fixed-positioning + flex-centring discipline as
		the character skill-scanner spawn (pointer-events-none so the guide
		click-blocker handles clicks). The cutout anchors on the strip's wrapper
		via data-guide-anchor; the surface module's anchor closure does a 2-phase
		priority cascade (this wrapper wins over the Overlay button once mounted).
	-->
	{#if guideState.isActive && demoTrackingLive}
		<!--
			Always-mounted slot for the demo overlay strip. Stays present for
			the whole duration of the dashboard guide (not just during the
			overlay-spawn step's visible-strip window) so the prose card's
			placementAnchor resolves immediately on step entry instead of
			falling back to viewport-pinned bottom-centre and snapping up
			when the strip mounts. Two anchor names cooperate:
			  - dashboard-overlay-spawn-slot: always present, for placement
			  - dashboard-overlay-spawn: only present when visible, for cutout
			so the cutout cascade still correctly stays on the Overlay button
			during Phase 1 of the looped play().
		-->
		<div class="fixed top-[350px] left-12 right-0 z-10 flex justify-center pointer-events-none">
			<span class="inline-flex" data-guide-anchor="dashboard-overlay-spawn-slot">
				<span
					class="inline-flex"
					style:opacity={demoOverlayVisible ? 1 : 0}
					data-guide-anchor={demoOverlayVisible ? 'dashboard-overlay-spawn' : undefined}
				>
					{#if overlayStripPhase === 'active'}
						<OverlayStrip data={demoTrackingLive} {status} armourSessionId="demo-session" />
					{:else}
						<!-- Idle synth: status='idle' + nulled session fields. Carries
							 the live response's trifectaAttribution + weaponAttribution
							 so the trifecta dropdown stays populated (the Calypso preset
							 reads as waiting-to-be-selected, not a "—" placeholder).
							 status={null} passed to OverlayStrip so stat pills render as
							 em-dashes until tracking actually starts. -->
						<OverlayStrip
							data={{
								status: 'idle',
								weaponAttribution: demoTrackingLive.weaponAttribution,
								trifectaAttribution: demoTrackingLive.trifectaAttribution,
								repairOcrEnabled: demoTrackingLive.repairOcrEnabled,
								mobEntryMode: 'mob',
								currentMob: null,
								mobSource: null,
								currentTool: null
							}}
							status={null}
						/>
					{/if}
				</span>
			</span>
		</div>
	{/if}

	<!--
		Guide-only: fake armour-cost popup. Mirrors RepairCostPanel's initial
		state (label + Record + Enter manually) styled to match the OverlayStrip's
		glassmorphic look. Positioned dynamically below the strip's Cost
		button via syncArmourPopupPosition(). pointer-events-none on the
		wrapper so the live cursor never interacts with the stand-in;
		the virtual cursor uses clickRipple() rather than el.click() so the
		visual click does not dispatch a real event.
	-->
	{#if guideState.isActive && demoArmourPopupVisible}
		<div
			class="fixed z-10 pointer-events-none flex"
			style:left={`${armourPopupLeft}px`}
			style:top={`${armourPopupTop}px`}
			style:width={`${ARMOUR_POPUP_WIDTH}px`}
		>
			<div
				class="fake-armour-popup flex items-center justify-center gap-2 rounded-xl px-3 py-1.5 w-full"
				data-guide-anchor="overlay-armour-popup"
			>
				{#if demoArmourPopupRecorded}
					<span class="text-xs text-white/60 shrink-0">Cost recorded:</span>
					<span class="text-sm font-semibold text-emerald-400 tabular-nums">1.23 PED</span>
				{:else}
					<span class="text-xs text-white/50 shrink-0">Armour cost:</span>
					<button
						type="button"
						class="fake-armour-record-btn"
						data-guide-anchor="overlay-armour-record-btn"
					>Record</button>
					<button type="button" class="fake-armour-manual-btn">Enter manually</button>
				{/if}
			</div>
		</div>
	{/if}
</div>

<style>
	.dashboard-stat-grid {
		grid-template-columns: repeat(
			auto-fill,
			minmax(clamp(112px, calc((100% - 2rem) / 5), 140px), 1fr)
		);
	}

	:global(body.stat-drag-active),
	:global(body.stat-drag-active *) {
		cursor: grabbing !important;
		user-select: none;
	}

	/* Guide-only: fake armour-cost popup. Glassmorphic palette matches the
	   OverlayStrip's .glass-panel rule (intentionally duplicated rather than
	   refactored to :global since the stand-in only lives in the dashboard
	   guide and never co-mounts with the real popup window). */
	.fake-armour-popup {
		background: rgba(10, 14, 23, 0.85);
		backdrop-filter: blur(16px) saturate(150%);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}
	.fake-armour-record-btn {
		padding: 3px 10px;
		border-radius: 4px;
		background: rgba(99, 179, 237, 0.18);
		border: 1px solid rgba(99, 179, 237, 0.42);
		color: rgb(125, 191, 240);
		font-size: 11px;
		font-weight: 500;
		line-height: 1;
	}
	.fake-armour-manual-btn {
		padding: 3px 10px;
		border-radius: 4px;
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.16);
		color: rgba(255, 255, 255, 0.75);
		font-size: 11px;
		font-weight: 500;
		line-height: 1;
	}
</style>
