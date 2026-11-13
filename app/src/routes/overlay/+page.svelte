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
		pinMob,
		unpinMob,
		getActivityOptions,
		activateActivity,
		deactivateActivity,
		beginQuestHandIn,
		decideWeaponMismatch,
		type TrackingLive,
		type TrackingStatus,
		type TrackingSnapshot,
		type WeaponMismatchDecision
	} from '$lib/api';
	import { untrack } from 'svelte';
	import { useVisiblePoll, windowGeometryPoll } from '$lib/realtime/useVisiblePoll';
	import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
	import { createPostSessionFlow } from '$lib/features/tracking/postSession.svelte';
	import { createSessionFacets } from '$lib/features/tracking/sessionFacets.svelte';
	import { createActivitiesModel } from '$lib/features/tracking/activitiesModel.svelte';
	import { createMobSourceModel } from '$lib/features/tracking/mobSourceModel.svelte';
	import { createMobEntry } from '$lib/features/tracking/mobEntry.svelte';
	import { createOverlayNotices } from '$lib/features/tracking/overlayNotices.svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { PhysicalPosition } from '@tauri-apps/api/dpi';
	import { listen } from '@tauri-apps/api/event';
	import {
		ARMOUR_COST_SUB_WINDOW as ARMOUR_COST,
		createOverlaySubWindows
	} from '$lib/windows/overlaySubWindows.svelte';
	import { createWindowSizeSync } from '$lib/windows/windowSize';
	import {
		OVERLAY_MENU_SELECT_EVENT,
		buildActivitiesMenuState,
		buildConsumablesMenuState,
		buildHeldToolState,
		buildMobSourceState,
		buildQuestHandInMenuState,
		buildDefinitionMenuState,
		type OverlayArmourCostState,
		type OverlayMenuKind,
		type OverlayMenuSelection,
		type OverlayMenuState,
		type ReadoutPanel
	} from '$lib/windows/overlayMenu';
	import OverlayReadout from '$lib/components/overlay/OverlayReadout.svelte';
	import { createDosesModel } from '$lib/features/consumables/dosesModel.svelte';
	import { createEquipmentDeclare } from '$lib/features/tracking/equipmentDeclare.svelte';
	import {
		cueCopy,
		heldToolDisplay,
		screenNotReadLine,
		toolInHand
	} from '$lib/features/tracking/heldTool';
	import { isSteered } from '$lib/features/tracking/mobSource';
	import { doseState } from '$lib/features/consumables/doses';
	import OverlayNotices from '$lib/components/overlay/OverlayNotices.svelte';
	import { attachLifetimeLootValuation } from '$lib/features/dashboard/lifetimeLootValuation.svelte';
	import { overlayStats } from '$lib/statsCustomisation.svelte';
	import { drawsLifetimeValuation } from '$lib/statsRegistry';
	import { statsScope } from '$lib/statsScope.svelte';
	import { attachLiveLootValuation } from '$lib/features/dashboard/liveLootValuation.svelte';
	import { liveMarkupSettings } from '$lib/liveMarkupSettings.svelte';

	// The colon-form Tauri topic the shell's event bridge emits each backend
	// tracking frame on (the wire topic `tracking.session.updated`; Tauri event
	// names forbid dots).
	const TRACKING_TOPIC = 'tracking:session:updated';
	// Emitted by the shell (toggle_overlay) when this hidden window is shown, so
	// the overlay can re-read config/runtime fields no tracking frame announces.
	const OVERLAY_SHOWN_EVENT = 'overlay-shown';

	let overlayRoot: HTMLDivElement | null = $state(null);
	let decidingWeapon = $state(false);
	let data = $state<TrackingLive>({ status: 'idle' });
	let status = $state<TrackingStatus | null>(null);
	// The session's loot at market markup, for the markup pills.
	const liveValuation = attachLiveLootValuation(() => status, () => liveMarkupSettings.confidence);
	const lifetimeValuation = attachLifetimeLootValuation(
		() => status,
		() => liveMarkupSettings.confidence,
		() => statsScope.current === 'lifetime' && drawsLifetimeValuation(overlayStats.current),
		() => liveValuation.loot,
	);
	// Session start in epoch-ms (parsed from the snapshot's started_at), the basis
	// for the client-side elapsed tick. null when no active session is timed.
	let sessionStartedAtMs = $state<number | null>(null);
	let starting = $state(false);



	// The overlay's sub-windows: every menu and panel it opens beside
	// itself, all opened, placed and closed by the one host (the rules are
	// in `lib/windows/subWindow.ts`).
	const subWindows = createOverlaySubWindows({
		onError: (id, error) => {
			console.error(`Overlay ${id} popup failed`, error);
			notices.push(describeOverlayMenuError(error));
		},
		// The typed mob's suggestions follow its field: anything that happens
		// in them, or to them, settles the field's own pending close.
		onClosed: (id) => id === 'mob' && mobEntry.clearCloseTimer(),
		onInteract: (id) => id === 'mob' && mobEntry.clearCloseTimer()
	});
	$effect(() => subWindows.connect());
	/** What the menu window shows, if anything. */
	const overlayMenuKind = $derived(subWindows.shownIn('menu') as OverlayMenuKind | 'screenUnread' | null);

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

	// The mob source: what the strip shows of it, and the writes that steer it.
	const mobSource = createMobSourceModel({
		view: () => data.mobSource ?? null,
		pin: pinMob,
		unpin: unpinMob,
		setTyped: lockManualMob,
		clearTyped: releaseMob,
		refresh: () => snapshot.hydrate()
	});
	// Its typed entry: the catalogue typeahead and the suggestion menu.
	const mobEntry = createMobEntry({
		source: mobSource,
		available: () => data.status === 'active' || data.status === 'idle',
		search: getManualMobSuggestions,
		menu: {
			show: (anchor, state) => showOverlayMenu('mob', anchor, state),
			hide: () => subWindows.close('mob'),
			isOpen: () => subWindows.isOpen('mob'),
			isFocused: subWindows.menuFocused
		}
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
		// A sub-window hangs from where the overlay is: none follows a drag.
		await subWindows.closeAll();
		await getCurrentWindow().startDragging();
	}



	function describeOverlayMenuError(error: unknown) {
		if (error instanceof ApiError || error instanceof Error) return error.message;
		if (typeof error === 'string' && error.trim()) return error;
		return 'Popup window failed to open';
	}

	// Keep this window's OS footprint exactly the readout and its notices;
	// each sync lets the open sub-windows follow their controls.
	const windowSizeSync = createWindowSizeSync(() => overlayRoot, {
		afterSync: () => void subWindows.reanchor()
	});


	/** Show a menu or panel in the menu window. The typed mob's suggestions
	 * follow its field and take no keyboard; a quest hand-in stays while
	 * the player goes to hand the quest in. */
	async function showOverlayMenu(
		kind: OverlayMenuKind,
		anchor: HTMLElement,
		state: OverlayMenuState,
		options: { focusPopup?: boolean } = {}
	) {
		await subWindows.open({
			slot: 'menu',
			id: kind,
			anchor,
			state,
			focus: options.focusPopup === true,
			attached: kind === 'mob',
			stays: kind === 'questHandIn'
		});
	}

	/** A menu's control was clicked: close it when it is open, else open
	 * it with what `build` reads for it. */
	function toggleOverlayMenu(
		kind: OverlayMenuKind,
		anchor: HTMLElement,
		build: () => OverlayMenuState | null | Promise<OverlayMenuState | null>
	) {
		return subWindows.toggleWith({ slot: 'menu', id: kind, anchor, focus: true }, build);
	}


	/** The session picker off its title: the authored definitions, fetched
	 * fresh (the dashboard authors them; the overlay must never present a
	 * stale catalogue), with the current selection marked. */
	function toggleDefinitionMenu(anchor: HTMLButtonElement) {
		return toggleOverlayMenu('definition', anchor, async () => {
			try {
				const definitions = await getSessionDefinitions();
				return buildDefinitionMenuState(
					anchor.getBoundingClientRect().width,
					definitions,
					data.sessionDefinitionId ?? null
				);
			} catch (error) {
				notices.push(describeOverlayMenuError(error));
				return null;
			}
		});
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
		// A hand-in is the activities control's too: its click closes either.
		if (subWindows.isOpen('questHandIn')) {
			await subWindows.close('questHandIn');
			return;
		}
		if (subWindows.isOpen('activities')) {
			await subWindows.close('activities');
			return;
		}
		if (subWindows.reclicked('activities')) return;
		await openActivitiesMenu(anchor);
	}

	// The doses in force, read on each consumables frame and ticked for
	// their countdowns while any is on show. They belong to the running
	// session, so its start or stop re-reads them.
	const doses = createDosesModel();
	$effect(() => doses.connect());
	const sessionActive = $derived(data.status === 'active');
	$effect(() => {
		void sessionActive;
		void doses.refresh();
	});
	$effect(() => {
		if (!doses.ticking) return;
		return useVisiblePoll(doses.tick, { intervalMs: 1000 });
	});
	$effect(() => {
		if (doses.error) {
			notices.push(doses.error);
			doses.dismissError();
		}
	});

	function toggleDosesMenu(anchor: HTMLElement) {
		return toggleOverlayMenu('consumables', anchor, () => {
			const running = doses.rows.flatMap((dose) =>
				doseState(dose, doses.now) === 'running' && dose.equipmentId !== null ? [dose.equipmentId] : []
			);
			const width = anchor.getBoundingClientRect().width;
			return buildConsumablesMenuState(width, doses.options, running);
		});
	}

	// The equipment picker: declare what is in hand without a hotbar press.
	const equipment = createEquipmentDeclare({
		onDeclared: () => snapshot.hydrate(),
		onError: (message) => notices.push(message)
	});

	function toggleEquipmentMenu(anchor: HTMLElement) {
		return toggleOverlayMenu('equipment', anchor, async () => {
			try {
				const width = anchor.getBoundingClientRect().width;
				const inHand = toolInHand(data.toolSource, data.currentTool);
				return await equipment.menuState(width, inHand, status?.healing?.toolName ?? null);
			} catch (error) {
				notices.push(describeOverlayMenuError(error));
				return null;
			}
		});
	}

	// The readout's lines that say more in a sub-window: where the mob
	// comes from, the tool in hand in full, and why the screen is not read.
	// Each is built from the snapshot, and kept up to date while it is open.
	const inputAvailable = $derived(data.status === 'active' || data.status === 'idle');
	const toolDisplay = $derived(heldToolDisplay(data.toolSource));
	const unreadLine = $derived(screenNotReadLine(data.toolSource));
	const panelStates = $derived<Record<ReadoutPanel, OverlayMenuState | null>>({
		mobSource: isSteered(data.mobSource)
			? buildMobSourceState(data.mobSource, inputAvailable, mobSource.busy !== null)
			: null,
		heldTool: toolDisplay
			? buildHeldToolState(
					toolDisplay,
					data.weaponHitCue
						? cueCopy(data.toolSource?.readingScreen === true).hit(data.weaponHitCue)
						: null,
					equipment.declaring
				)
			: null,
		screenUnread: unreadLine ? { kind: 'note', heading: 'Tool', text: unreadLine } : null
	});
	// The control each panel hangs from, for what one panel opens in turn.
	const panelAnchors: Partial<Record<ReadoutPanel, HTMLElement>> = {};

	function togglePanel(panel: ReadoutPanel, anchor: HTMLElement) {
		panelAnchors[panel] = anchor;
		return subWindows.toggleWith({ slot: 'menu', id: panel, anchor, focus: true }, () => panelStates[panel]);
	}

	// An open panel follows the snapshot, and closes when its line no longer
	// has anything to say (the session ended, the source moved).
	$effect(() => {
		for (const panel of ['mobSource', 'heldTool', 'screenUnread'] as const) {
			const state = panelStates[panel];
			if (!subWindows.isOpen(panel)) continue;
			untrack(() => void (state ? subWindows.update(panel, state) : subWindows.close(panel)));
		}
	});

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
		if (subWindows.isOpen('activities') && activitiesAnchor?.isConnected) {
			await openActivitiesMenu(activitiesAnchor);
		}
	}

	/** The armour recording: it stays while the player goes to the game to
	 * repair or to place items in a terminal, centred under its control. */
	function toggleArmourCost(event: MouseEvent) {
		const anchor = event.currentTarget as HTMLElement | null;
		if (!anchor) return;
		return subWindows.toggleWith(
			{ slot: 'armour', id: ARMOUR_COST, anchor, stays: true, align: 'centre' },
			(): OverlayArmourCostState => ({ repairOcrEnabled: data.repairOcrEnabled === true })
		);
	}

	/** The player's call on a standing weapon mismatch. A stale control (the
	 * cue already resolved by a press) changes nothing; either way the
	 * readout re-reads. */
	async function handleWeaponDecision(decision: WeaponMismatchDecision) {
		if (decidingWeapon) return;
		decidingWeapon = true;
		try {
			await decideWeaponMismatch(decision);
			await snapshot.hydrate();
		} catch (error) {
			notices.push(
				error instanceof ApiError || error instanceof Error
					? error.message
					: 'The weapon decision could not be saved'
			);
		}
		decidingWeapon = false;
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

		const disconnectSizeSync = windowSizeSync.connect();

		const handleVisibilityChange = () => {
			if (document.visibilityState === 'visible') {
				windowSizeSync.schedule();
			} else {
				// A hand-in waits on the user going to the game to hand
				// the quest in, so it outlives the overlay going away.
				void subWindows.closeAll((id) => id === 'questHandIn');
			}
		};
		const handleFocus = () => {
			windowSizeSync.schedule();
			void subWindows.reanchor();
		};

		document.addEventListener('visibilitychange', handleVisibilityChange);
		window.addEventListener('focus', handleFocus);

		return () => {
			disconnectSizeSync();
			document.removeEventListener('visibilitychange', handleVisibilityChange);
			window.removeEventListener('focus', handleFocus);
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
	// to refresh config/runtime fields no tracking frame announces (the
	// hotbar key setting, mob-entry mode, repair-OCR), which would
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

		void (async () => {
			unlistenSelect = await listen<OverlayMenuSelection>(OVERLAY_MENU_SELECT_EVENT, async (event) => {
				if (disposed) return;

				if (event.payload.kind === 'definition') {
					// Tapping another row switches; tapping the selected one just
					// closes. A session always runs under one, so there is no
					// clear here any more than there is on the chip.
					if (!event.payload.selected) {
						await facets.selectDefinition(event.payload.definitionId);
					}
					return;
				}

				if (event.payload.kind === 'consumables') {
					await doses.start(event.payload.equipmentId, event.payload.untimed);
					return;
				}

				if (event.payload.kind === 'equipment') {
					await equipment.declare(event.payload.equipmentId);
					return;
				}

				if (event.payload.kind === 'mobSource') {
					const picked = event.payload;
					if (picked.action === 'clearBackstop') await mobSource.clearTyped();
					else if (picked.action === 'pin') {
						await mobSource.pinTyped(picked.species, picked.maturity);
					} else await mobSource.backstop(picked.species, picked.maturity);
					return;
				}

				if (event.payload.kind === 'heldTool') {
					if (event.payload.action === 'declare') {
						await equipment.declare(event.payload.equipmentId);
						return;
					}
					// "Choose a tool": the picker opens where the panel hung.
					const anchor = panelAnchors.heldTool;
					if (anchor?.isConnected && !subWindows.isOpen('equipment')) {
						await toggleEquipmentMenu(anchor);
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
					await activities.refresh();
					return;
				}

				await mobEntry.select({
					display: event.payload.maturity
						? `${event.payload.species} ${event.payload.maturity}`.trim()
						: event.payload.species,
					species: event.payload.species,
					maturity: event.payload.maturity
				});
			});

		})();

		return () => {
			disposed = true;
			unlistenSelect?.();
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
			hotbarKeysEnabled: snap.hotbarKeysEnabled,
			hotbarListenerActive: snap.hotbarListenerActive,
			repairOcrEnabled: snap.repairOcrEnabled,
			sessionName: snap.sessionName,
			sessionDefinitionId: snap.sessionDefinitionId,
			trackProtectionCosts: snap.trackProtectionCosts,
			skillBoostPercent: snap.skillBoostPercent,
			mobSource: snap.mobSource,
			toolSource: snap.toolSource,
			currentTool: snap.currentTool,
			currentToolKind: snap.currentToolKind,
			currentActivity: snap.currentActivity,
			activities: snap.activities,
			harvestGuardrail: snap.harvestGuardrail,
			weaponGuardrail: snap.weaponGuardrail,
			unpricedShots: snap.unpricedShots,
			weaponDeclared: snap.weaponDeclared,
			keylessInput: snap.keylessInput,
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
	notices.follow(() => mobSource.error);
	$effect(() => () => notices.destroy());


	async function handleStart() {
		starting = true;
		try {
			await startTracking();
			await snapshot.hydrate();
		} catch (error) {
			// A refused start (no tool configured) says what to set up;
			// TRACK stays in place for the retry.
			if (error instanceof ApiError && error.kind === 'badRequest') {
				notices.push(error.message, 'warning');
			}
		}
		starting = false;
	}

</script>

<!-- Kept: mousedown is the frameless overlay window's drag handle (pointer-only by nature); the controls inside are native buttons. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="flex flex-col items-start overlay-frame w-max" bind:this={overlayRoot} onmousedown={handleDrag}>
	<OverlayReadout
		{data}
		{status}
		statContext={{
			valuation: liveValuation.view,
			lifetimeValuation: lifetimeValuation.view,
			looterBasis: liveMarkupSettings.looterBasis,
		}}
		{toggling}
		releasing={mobEntry.releasing}
		selectingMob={mobEntry.selecting}
		{mobSource}
		{decidingWeapon}
		armourCostOpen={subWindows.isOpen(ARMOUR_COST)}
		definitionMenuOpen={overlayMenuKind === 'definition'}
		savingDefinition={facets.savingDefinition}
		definitionEditable={facets.definitionEditable}
		savingActivity={activities.saving}
		activitiesMenuOpen={overlayMenuKind === 'activities' || overlayMenuKind === 'questHandIn'}
		bind:mobQuery={mobEntry.query}
		bind:mobInput={mobEntry.input}
		onStart={handleStart}
		onStop={flow.requestStop}
		onReleaseMob={mobEntry.release}
		onMobFocus={mobEntry.focus}
		onMobBlur={mobEntry.blur}
		onMobKeydown={mobEntry.keydown}
		onDefinitionTrigger={toggleDefinitionMenu}
		onActivitiesTrigger={toggleActivitiesMenu}
		onWeaponDecision={handleWeaponDecision}
		onArmourCostToggle={toggleArmourCost}
		{doses}
		dosesMenuOpen={overlayMenuKind === 'consumables'}
		onDosesTrigger={toggleDosesMenu}
		equipmentMenuOpen={overlayMenuKind === 'equipment'}
		declaringEquipment={equipment.declaring}
		onEquipmentTrigger={toggleEquipmentMenu}
		openPanel={overlayMenuKind === 'mobSource' ||
		overlayMenuKind === 'heldTool' ||
		overlayMenuKind === 'screenUnread'
			? overlayMenuKind
			: null}
		onPanelTrigger={togglePanel}
	/>
	<OverlayNotices notices={notices.current} onHold={notices.hold} onRelease={notices.release} />
</div>
<style>
	.overlay-frame {
		overflow: visible;
	}
</style>
