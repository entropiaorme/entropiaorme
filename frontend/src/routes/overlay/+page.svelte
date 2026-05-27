<script lang="ts">
	import {
		ApiError,
		getTrackingLive,
		getTrackingStatus,
		startTracking,
		stopTracking,
		releaseMob,
		getOverlayPosition,
		saveOverlayPosition,
		getTrackingTagSuggestions,
		lockTrackingTag,
		getManualMobSuggestions,
		lockManualMob,
		getSessionQuestLinkSuggestion,
		decideSessionQuestLink,
		updateSettings,
		type TrackingLive,
		type TrackingStatus,
		type ManualMobSuggestion,
		type SessionQuestLinkSuggestion
	} from '$lib/api';
	import { tick } from 'svelte';
	import type { MobTrackingMode } from '$lib/types/settings';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { LogicalSize, PhysicalPosition } from '@tauri-apps/api/dpi';
	import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
	import { emit, listen } from '@tauri-apps/api/event';
	import {
		OVERLAY_MENU_CLOSED_EVENT,
		OVERLAY_MENU_HIDE_EVENT,
		OVERLAY_MENU_INTERACT_EVENT,
		OVERLAY_MENU_READY_EVENT,
		OVERLAY_MENU_SELECT_EVENT,
		OVERLAY_MENU_SHOW_EVENT,
		OVERLAY_MENU_WINDOW_LABEL,
		type OverlayMenuKind,
		type OverlayMenuSelection,
		type OverlayMenuState
	} from '$lib/overlayMenu';
	import {
		OVERLAY_ARMOUR_COST_CLOSED_EVENT,
		OVERLAY_ARMOUR_COST_HIDE_EVENT,
		OVERLAY_ARMOUR_COST_READY_EVENT,
		OVERLAY_ARMOUR_COST_SHOW_EVENT,
		OVERLAY_ARMOUR_COST_UPDATE_EVENT,
		OVERLAY_ARMOUR_COST_WINDOW_LABEL,
		type OverlayArmourCostState
	} from '$lib/overlayArmourCost';
	import OverlayStrip from '$lib/components/overlay/OverlayStrip.svelte';

	const TRACKING_STATE_CHANGED_EVENT = 'tracking-state-changed';
	const OVERLAY_SIZE_SLACK = 36;
	const OVERLAY_MENU_VERTICAL_GAP = 6;
	const OVERLAY_MENU_MAX_HEIGHT = 220;
	const OVERLAY_MENU_MAX_WIDTH = 340;
	const OVERLAY_MENU_MIN_WIDTH = 180;

	let overlayRoot: HTMLDivElement | null = $state(null);
	let resizeFrame: number | null = null;
	let lastWindowWidth: number | null = null;
	let lastWindowHeight: number | null = null;
	let overlayMenuKind = $state<OverlayMenuKind | null>(null);
	let overlayMenuWindowPromise: Promise<WebviewWindow> | null = null;
	let overlayMenuReady = false;
	let overlayMenuReadyPromise: Promise<void> | null = null;
	let armourCostOpen = $state(false);
	// Stamped when the popup self-closes (blur, ESC, post-save). The Cost-button
	// click handler races against the CLOSED event: if blur arrives first,
	// armourCostOpen flips to false before toggleArmourCost reads it, and the
	// click would reopen the popup that the same gesture just dismissed.
	// Gating the open branch on this timestamp suppresses that reopen.
	let armourCostClosedAt = 0;
	let armourCostWindowPromise: Promise<WebviewWindow> | null = null;
	let armourCostReady = false;
	let armourCostReadyPromise: Promise<void> | null = null;
	let armourCostError = $state<string | null>(null);
	let armourCostAnchor: HTMLElement | null = $state(null);
	let armourCostAnchorFrame: number | null = null;
	let postSessionArmourButton: HTMLButtonElement | null = $state(null);
	// Yellow "Track armour?" prompt that replaces the Stop button after the user
	// clicks Stop while the end-of-session armour reminder is enabled. The actual
	// stop sequence runs only after the user picks Yes/No.
	let awaitingArmourTrackDecision = $state(false);
	// Yellow attribution-not-ready warning that replaces the TRACK button when
	// startTracking is refused by the backend (no hotbar slot bound in hotbar
	// mode, or trifecta not configured in trifecta mode). Persists until the
	// user closes it; clicking TRACK again clears it implicitly on success.
	let attributionWarning = $state<string | null>(null);
	let mobInput: HTMLInputElement | null = $state(null);
	let mobInputFocused = $state(false);
	let trifectaSaving = $state(false);
	let trifectaError = $state<string | null>(null);
	let overlayMenuLaunchError = $state<string | null>(null);

	async function handleDrag(e: MouseEvent) {
		const target = e.target as HTMLElement;
		if (target.closest('button, [role="button"], input, select, textarea')) return;
		if (overlayMenuKind) {
			await hideOverlayMenu();
		}
		if (armourCostOpen) {
			await hideArmourCost();
		}
		await getCurrentWindow().startDragging();
	}

	let data = $state<TrackingLive>({ status: 'idle' });
	let status = $state<TrackingStatus | null>(null);
	let releasing = $state(false);
	let toggling = $state(false);

	// Post-session quest link flow
	let lastSessionId = $state<string | null>(null);
	let lastSessionStats = $state<{ cost: number; returns: number; pes: number; net: number } | null>(null);
	let questLinkSuggestion = $state<SessionQuestLinkSuggestion | null>(null);
	let questLinkMessage = $state<string | null>(null);
	let questLinkSaving = $state(false);
	let postSessionClearPending = $state(false);
	let mobQuery = $state('');
	let tagSuggestions = $state<string[]>([]);
	let mobSuggestions = $state<ManualMobSuggestion[]>([]);
	let mobLoading = $state(false);
	let mobError = $state<string | null>(null);
	let selectingMob = $state(false);
	let mobCloseTimer: ReturnType<typeof setTimeout> | undefined;

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
		const message = describeOverlayMenuError(error);
		console.error(`Overlay ${kind} popup failed`, error);
		if (kind === 'trifecta') {
			trifectaError = message;
			return;
		}
		overlayMenuLaunchError = message;
	}

	function measureOverlaySize(root: HTMLDivElement) {
		const rootRect = root.getBoundingClientRect();
		return {
			width: Math.max(1, Math.ceil(rootRect.width + OVERLAY_SIZE_SLACK)),
			height: Math.max(1, Math.ceil(rootRect.height + OVERLAY_SIZE_SLACK))
		};
	}

	async function syncOverlayWindowSize() {
		if (!overlayRoot) return;

		const { width, height } = measureOverlaySize(overlayRoot);
		if (width === lastWindowWidth && height === lastWindowHeight) return;

		lastWindowWidth = width;
		lastWindowHeight = height;

		try {
			await getCurrentWindow().setSize(new LogicalSize(width, height));
		} catch {
			lastWindowWidth = null;
			lastWindowHeight = null;
		}
	}

	function scheduleOverlayWindowSizeSync() {
		if (!overlayRoot || resizeFrame != null) return;

		resizeFrame = window.requestAnimationFrame(() => {
			resizeFrame = null;
			void (async () => {
				await syncOverlayWindowSize();
				scheduleArmourCostAnchorSync();
			})();
		});
	}

	function measureMenuTextWidth(labels: string[], font = '500 12px Inter, system-ui, sans-serif') {
		if (labels.length === 0) return 0;
		const canvas = document.createElement('canvas');
		const context = canvas.getContext('2d');
		if (!context) return labels.reduce((longest, label) => Math.max(longest, label.length * 8), 0);

		context.font = font;
		return labels.reduce((longest, label) => Math.max(longest, context.measureText(label).width), 0);
	}

	function computeMenuWidth(minWidth: number, labels: string[], padding: number) {
		const contentWidth = measureMenuTextWidth(labels);
		return Math.max(
			Math.ceil(minWidth),
			Math.min(OVERLAY_MENU_MAX_WIDTH, Math.max(OVERLAY_MENU_MIN_WIDTH, Math.ceil(contentWidth + padding)))
		);
	}

	function computeMenuHeight(rows: number) {
		return Math.min(OVERLAY_MENU_MAX_HEIGHT, Math.max(44, rows * 34 + 12));
	}

	function buildMobMenuState(anchorWidth: number): OverlayMenuState | null {
		const trimmedQuery = mobQuery.trim();
		const shouldShow = mobLoading
			|| !!mobError
			|| mobSuggestions.length > 0
			|| tagSuggestions.length > 0
			|| !!trimmedQuery;
		if (!shouldShow) return null;

		const labels = mobLoading
			? ['Searching...']
			: mobError
				? [mobError]
				: showTagInput
					? (tagSuggestions.length > 0 ? tagSuggestions : [`Press Enter to set "${trimmedQuery}"`])
					: (mobSuggestions.length > 0 ? mobSuggestions.map((option) => option.display) : ['No matches']);

		return {
			kind: 'mob',
			width: computeMenuWidth(anchorWidth, labels, 28),
			mode: showTagInput ? 'tag' : 'manual',
			query: trimmedQuery,
			loading: mobLoading,
			error: mobError,
			tagSuggestions,
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

	async function getAnchorPosition(anchor: HTMLElement) {
		const currentWindow = getCurrentWindow();
		const [windowPosition, scaleFactor] = await Promise.all([
			currentWindow.outerPosition(),
			currentWindow.scaleFactor()
		]);
		const rect = anchor.getBoundingClientRect();
		return {
			x: Math.round(windowPosition.x + rect.left * scaleFactor),
			y: Math.round(windowPosition.y + (rect.bottom + OVERLAY_MENU_VERTICAL_GAP) * scaleFactor),
			width: rect.width
		};
	}

	async function getArmourCostAnchor(anchor: HTMLElement) {
		const currentWindow = getCurrentWindow();
		const [windowOuterPosition, scaleFactor] = await Promise.all([
			currentWindow.outerPosition(),
			currentWindow.scaleFactor()
		]);
		const rect = anchor.getBoundingClientRect();
		const windowLogicalX = windowOuterPosition.x / scaleFactor;
		const windowLogicalY = windowOuterPosition.y / scaleFactor;
		return {
			centerX: windowLogicalX + rect.left + rect.width / 2,
			top: windowLogicalY + rect.bottom + OVERLAY_MENU_VERTICAL_GAP
		};
	}

	async function buildArmourCostState(anchor: HTMLElement): Promise<OverlayArmourCostState | null> {
		const sessionId = armourSessionId;
		if (!sessionId || !anchor.isConnected) return null;

		return {
			sessionId,
			repairOcrEnabled: data.repairOcrEnabled === true,
			anchor: await getArmourCostAnchor(anchor)
		};
	}

	function ensureOverlayMenuReadyListener() {
		if (overlayMenuReady || overlayMenuReadyPromise) return overlayMenuReadyPromise ?? Promise.resolve();

		overlayMenuReadyPromise = new Promise((resolve) => {
			let unlisten: (() => void) | undefined;
			void listen<{ label?: string }>(OVERLAY_MENU_READY_EVENT, (event) => {
				if (event.payload?.label !== OVERLAY_MENU_WINDOW_LABEL) return;
				overlayMenuReady = true;
				overlayMenuReadyPromise = null;
				unlisten?.();
				resolve();
			}).then((fn) => {
				unlisten = fn;
			});
		});

		return overlayMenuReadyPromise;
	}

	async function ensureOverlayMenuWindow() {
		if (overlayMenuWindowPromise) return overlayMenuWindowPromise;

		overlayMenuWindowPromise = (async () => {
			const existing = await WebviewWindow.getByLabel(OVERLAY_MENU_WINDOW_LABEL);
			if (existing) {
				overlayMenuReady = true;
				return existing;
			}

			const readyPromise = ensureOverlayMenuReadyListener();
			const popupWindow = new WebviewWindow(OVERLAY_MENU_WINDOW_LABEL, {
				url: '/overlay-menu',
				width: OVERLAY_MENU_MIN_WIDTH,
				height: 44,
				visible: false,
				decorations: false,
				transparent: true,
				alwaysOnTop: true,
				skipTaskbar: true,
				shadow: false,
				resizable: false,
				focus: false
			});

			await new Promise<void>((resolve, reject) => {
				const timeoutId = window.setTimeout(() => {
					reject(new Error('Popup window creation timed out'));
				}, 3000);

				void popupWindow.once('tauri://created', () => {
					window.clearTimeout(timeoutId);
					resolve();
				});

				void popupWindow.once('tauri://error', (event) => {
					window.clearTimeout(timeoutId);
					const payload = typeof event.payload === 'string'
						? event.payload
						: JSON.stringify(event.payload);
					reject(new Error(payload || 'Unknown Tauri popup creation error'));
				});
			});

			await Promise.race([
				readyPromise,
				new Promise<never>((_, reject) => {
					window.setTimeout(() => {
						reject(new Error('Popup route did not become ready'));
					}, 3000);
				})
			]);
			return popupWindow;
		})().catch((error) => {
			overlayMenuWindowPromise = null;
			overlayMenuReady = false;
			overlayMenuReadyPromise = null;
			throw error;
		});

		return overlayMenuWindowPromise;
	}

	async function showOverlayMenu(
		kind: OverlayMenuKind,
		anchor: HTMLElement,
		state: OverlayMenuState,
		options: { focusPopup?: boolean } = {}
	) {
		try {
			const [popupWindow, anchorPosition] = await Promise.all([
				ensureOverlayMenuWindow(),
				getAnchorPosition(anchor)
			]);
			const height = state.kind === 'trifecta'
				? computeMenuHeight(state.options.length)
				: computeMenuHeight(
					state.loading || state.error
						? 1
						: state.mode === 'tag'
							? Math.max(1, state.tagSuggestions.length)
							: Math.max(1, state.mobSuggestions.length)
				);

			await popupWindow.setSize(new LogicalSize(state.width, height));
			await popupWindow.setPosition(new PhysicalPosition(anchorPosition.x, anchorPosition.y));
			await popupWindow.emit(OVERLAY_MENU_SHOW_EVENT, state);
			await popupWindow.show();
			if (options.focusPopup) {
				await popupWindow.setFocus().catch(() => {});
			}
			if (kind === 'mob') {
				overlayMenuLaunchError = null;
			}
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
		const popupWindow = overlayMenuWindowPromise
			? await overlayMenuWindowPromise.catch(() => null)
			: await WebviewWindow.getByLabel(OVERLAY_MENU_WINDOW_LABEL);
		if (!popupWindow) return;
		await popupWindow.emit(OVERLAY_MENU_HIDE_EVENT).catch(() => {});
	}

	async function openMobMenu() {
		if (!mobInput) return;
		const state = buildMobMenuState(mobInput.getBoundingClientRect().width);
		if (!state) return;
		overlayMenuLaunchError = null;
		await showOverlayMenu('mob', mobInput, state);
	}

	async function closeMobMenu() {
		clearMobCloseTimer();
		if (overlayMenuKind !== 'mob') return;
		await hideOverlayMenu();
	}

	async function toggleTrifectaMenu(anchor: HTMLButtonElement) {
		if (overlayMenuKind === 'trifecta') {
			await hideOverlayMenu();
			return;
		}

		trifectaError = null;
		const state = buildTrifectaMenuState(anchor.getBoundingClientRect().width);
		if (!state) return;
		await showOverlayMenu('trifecta', anchor, state, { focusPopup: true });
	}

	function ensureArmourCostReadyListener() {
		if (armourCostReady || armourCostReadyPromise) return armourCostReadyPromise ?? Promise.resolve();

		armourCostReadyPromise = new Promise((resolve) => {
			let unlisten: (() => void) | undefined;
			void listen<{ label?: string }>(OVERLAY_ARMOUR_COST_READY_EVENT, (event) => {
				if (event.payload?.label !== OVERLAY_ARMOUR_COST_WINDOW_LABEL) return;
				armourCostReady = true;
				armourCostReadyPromise = null;
				unlisten?.();
				resolve();
			}).then((fn) => {
				unlisten = fn;
			});
		});

		return armourCostReadyPromise;
	}

	async function ensureArmourCostWindow() {
		if (armourCostWindowPromise) return armourCostWindowPromise;

		armourCostWindowPromise = (async () => {
			const existing = await WebviewWindow.getByLabel(OVERLAY_ARMOUR_COST_WINDOW_LABEL);
			if (existing) {
				armourCostReady = true;
				return existing;
			}

			const readyPromise = ensureArmourCostReadyListener();
			const popupWindow = new WebviewWindow(OVERLAY_ARMOUR_COST_WINDOW_LABEL, {
				url: '/overlay-armour-cost',
				width: 320,
				height: 64,
				visible: false,
				decorations: false,
				transparent: true,
				alwaysOnTop: true,
				skipTaskbar: true,
				shadow: false,
				resizable: false,
				focus: false
			});

			await new Promise<void>((resolve, reject) => {
				const timeoutId = window.setTimeout(() => {
					reject(new Error('Armour cost popup creation timed out'));
				}, 3000);

				void popupWindow.once('tauri://created', () => {
					window.clearTimeout(timeoutId);
					resolve();
				});

				void popupWindow.once('tauri://error', (event) => {
					window.clearTimeout(timeoutId);
					const payload = typeof event.payload === 'string'
						? event.payload
						: JSON.stringify(event.payload);
					reject(new Error(payload || 'Unknown Tauri popup creation error'));
				});
			});

			await Promise.race([
				readyPromise,
				new Promise<never>((_, reject) => {
					window.setTimeout(() => {
						reject(new Error('Armour cost popup did not become ready'));
					}, 3000);
				})
			]);
			return popupWindow;
		})().catch((error) => {
			armourCostWindowPromise = null;
			armourCostReady = false;
			armourCostReadyPromise = null;
			throw error;
		});

		return armourCostWindowPromise;
	}

	async function showArmourCost(anchor: HTMLElement) {
		try {
			const popupWindow = await ensureArmourCostWindow();
			const state = await buildArmourCostState(anchor);
			if (!state) return;

			armourCostAnchor = anchor;
			// The popup measures its panel, sizes+positions itself accurately, then
			// reveals + focuses on its own — avoids a one-frame flash at the wrong
			// (initial-guess) location.
			await popupWindow.emit(OVERLAY_ARMOUR_COST_SHOW_EVENT, state);
			armourCostError = null;
			armourCostOpen = true;
			scheduleArmourCostAnchorSync();
		} catch (error) {
			armourCostOpen = false;
			armourCostAnchor = null;
			armourCostError = error instanceof ApiError || error instanceof Error
				? error.message
				: 'Popup window failed to open';
			console.error('Armour cost popup failed', error);
		}
	}

	async function syncArmourCostAnchor() {
		if (!armourCostOpen || !armourCostAnchor) return;
		const state = await buildArmourCostState(armourCostAnchor);
		if (!state) return;

		const popupWindow = armourCostWindowPromise
			? await armourCostWindowPromise.catch(() => null)
			: await WebviewWindow.getByLabel(OVERLAY_ARMOUR_COST_WINDOW_LABEL);
		await popupWindow?.emit(OVERLAY_ARMOUR_COST_UPDATE_EVENT, state).catch(() => {});
	}

	function scheduleArmourCostAnchorSync() {
		if (!armourCostOpen || !armourCostAnchor || armourCostAnchorFrame != null) return;

		armourCostAnchorFrame = window.requestAnimationFrame(() => {
			armourCostAnchorFrame = null;
			void syncArmourCostAnchor();
		});
	}

	function clearArmourCostOpenState() {
		armourCostOpen = false;
		armourCostAnchor = null;
		if (armourCostAnchorFrame != null) {
			window.cancelAnimationFrame(armourCostAnchorFrame);
			armourCostAnchorFrame = null;
		}
		clearDeferredPostSessionState();
	}

	async function hideArmourCost() {
		clearArmourCostOpenState();
		const popupWindow = armourCostWindowPromise
			? await armourCostWindowPromise.catch(() => null)
			: await WebviewWindow.getByLabel(OVERLAY_ARMOUR_COST_WINDOW_LABEL);
		if (!popupWindow) return;
		await popupWindow.emit(OVERLAY_ARMOUR_COST_HIDE_EVENT).catch(() => {});
	}

	async function toggleArmourCost(event: MouseEvent) {
		if (armourCostOpen) {
			await hideArmourCost();
			return;
		}
		if (Date.now() - armourCostClosedAt < 250) return;
		const anchor = event.currentTarget as HTMLElement | null;
		if (!anchor) return;
		await showArmourCost(anchor);
	}

	async function handleTrifectaPresetSelection(presetId: string) {
		const trifecta = data.trifectaAttribution;
		if (!trifecta || trifectaSaving || presetId === trifecta.activePresetId) return;

		trifectaSaving = true;
		trifectaError = null;
		try {
			await updateSettings({ active_trifecta_preset_id: presetId });
			await fetchLive();
		} catch (error) {
			trifectaError = error instanceof ApiError || error instanceof Error
				? error.message
				: 'Failed to switch trifecta preset';
		}
		trifectaSaving = false;
	}

	// Restore saved overlay position; periodically persist if moved
	$effect(() => {
		let lastSavedX: number | null = null;
		let lastSavedY: number | null = null;
		let interval: ReturnType<typeof setInterval>;

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

			// Poll position every 5s — save only if changed (avoids onMoved IPC drag interference)
			interval = setInterval(async () => {
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
			clearInterval(interval);
		};
	});

	$effect(() => {
		if (!overlayRoot) return;

		scheduleOverlayWindowSizeSync();

		const handleVisibilityChange = () => {
			if (document.visibilityState === 'visible') {
				scheduleOverlayWindowSizeSync();
			} else {
				void hideOverlayMenu();
				void hideArmourCost();
			}
		};
		const handleFocus = () => {
			scheduleOverlayWindowSizeSync();
			scheduleArmourCostAnchorSync();
		};

		const resizeObserver = new ResizeObserver(() => {
			scheduleOverlayWindowSizeSync();
			scheduleArmourCostAnchorSync();
		});
		resizeObserver.observe(overlayRoot);

		document.addEventListener('visibilitychange', handleVisibilityChange);
		window.addEventListener('focus', handleFocus);

		return () => {
			if (resizeFrame != null) {
				window.cancelAnimationFrame(resizeFrame);
				resizeFrame = null;
			}
			document.removeEventListener('visibilitychange', handleVisibilityChange);
			window.removeEventListener('focus', handleFocus);
			resizeObserver.disconnect();
		};
	});



	// Poll live data every 2 seconds
	$effect(() => {
		fetchLive();
		const interval = setInterval(fetchLive, 2000);
		return () => clearInterval(interval);
	});

	$effect(() => {
		let disposed = false;
		let unlisten: (() => void) | undefined;

		(async () => {
			unlisten = await listen<{ status?: 'active' | 'idle' }>(
				TRACKING_STATE_CHANGED_EVENT,
				async () => {
					if (disposed) return;
					await fetchLive();
				}
			);
		})();

		return () => {
			disposed = true;
			unlisten?.();
		};
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

				if (event.payload.kind === 'tag') {
					overlayMenuKind = null;
					await handleApplyTag(event.payload.tag);
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
				if (disposed || overlayMenuKind !== 'mob') return;
				clearMobCloseTimer();
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
				armourCostClosedAt = Date.now();
				clearArmourCostOpenState();
			});
		})();

		return () => {
			disposed = true;
			unlistenClosed?.();
		};
	});



	async function fetchLive() {
		try {
			const [live, full] = await Promise.all([
				getTrackingLive(),
				getTrackingStatus().catch(() => null),
			]);
			data = live;
			status = full;
		} catch {
			// Preserve last-good state across transient fetch errors. A single
			// failed poll (sidecar briefly busy during a hotbar tool-switch,
			// concurrent fetches racing on a TRACKING_STATE_CHANGED_EVENT
			// re-fetch atop the 2s poll) used to wipe `data` to a dormant
			// 'unavailable' default and flicker the overlay through its
			// no-active-session render every poll midpoint. Sticky state holds
			// the last reading until the next successful poll restores live
			// state; if the sidecar is genuinely down, other surfaces signal
			// that more authoritatively.
		}
	}

	const isTrifectaAttribution = $derived(data.weaponAttribution === 'trifecta');

	const armourSessionId = $derived(data.sessionId ?? lastSessionId);
	const isTagEntryMode = $derived(data.mobEntryMode === 'tag');
	const mobLabel = $derived(isTagEntryMode ? 'Tag' : 'Mob');
	const showTagInput = $derived(
		(data.status === 'active' || data.status === 'idle')
			&& isTagEntryMode
			&& !data.currentMob
	);
	const showManualMobInput = $derived(
		(data.status === 'active' || data.status === 'idle')
			&& !isTagEntryMode
			&& !data.currentMob
	);
	const showManualInput = $derived(showTagInput || showManualMobInput);

	$effect(() => {
		if (!showManualInput) {
			tagSuggestions = [];
			mobSuggestions = [];
			mobLoading = false;
			void closeMobMenu();
			mobError = null;
			overlayMenuLaunchError = null;
			return;
		}

		const query = mobQuery.trim();
		if (!query) {
			tagSuggestions = [];
			mobSuggestions = [];
			mobLoading = false;
			mobError = null;
			void closeMobMenu();
			overlayMenuLaunchError = null;
			return;
		}

		let cancelled = false;
		const handle = setTimeout(async () => {
			mobLoading = true;
			if (mobInputFocused || overlayMenuKind === 'mob') {
				void openMobMenu();
			}
			try {
				if (showTagInput) {
					const suggestions = await getTrackingTagSuggestions(query);
					if (!cancelled && mobQuery.trim() === query) {
						tagSuggestions = suggestions;
						mobSuggestions = [];
						if (mobInputFocused || overlayMenuKind === 'mob') {
							void openMobMenu();
						}
						mobError = null;
					}
				} else {
					const suggestions = await getManualMobSuggestions(query);
					if (!cancelled && mobQuery.trim() === query) {
						mobSuggestions = suggestions;
						tagSuggestions = [];
						if (mobInputFocused || overlayMenuKind === 'mob') {
							void openMobMenu();
						}
						mobError = null;
					}
				}
			} catch (error) {
				if (!cancelled && mobQuery.trim() === query) {
					tagSuggestions = [];
					mobSuggestions = [];
					mobError = error instanceof ApiError
						? error.message
						: showTagInput ? 'Tag lookup failed' : 'Mob lookup failed';
					if (mobInputFocused || overlayMenuKind === 'mob') {
						void openMobMenu();
					}
				}
			} finally {
				if (!cancelled && mobQuery.trim() === query) {
					mobLoading = false;
					if (mobInputFocused || overlayMenuKind === 'mob') {
						void openMobMenu();
					}
				}
			}
		}, 120);

		return () => {
			cancelled = true;
			clearTimeout(handle);
		};
	});

	async function handleMobTrackingModeChange(mode: MobTrackingMode) {
		if (data.status === 'active' || data.mobEntryMode === mode) return;
		try {
			await updateSettings({ mob_tracking_mode: mode });
			data.mobEntryMode = mode;
		} catch { /* ignore */ }
	}

	async function handleStart() {
		toggling = true;
		attributionWarning = null;
		try {
			await startTracking();
			await fetchLive();
			await emit(TRACKING_STATE_CHANGED_EVENT, { status: 'active' });
		} catch (error) {
			if (error instanceof ApiError && error.status === 400) {
				attributionWarning = error.message;
			}
		}
		toggling = false;
	}

	function dismissAttributionWarning() {
		attributionWarning = null;
	}

	async function handleStopRequest() {
		if (data.status !== 'active' || toggling) return;
		// Gate behind the yellow "Track armour?" prompt when the reminder is on.
		// Yes/No on the prompt drive the actual stop via handleArmourTrackDecision.
		if (data.endOfSessionArmourReminderEnabled === true) {
			awaitingArmourTrackDecision = true;
			return;
		}
		await handleStop({ showArmour: false });
	}

	async function handleArmourTrackDecision(action: 'yes' | 'no') {
		if (!awaitingArmourTrackDecision) return;
		awaitingArmourTrackDecision = false;
		await handleStop({ showArmour: action === 'yes' });
	}

	async function handleStop({ showArmour }: { showArmour: boolean }) {
		toggling = true;
		const wasActive = data.status === 'active';
		let stoppedSessionId: string | null = null;
		try {
			// Capture session stats before stopping
			lastSessionStats = wasActive ? {
				cost: data.cost ?? 0,
				returns: data.returns ?? 0,
				pes: data.pes ?? 0,
				net: data.net ?? 0,
			} : null;

			const result = await stopTracking();
			stoppedSessionId = result.session_id;
			lastSessionId = stoppedSessionId;
			await fetchLive();
			await emit(TRACKING_STATE_CHANGED_EVENT, { status: 'idle' });
		} catch { /* ignore */ }
		toggling = false;

		// Armour-cost popup is opt-in via the prompt's Yes branch; suppressed
		// when the user picked No or when the reminder is disabled wholesale.
		if (wasActive && showArmour) {
			await tick();
			if (postSessionArmourButton && armourSessionId && !armourCostOpen) {
				await showArmourCost(postSessionArmourButton);
			}
		}

		if (stoppedSessionId) {
			void loadQuestLinkSuggestion(stoppedSessionId);
		}
	}

	async function loadQuestLinkSuggestion(sessionId: string) {
		questLinkSuggestion = null;
		questLinkMessage = null;
		try {
			const suggestion = await getSessionQuestLinkSuggestion(sessionId);
			if (suggestion.suggestionType === 'quest' || suggestion.suggestionType === 'playlist') {
				questLinkSuggestion = suggestion;
				void tick().then(scheduleArmourCostAnchorSync);
				return;
			}
			if (suggestion.reason === 'unclean' || suggestion.reason === 'ambiguous_playlist') {
				questLinkMessage = 'Unclean quest record, skipping linkage';
				void tick().then(scheduleArmourCostAnchorSync);
				return;
			}
		} catch { /* ignore */ }
		clearPostSessionStateWhenReady();
	}

	async function handleQuestLinkDecision(action: 'accept' | 'decline') {
		if (!lastSessionId) return;
		questLinkSaving = true;
		try {
			await decideSessionQuestLink(lastSessionId, action);
		} catch { /* ignore */ }
		clearPostSessionState();
		questLinkSaving = false;
	}

	function handleDismissQuestLinkMessage() {
		clearPostSessionState();
	}

	function clearPostSessionState() {
		postSessionClearPending = false;
		lastSessionId = null;
		lastSessionStats = null;
		questLinkSuggestion = null;
		questLinkMessage = null;
		questLinkSaving = false;
	}

	function clearPostSessionStateWhenReady() {
		if (armourCostOpen) {
			postSessionClearPending = true;
			return;
		}
		clearPostSessionState();
	}

	function clearDeferredPostSessionState() {
		if (!postSessionClearPending) return;
		clearPostSessionState();
	}

	async function handleReleaseMob() {
		releasing = true;
		try {
			await releaseMob();
			mobQuery = '';
			tagSuggestions = [];
			mobSuggestions = [];
			await closeMobMenu();
			mobError = null;
			await fetchLive();
		} catch { /* ignore */ }
		releasing = false;
	}

	function handleMobFocus() {
		clearMobCloseTimer();
		mobInputFocused = true;
		if (mobQuery.trim() && (mobSuggestions.length > 0 || tagSuggestions.length > 0 || mobLoading || !!mobError)) {
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

		if (showTagInput) {
			event.preventDefault();
			await handleApplyTag(mobQuery.trim());
			return;
		}

		if (mobSuggestions.length > 0) {
			event.preventDefault();
			await handleSelectMob(mobSuggestions[0]);
		}
	}

	async function handleApplyTag(tag: string) {
		if (!tag) return;
		clearMobCloseTimer();
		selectingMob = true;
		mobError = null;
		try {
			await lockTrackingTag(tag);
			mobQuery = '';
			tagSuggestions = [];
			mobSuggestions = [];
			overlayMenuLaunchError = null;
			await closeMobMenu();
			await fetchLive();
		} catch (error) {
			mobError = error instanceof ApiError ? error.message : 'Failed to set tag';
		}
		selectingMob = false;
	}

	async function handleSelectMob(option: ManualMobSuggestion) {
		clearMobCloseTimer();
		selectingMob = true;
		mobError = null;
		try {
			await lockManualMob(option.species, option.maturity);
			mobQuery = '';
			mobSuggestions = [];
			overlayMenuLaunchError = null;
			await closeMobMenu();
			await fetchLive();
		} catch (error) {
			mobError = error instanceof ApiError ? error.message : 'Failed to lock mob';
		}
		selectingMob = false;
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="p-2 flex flex-col items-start overlay-frame w-max" bind:this={overlayRoot} onmousedown={handleDrag}>
	<OverlayStrip
		{data}
		{status}
		{toggling}
		{releasing}
		{selectingMob}
		{trifectaSaving}
		{trifectaError}
		{armourCostOpen}
		{armourCostError}
		{armourSessionId}
		mobMenuOpen={overlayMenuKind === 'mob'}
		trifectaMenuOpen={overlayMenuKind === 'trifecta'}
		{overlayMenuLaunchError}
		{lastSessionId}
		{lastSessionStats}
		{questLinkSuggestion}
		{questLinkMessage}
		{questLinkSaving}
		bind:mobQuery
		bind:mobInput
		bind:postSessionArmourButton
		onStart={handleStart}
		onStop={handleStopRequest}
		awaitingArmourTrackDecision={awaitingArmourTrackDecision}
		onArmourTrackDecision={handleArmourTrackDecision}
		attributionWarning={attributionWarning}
		onDismissAttributionWarning={dismissAttributionWarning}
		onMobModeChange={handleMobTrackingModeChange}
		onReleaseMob={handleReleaseMob}
		onMobFocus={handleMobFocus}
		onMobBlur={handleMobBlur}
		onMobKeydown={handleMobKeydown}
		onTrifectaTrigger={toggleTrifectaMenu}
		onArmourCostToggle={toggleArmourCost}
		onQuestLinkDecision={handleQuestLinkDecision}
		onDismissQuestLinkMessage={handleDismissQuestLinkMessage}
	/>
</div>

<style>
	.overlay-frame {
		overflow: visible;
	}
</style>
