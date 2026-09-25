<script lang="ts">
	import {
		ApiError,
		getTrackingSnapshot,
		startTracking,
		stopTracking,
		releaseMob,
		getOverlayPosition,
		saveOverlayPosition,
		getSessionDefinitions,
		selectDefinition,
		setSessionConfig,
		getManualMobSuggestions,
		lockManualMob,
		getActivityOptions,
		activateActivity,
		deactivateActivity,
		beginQuestHandIn,
		updateSettings,
		type TrackingLive,
		type TrackingStatus,
		type TrackingSnapshot,
		type ManualMobSuggestion
	} from '$lib/api';
	import { untrack } from 'svelte';
	import { useVisiblePoll, windowGeometryPoll } from '$lib/realtime/useVisiblePoll';
	import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
	import { createPostSessionFlow } from '$lib/features/tracking/postSession.svelte';
	import { createSessionFacets } from '$lib/features/tracking/sessionFacets.svelte';
	import { createActivitiesModel } from '$lib/features/tracking/activitiesModel.svelte';
	import { createOverlayNotices } from '$lib/features/tracking/overlayNotices.svelte';
	import { createOverlayArmourCostModel } from '$lib/features/protection/overlayArmourCostModel.svelte';
	import { createTypeahead } from '$lib/view/typeahead.svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { PhysicalPosition } from '@tauri-apps/api/dpi';
	import { listen } from '@tauri-apps/api/event';
	import { anchorBelow } from '$lib/windows/anchor';
	import { createSatelliteWindow } from '$lib/windows/satellite';
	import { createWindowSizeSync } from '$lib/windows/windowSize';
	import {
		OVERLAY_MENU_CLOSED_EVENT,
		OVERLAY_MENU_HIDE_EVENT,
		OVERLAY_MENU_INTERACT_EVENT,
		OVERLAY_MENU_READY_EVENT,
		OVERLAY_MENU_SELECT_EVENT,
		OVERLAY_MENU_SHOW_EVENT,
		OVERLAY_MENU_WINDOW_LABEL,
		OVERLAY_MENU_MIN_WIDTH,
		buildActivitiesMenuState,
		buildQuestHandInMenuState,
		buildDefinitionMenuState,
		computeMenuHeight,
		computeMenuWidth,
		menuRowCount,
		type OverlayMenuKind,
		type OverlayMenuSelection,
		type OverlayMenuState
	} from '$lib/windows/overlayMenu';
	import {
		OVERLAY_ARMOUR_COST_CLOSED_EVENT,
		OVERLAY_ARMOUR_COST_HIDE_EVENT,
		OVERLAY_ARMOUR_COST_READY_EVENT,
		OVERLAY_ARMOUR_COST_SHOW_EVENT,
		OVERLAY_ARMOUR_COST_WINDOW_LABEL
	} from '$lib/windows/overlayArmourCost';
	import OverlayStrip from '$lib/components/overlay/OverlayStrip.svelte';
	import OverlayNotices from '$lib/components/overlay/OverlayNotices.svelte';

	// The colon-form Tauri topic the shell's event bridge emits each backend
	// tracking frame on (the wire topic `tracking.session.updated`; Tauri event
	// names forbid dots).
	const TRACKING_TOPIC = 'tracking:session:updated';
	// Emitted by the shell (toggle_overlay) when this hidden window is shown, so
	// the overlay can re-read config/runtime fields no tracking frame announces.
	const OVERLAY_SHOWN_EVENT = 'overlay-shown';
	const OVERLAY_MENU_VERTICAL_GAP = 6;

	let overlayRoot: HTMLDivElement | null = $state(null);
	let overlayMenuKind = $state<OverlayMenuKind | null>(null);
	let mobInput: HTMLInputElement | null = $state(null);
	let mobInputFocused = $state(false);
	let trifectaSaving = $state(false);
	let data = $state<TrackingLive>({ status: 'idle' });
	let status = $state<TrackingStatus | null>(null);
	// Session start in epoch-ms (parsed from the snapshot's started_at), the basis
	// for the client-side elapsed tick. null when no active session is timed.
	let sessionStartedAtMs = $state<number | null>(null);
	let releasing = $state(false);
	let starting = $state(false);

	let mobQuery = $state('');
	// The mob lookup's error channel, shared between the typeahead (search
	// failures, mirrored in by the presenter effect below) and the declare
	// action.
	let mobError = $state<string | null>(null);
	let selectingMob = $state(false);
	let mobCloseTimer: ReturnType<typeof setTimeout> | undefined;


	// The two satellite popovers this window drives. The failure messages keep
	// the overlay's established wording (they render in the strip).
	const menuWindow = createSatelliteWindow({
		label: OVERLAY_MENU_WINDOW_LABEL,
		url: '/overlay-menu',
		width: OVERLAY_MENU_MIN_WIDTH,
		height: 44,
		readyEvent: OVERLAY_MENU_READY_EVENT,
		showEvent: OVERLAY_MENU_SHOW_EVENT,
		hideEvent: OVERLAY_MENU_HIDE_EVENT,
		messages: {
			creationTimeout: 'Popup window creation timed out',
			creationFailed: 'Unknown Tauri popup creation error',
			readyTimeout: 'Popup route did not become ready'
		}
	});
	const armourCostWindow = createSatelliteWindow({
		label: OVERLAY_ARMOUR_COST_WINDOW_LABEL,
		url: '/overlay-armour-cost',
		width: 320,
		height: 64,
		readyEvent: OVERLAY_ARMOUR_COST_READY_EVENT,
		showEvent: OVERLAY_ARMOUR_COST_SHOW_EVENT,
		hideEvent: OVERLAY_ARMOUR_COST_HIDE_EVENT,
		messages: {
			creationTimeout: 'Armour cost popup creation timed out',
			creationFailed: 'Unknown Tauri popup creation error',
			readyTimeout: 'Armour cost popup did not become ready'
		}
	});

	// The consolidated snapshot, event-driven with coalesced re-reads (see the
	// factory). Each webview is its own JS context, so the overlay keeps its
	// own store instance beside the dashboard's.
	const snapshot = createSnapshotStore<TrackingSnapshot>(TRACKING_TOPIC, getTrackingSnapshot);

	// The stop flow (see the module). Render state comes off `flow`; the deps
	// close over this window's snapshot.
	const flow = createPostSessionFlow({
		isSessionActive: () => data.status === 'active',
		isBusy: () => toggling,
		refresh: () => snapshot.hydrate(),
		stopTracking
	});
	const toggling = $derived(starting || flow.stopping);

	// Everything the strip has to say (a session's tracking warnings, a
	// refused start, a failed action) surfaces as a self-clearing notice
	// under it, never as text wedged between its controls.
	const notices = createOverlayNotices();

	// The session facets (the session it runs as, and the boost): state
	// and writes live in the feature model; this route owns only the
	// popup plumbing.
	const facets = createSessionFacets({
		readFacets: () => ({
			name: data.sessionName ?? null,
			definitionId: data.sessionDefinitionId ?? null,
			boost: data.skillBoostPercent ?? null
		}),
		isSessionActive: () => data.status === 'active',
		refresh: () => snapshot.hydrate(),
		setSessionConfig,
		selectDefinition: (id) => selectDefinition(id)
	});

	// Activities owns offered/standing transitions; the strip reads live chips.
	const activities = createActivitiesModel({
		readOptions: getActivityOptions,
		activateQuest: (questId, additive) =>
			activateActivity('quest', questId, null, additive),
		activateSegment: (label, additive) =>
			activateActivity('segment', null, label, additive),
		deactivateQuest: (questId) => deactivateActivity('quest', questId, null),
		deactivateSegment: (label) => deactivateActivity('segment', null, label),
		beginHandIn: beginQuestHandIn,
		refresh: () => snapshot.hydrate()
	});

	async function handleDrag(e: MouseEvent) {
		const target = e.target as HTMLElement;
		if (target.closest('button, [role="button"], input, select, textarea')) return;
		if (overlayMenuKind) {
			await hideOverlayMenu();
		}
		if (armourCost.open) {
			await armourCost.hide();
		}
		await getCurrentWindow().startDragging();
	}

	function clearMobCloseTimer() {
		if (!mobCloseTimer) return;
		clearTimeout(mobCloseTimer);
		mobCloseTimer = undefined;
	}


	function describeOverlayMenuError(error: unknown) {
		if (error instanceof ApiError || error instanceof Error) return error.message;
		if (typeof error === 'string' && error.trim()) return error;
		return 'Popup window failed to open';
	}

	function reportOverlayMenuOpenError(kind: OverlayMenuKind, error: unknown) {
		console.error(`Overlay ${kind} popup failed`, error);
		notices.push(describeOverlayMenuError(error));
	}

	// Keep this window's OS size in step with the strip; each sync re-anchors
	// the armour-cost popup, which hangs off a strip button.
	const windowSizeSync = createWindowSizeSync(() => overlayRoot, {
		afterSync: () => armourCost.scheduleAnchorSync()
	});

	function buildMobMenuState(anchorWidth: number): OverlayMenuState | null {
		const trimmedQuery = mobQuery.trim();
		const shouldShow = mobLoading || !!mobError || mobSuggestions.length > 0 || !!trimmedQuery;
		if (!shouldShow) return null;

		const labels = mobLoading
			? ['Searching...']
			: mobError
				? [mobError]
				: (mobSuggestions.length > 0 ? mobSuggestions.map((option) => option.display) : ['No matches']);

		return {
			kind: 'mob',
			width: computeMenuWidth(anchorWidth, labels, 28),
			query: trimmedQuery,
			loading: mobLoading,
			error: mobError,
			mobSuggestions
		};
	}

	function buildTrifectaMenuState(anchorWidth: number): OverlayMenuState | null {
		const trifecta = data.trifectaAttribution;
		if (!trifecta || trifecta.presets.length === 0) return null;

		return {
			kind: 'trifecta',
			width: computeMenuWidth(anchorWidth, trifecta.presets.map((preset) => preset.name), 88),
			options: trifecta.presets.map((preset) => ({
				id: preset.id,
				name: preset.name,
				active: preset.id === trifecta.activePresetId
			}))
		};
	}

	async function showOverlayMenu(
		kind: OverlayMenuKind,
		anchor: HTMLElement,
		state: OverlayMenuState,
		options: { focusPopup?: boolean } = {}
	) {
		try {
			// Resolve the window (creating it on first use) while the anchor
			// maths runs; the show below re-adopts the settled window.
			const [, anchorPosition] = await Promise.all([
				menuWindow.ensure(),
				anchorBelow(anchor, OVERLAY_MENU_VERTICAL_GAP)
			]);
			const height = computeMenuHeight(menuRowCount(state));

			await menuWindow.show(
				state,
				{ x: anchorPosition.x, y: anchorPosition.y, width: state.width, height },
				{ focus: options.focusPopup }
			);
			overlayMenuKind = kind;
		} catch (error) {
			overlayMenuKind = null;
			reportOverlayMenuOpenError(kind, error);
		}
	}

	async function hideOverlayMenu() {
		if (overlayMenuKind === 'mob') {
			clearMobCloseTimer();
		}
		overlayMenuKind = null;
		await menuWindow.hide();
	}

	async function openMobMenu() {
		if (!mobInput) return;
		const state = buildMobMenuState(mobInput.getBoundingClientRect().width);
		if (!state) return;
		await showOverlayMenu('mob', mobInput, state);
	}

	async function closeMobMenu() {
		clearMobCloseTimer();
		if (overlayMenuKind !== 'mob') return;
		await hideOverlayMenu();
	}

	/** Open the session picker off its chip: fetch the authored
	 * definitions fresh (the dashboard authors them; the overlay must
	 * never present a stale catalogue) and present them with the
	 * current selection marked. */
	async function openDefinitionMenu(anchor: HTMLButtonElement) {
		let definitions;
		try {
			definitions = await getSessionDefinitions();
		} catch (error) {
			notices.push(describeOverlayMenuError(error));
			return;
		}
		const state = buildDefinitionMenuState(
			anchor.getBoundingClientRect().width,
			definitions,
			data.sessionDefinitionId ?? null
		);
		await showOverlayMenu('definition', anchor, state, { focusPopup: true });
	}

	async function toggleDefinitionMenu(anchor: HTMLButtonElement) {
		if (overlayMenuKind === 'definition') {
			await hideOverlayMenu();
			return;
		}
		await openDefinitionMenu(anchor);
	}

	async function toggleTrifectaMenu(anchor: HTMLButtonElement) {
		if (overlayMenuKind === 'trifecta') {
			await hideOverlayMenu();
			return;
		}

		const state = buildTrifectaMenuState(anchor.getBoundingClientRect().width);
		if (!state) return;
		await showOverlayMenu('trifecta', anchor, state, { focusPopup: true });
	}

	// The Activities control's anchor, kept so an action can re-present
	// the still-open menu with the refreshed rows. It is the section
	// element rather than the chip clicked, because a declaration swaps
	// the chips out from under the gesture that caused it.
	let activitiesAnchor: HTMLElement | null = null;

	async function openActivitiesMenu(anchor: HTMLElement) {
		// A failed read reports through the activities error channel.
		const options = await activities.load();
		if (!options) return;
		const state = buildActivitiesMenuState(
			anchor.getBoundingClientRect().width,
			options,
			data.status !== 'active',
			activities.segmentDraft,
		);
		activitiesAnchor = anchor;
		await showOverlayMenu('activities', anchor, state, { focusPopup: true });
	}

	async function toggleActivitiesMenu(anchor: HTMLElement) {
		if (overlayMenuKind === 'activities' || overlayMenuKind === 'questHandIn') {
			await hideOverlayMenu();
			return;
		}
		await openActivitiesMenu(anchor);
	}

	async function openQuestHandIn(key: string) {
		const anchor = activitiesAnchor;
		const option = activities.find(key);
		if (!anchor?.isConnected || !option) return;
		const handIn = await activities.beginHandIn(option);
		if (!handIn) return;
		await showOverlayMenu('questHandIn', anchor, buildQuestHandInMenuState(anchor.getBoundingClientRect().width, handIn), { focusPopup: true });
	}

	/** An Activities action keeps the control open (declaring one thing
	 * after another must not be a close-and-reopen, and the switch's
	 * effect should be visible where it happened): apply the write, then
	 * re-present the menu with the refreshed rows off the unchanged
	 * anchor. */
	async function handleActivityAction(action: () => Promise<unknown>) {
		await action();
		if (overlayMenuKind === 'activities' && activitiesAnchor?.isConnected) {
			await openActivitiesMenu(activitiesAnchor);
		}
	}

	const armourCost = createOverlayArmourCostModel({
		window: armourCostWindow,
		anchorGap: OVERLAY_MENU_VERTICAL_GAP,
		repairOcrEnabled: () => data.repairOcrEnabled === true
	});

	async function handleTrifectaPresetSelection(presetId: string) {
		const trifecta = data.trifectaAttribution;
		if (!trifecta || trifectaSaving || presetId === trifecta.activePresetId) return;

		trifectaSaving = true;
		try {
			await updateSettings({ active_trifecta_preset_id: presetId });
			await snapshot.hydrate();
		} catch (error) {
			notices.push(
				error instanceof ApiError || error instanceof Error
					? error.message
					: 'Failed to switch trifecta preset'
			);
		}
		trifectaSaving = false;
	}

	// Restore saved overlay position; periodically persist if moved
	$effect(() => {
		let lastSavedX: number | null = null;
		let lastSavedY: number | null = null;
		let stopPersist: (() => void) | undefined;

		(async () => {
			const win = getCurrentWindow();

			// Restore saved position on mount
			try {
				const pos = await getOverlayPosition();
				if (pos.x != null && pos.y != null) {
					await win.setPosition(new PhysicalPosition(pos.x, pos.y));
					lastSavedX = pos.x;
					lastSavedY = pos.y;
				}
			} catch { /* first launch or backend unreachable */ }

			// Persist position every 5s: save only if changed (avoids onMoved IPC
			// drag interference). windowGeometryPoll keeps running while the overlay
			// is hidden: its hidden/shown state is not reliably observable from
			// inside its own webview, so this is the one poll the visibility gate
			// deliberately does not pause.
			stopPersist = windowGeometryPoll(async () => {
				try {
					const pos = await win.outerPosition();
					if (pos.x !== lastSavedX || pos.y !== lastSavedY) {
						lastSavedX = pos.x;
						lastSavedY = pos.y;
						saveOverlayPosition(pos.x, pos.y).catch(() => {});
					}
				} catch { /* window may be hidden */ }
			}, 5000);
		})();

		return () => {
			stopPersist?.();
		};
	});

	$effect(() => {
		if (!overlayRoot) return;

		windowSizeSync.schedule();

		const handleVisibilityChange = () => {
			if (document.visibilityState === 'visible') {
				windowSizeSync.schedule();
			} else {
				// A hand-in waits on the user going to the game to hand
				// the quest in, so it outlives the strip going away.
				if (overlayMenuKind !== 'questHandIn') {
					void hideOverlayMenu();
				}
				void armourCost.hide();
			}
		};
		const handleFocus = () => {
			windowSizeSync.schedule();
			armourCost.scheduleAnchorSync();
		};

		const resizeObserver = new ResizeObserver(() => {
			windowSizeSync.schedule();
			armourCost.scheduleAnchorSync();
		});
		resizeObserver.observe(overlayRoot);

		document.addEventListener('visibilitychange', handleVisibilityChange);
		window.addEventListener('focus', handleFocus);

		return () => {
			windowSizeSync.cancel();
			document.removeEventListener('visibilitychange', handleVisibilityChange);
			window.removeEventListener('focus', handleFocus);
			resizeObserver.disconnect();
		};
	});



	// Re-read the consolidated snapshot on each backend tracking frame. The
	// listener attaches FIRST and the initial hydrate runs after it settles,
	// so a frame arriving during subscription setup is not lost (it simply
	// re-triggers a read). A payload-less frame on this topic
	// re-hydrates the same way, so it can never be mistaken for an idle
	// session.
	$effect(() => {
		let disposed = false;
		let unlisten: (() => void) | undefined;

		void snapshot.subscribe().then((fn) => {
			if (disposed) {
				fn();
				return;
			}
			unlisten = fn;
			void snapshot.hydrate();
		});

		return () => {
			disposed = true;
			unlisten?.();
		};
	});

	// The overlay is a hidden pre-spawned window shown (not focused) by
	// toggle_overlay, so no focus/visibility event fires on the frontend when it
	// appears. The shell emits `overlay-shown` from the show path; re-read on it
	// to refresh config/runtime fields no tracking frame announces (weapon
	// attribution, trifecta presets, mob-entry mode, repair-OCR), which would
	// otherwise stay stale and wedge a control after a settings change made
	// while the overlay was hidden.
	$effect(() => {
		let disposed = false;
		let unlisten: (() => void) | undefined;

		(async () => {
			unlisten = await listen(OVERLAY_SHOWN_EVENT, () => {
				if (disposed) return;
				void snapshot.hydrate();
			});
		})();

		return () => {
			disposed = true;
			unlisten?.();
		};
	});

	// Drive the elapsed timer client-side while active. With no poll, data.elapsed
	// would otherwise advance only on coalesced backend frames and the headline
	// timer would stutter; derive seconds from the session's started_at (the same
	// basis the stat pills use) and tick once a second. Idle tears the tick down.
	// Writing data.elapsed here cannot retrigger this effect: Svelte 5 tracks per
	// property, so the .elapsed write is isolated from the .status read (and
	// applySnapshot always materialises the elapsed key, so the write is a
	// value-update, never a key-add that would invalidate the read).
	$effect(() => {
		if (data.status !== 'active' || sessionStartedAtMs == null) return;
		const startedAt = sessionStartedAtMs;
		const tickElapsed = () => {
			data.elapsed = Math.max(0, Math.floor((Date.now() - startedAt) / 1000));
		};
		tickElapsed();
		return useVisiblePoll(tickElapsed, { intervalMs: 1000, immediate: false });
	});

	$effect(() => {
		let disposed = false;
		let unlistenSelect: (() => void) | undefined;
		let unlistenClosed: (() => void) | undefined;
		let unlistenInteract: (() => void) | undefined;

		void (async () => {
			unlistenSelect = await listen<OverlayMenuSelection>(OVERLAY_MENU_SELECT_EVENT, async (event) => {
				if (disposed) return;

				if (event.payload.kind === 'trifecta') {
					overlayMenuKind = null;
					await handleTrifectaPresetSelection(event.payload.presetId);
					return;
				}

				if (event.payload.kind === 'definition') {
					overlayMenuKind = null;
					// Tapping another row switches; tapping the selected one just
					// closes. A session always runs under one, so there is no
					// clear here any more than there is on the chip.
					if (!event.payload.selected) {
						await facets.selectDefinition(event.payload.definitionId);
					}
					return;
				}

				if (event.payload.kind === 'activities') {
					const payload = event.payload;
					if (payload.action === 'handIn') {
						await openQuestHandIn(payload.key);
						return;
					}
					await handleActivityAction(() => {
						if (payload.action === 'declare') {
							activities.segmentDraft = payload.label;
							return activities.declareTyped();
						}
						const option = activities.find(payload.key);
						if (!option) return Promise.resolve(false);
						return payload.action === 'toggle'
							? activities.toggle(option)
							: activities.declare(option, true);
					});
					return;
				}

				if (event.payload.kind === 'questHandIn') {
					overlayMenuKind = null;
					await activities.refresh();
					return;
				}

				overlayMenuKind = null;
				await handleSelectMob({
					display: event.payload.maturity
						? `${event.payload.species} ${event.payload.maturity}`.trim()
						: event.payload.species,
					species: event.payload.species,
					maturity: event.payload.maturity
				});
			});

			unlistenClosed = await listen(OVERLAY_MENU_CLOSED_EVENT, async () => {
				if (disposed) return;
				overlayMenuKind = null;
				clearMobCloseTimer();
			});

			unlistenInteract = await listen(OVERLAY_MENU_INTERACT_EVENT, async () => {
				if (disposed) return;
				if (overlayMenuKind === 'mob') clearMobCloseTimer();
			});
		})();

		return () => {
			disposed = true;
			unlistenSelect?.();
			unlistenClosed?.();
			unlistenInteract?.();
		};
	});

	$effect(() => {
		let disposed = false;
		let unlistenClosed: (() => void) | undefined;

		void (async () => {
			unlistenClosed = await listen(OVERLAY_ARMOUR_COST_CLOSED_EVENT, () => {
				if (disposed) return;
				armourCost.noteClosed();
			});
		})();

		return () => {
			disposed = true;
			unlistenClosed?.();
		};
	});



	// Map the one consolidated snapshot onto the overlay's two render bindings:
	// `data` (TrackingLive, the strip) and `status` (TrackingStatus, the stat
	// pills). TrackingSnapshot is a strict superset of TrackingStatus, so `status`
	// takes it directly; `data` is mapped field by field, bridging the snapshot's
	// snake `session_id` / `kill_count` onto the live shape's camel `sessionId` /
	// `killCount`. The activity feed (`recentEvents`) is deliberately not mapped:
	// the overlay renders no feed.
	function applySnapshot(snap: TrackingSnapshot) {
		status = snap;
		data = {
			status: snap.status ?? 'idle',
			sessionId: snap.session_id,
			elapsed: snap.elapsed,
			killCount: snap.kill_count,
			cost: snap.cost,
			returns: snap.returns,
			pes: snap.pes,
			net: snap.net,
			returnRate: snap.returnRate,
			weaponAttribution: snap.weaponAttribution,
			repairOcrEnabled: snap.repairOcrEnabled,
			sessionName: snap.sessionName,
			sessionDefinitionId: snap.sessionDefinitionId,
			trackProtectionCosts: snap.trackProtectionCosts,
			skillBoostPercent: snap.skillBoostPercent,
			currentMob: snap.currentMob,
			currentTool: snap.currentTool,
			currentToolKind: snap.currentToolKind,
			currentActivity: snap.currentActivity,
			activities: snap.activities,
			trifectaAttribution: snap.trifectaAttribution,
			harvestGuardrail: snap.harvestGuardrail,
			warnings: snap.warnings,
		};
		const startedMs = snap.started_at ? new Date(snap.started_at).getTime() : NaN;
		sessionStartedAtMs = Number.isNaN(startedMs) ? null : startedMs;
	}

	$effect(() => {
		const current = snapshot.current;
		if (!current) return;
		applySnapshot(current);
	});

	// A session's warnings arrive as its cumulative list on every frame;
	// each one notifies once, when it first appears.
	$effect(() => {
		const sessionId = data.sessionId ?? null;
		const warnings = (data.warnings ?? []).map((warning) => warning.description);
		untrack(() => notices.observeWarnings(sessionId, warnings));
	});

	// The feature models' failure channels: each new message is a notice.
	notices.follow(() => facets.facetError);
	notices.follow(() => activities.error);
	notices.follow(() => armourCost.error);
	$effect(() => () => notices.destroy());

	const isTrifectaAttribution = $derived(data.weaponAttribution === 'trifecta');

	const showManualInput = $derived(
		(data.status === 'active' || data.status === 'idle') && !data.currentMob
	);
	// The declared-mob typeahead (the one remaining free-text facet; the
	// session is picked from the authored definitions instead).
	// Search failures are mapped to the overlay's established wording
	// before the typeahead records them.
	const mobTypeahead = createTypeahead<ManualMobSuggestion>({
		search: async (query) => {
			try {
				return await getManualMobSuggestions(query);
			} catch (error) {
				throw new Error(error instanceof ApiError ? error.message : 'Mob lookup failed');
			}
		},
		debounceMs: 120,
		minLength: 1
	});
	const mobSuggestions = $derived(mobTypeahead.results);
	const mobLoading = $derived(mobTypeahead.loading);

	// Drive each typeahead from its input state. Hiding the input or
	// emptying the query suspends the search and closes that menu, keeping
	// the typed text.
	$effect(() => {
		if (!showManualInput) {
			mobTypeahead.cancel();
			void closeMobMenu();
			return;
		}

		mobTypeahead.query = mobQuery;
		if (!mobQuery.trim()) {
			mobTypeahead.cancel();
			void closeMobMenu();
			return;
		}

		mobTypeahead.refresh();
	});

	// Present the search lifecycle in the menu window: mirror the typeahead's
	// settled error into the shared channel and re-sync the menu at each
	// transition (the loading flip, a results publication, an error) while the
	// input is focused or the menu already open. Only the lifecycle is
	// tracked; the gate reads are untracked so a bare focus change cannot
	// re-open a menu with nothing new to show.
	$effect(() => {
		void mobTypeahead.loading;
		void mobTypeahead.results;
		mobError = mobTypeahead.error;
		untrack(() => {
			if (!showManualInput || !mobQuery.trim()) return;
			if (mobInputFocused || overlayMenuKind === 'mob') {
				void openMobMenu();
			}
		});
	});

	$effect(() => {
		return () => {
			mobTypeahead.destroy();
		};
	});

	// Keep the boost buffer in step with its persisted facet while the
	// user is not editing it (an idle overlay re-read, a session start
	// snapshotting the declaration).
	$effect(() => {
		void data.skillBoostPercent;
		untrack(() => {
			facets.syncBoostDraft();
		});
	});

	async function handleStart() {
		starting = true;
		try {
			await startTracking();
			await snapshot.hydrate();
		} catch (error) {
			// A refused start (no hotbar slot bound, trifecta not configured)
			// says what to set up; TRACK stays in place for the retry.
			if (error instanceof ApiError && error.kind === 'badRequest') {
				notices.push(error.message, 'warning');
			}
		}
		starting = false;
	}

	async function handleReleaseMob() {
		releasing = true;
		try {
			await releaseMob();
			mobQuery = '';
			mobTypeahead.cancel();
			await closeMobMenu();
			mobError = null;
			await snapshot.hydrate();
		} catch { /* ignore */ }
		releasing = false;
	}

	function handleMobFocus() {
		clearMobCloseTimer();
		mobInputFocused = true;
		if (mobQuery.trim() && (mobSuggestions.length > 0 || mobLoading || !!mobError)) {
			void openMobMenu();
		}
	}

	function handleMobBlur() {
		mobInputFocused = false;
		clearMobCloseTimer();
		mobCloseTimer = setTimeout(() => {
			void closeMobMenu();
		}, 120);
	}

	async function handleMobKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape') {
			await closeMobMenu();
			return;
		}
		if (event.key !== 'Enter') return;

		// Only a catalogue mob can be declared, so Enter takes the top
		// match rather than inventing a name the catalogue cannot resolve.
		if (mobSuggestions.length > 0) {
			event.preventDefault();
			await handleSelectMob(mobSuggestions[0]);
		}
	}

	async function handleSelectMob(option: ManualMobSuggestion) {
		clearMobCloseTimer();
		selectingMob = true;
		mobError = null;
		try {
			await lockManualMob(option.species, option.maturity);
			mobQuery = '';
			mobTypeahead.cancel();
			await closeMobMenu();
			await snapshot.hydrate();
		} catch (error) {
			mobError = error instanceof ApiError ? error.message : 'Failed to declare mob';
		}
		selectingMob = false;
	}
</script>

<!-- Kept: mousedown is the frameless overlay window's drag handle (pointer-only by nature); the controls inside are native buttons. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="p-2 flex flex-col items-start overlay-frame w-max" bind:this={overlayRoot} onmousedown={handleDrag}>
	<OverlayStrip
		{data}
		{status}
		{toggling}
		{releasing}
		{selectingMob}
		{trifectaSaving}
		armourCostOpen={armourCost.open}
		definitionMenuOpen={overlayMenuKind === 'definition'}
		trifectaMenuOpen={overlayMenuKind === 'trifecta'}
		savingDefinition={facets.savingDefinition}
		definitionEditable={facets.definitionEditable}
		savingBoost={facets.savingBoost}
		savingActivity={activities.saving}
		activitiesMenuOpen={overlayMenuKind === 'activities' || overlayMenuKind === 'questHandIn'}
		bind:mobQuery
		bind:mobInput
		bind:boostDraft={facets.boostDraft}
		onStart={handleStart}
		onStop={flow.requestStop}
		onReleaseMob={handleReleaseMob}
		onMobFocus={handleMobFocus}
		onMobBlur={handleMobBlur}
		onMobKeydown={handleMobKeydown}
		onDefinitionTrigger={toggleDefinitionMenu}
		onBoostCommit={facets.commitBoost}
		onActivitiesTrigger={toggleActivitiesMenu}
		onTrifectaTrigger={toggleTrifectaMenu}
		onArmourCostToggle={armourCost.toggle}
	/>
	<OverlayNotices notices={notices.current} onHold={notices.hold} onRelease={notices.release} />
</div>
<style>
	.overlay-frame {
		overflow: visible;
	}
</style>
